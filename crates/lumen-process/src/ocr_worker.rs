//! Async OCR job consumer — never on the capture path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lumen_platform::{OcrEngine, OcrResult};
use lumen_store::{JobStatus, SqliteStore};
use serde_json::json;
use tracing::{debug, info, warn};

pub const JOB_KIND_OCR_SCREEN: &str = "ocr_screen";
pub const DERIVED_OCR_V1: &str = "ocr.v1";

#[derive(Debug, Clone)]
pub struct OcrWorkerConfig {
    pub languages: Vec<String>,
    pub poll_interval: Duration,
    pub batch_size: usize,
    pub include_boxes: bool,
    pub max_attempts: i64,
}

impl Default for OcrWorkerConfig {
    fn default() -> Self {
        Self {
            languages: vec!["zh-Hans".into(), "en-US".into()],
            poll_interval: Duration::from_secs(2),
            batch_size: 4,
            include_boxes: true,
            max_attempts: 5,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct OcrWorkerStats {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub empty: u64,
}

pub struct OcrWorker {
    store: Arc<SqliteStore>,
    engine: Arc<dyn OcrEngine>,
    config: OcrWorkerConfig,
    processed: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    empty: AtomicU64,
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
            failed: AtomicU64::new(0),
            empty: AtomicU64::new(0),
        }
    }

    pub fn stats(&self) -> OcrWorkerStats {
        OcrWorkerStats {
            processed: self.processed.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            empty: self.empty.load(Ordering::Relaxed),
        }
    }

    /// Process one batch of pending OCR jobs. Returns how many jobs claimed.
    pub async fn tick_once(&self) -> Result<usize, String> {
        if !self.engine.is_supported() {
            return Ok(0);
        }
        let jobs = self
            .store
            .claim_pending_jobs(JOB_KIND_OCR_SCREEN, self.config.batch_size)
            .map_err(|e| e.to_string())?;
        let n = jobs.len();
        for job in jobs {
            self.processed.fetch_add(1, Ordering::Relaxed);
            match self.process_job(&job).await {
                Ok(true) => {
                    self.succeeded.fetch_add(1, Ordering::Relaxed);
                    let _ = self.store.complete_job(job.id, JobStatus::Done, None);
                }
                Ok(false) => {
                    self.empty.fetch_add(1, Ordering::Relaxed);
                    let _ = self.store.complete_job(job.id, JobStatus::Done, None);
                }
                Err(e) => {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                    warn!(job = %job.id, error = %e, "ocr job failed");
                    // Do not auto-loop: mark failed/dead. Ops can re-enqueue later.
                    let status = if job.attempts >= self.config.max_attempts {
                        JobStatus::Dead
                    } else {
                        JobStatus::Failed
                    };
                    let _ = self.store.complete_job(job.id, status, Some(&e));
                }
            }
        }
        Ok(n)
    }

    async fn process_job(&self, job: &lumen_store::JobRecord) -> Result<bool, String> {
        // Skip if already have ocr.v1
        let existing = self
            .store
            .list_derived_for_event(job.event_id)
            .map_err(|e| e.to_string())?;
        if existing.iter().any(|(_, k, _)| k == DERIVED_OCR_V1) {
            debug!(event = %job.event_id, "ocr already present");
            return Ok(true);
        }

        let Some((_media, bytes)) = self
            .store
            .load_first_artifact_bytes(job.event_id)
            .map_err(|e| e.to_string())?
        else {
            return Err("no artifact for event".into());
        };

        let mut result = self
            .engine
            .recognize_text(&bytes, &self.config.languages)
            .await
            .map_err(|e| e.to_string())?;

        if result.text.trim().is_empty() && self.config.include_boxes {
            let boxes = self
                .engine
                .recognize_boxes(&bytes, &self.config.languages)
                .await
                .map_err(|e| e.to_string())?;
            if !boxes.text.trim().is_empty() {
                result = boxes;
            }
        } else if self.config.include_boxes {
            // Attach boxes without replacing quality text when possible.
            if let Ok(layout) = self
                .engine
                .recognize_boxes(&bytes, &self.config.languages)
                .await
            {
                result.boxes = layout.boxes;
            }
        }

        let body = ocr_body_json(&result, job.event_id);
        self.store
            .insert_derived(job.event_id, DERIVED_OCR_V1, body)
            .map_err(|e| e.to_string())?;

        let nonempty = !result.text.trim().is_empty();
        info!(
            event = %job.event_id,
            chars = result.text.chars().count(),
            conf = result.confidence,
            mode = %result.mode,
            boxes = result.boxes.len(),
            "ocr derived written"
        );
        Ok(nonempty)
    }

    /// Background loop until `cancel` is notified (or drop join handle).
    pub async fn run_until_cancelled(&self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        if !self.engine.is_supported() {
            warn!("OCR engine not supported; worker idle");
            return;
        }
        info!(
            langs = ?self.config.languages,
            "OCR worker started"
        );
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
        info!(stats = ?self.stats(), "OCR worker stopped");
    }
}

fn ocr_body_json(result: &OcrResult, event_id: uuid::Uuid) -> String {
    json!({
        "payload_version": 1,
        "event_id": event_id,
        "text": result.text,
        "confidence": result.confidence,
        "languages": result.languages,
        "mode": result.mode,
        "boxes": result.boxes,
    })
    .to_string()
}
