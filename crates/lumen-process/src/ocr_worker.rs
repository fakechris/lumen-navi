//! Production OCR job consumer — never on the capture path.
//!
//! Guarantees:
//! - Idempotent per event (`ocr.v1` derived + open-job uniqueness)
//! - Retry with exponential backoff (pending + available_at)
//! - Stale `running` reclaim
//! - Timeouts on engine calls
//! - Permanent vs transient error classification
//! - Image size / empty artifact handling

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use lumen_platform::{OcrEngine, OcrResult, PlatformError};
use lumen_store::{JobRecord, JobStatus, SqliteStore};
use serde_json::json;
use tracing::{debug, info, warn};

pub const JOB_KIND_OCR_SCREEN: &str = "ocr_screen";
pub const DERIVED_OCR_V1: &str = "ocr.v1";

#[derive(Debug, Clone)]
pub struct OcrWorkerConfig {
    pub languages: Vec<String>,
    pub poll_interval: Duration,
    pub batch_size: usize,
    /// Run layout boxes OCR.
    pub include_boxes: bool,
    /// If true, only run boxes when accurate text is empty (cheaper).
    pub boxes_when_empty_only: bool,
    pub max_attempts: i64,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub engine_timeout: Duration,
    pub stale_running: Duration,
    pub max_image_bytes: usize,
    pub max_text_chars: usize,
    /// Drain deadline when stopping after finite capture runs.
    pub shutdown_drain: Duration,
}

