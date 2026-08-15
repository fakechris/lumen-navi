//! Product CaptureOrchestrator — focus, probe, multi-display, gates, backpressure.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lumen_config::{CaptureConfig, PrivacyConfig};
use lumen_platform::{
    bgra_to_gray, dhash, gray_distance, hamming64, DisplayEnumerator, DisplayInfo, DisplaySleepProbe,
    FrontmostApp, FrontmostAppProbe, IdleProbe, ScreenCapturer, ScreenLockProbe, ScreenshotFrame,
};
use lumen_types::{event_kind, ActivitySession, SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tracing::{debug, info};
use uuid::Uuid;

use crate::activity::{ActivityAccumulator, ActivitySample};
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
    pub skipped_near_duplicate: u64,
    pub skipped_debounce: u64,
    pub skipped_gate: u64,
    pub dropped_backpressure: u64,
}

pub struct CaptureOrchestrator {
    displays: Arc<dyn DisplayEnumerator>,
    capturer: Arc<dyn ScreenCapturer>,
    frontmost: Arc<dyn FrontmostAppProbe>,
    lock: Arc<dyn ScreenLockProbe>,
    idle: Arc<dyn IdleProbe>,
    power: Arc<dyn DisplaySleepProbe>,
    capture: CaptureConfig,
    privacy: PrivacyConfig,
    /// Runtime overrides (daemon can flip without reload).
    pub paused: AtomicBool,
    pub closed_eyes: AtomicBool,

    last_capture_at: Option<Instant>,
    last_capture_bundle: Option<String>,
    last_focus: Option<FrontmostApp>,
    probe_gray: HashMap<u32, Vec<u8>>,
    /// dHash fingerprints of recent frames per display (near-duplicate detection).
    dhash_history: HashMap<u32, VecDeque<(u64, Instant)>>,
    /// Safety valve: last full capture per display (force one every 10s).
    last_full_capture: HashMap<u32, Instant>,
    sessions: SessionManager,
    activity: ActivityAccumulator,

    stats_full: AtomicU64,
    stats_probes: AtomicU64,
    stats_skip_visual: AtomicU64,
    stats_skip_near_dup: AtomicU64,
    stats_skip_debounce: AtomicU64,
    stats_skip_gate: AtomicU64,
    stats_drop_bp: AtomicU64,
}

const DHASH_HISTORY_LEN: usize = 12;
const DHASH_HAMMING_THRESHOLD: u32 = 5;
/// Heartbeat interval for a visually-static screen: even when the MAD probe
/// reports no change (a browser page being read, a paused video, a PDF), force
/// one capture this often so the user still gets occasional evidence frames.
///
/// Tuned to 2 minutes. At the previous 10s this fired every 10s on a static
/// browser page (Safari reading), producing ~6 screenshots/minute that crowded
/// out every other app in the 60-item timeline ("Comet never shows up"). A
/// 2-minute cadence is enough to record "user still on this page" without
/// flooding storage or the timeline. Dynamic activity still captures at its
/// own cadence (same_app_min_ms / visual_change_threshold), unaffected.
const DHASH_SAFETY_VALVE: Duration = Duration::from_secs(120);
const DHASH_HISTORY_TTL: Duration = Duration::from_secs(60);

