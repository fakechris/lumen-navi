//! Media intake — product Observe capture orchestrator.
//!
//! See `docs/OBSERVE_CAPTURE.md`. OCR is intentionally out of scope here.

mod orchestrator;
mod session;

pub use orchestrator::{CaptureOrchestrator, CaptureStats, CapturedBatch};
pub use session::SessionManager;

// Keep thin AudioSource shell for S3.
use async_trait::async_trait;
use lumen_intake::{IntakeError, Source};

pub struct AudioSource {
    running: bool,
}

impl AudioSource {
    pub fn new() -> Self {
        Self { running: false }
    }
}

impl Default for AudioSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Source for AudioSource {
    fn id(&self) -> &str {
        "audio"
    }

    async fn start(&mut self) -> Result<(), IntakeError> {
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), IntakeError> {
        self.running = false;
        Ok(())
    }

    async fn poll(&mut self) -> Result<Vec<lumen_types::SourceEvent>, IntakeError> {
        if !self.running {
            return Err(IntakeError::NotRunning("audio".into()));
        }
        Ok(vec![])
    }
}
