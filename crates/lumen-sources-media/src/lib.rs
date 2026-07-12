//! Media intake adapters (screen / audio / video).
//!
//! These are the **first** real sources. Implementations call `lumen-platform`
//! ports and emit versioned `SourceEvent`s (`screenshot.v1`, etc.).

use std::sync::Arc;

use async_trait::async_trait;
use lumen_config::CaptureConfig;
use lumen_intake::{IntakeError, Source};
use lumen_platform::{FrontmostAppProbe, ScreenCapturer};
use lumen_types::{SourceEvent, SourceKind};
use serde_json::json;

/// Interval screen source. Real capture uses [`ScreenCapturer`]; when capture
/// is unsupported, `poll` returns empty (degraded) rather than crashing.
pub struct ScreenSource {
    capturer: Arc<dyn ScreenCapturer>,
    frontmost: Arc<dyn FrontmostAppProbe>,
    interval_ms: u64,
    running: bool,
    /// When true, emit a synthetic event for pipeline tests without OS capture.
    synthetic: bool,
}

impl ScreenSource {
    pub fn new(
        capturer: Arc<dyn ScreenCapturer>,
        frontmost: Arc<dyn FrontmostAppProbe>,
        capture: &CaptureConfig,
    ) -> Self {
        Self {
            capturer,
            frontmost,
            interval_ms: capture.screen_interval_ms,
            running: false,
            synthetic: false,
        }
    }

    /// Test helper: emit one synthetic `screenshot.v1` per poll while running.
    pub fn with_synthetic(mut self, on: bool) -> Self {
        self.synthetic = on;
        self
    }
}

#[async_trait]
impl Source for ScreenSource {
    fn id(&self) -> &str {
        "screen"
    }

    async fn start(&mut self) -> Result<(), IntakeError> {
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), IntakeError> {
        self.running = false;
        Ok(())
    }

    async fn poll(&mut self) -> Result<Vec<SourceEvent>, IntakeError> {
        if !self.running {
            return Err(IntakeError::NotRunning("screen".into()));
        }

        if self.synthetic {
            let front = self.frontmost.frontmost().await.ok().flatten();
            let payload = json!({
                "payload_version": 1,
                "app_name": front.as_ref().map(|f| f.app_name.clone()),
                "bundle_id": front.as_ref().and_then(|f| f.bundle_id.clone()),
                "window_title": front.as_ref().and_then(|f| f.window_title.clone()),
                "reason": "synthetic",
                "interval_ms": self.interval_ms,
            });
            return Ok(vec![SourceEvent::new(
                SourceKind::Screen,
                "screenshot.v1",
                payload,
            )]);
        }

        match self.capturer.capture_main_display().await {
            Ok(_frame) => {
                // S2: write blob + full payload. Skeleton only probes the port.
                Ok(vec![])
            }
            Err(lumen_platform::PlatformError::Unsupported(_)) => Ok(vec![]),
            Err(e) => Err(IntakeError::Source(e.to_string())),
        }
    }
}

/// Placeholder audio source shell (S3).
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

    async fn poll(&mut self) -> Result<Vec<SourceEvent>, IntakeError> {
        if !self.running {
            return Err(IntakeError::NotRunning("audio".into()));
        }
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_config::CaptureConfig;
    use lumen_platform::{NullFrontmost, PlatformError, ScreenshotFrame};

    struct BoomCapturer;

    #[async_trait]
    impl ScreenCapturer for BoomCapturer {
        async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError> {
            Err(PlatformError::Unsupported("test".into()))
        }
    }

    #[tokio::test]
    async fn synthetic_screen_emits_event() {
        let mut src = ScreenSource::new(
            Arc::new(BoomCapturer),
            Arc::new(NullFrontmost),
            &CaptureConfig {
                screen_interval_ms: 3000,
                screen_dedup_window_ms: 5000,
            },
        )
        .with_synthetic(true);
        src.start().await.unwrap();
        let batch = src.poll().await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, "screenshot.v1");
    }
}
