//! Production ASR job consumer — never on the capture path.
//!
//! Mirrors [`crate::OcrWorker`]: claim `transcribe_audio` → engine → `transcript.v1`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use lumen_platform::{AsrEngine, AsrResult, PlatformError};
use lumen_store::{JobRecord, JobStatus, SqliteStore};
use serde_json::json;
use tracing::{debug, info, warn};

pub const JOB_KIND_TRANSCRIBE_AUDIO: &str = "transcribe_audio";
pub const DERIVED_TRANSCRIPT_V1: &str = "transcript.v1";

#[derive(Debug, Clone)]
pub struct TranscribeWorkerConfig {
    pub locale: String,
    pub poll_interval: Duration,
    pub batch_size: usize,
    pub max_attempts: i64,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub engine_timeout: Duration,
    pub stale_running: Duration,
    pub max_audio_bytes: usize,
    pub max_text_chars: usize,
    pub shutdown_drain: Duration,
}

impl Default for TranscribeWorkerConfig {
    fn default() -> Self {
        Self {
            locale: "zh-CN".into(),
            poll_interval: Duration::from_millis(1500),
            batch_size: 1,
            max_attempts: 5,
            retry_base: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            engine_timeout: Duration::from_secs(120),
            stale_running: Duration::from_secs(5 * 60),
            max_audio_bytes: 8 * 1024 * 1024,
            max_text_chars: 200_000,
            shutdown_drain: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TranscribeWorkerStats {
    pub processed: u64,
    pub succeeded: u64,
    pub empty: u64,
    pub failed: u64,
    pub dead: u64,
    pub skipped_existing: u64,
    pub reclaimed: u64,
    pub timed_out: u64,
}

pub struct TranscribeWorker {
    store: Arc<SqliteStore>,
    engine: Arc<dyn AsrEngine>,
    config: TranscribeWorkerConfig,
    processed: AtomicU64,
    succeeded: AtomicU64,
    empty: AtomicU64,
    failed: AtomicU64,
    dead: AtomicU64,
    skipped_existing: AtomicU64,
    reclaimed: AtomicU64,
    timed_out: AtomicU64,
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

impl TranscribeWorker {
    pub fn new(
        store: Arc<SqliteStore>,
        engine: Arc<dyn AsrEngine>,
        config: TranscribeWorkerConfig,
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

    pub fn stats(&self) -> TranscribeWorkerStats {
        TranscribeWorkerStats {
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
                JOB_KIND_TRANSCRIBE_AUDIO,
                ChronoDuration::from_std(self.config.stale_running)
                    .unwrap_or_else(|_| ChronoDuration::minutes(5)),
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            self.reclaimed.fetch_add(n as u64, Ordering::Relaxed);
            warn!(count = n, "reclaimed stale ASR running jobs");
        }
        Ok(n)
    }

    pub async fn tick_once(&self) -> Result<usize, String> {
        if !self.engine.is_supported() {
            return Ok(0);
        }
        let _ = self.reclaim_stale();
        let jobs = self
            .store
            .claim_pending_jobs(JOB_KIND_TRANSCRIBE_AUDIO, self.config.batch_size)
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
                if e.timeout {
                    self.timed_out.fetch_add(1, Ordering::Relaxed);
                }
                if e.permanent || job.attempts >= self.config.max_attempts {
                    self.dead.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        job = %job.id,
                        event = %job.event_id,
                        attempts = job.attempts,
                        error = %e.message,
                        "asr job dead"
                    );
                    let _ = self
                        .store
                        .complete_job(job.id, JobStatus::Dead, Some(&e.message));
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
                        error = %e.message,
                        "asr job retry scheduled"
                    );
                    let _ = self.store.complete_job_at(
                        job.id,
                        JobStatus::Pending,
                        Some(&e.message),
                        Some(available),
                    );
                }
            }
        }
    }

    async fn process_job(&self, job: &JobRecord) -> Result<JobOutcome, JobError> {
        if self
            .store
            .has_derived(job.event_id, DERIVED_TRANSCRIPT_V1)
            .map_err(|e| JobError::transient(e.to_string()))?
        {
            debug!(event = %job.event_id, "transcript.v1 already present");
            return Ok(JobOutcome::AlreadyDone);
        }

        let Some((media, bytes)) = self
            .store
            .load_first_artifact_bytes(job.event_id)
            .map_err(|e| JobError::transient(e.to_string()))?
        else {
            return Err(JobError::permanent("no artifact for event".into()));
        };
        if bytes.is_empty() {
            return Err(JobError::permanent("artifact is empty".into()));
        }
        if bytes.len() > self.config.max_audio_bytes {
            return Err(JobError::permanent(format!(
                "audio too large: {} bytes",
                bytes.len()
            )));
        }
        if !media.starts_with("audio/") && media != "application/octet-stream" {
            debug!(%media, "unexpected media type for ASR; continuing");
        }

        let mut result = self.run_engine(&bytes).await?;
        if result.text.chars().count() > self.config.max_text_chars {
            result.text = result
                .text
                .chars()
                .take(self.config.max_text_chars)
                .collect::<String>()
                + "\n…[truncated]";
        }

        let body = transcript_body_json(&result, job.event_id, &bytes);
        self.store
            .insert_derived(job.event_id, DERIVED_TRANSCRIPT_V1, body)
            .map_err(|e| JobError::transient(e.to_string()))?;

        let nonempty = !result.text.trim().is_empty();
        info!(
            event = %job.event_id,
            chars = result.text.chars().count(),
            engine = %result.engine,
            nonempty,
            "transcript.v1 written"
        );
        Ok(JobOutcome::Success { nonempty })
    }

    async fn run_engine(&self, bytes: &[u8]) -> Result<AsrResult, JobError> {
        let fut = self.engine.transcribe(bytes, &self.config.locale);
        match tokio::time::timeout(self.config.engine_timeout, fut).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(classify_platform_err(e)),
            Err(_) => Err(JobError {
                message: format!(
                    "asr timed out after {}ms",
                    self.config.engine_timeout.as_millis()
                ),
                permanent: false,
                timeout: true,
            }),
        }
    }

