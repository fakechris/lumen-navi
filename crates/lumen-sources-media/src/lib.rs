//! Media intake adapters (screen / audio / video).
//!
//! Screen is the first real source: capture → optional dedup → event + PNG blob.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use lumen_config::CaptureConfig;
use lumen_intake::{IntakeError, Source};
use lumen_platform::{FrontmostAppProbe, ScreenCapturer, ScreenshotFrame};
use lumen_types::{event_kind, SourceEvent, SourceKind};
use serde_json::json;
use tracing::{debug, info};

/// Event ready to persist, with optional pending blob bytes.
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    pub event: SourceEvent,
    /// `(media_type, bytes)` when a new blob must be written.
    pub blob: Option<(String, Vec<u8>)>,
}

/// Interval screen source with pixel-hash dedup.
pub struct ScreenSource {
    capturer: Arc<dyn ScreenCapturer>,
    frontmost: Arc<dyn FrontmostAppProbe>,
    interval_ms: u64,
    dedup_window: Duration,
    running: bool,
    synthetic: bool,
    last_hash: Option<String>,
    last_hash_at: Option<Instant>,
    ticks: u64,
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
            dedup_window: Duration::from_millis(capture.screen_dedup_window_ms),
            running: false,
            synthetic: false,
            last_hash: None,
            last_hash_at: None,
            ticks: 0,
        }
    }

    /// Test helper: emit one synthetic `screenshot.v1` per poll while running.
    pub fn with_synthetic(mut self, on: bool) -> Self {
        self.synthetic = on;
        self
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// One capture cycle for the real media path (preferred over [`Source::poll`]).
    pub async fn capture_tick(&mut self) -> Result<Vec<CapturedEvent>, IntakeError> {
        if !self.running {
            return Err(IntakeError::NotRunning("screen".into()));
        }
        self.ticks += 1;

        if self.synthetic {
            return Ok(vec![CapturedEvent {
                event: self.synthetic_event("synthetic"),
                blob: None,
            }]);
        }

        let frame = match self.capturer.capture_main_display().await {
            Ok(f) => f,
            Err(lumen_platform::PlatformError::Unsupported(_)) => {
                debug!("screen capturer unsupported; skip tick");
                return Ok(vec![]);
            }
            Err(lumen_platform::PlatformError::PermissionDenied(msg)) => {
                return Err(IntakeError::Source(format!("permission denied: {msg}")));
            }
            Err(e) => return Err(IntakeError::Source(e.to_string())),
        };

        let hash = pixel_hash(&frame);
        if self.should_dedup(&hash) {
            debug!(%hash, "screenshot deduped");
            return Ok(vec![]);
        }
        self.last_hash = Some(hash.clone());
        self.last_hash_at = Some(Instant::now());

        let front = self.frontmost.frontmost().await.ok().flatten();
        let payload = json!({
            "payload_version": 1,
            "app_name": front.as_ref().map(|f| f.app_name.clone()),
            "bundle_id": front.as_ref().and_then(|f| f.bundle_id.clone()),
            "window_title": front.as_ref().and_then(|f| f.window_title.clone()),
            "display_id": frame.display_id,
            "width": frame.width,
            "height": frame.height,
            "pixel_hash": hash,
            "reason": "interval",
            "interval_ms": self.interval_ms,
            "bytes": frame.png_bytes.len(),
        });

        let event = SourceEvent::new(SourceKind::Screen, event_kind::SCREENSHOT_V1, payload);
        info!(
            tick = self.ticks,
            w = frame.width,
            h = frame.height,
            png = frame.png_bytes.len(),
            "screen capture"
        );

        Ok(vec![CapturedEvent {
            event,
            blob: Some(("image/png".into(), frame.png_bytes)),
        }])
    }

    fn should_dedup(&self, hash: &str) -> bool {
        match (&self.last_hash, self.last_hash_at) {
            (Some(prev), Some(at)) if prev == hash && at.elapsed() < self.dedup_window => true,
            _ => false,
        }
    }

    fn synthetic_event(&self, reason: &str) -> SourceEvent {
        SourceEvent::new(
            SourceKind::Screen,
            event_kind::SCREENSHOT_V1,
            json!({
                "payload_version": 1,
                "reason": reason,
                "interval_ms": self.interval_ms,
            }),
        )
    }
}

fn pixel_hash(frame: &ScreenshotFrame) -> String {
    blake3::hash(&frame.png_bytes).to_hex().to_string()
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
        // Trait path: events only (no blob). Prefer `capture_tick` for media.
        let batch = self.capture_tick().await?;
        Ok(batch.into_iter().map(|c| c.event).collect())
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
    use lumen_platform::{NullFrontmost, PlatformError};

    struct FakeCapturer {
        n: std::sync::Mutex<u32>,
    }

    #[async_trait]
    impl ScreenCapturer for FakeCapturer {
        async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError> {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            // Same PNG every time → dedup after first
            Ok(ScreenshotFrame {
                png_bytes: b"\x89PNG\r\n\x1a\nfake".to_vec(),
                width: 10,
                height: 10,
                display_id: Some(1),
            })
        }
    }

    #[tokio::test]
    async fn capture_tick_emits_then_dedups() {
        let mut src = ScreenSource::new(
            Arc::new(FakeCapturer {
                n: std::sync::Mutex::new(0),
            }),
            Arc::new(NullFrontmost),
            &CaptureConfig {
                screen_interval_ms: 100,
                screen_dedup_window_ms: 60_000,
                screen_max_edge: 1920,
                screen_ticks: 3,
            },
        );
        src.start().await.unwrap();
        let first = src.capture_tick().await.unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].blob.is_some());
        let second = src.capture_tick().await.unwrap();
        assert!(second.is_empty(), "same hash within window should dedup");
    }

    #[tokio::test]
    async fn synthetic_screen_emits_event() {
        struct Boom;
        #[async_trait]
        impl ScreenCapturer for Boom {
            async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError> {
                Err(PlatformError::Unsupported("test".into()))
            }
        }

        let mut src = ScreenSource::new(
            Arc::new(Boom),
            Arc::new(NullFrontmost),
            &CaptureConfig {
                screen_interval_ms: 3000,
                screen_dedup_window_ms: 5000,
                screen_max_edge: 1920,
                screen_ticks: 3,
            },
        )
        .with_synthetic(true);
        src.start().await.unwrap();
        let batch = src.poll().await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, event_kind::SCREENSHOT_V1);
    }
}