impl Default for OcrWorkerConfig {
    fn default() -> Self {
        Self {
            languages: vec!["zh-Hans".into(), "en-US".into()],
            poll_interval: Duration::from_millis(1500),
            batch_size: 2,
            include_boxes: true,
            boxes_when_empty_only: true,
            max_attempts: 5,
            retry_base: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            engine_timeout: Duration::from_secs(90),
            stale_running: Duration::from_secs(5 * 60),
            max_image_bytes: 25 * 1024 * 1024,
            max_text_chars: 500_000,
            shutdown_drain: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct OcrWorkerStats {
    pub processed: u64,
    pub succeeded: u64,
    pub empty: u64,
    pub failed: u64,
    pub dead: u64,
    pub skipped_existing: u64,
    pub reclaimed: u64,
    pub timed_out: u64,
}

pub struct OcrWorker {
    store: Arc<SqliteStore>,
    engine: Arc<dyn OcrEngine>,
    config: OcrWorkerConfig,
    processed: AtomicU64,
    succeeded: AtomicU64,
    empty: AtomicU64,
    failed: AtomicU64,
    dead: AtomicU64,
    skipped_existing: AtomicU64,
    reclaimed: AtomicU64,
    timed_out: AtomicU64,
}

impl OcrWorker {
    pub fn new(
        store: Arc<SqliteStore>,
        engine: Arc<dyn OcrEngine>,
        config: OcrWorkerConfig,
    ) -> Self {
        Self {
            store,
            engine,
            config,
            processed: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            empty: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            dead: AtomicU64::new(0),
            skipped_existing: AtomicU64::new(0),
            reclaimed: AtomicU64::new(0),
            timed_out: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &OcrWorkerConfig {
        &self.config
    }

    pub fn stats(&self) -> OcrWorkerStats {
        OcrWorkerStats {
            processed: self.processed.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            empty: self.empty.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            dead: self.dead.load(Ordering::Relaxed),
            skipped_existing: self.skipped_existing.load(Ordering::Relaxed),
            reclaimed: self.reclaimed.load(Ordering::Relaxed),
            timed_out: self.timed_out.load(Ordering::Relaxed),
        }
    }

    pub fn reclaim_stale(&self) -> Result<usize, String> {
        let n = self
            .store
            .reclaim_stale_running(
                JOB_KIND_OCR_SCREEN,
                ChronoDuration::from_std(self.config.stale_running)
                    .unwrap_or_else(|_| ChronoDuration::minutes(5)),
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            self.reclaimed.fetch_add(n as u64, Ordering::Relaxed);
            warn!(count = n, "reclaimed stale OCR running jobs");
        }
        Ok(n)
    }

    /// Process one batch. Returns jobs claimed.
    pub async fn tick_once(&self) -> Result<usize, String> {
        if !self.engine.is_supported() {
            return Ok(0);
        }
        let _ = self.reclaim_stale();
        let jobs = self
            .store
            .claim_pending_jobs(JOB_KIND_OCR_SCREEN, self.config.batch_size)
            .map_err(|e| e.to_string())?;
        let n = jobs.len();
        for job in jobs {
            self.processed.fetch_add(1, Ordering::Relaxed);
            self.handle_job(job).await;
        }
        Ok(n)
    }

    async fn handle_job(&self, job: JobRecord) {
        match self.process_job(&job).await {
            Ok(JobOutcome::Success { nonempty }) => {
                if nonempty {
                    self.succeeded.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.empty.fetch_add(1, Ordering::Relaxed);
                }
                let _ = self.store.complete_job(job.id, JobStatus::Done, None);
            }
            Ok(JobOutcome::AlreadyDone) => {
                self.skipped_existing.fetch_add(1, Ordering::Relaxed);
                let _ = self.store.complete_job(job.id, JobStatus::Done, None);
            }
            Err(e) => {
                let permanent = e.permanent;
                let msg = e.message;
                if e.timeout {
                    self.timed_out.fetch_add(1, Ordering::Relaxed);
                }
                if permanent || job.attempts >= self.config.max_attempts {
                    self.dead.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        job = %job.id,
                        event = %job.event_id,
                        attempts = job.attempts,
                        error = %msg,
                        "ocr job dead"
                    );
                    let _ = self
                        .store
                        .complete_job(job.id, JobStatus::Dead, Some(&msg));
                } else {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                    let backoff = retry_delay(
                        job.attempts,
                        self.config.retry_base,
                        self.config.retry_max,
                    );
                    let available = Utc::now()
                        + ChronoDuration::from_std(backoff)
                            .unwrap_or_else(|_| ChronoDuration::seconds(2));
                    warn!(
                        job = %job.id,
                        event = %job.event_id,
                        attempts = job.attempts,
                        backoff_ms = backoff.as_millis() as u64,
                        error = %msg,
                        "ocr job retry scheduled"
                    );
                    let _ = self.store.complete_job_at(
                        job.id,
                        JobStatus::Pending,
                        Some(&msg),
                        Some(available),
                    );
                }
            }
        }
    }

    async fn process_job(&self, job: &JobRecord) -> Result<JobOutcome, JobError> {
        if self
            .store
            .has_derived(job.event_id, DERIVED_OCR_V1)
            .map_err(|e| JobError::transient(e.to_string()))?
        {
            debug!(event = %job.event_id, "ocr.v1 already present");
            return Ok(JobOutcome::AlreadyDone);
        }

        let Some((_media, bytes)) = self
            .store
            .load_first_artifact_bytes(job.event_id)
            .map_err(|e| JobError::transient(e.to_string()))?
        else {
            return Err(JobError::permanent("no artifact for event".into()));
        };

        if bytes.is_empty() {
            return Err(JobError::permanent("artifact is empty".into()));
        }
        if bytes.len() > self.config.max_image_bytes {
            return Err(JobError::permanent(format!(
                "artifact too large: {} bytes",
                bytes.len()
            )));
        }

        let mut result = self
            .run_engine_text(&bytes)
            .await?;

        let need_boxes = self.config.include_boxes
            && (!self.config.boxes_when_empty_only || result.text.trim().is_empty());

        if need_boxes {
            match self.run_engine_boxes(&bytes).await {
                Ok(layout) => {
                    if result.text.trim().is_empty() && !layout.text.trim().is_empty() {
                        result.text = layout.text;
                        result.confidence = layout.confidence;
                        result.mode = format!("{}/boxes_fallback", result.mode);
                    }
                    result.boxes = layout.boxes;
                }
                Err(e) if result.text.trim().is_empty() => return Err(e),
                Err(e) => {
                    // Text succeeded; boxes are optional.
                    warn!(event = %job.event_id, error = %e.message, "boxes OCR failed; keeping text");
                }
            }
        }

        if result.text.chars().count() > self.config.max_text_chars {
            result.text = result
                .text
                .chars()
                .take(self.config.max_text_chars)
                .collect::<String>()
                + "\n…[truncated]";
        }

        let body = ocr_body_json(&result, job.event_id, &bytes);
        self.store
            .insert_derived(job.event_id, DERIVED_OCR_V1, body)
            .map_err(|e| JobError::transient(e.to_string()))?;

        let nonempty = !result.text.trim().is_empty();
        info!(
            event = %job.event_id,
            chars = result.text.chars().count(),
            conf = result.confidence,
            mode = %result.mode,
            boxes = result.boxes.len(),
            nonempty,
            "ocr.v1 written"
        );
        Ok(JobOutcome::Success { nonempty })
    }

    async fn run_engine_text(&self, bytes: &[u8]) -> Result<OcrResult, JobError> {
        let fut = self
            .engine
            .recognize_text(bytes, &self.config.languages);
        match tokio::time::timeout(self.config.engine_timeout, fut).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(classify_platform_err(e)),
            Err(_) => Err(JobError {
                message: format!(
                    "ocr text timed out after {}ms",
                    self.config.engine_timeout.as_millis()
                ),
                permanent: false,
                timeout: true,
            }),
        }
    }

    async fn run_engine_boxes(&self, bytes: &[u8]) -> Result<OcrResult, JobError> {
        let fut = self
            .engine
            .recognize_boxes(bytes, &self.config.languages);
        match tokio::time::timeout(self.config.engine_timeout, fut).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(classify_platform_err(e)),
            Err(_) => Err(JobError {
                message: format!(
                    "ocr boxes timed out after {}ms",
                    self.config.engine_timeout.as_millis()
                ),
                permanent: false,
                timeout: true,
            }),
        }
    }

    pub async fn run_until_cancelled(&self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        if !self.engine.is_supported() {
            warn!("OCR engine not supported; worker idle");
            return;
        }
        info!(
            langs = ?self.config.languages,
            batch = self.config.batch_size,
            timeout_ms = self.config.engine_timeout.as_millis() as u64,
            "OCR worker started"
        );
        let _ = self.reclaim_stale();
        loop {
            if *cancel.borrow() {
                break;
            }
            match self.tick_once().await {
                Ok(0) => {}
                Ok(n) => debug!(claimed = n, "ocr batch"),
                Err(e) => warn!(error = %e, "ocr tick error"),
            }
            tokio::select! {
                _ = cancel.changed() => {
                    if *cancel.borrow() { break; }
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
        // Best-effort drain
        let deadline = tokio::time::Instant::now() + self.config.shutdown_drain;
        while tokio::time::Instant::now() < deadline {
            match self.tick_once().await {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => {
                    warn!(error = %e, "ocr drain error");
                    break;
                }
            }
        }
        info!(stats = ?self.stats(), "OCR worker stopped");
    }

    /// Drain until idle or `max_rounds` (for finite daemon runs / tests).
    pub async fn drain(&self, max_rounds: usize) -> OcrWorkerStats {
        for _ in 0..max_rounds {
            let n = self.tick_once().await.unwrap_or(0);
            if n == 0 {
                // one grace poll after short wait for late enqueues
                tokio::time::sleep(Duration::from_millis(100)).await;
                let n2 = self.tick_once().await.unwrap_or(0);
                if n2 == 0 {
                    break;
                }
            }
        }
        self.stats()
    }
}

enum JobOutcome {
    Success { nonempty: bool },
    AlreadyDone,
}

struct JobError {
    message: String,
    permanent: bool,
    timeout: bool,
}

impl JobError {
    fn permanent(message: String) -> Self {
        Self {
            message,
            permanent: true,
            timeout: false,
        }
    }
    fn transient(message: String) -> Self {
        Self {
            message,
            permanent: false,
            timeout: false,
        }
    }
}

fn classify_platform_err(e: PlatformError) -> JobError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    let permanent = lower.contains("decode failed")
        || lower.contains("empty image")
        || lower.contains("too large")
        || lower.contains("unsupported")
        || lower.contains("zero dimensions");
    JobError {
        message: msg,
        permanent,
        timeout: false,
    }
}

fn retry_delay(attempts: i64, base: Duration, max: Duration) -> Duration {
    // attempts already incremented on claim (1-based after first fail -> attempts=1)
    let shift = (attempts.saturating_sub(1)).min(8) as u32;
    let mult = 1u64 << shift;
    let ms = base.as_millis() as u64 * mult;
    Duration::from_millis(ms.min(max.as_millis() as u64))
}

fn ocr_body_json(result: &OcrResult, event_id: uuid::Uuid, image: &[u8]) -> String {
    let hash = blake3::hash(image);
    json!({
        "payload_version": 1,
        "event_id": event_id,
        "text": result.text,
        "confidence": result.confidence,
        "languages": result.languages,
        "mode": result.mode,
        "boxes": result.boxes,
        "image_bytes": image.len(),
        "image_blake3": hash.to_hex().to_string(),
        "engine": "vision",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lumen_platform::{OcrBox, PlatformError};
    use lumen_store::EventStore;
    use lumen_types::{event_kind, SourceEvent, SourceKind};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    struct FakeEngine {
        calls: AtomicUsize,
        text: Mutex<String>,
        fail_times: AtomicUsize,
    }

    #[async_trait]
    impl OcrEngine for FakeEngine {
        fn is_supported(&self) -> bool {
            true
        }
        async fn recognize_text(
            &self,
            _image: &[u8],
            languages: &[String],
        ) -> Result<OcrResult, PlatformError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let left = self.fail_times.load(Ordering::SeqCst);
            if left > 0 {
                self.fail_times.fetch_sub(1, Ordering::SeqCst);
                return Err(PlatformError::Message("transient boom".into()));
            }
            Ok(OcrResult {
                text: self.text.lock().unwrap().clone(),
                confidence: 0.9,
                languages: languages.to_vec(),
                mode: "accurate".into(),
                boxes: vec![],
            })
        }
        async fn recognize_boxes(
            &self,
            _image: &[u8],
            languages: &[String],
        ) -> Result<OcrResult, PlatformError> {
            Ok(OcrResult {
                text: "box".into(),
                confidence: 0.5,
                languages: languages.to_vec(),
                mode: "fast".into(),
                boxes: vec![OcrBox {
                    x: 0.0,
                    y: 0.1,
                    w: 0.2,
                    h: 0.05,
                    text: "box".into(),
                    confidence: 0.5,
                }],
            })
        }
    }

    async fn seed_event(store: &SqliteStore, bytes: &[u8]) -> uuid::Uuid {
        let event = SourceEvent::new(
            SourceKind::Screen,
            event_kind::SCREENSHOT_V1,
            json!({"reason": "test"}),
        );
        let id = event.id;
        store
            .put_and_append(event, "image/jpeg", bytes)
            .unwrap();
        id
    }

    #[tokio::test]
    async fn processes_job_to_derived() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
        let eid = seed_event(&store, b"jpeg-bytes").await;
        assert!(store.enqueue_job(eid, JOB_KIND_OCR_SCREEN).unwrap().is_some());
        let engine = Arc::new(FakeEngine {
            calls: AtomicUsize::new(0),
            text: Mutex::new("hello\nworld".into()),
            fail_times: AtomicUsize::new(0),
        });
        let worker = OcrWorker::new(store.clone(), engine, OcrWorkerConfig {
            include_boxes: false,
            ..OcrWorkerConfig::default()
        });
        assert_eq!(worker.tick_once().await.unwrap(), 1);
        assert!(store.has_derived(eid, DERIVED_OCR_V1).unwrap());
        let st = worker.stats();
        assert_eq!(st.succeeded, 1);
        // second enqueue while done allowed; processing skips to AlreadyDone
        assert!(store.enqueue_job(eid, JOB_KIND_OCR_SCREEN).unwrap().is_some());
        assert_eq!(worker.tick_once().await.unwrap(), 1);
        assert_eq!(worker.stats().skipped_existing, 1);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
        let eid = seed_event(&store, b"x").await;
        store.enqueue_job(eid, JOB_KIND_OCR_SCREEN).unwrap();
        let engine = Arc::new(FakeEngine {
            calls: AtomicUsize::new(0),
            text: Mutex::new("ok".into()),
            fail_times: AtomicUsize::new(1),
        });
        let worker = OcrWorker::new(
            store.clone(),
            engine,
            OcrWorkerConfig {
                include_boxes: false,
                retry_base: Duration::from_millis(1),
                retry_max: Duration::from_millis(5),
                ..OcrWorkerConfig::default()
            },
        );
        assert_eq!(worker.tick_once().await.unwrap(), 1);
        // job pending with future available_at — force available
        let jobs = store.list_jobs(10).unwrap();
        let j = jobs.iter().find(|j| j.event_id == eid).unwrap();
        store
            .complete_job_at(j.id, JobStatus::Pending, None, Some(Utc::now()))
            .unwrap();
        assert_eq!(worker.tick_once().await.unwrap(), 1);
        assert!(store.has_derived(eid, DERIVED_OCR_V1).unwrap());
    }

    #[tokio::test]
    async fn permanent_missing_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
        let event = SourceEvent::new(SourceKind::Screen, event_kind::SCREENSHOT_V1, json!({}));
        let eid = event.id;
        store.append(vec![event]).await.unwrap();
        store.enqueue_job(eid, JOB_KIND_OCR_SCREEN).unwrap();
        let engine = Arc::new(FakeEngine {
            calls: AtomicUsize::new(0),
            text: Mutex::new("x".into()),
            fail_times: AtomicUsize::new(0),
        });
        let worker = OcrWorker::new(
            store.clone(),
            engine,
            OcrWorkerConfig {
                max_attempts: 1,
                include_boxes: false,
                ..Default::default()
            },
        );
        worker.tick_once().await.unwrap();
        let jobs = store.list_jobs(5).unwrap();
        assert_eq!(jobs[0].status, JobStatus::Dead);
    }
}
