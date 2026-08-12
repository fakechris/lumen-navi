//! AX tree capture job consumer — deep accessibility text extraction.
//!
//! Mirrors `ocr_worker.rs`: claims `ax_screen` jobs, walks the AX tree of the
//! app that was frontmost when the screenshot was captured, writes the
//! flattened text as a `derived` row (kind `ax.v1`) which auto-indexes into
//! FTS5. Never on the capture hot path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use lumen_platform::{AxTreeSnapshot, AxTreeWalkConfig, AxTreeWalker, PlatformError};
use lumen_store::{JobRecord, JobStatus, SqliteStore};
use serde_json::json;
use tracing::{debug, info, warn};

pub const JOB_KIND_AX_SCREEN: &str = "ax_screen";
pub const DERIVED_AX_V1: &str = "ax.v1";

#[derive(Debug, Clone)]
pub struct AxWorkerConfig {
    pub poll_interval: Duration,
    pub batch_size: usize,
    pub max_attempts: i64,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub stale_running: Duration,
    pub max_text_chars: usize,
    pub shutdown_drain: Duration,
    /// AX tree walk budget.
    pub walk: AxTreeWalkConfig,
}

impl Default for AxWorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(1500),
            batch_size: 2,
            max_attempts: 3,
            retry_base: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            stale_running: Duration::from_secs(5 * 60),
            max_text_chars: 50_000,
            shutdown_drain: Duration::from_secs(30),
            walk: AxTreeWalkConfig::default(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct AxWorkerStats {
    pub processed: u64,
    pub succeeded: u64,
    pub empty: u64,
    pub failed: u64,
    pub dead: u64,
    pub reclaimed: u64,
}

pub struct AxWorker {
    store: Arc<SqliteStore>,
    walker: Arc<dyn AxTreeWalker>,
    config: AxWorkerConfig,
    processed: AtomicU64,
    succeeded: AtomicU64,
    empty: AtomicU64,
    failed: AtomicU64,
    dead: AtomicU64,
    reclaimed: AtomicU64,
}

impl AxWorker {
    pub fn new(store: Arc<SqliteStore>, walker: Arc<dyn AxTreeWalker>, config: AxWorkerConfig) -> Self {
        Self {
            store,
            walker,
            config,
            processed: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            empty: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            dead: AtomicU64::new(0),
            reclaimed: AtomicU64::new(0),
        }
    }

    pub fn reclaim_stale(&self) -> Result<usize, String> {
        let n = self
            .store
            .reclaim_stale_running(
                JOB_KIND_AX_SCREEN,
                ChronoDuration::from_std(self.config.stale_running)
                    .unwrap_or_else(|_| ChronoDuration::minutes(5)),
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            self.reclaimed.fetch_add(n as u64, Ordering::Relaxed);
            warn!(count = n, "reclaimed stale AX running jobs");
        }
        Ok(n)
    }

    pub async fn tick_once(&self) -> Result<usize, String> {
        if !self.walker.is_supported() {
            return Ok(0);
        }
        let _ = self.reclaim_stale();
        let jobs = self
            .store
            .claim_pending_jobs(JOB_KIND_AX_SCREEN, self.config.batch_size)
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
                let msg = e.message;
                if job.attempts >= self.config.max_attempts {
                    self.dead.fetch_add(1, Ordering::Relaxed);
                    let _ = self.store.complete_job(job.id, JobStatus::Dead, Some(&msg));
                    warn!(event = %job.event_id, error = %msg, "ax_screen job dead");
                } else {
                    let delay = retry_delay(job.attempts, self.config.retry_base, self.config.retry_max);
                    let _ = self.store.complete_job_at(
                        job.id,
                        JobStatus::Pending,
                        Some(&msg),
                        Some(Utc::now() + ChronoDuration::milliseconds(delay as i64)),
                    );
                    debug!(event = %job.event_id, attempt = job.attempts, "ax_screen retry scheduled");
                }
            }
        }
    }

    async fn process_job(&self, job: &JobRecord) -> Result<bool, AxJobError> {
        // Read the event payload to get pid (needed by the walker).
        let payload = self
            .store
            .get_event_payload(job.event_id)
            .map_err(|e| AxJobError::transient(e.to_string()))?
            .ok_or_else(|| AxJobError::permanent("event not found".into()))?;

        let pid: i32 = payload
            .get("pid")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32)
            .filter(|n| *n > 0)
            .ok_or_else(|| AxJobError::permanent("screenshot event has no valid pid".into()))?;

        // Skip browsers/Electron apps — their AX providers hang on deep tree
        // traversal. These apps already get URL + title via AppleScript.
        let bundle_id = payload
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if is_ax_problematic_bundle(bundle_id) {
            tracing::debug!(event = %job.event_id, bundle = bundle_id, "skipping AX walk for browser/electron app");
            return Ok(false);
        }

        let snapshot = self
            .walker
            .walk(pid, self.config.walk.clone())
            .await
            .map_err(|e| AxJobError::transient(e.to_string()))?;

        let nonempty = !snapshot.text_content.trim().is_empty();
        let body = ax_body_json(&snapshot, &job.event_id.to_string());

        self.store
            .insert_derived(job.event_id, DERIVED_AX_V1, body)
            .map_err(|e| AxJobError::transient(e.to_string()))?;

        info!(
            event = %job.event_id,
            chars = snapshot.text_content.chars().count(),
            nodes = snapshot.node_count,
            walk_ms = snapshot.walk_duration_ms,
            truncated = snapshot.truncated,
            nonempty,
            "ax.v1 written"
        );
        Ok(nonempty)
    }

    pub async fn run_until_cancelled(&self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        if !self.walker.is_supported() {
            warn!("AX tree walker not supported; worker idle");
            return;
        }
        info!(batch = self.config.batch_size, "AX worker started");
        let _ = self.reclaim_stale();
        loop {
            if *cancel.borrow() {
                break;
            }
            match self.tick_once().await {
                Ok(0) => {}
                Ok(n) => debug!(claimed = n, "ax batch"),
                Err(e) => warn!(error = %e, "ax tick error"),
            }
            tokio::select! {
                _ = cancel.changed() => {
                    if *cancel.borrow() { break; }
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
        // Best-effort drain on shutdown.
        let deadline = tokio::time::Instant::now() + self.config.shutdown_drain;
        while tokio::time::Instant::now() < deadline {
            match self.tick_once().await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => { warn!(error = %e, "ax drain error"); break; }
            }
        }
        info!("AX worker stopped");
    }
}

fn retry_delay(attempts: i64, base: Duration, max: Duration) -> u64 {
    let factor = 2u64.saturating_pow(attempts.min(10) as u32);
    let delay = base.as_millis() as u64 * factor;
    delay.min(max.as_millis() as u64)
}

/// Build the `ax.v1` derived body JSON envelope.
fn ax_body_json(snapshot: &AxTreeSnapshot, event_id: &str) -> String {
    json!({
        "payload_version": 1,
        "text": snapshot.text_content,
        "node_count": snapshot.node_count,
        "content_hash": snapshot.content_hash,
        "walk_duration_ms": snapshot.walk_duration_ms,
        "truncated": snapshot.truncated,
        "app_name": snapshot.app_name,
        "window_title": snapshot.window_title,
        "document_path": snapshot.document_path,
        "browser_url": snapshot.browser_url,
        "event_id": event_id,
    })
    .to_string()
}

/// Bundle-id prefixes for apps whose AX providers hang on deep tree traversal.
/// Browsers get URL + title via AppleScript; Electron apps have unpredictable
/// AX tree depth. Skip these to keep cua stable.
fn is_ax_problematic_bundle(bundle_id: &str) -> bool {
    const SKIP_PREFIXES: &[&str] = &[
        "com.apple.Safari",
        "com.google.Chrome",
        "org.chromium.Chromium",
        "com.microsoft.edgemac",
        "com.brave.Browser",
        "company.thebrowser.Browser", // Arc
        "ai.perplexity.comet",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
        "org.mozilla.firefox",
        "com.tinyspeck.slackmacgap",  // Slack
        "com.microsoft.VSCode",       // VS Code
        "com.microsoft.VSCodeInsiders",
        "md.obsidian",                // Obsidian
        "com.hnc.Discord",            // Discord
        "notion.id",                  // Notion
    ];
    SKIP_PREFIXES.iter().any(|p| bundle_id.starts_with(p))
}

#[derive(Debug)]
struct AxJobError {
    message: String,
    permanent: bool,
}

impl AxJobError {
    fn transient(s: String) -> Self {
        Self { message: s, permanent: false }
    }
    fn permanent(s: String) -> Self {
        Self { message: s, permanent: true }
    }
}

impl From<PlatformError> for AxJobError {
    fn from(e: PlatformError) -> Self {
        AxJobError::transient(e.to_string())
    }
}