impl CaptureOrchestrator {
    pub fn new(
        displays: Arc<dyn DisplayEnumerator>,
        capturer: Arc<dyn ScreenCapturer>,
        frontmost: Arc<dyn FrontmostAppProbe>,
        lock: Arc<dyn ScreenLockProbe>,
        idle: Arc<dyn IdleProbe>,
        power: Arc<dyn DisplaySleepProbe>,
        capture: CaptureConfig,
        privacy: PrivacyConfig,
    ) -> Self {
        let idle_session_ms = capture.idle_session_ms;
        let paused = AtomicBool::new(privacy.paused);
        let closed_eyes = AtomicBool::new(privacy.closed_eyes);
        // AFK threshold mirrors the existing session idle; heartbeats every 5s
        // so a steady 30-minute reading session still closes its segment.
        let activity = ActivityAccumulator::new(
            Duration::from_millis(idle_session_ms.max(1)),
            Duration::from_secs(5),
        );
        Self {
            displays,
            capturer,
            frontmost,
            lock,
            idle,
            power,
            capture,
            privacy,
            paused,
            closed_eyes,
            last_capture_at: None,
            last_capture_bundle: None,
            last_focus: None,
            probe_gray: HashMap::new(),
            dhash_history: HashMap::new(),
            last_full_capture: HashMap::new(),
            sessions: SessionManager::new(idle_session_ms),
            activity,
            stats_full: AtomicU64::new(0),
            stats_probes: AtomicU64::new(0),
            stats_skip_visual: AtomicU64::new(0),
            stats_skip_near_dup: AtomicU64::new(0),
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
            skipped_near_duplicate: self.stats_skip_near_dup.load(Ordering::Relaxed),
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

    pub fn drain_session_lifecycle(&mut self) -> Vec<lumen_types::SourceEvent> {
        self.sessions.drain_lifecycle()
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

    /// Sample the current frontmost app + system idle and return a lightweight
    /// `activity.focus.v1` event when the accumulator decides a row is worth
    /// keeping (state change or heartbeat due). Independent of the screenshot
    /// path's visual debounce, so reading a static page still accrues time.
    ///
    /// Respects pause/closed-eyes (no event emitted while user opted out) but
    /// **not** screen-lock (a lock is itself meaningful activity context, so we
    /// record it with `is_locked=true`).
    pub async fn poll_activity(&mut self) -> Vec<SourceEvent> {
        if self.paused.load(Ordering::Relaxed) || self.privacy.paused {
            return Vec::new();
        }
        if self.closed_eyes.load(Ordering::Relaxed) || self.privacy.closed_eyes {
            return Vec::new();
        }

        let frontmost = self.frontmost.frontmost().await.ok().flatten();
        let idle_seconds = self.idle.idle_seconds().await.unwrap_or(0.0).max(0.0);
        let is_locked = self.lock.is_locked().await.unwrap_or(false);
        let display_sleep_prevented = self.power.display_sleep_prevented().await.unwrap_or(false);

        if let Some(ref front) = frontmost {
            if !is_locked {
                let _ = self.sessions.touch(
                    Some(front.app_name.as_str()),
                    front.bundle_id.as_deref(),
                    "focus_change",
                );
            }
        }

        let tick = self.activity.ingest_detailed(
            ActivitySample {
                frontmost,
                idle_seconds,
                is_locked,
                display_sleep_prevented,
            },
            chrono::Utc::now(),
        );
        let sid = self.sessions.current().map(|s| s.id);
        let bind = |mut ev: SourceEvent| {
            if let Some(id) = sid {
                ev.session_id = Some(id);
            }
            ev
        };
        let mut out = self.sessions.drain_lifecycle();
        if let Some(ev) = tick.window_changed {
            out.push(bind(ev));
        }
        if let Some(ev) = tick.focus {
            out.push(bind(ev));
        }
        out
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
        // dHash of each display's current probe (for recording into history on capture).
        let mut probe_hashes: HashMap<u32, u64> = HashMap::new();
        if !reason.forces_full_capture() {
            let mut any_change = false;
            let mut skipped_near_dup = false;
            let now = Instant::now();
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
                self.probe_gray.insert(d.id.0, gray.clone());

                // Safety valve must be evaluated BEFORE the MAD stable-skip
                // below: a visually static frame (a browser showing a fixed
                // article, a paused video, a PDF) never exceeds the MAD
                // threshold, so the old code `continue`d here and the safety
                // valve — written after the continue — was unreachable. That
                // meant a stable frame produced zero screenshots indefinitely.
                // Evaluating safety_due first guarantees a heartbeat capture
                // every DHASH_SAFETY_VALVE regardless of visual change.
                let safety_due = self
                    .last_full_capture
                    .get(&d.id.0)
                    .is_none_or(|t| now.duration_since(*t) >= DHASH_SAFETY_VALVE);

                if dist < self.capture.visual_change_threshold && !safety_due {
                    continue; // MAD stable and not overdue — no change
                }

                if safety_due {
                    // Force a heartbeat capture; skip the near-dup check so a
                    // long-static screen still captures on schedule.
                    any_change = true;
                    continue;
                }

                // MAD says changed. Second layer: dHash near-duplicate check.
                let hash = dhash(&gray, raw.width as usize, raw.height as usize);
                probe_hashes.insert(d.id.0, hash);

                let history = self.dhash_history.entry(d.id.0).or_default();
                while history.front().is_some_and(|(_, t)| now.duration_since(*t) > DHASH_HISTORY_TTL) {
                    history.pop_front();
                }
                let near_dup = history
                    .iter()
                    .any(|(h, _)| hamming64(hash, *h) <= DHASH_HAMMING_THRESHOLD);

                if near_dup {
                    skipped_near_dup = true;
                } else {
                    any_change = true;
                }
            }

            if !any_change {
                if skipped_near_dup {
                    self.stats_skip_near_dup.fetch_add(1, Ordering::Relaxed);
                    debug!(max_distance, "skip: near-duplicate");
                } else {
                    self.stats_skip_visual.fetch_add(1, Ordering::Relaxed);
                    debug!(max_distance, "skip: visual stable");
                }
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
                "pid": front.as_ref().and_then(|f| f.pid),
                "window_id": front.as_ref().and_then(|f| f.window_id),
                "window_title": front.as_ref().and_then(|f| f.window_title.as_ref()),
                "url": front.as_ref().and_then(|f| f.tab_url.as_ref()),
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

        // Record captured frame dHashes into history + update safety valve.
        // On force path (focus/manual), probe_hashes is empty — next interval
        // tick will establish a new dHash baseline.
        let now = Instant::now();
        for d in &displays {
            if let Some(&hash) = probe_hashes.get(&d.id.0) {
                let history = self.dhash_history.entry(d.id.0).or_default();
                history.push_back((hash, now));
                while history.len() > DHASH_HISTORY_LEN {
                    history.pop_front();
                }
            }
            self.last_full_capture.insert(d.id.0, now);
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

    struct FakeIdle;
    #[async_trait]
    impl IdleProbe for FakeIdle {
        async fn idle_seconds(&self) -> Result<f64, PlatformError> {
            Ok(0.0)
        }
    }

    struct FakePower;
    #[async_trait]
    impl DisplaySleepProbe for FakePower {
        async fn display_sleep_prevented(&self) -> Result<bool, PlatformError> {
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
                    ls_category_type: None,
                    tab_url: None,
                    pid: None,
                    window_id: None,
                }),
            }),
            Arc::new(FakeLock),
            Arc::new(FakeIdle),
            Arc::new(FakePower),
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

    /// Regression: a visually static screen (browser showing a fixed page,
    /// paused video, PDF) used to never capture because the MAD `continue`
    /// fired before the safety valve was consulted. The safety valve must
    /// force a heartbeat every DHASH_SAFETY_VALVE even when MAD is stable.
    #[tokio::test]
    async fn safety_valve_captures_static_screen_when_overdue() {
        // FakeCap returns the same gray value every probe -> MAD is always 0
        // below threshold after the first capture, i.e. a perfectly static frame.
        let mut o = orch(FakeCap { n: Mutex::new(10) });

        // First tick captures (establishes baseline).
        let first = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(first.is_some(), "first tick should capture");

        // Second tick, immediately after: stable + not overdue -> skip.
        let second = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(second.is_none(), "stable frame within safety window should skip");

        // Pretend DHASH_SAFETY_VALVE has elapsed by backdating last_full_capture.
        for d in o.select_displays().await.unwrap() {
            o.last_full_capture.insert(
                d.id.0,
                Instant::now() - Duration::from_secs(121),
            );
        }

        // Third tick, same static screen but safety valve overdue -> MUST capture.
        let third = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(
            third.is_some(),
            "safety valve must force a capture even on a static screen when overdue"
        );
        assert_eq!(
            third.unwrap().frames.len(),
            2,
            "heartbeat should capture all displays"
        );
    }
}