    pub async fn run_until_cancelled(&self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        if !self.engine.is_supported() {
            warn!("ASR engine not supported; worker idle");
            return;
        }
        info!(
            locale = %self.config.locale,
            batch = self.config.batch_size,
            timeout_ms = self.config.engine_timeout.as_millis() as u64,
            "ASR worker started"
        );
        let _ = self.reclaim_stale();
        loop {
            if *cancel.borrow() {
                break;
            }
            match self.tick_once().await {
                Ok(0) => {}
                Ok(n) => debug!(claimed = n, "asr batch"),
                Err(e) => warn!(error = %e, "asr tick error"),
            }
            tokio::select! {
                _ = cancel.changed() => {
                    if *cancel.borrow() { break; }
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
        let deadline = tokio::time::Instant::now() + self.config.shutdown_drain;
        while tokio::time::Instant::now() < deadline {
            match self.tick_once().await {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => {
                    warn!(error = %e, "asr drain error");
                    break;
                }
            }
        }
        info!(stats = ?self.stats(), "ASR worker stopped");
    }

    pub async fn drain(&self, max_ticks: usize) -> TranscribeWorkerStats {
        for _ in 0..max_ticks {
            match self.tick_once().await {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        self.stats()
    }
}

fn transcript_body_json(result: &AsrResult, event_id: uuid::Uuid, audio: &[u8]) -> String {
    let hash = blake3::hash(audio);
    json!({
        "payload_version": 1,
        "event_id": event_id,
        "text": result.text,
        "confidence": result.confidence,
        "language": result.language,
        "engine": result.engine,
        "audio_bytes": audio.len(),
        "audio_blake3": hash.to_hex().to_string(),
    })
    .to_string()
}

fn retry_delay(attempts: i64, base: Duration, max: Duration) -> Duration {
    let shift = (attempts.saturating_sub(1)).min(8) as u32;
    let mult = 1u64 << shift;
    let ms = base.as_millis() as u64 * mult;
    Duration::from_millis(ms.min(max.as_millis() as u64))
}

fn classify_platform_err(e: PlatformError) -> JobError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    let permanent = lower.contains("empty")
        || lower.contains("too large")
        || lower.contains("not authorized")
        || lower.contains("unsupported")
        || lower.contains("permission denied");
    JobError {
        message: msg,
        permanent,
        timeout: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_platform::StubAsr;
    use lumen_types::{event_kind, SourceEvent, SourceKind};
    use serde_json::json;

    #[tokio::test]
    async fn processes_audio_to_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
        let event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({}),
        );
        let eid = event.id;
        store
            .put_and_append(event, "audio/wav", b"RIFF....fake-wav-bytes")
            .unwrap();
        assert!(store
            .enqueue_job(eid, JOB_KIND_TRANSCRIBE_AUDIO)
            .unwrap()
            .is_some());

        let worker = TranscribeWorker::new(
            Arc::clone(&store),
            Arc::new(StubAsr::new("你好世界")),
            TranscribeWorkerConfig {
                poll_interval: Duration::from_millis(10),
                engine_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        );
        assert_eq!(worker.tick_once().await.unwrap(), 1);
        assert!(store.has_derived(eid, DERIVED_TRANSCRIPT_V1).unwrap());
        let list = store.list_derived_for_event(eid).unwrap();
        assert!(list[0].2.contains("你好世界"));
        // searchable index
        let hits = store.search_ocr("你好", 5).unwrap();
        assert_eq!(hits.len(), 1);
        let st = worker.stats();
        assert_eq!(st.succeeded, 1);
    }

    #[tokio::test]
    async fn idempotent_skip_existing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
        let event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({}),
        );
        let eid = event.id;
        store
            .put_and_append(event, "audio/wav", b"RIFF")
            .unwrap();
        store
            .insert_derived(eid, DERIVED_TRANSCRIPT_V1, r#"{"text":"already"}"#)
            .unwrap();
        store
            .enqueue_job(eid, JOB_KIND_TRANSCRIBE_AUDIO)
            .unwrap();
        let worker = TranscribeWorker::new(
            Arc::clone(&store),
            Arc::new(StubAsr::new("new")),
            TranscribeWorkerConfig::default(),
        );
        worker.tick_once().await.unwrap();
        assert_eq!(worker.stats().skipped_existing, 1);
    }
}
