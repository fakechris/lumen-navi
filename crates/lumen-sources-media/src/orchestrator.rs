//! Product CaptureOrchestrator — focus, probe, multi-display, gates, backpressure.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lumen_config::{CaptureConfig, PrivacyConfig};
use lumen_platform::{
    bgra_to_gray, gray_distance, DisplayEnumerator, DisplayInfo, FrontmostApp, FrontmostAppProbe,
    ScreenCapturer, ScreenLockProbe, ScreenshotFrame,
};
use lumen_types::{event_kind, ActivitySession, SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tracing::{debug, info};
use uuid::Uuid;

use crate::session::SessionManager;

#[derive(Debug, Clone)]
pub struct CapturedBatch {
    pub capture_id: Uuid,
    pub session_id: Uuid,
    pub reason: TriggerReason,
    pub frames: Vec<(SourceEvent, ScreenshotFrame)>,
    pub closed_session: Option<ActivitySession>,
    /// Session row to upsert (open).
    pub open_session: Option<ActivitySession>,
}

#[derive(Debug, Default, Clone)]
pub struct CaptureStats {
    pub full_captures: u64,
    pub probes: u64,
    pub skipped_visual: u64,
    pub skipped_debounce: u64,
    pub skipped_gate: u64,
    pub dropped_backpressure: u64,
}

pub struct CaptureOrchestrator {
    displays: Arc<dyn DisplayEnumerator>,
    capturer: Arc<dyn ScreenCapturer>,
    frontmost: Arc<dyn FrontmostAppProbe>,
    lock: Arc<dyn ScreenLockProbe>,
    capture: CaptureConfig,
    privacy: PrivacyConfig,
    /// Runtime overrides (daemon can flip without reload).
    pub paused: AtomicBool,
    pub closed_eyes: AtomicBool,

    last_capture_at: Option<Instant>,
    last_capture_bundle: Option<String>,
    last_focus: Option<FrontmostApp>,
    probe_gray: HashMap<u32, Vec<u8>>,
    sessions: SessionManager,

    stats_full: AtomicU64,
    stats_probes: AtomicU64,
    stats_skip_visual: AtomicU64,
    stats_skip_debounce: AtomicU64,
    stats_skip_gate: AtomicU64,
    stats_drop_bp: AtomicU64,
}

impl CaptureOrchestrator {
    pub fn new(
        displays: Arc<dyn DisplayEnumerator>,
        capturer: Arc<dyn ScreenCapturer>,
        frontmost: Arc<dyn FrontmostAppProbe>,
        lock: Arc<dyn ScreenLockProbe>,
        capture: CaptureConfig,
        privacy: PrivacyConfig,
    ) -> Self {
        let idle = capture.idle_session_ms;
        let paused = AtomicBool::new(privacy.paused);
        let closed_eyes = AtomicBool::new(privacy.closed_eyes);
        Self {
            displays,
            capturer,
            frontmost,
            lock,
            capture,
            privacy,
            paused,
            closed_eyes,
            last_capture_at: None,
            last_capture_bundle: None,
            last_focus: None,
            probe_gray: HashMap::new(),
            sessions: SessionManager::new(idle),
            stats_full: AtomicU64::new(0),
            stats_probes: AtomicU64::new(0),
            stats_skip_visual: AtomicU64::new(0),
            stats_skip_debounce: AtomicU64::new(0),
            stats_skip_gate: AtomicU64::new(0),
            stats_drop_bp: AtomicU64::new(0),
        }
    }

    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            full_captures: self.stats_full.load(Ordering::Relaxed),
            probes: self.stats_probes.load(Ordering::Relaxed),
            skipped_visual: self.stats_skip_visual.load(Ordering::Relaxed),
            skipped_debounce: self.stats_skip_debounce.load(Ordering::Relaxed),
            skipped_gate: self.stats_skip_gate.load(Ordering::Relaxed),
            dropped_backpressure: self.stats_drop_bp.load(Ordering::Relaxed),
        }
    }

    pub fn set_paused(&self, v: bool) {
        self.paused.store(v, Ordering::Relaxed);
    }

    pub fn set_closed_eyes(&self, v: bool) {
        self.closed_eyes.store(v, Ordering::Relaxed);
    }

    pub fn note_backpressure_drop(&self) {
        self.stats_drop_bp.fetch_add(1, Ordering::Relaxed);
    }

    pub fn force_close_session(&mut self) -> Option<ActivitySession> {
        self.sessions.force_close()
    }

    pub fn close_idle_session(&mut self) -> Option<ActivitySession> {
        self.sessions.close_if_idle()
    }

    /// Poll frontmost app; returns a focus/title trigger if changed.
    pub async fn poll_focus_trigger(&mut self) -> Option<TriggerReason> {
        let cur = self.frontmost.frontmost().await.ok().flatten()?;
        let reason = match &self.last_focus {
            None => {
                self.last_focus = Some(cur);
                return None; // establish baseline without force capture
            }
            Some(prev) => {
                let bundle_changed = prev.bundle_id != cur.bundle_id || prev.app_name != cur.app_name;
                let title_changed = prev.window_title != cur.window_title;
                if bundle_changed {
                    Some(TriggerReason::FocusChange)
                } else if title_changed {
                    Some(TriggerReason::TitleChange)
                } else {
                    None
                }
            }
        };
        if reason.is_some() {
            self.last_focus = Some(cur);
        }
        reason
    }

    /// Run one capture decision for `reason`. Returns None if gated/skipped.
    pub async fn capture_tick(
        &mut self,
        reason: TriggerReason,
    ) -> Result<Option<CapturedBatch>, String> {
        if self.paused.load(Ordering::Relaxed) || self.privacy.paused {
            self.stats_skip_gate.fetch_add(1, Ordering::Relaxed);
            debug!("gate: paused");
            return Ok(None);
        }
        if self.closed_eyes.load(Ordering::Relaxed) || self.privacy.closed_eyes {
            self.stats_skip_gate.fetch_add(1, Ordering::Relaxed);
            debug!("gate: closed_eyes");
            return Ok(None);
        }
        if self.lock.is_locked().await.unwrap_or(false) {
            self.stats_skip_gate.fetch_add(1, Ordering::Relaxed);
            debug!("gate: screen_locked");
            return Ok(None);
        }

        let front = self.frontmost.frontmost().await.ok().flatten();
        let bundle = front.as_ref().and_then(|f| f.bundle_id.clone());

        if !self.allow_debounce(reason, bundle.as_deref()) {
            self.stats_skip_debounce.fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        }

        let displays = self.select_displays().await?;
        if displays.is_empty() {
            return Err("no displays".into());
        }

        let mut max_distance = 0.0f64;
        if !reason.forces_full_capture() {
            let mut any_change = false;
            for d in &displays {
                self.stats_probes.fetch_add(1, Ordering::Relaxed);
                let raw = self
                    .capturer
                    .capture_display_raw(d.id, self.capture.probe_scale)
                    .await
                    .map_err(|e| e.to_string())?;
                let gray = bgra_to_gray(&raw);
                let dist = match self.probe_gray.get(&d.id.0) {
                    Some(prev) => gray_distance(prev, &gray),
                    None => 1.0, // first probe always "changed"
                };
                max_distance = max_distance.max(dist);
                if dist >= self.capture.visual_change_threshold {
                    any_change = true;
                }
                self.probe_gray.insert(d.id.0, gray);
            }
            if !any_change {
                self.stats_skip_visual.fetch_add(1, Ordering::Relaxed);
                debug!(max_distance, "skip: visual stable");
                return Ok(None);
            }
        } else {
            // Still refresh probe buffers on force path for future interval ticks.
            for d in &displays {
                if let Ok(raw) = self
                    .capturer
                    .capture_display_raw(d.id, self.capture.probe_scale)
                    .await
                {
                    self.probe_gray.insert(d.id.0, bgra_to_gray(&raw));
                }
            }
            max_distance = 1.0;
        }

        let capture_id = Uuid::new_v4();
        let app_name = front.as_ref().map(|f| f.app_name.as_str());
        let bundle_s = front.as_ref().and_then(|f| f.bundle_id.as_deref());
        let (session_id, closed_session) =
            self.sessions
                .touch(app_name, bundle_s, reason.as_str());
        let open_session = self.sessions.current().cloned();

        let mut frames = Vec::new();
        for (index, d) in displays.iter().enumerate() {
            let frame = self
                .capturer
                .capture_display(
                    d.id,
                    self.capture.screen_max_edge,
                    self.capture.use_jpeg(),
                    self.capture.jpeg_quality,
                )
                .await
                .map_err(|e| e.to_string())?;

            let payload = json!({
                "payload_version": 1,
                "reason": reason.as_str(),
                "app_name": front.as_ref().map(|f| &f.app_name),
                "bundle_id": front.as_ref().and_then(|f| f.bundle_id.as_ref()),
                "window_title": front.as_ref().and_then(|f| f.window_title.as_ref()),
                "display_id": d.id.0,
                "display_index": index,
                "is_main": d.is_main,
                "display_origin": [d.origin_x, d.origin_y],
                "width": frame.width,
                "height": frame.height,
                "probe_distance": max_distance,
                "capture_id": capture_id,
                "bytes": frame.png_or_jpeg_bytes.len(),
                "media_type": frame.media_type,
            });

            let event = SourceEvent::new(SourceKind::Screen, event_kind::SCREENSHOT_V1, payload)
                .with_session(session_id);
            frames.push((event, frame));
        }

        self.last_capture_at = Some(Instant::now());
        self.last_capture_bundle = bundle;
        self.stats_full.fetch_add(1, Ordering::Relaxed);

        info!(
            reason = reason.as_str(),
            displays = frames.len(),
            %session_id,
            %capture_id,
            "full capture batch"
        );

        Ok(Some(CapturedBatch {
            capture_id,
            session_id,
            reason,
            frames,
            closed_session,
            open_session,
        }))
    }

    fn allow_debounce(&self, reason: TriggerReason, bundle: Option<&str>) -> bool {
        let Some(last) = self.last_capture_at else {
            return true;
        };
        let elapsed = last.elapsed();
        let min = if reason.is_churn() {
            Duration::from_millis(self.capture.debounce_churn_ms)
        } else if reason.forces_full_capture() {
            Duration::from_millis(self.capture.debounce_default_ms)
        } else {
            Duration::from_millis(self.capture.debounce_default_ms)
        };
        if elapsed < min {
            return false;
        }
        // Same-app throttle for non-force reasons
        if !reason.forces_full_capture() {
            if let (Some(prev), Some(b)) = (&self.last_capture_bundle, bundle) {
                if prev == b
                    && elapsed < Duration::from_millis(self.capture.same_app_min_ms)
                {
                    return false;
                }
            }
        }
        true
    }

    async fn select_displays(&self) -> Result<Vec<DisplayInfo>, String> {
        let mut list = self.displays.list_displays().await.map_err(|e| e.to_string())?;
        if !self.capture.all_displays() {
            list.retain(|d| d.is_main);
            if list.is_empty() {
                // fall back to first
                list = self.displays.list_displays().await.map_err(|e| e.to_string())?;
                list.truncate(1);
            }
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lumen_platform::{
        DisplayId, DisplayInfo, FrontmostApp, PlatformError, RawFrame, ScreenCapturer,
        ScreenshotFrame,
    };
    use std::sync::Mutex;

    struct FakeDisplays;
    #[async_trait]
    impl DisplayEnumerator for FakeDisplays {
        async fn list_displays(&self) -> Result<Vec<DisplayInfo>, PlatformError> {
            Ok(vec![
                DisplayInfo {
                    id: DisplayId(1),
                    width: 100,
                    height: 100,
                    origin_x: 0,
                    origin_y: 0,
                    is_main: true,
                },
                DisplayInfo {
                    id: DisplayId(2),
                    width: 80,
                    height: 80,
                    origin_x: 100,
                    origin_y: 0,
                    is_main: false,
                },
            ])
        }
    }

    struct FakeFront {
        app: Mutex<FrontmostApp>,
    }
    #[async_trait]
    impl FrontmostAppProbe for FakeFront {
        async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
            Ok(Some(self.app.lock().unwrap().clone()))
        }
    }

    struct FakeLock;
    #[async_trait]
    impl ScreenLockProbe for FakeLock {
        async fn is_locked(&self) -> Result<bool, PlatformError> {
            Ok(false)
        }
    }

    struct FakeCap {
        /// Increment gray by this each probe for display 1
        n: Mutex<u8>,
    }
    #[async_trait]
    impl ScreenCapturer for FakeCap {
        async fn capture_display(
            &self,
            id: DisplayId,
            _max_edge: u32,
            _jpeg: bool,
            _q: u8,
        ) -> Result<ScreenshotFrame, PlatformError> {
            Ok(ScreenshotFrame {
                png_or_jpeg_bytes: vec![1, 2, 3, id.0 as u8],
                media_type: "image/jpeg".into(),
                width: 10,
                height: 10,
                display_id: id,
            })
        }

        async fn capture_display_raw(
            &self,
            id: DisplayId,
            _scale_div: u32,
        ) -> Result<RawFrame, PlatformError> {
            let n = self.n.lock().unwrap();
            let v = *n;
            let mut bgra = vec![0u8; 4 * 4]; // 2x2
            for px in bgra.chunks_exact_mut(4) {
                px[0] = v;
                px[1] = v;
                px[2] = v;
                px[3] = 255;
            }
            Ok(RawFrame {
                bgra,
                width: 2,
                height: 2,
                bytes_per_row: 8,
                display_id: id,
            })
        }
    }

    fn orch(cap: FakeCap) -> CaptureOrchestrator {
        CaptureOrchestrator::new(
            Arc::new(FakeDisplays),
            Arc::new(cap),
            Arc::new(FakeFront {
                app: Mutex::new(FrontmostApp {
                    app_name: "A".into(),
                    bundle_id: Some("a.b".into()),
                    window_title: None,
                }),
            }),
            Arc::new(FakeLock),
            CaptureConfig {
                visual_change_threshold: 0.05,
                debounce_default_ms: 0,
                debounce_churn_ms: 0,
                same_app_min_ms: 0,
                probe_scale: 1,
                displays: "all".into(),
                ..CaptureConfig::default()
            },
            PrivacyConfig::default(),
        )
    }

    #[tokio::test]
    async fn interval_skips_when_visual_stable() {
        let mut o = orch(FakeCap { n: Mutex::new(10) });
        let first = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(first.is_some());
        assert_eq!(first.unwrap().frames.len(), 2); // dual display
        let second = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn focus_forces_capture() {
        let mut o = orch(FakeCap { n: Mutex::new(10) });
        let _ = o.capture_tick(TriggerReason::Interval).await.unwrap();
        let forced = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        assert!(forced.is_some());
    }

    #[tokio::test]
    async fn closed_eyes_blocks() {
        let mut o = orch(FakeCap { n: Mutex::new(1) });
        o.set_closed_eyes(true);
        let r = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        assert!(r.is_none());
    }
}
