//! Product CaptureOrchestrator — focus, probe, multi-display, gates, backpressure.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lumen_config::{CaptureConfig, PolicyGate, PrivacyConfig};
use lumen_platform::{
    bgra_to_gray, dhash, gray_distance, hamming64, DisplayEnumerator, DisplayInfo,
    DisplaySleepProbe, FrontmostApp, FrontmostAppProbe, IdleProbe, ScreenCapturer, ScreenLockProbe,
    ScreenshotFrame,
};
use lumen_types::{event_kind, ActivitySession, SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tracing::{debug, info};
use uuid::Uuid;

use crate::activity::{ActivityAccumulator, ActivitySample};
use crate::session::{SessionTransition, SharedSessionBinder};

#[derive(Debug, Clone)]
pub struct CapturedBatch {
    pub capture_id: Uuid,
    pub session_id: Uuid,
    pub reason: TriggerReason,
    pub frames: Vec<(SourceEvent, ScreenshotFrame)>,
    pub closed_session: Option<ActivitySession>,
    /// Session row to upsert (open).
    pub open_session: Option<ActivitySession>,
    /// Lifecycle facts from the capture-time bind (ended/started).
    pub session_events: Vec<SourceEvent>,
}

/// One discardable safety-valve frame. Not a `screenshot.v1` event.
#[derive(Debug, Clone)]
pub struct LivenessFrame {
    pub display_id: u32,
    pub display_index: usize,
    pub is_main: bool,
    pub width: u32,
    pub height: u32,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Overwrite-only proof that the capture loop is still running.
#[derive(Debug, Clone)]
pub struct LivenessSnapshot {
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub frames: Vec<LivenessFrame>,
}

#[derive(Debug, Clone)]
pub enum CaptureOutcome {
    /// Evidence: session bind, blob, OCR/AX, timeline.
    Evidence(CapturedBatch),
    /// Safety valve: last frame only, no processing.
    Liveness(LivenessSnapshot),
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
    pub liveness_captures: u64,
}

#[derive(Debug, Default)]
pub struct ActivityPoll {
    pub events: Vec<SourceEvent>,
    pub upsert_sessions: Vec<ActivitySession>,
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
    sessions: Arc<SharedSessionBinder>,
    activity: ActivityAccumulator,

    stats_full: AtomicU64,
    stats_probes: AtomicU64,
    stats_skip_visual: AtomicU64,
    stats_skip_near_dup: AtomicU64,
    stats_skip_debounce: AtomicU64,
    stats_skip_gate: AtomicU64,
    stats_drop_bp: AtomicU64,
    stats_liveness: AtomicU64,
}

const DHASH_HISTORY_LEN: usize = 12;
const DHASH_HAMMING_THRESHOLD: u32 = 5;
/// Heartbeat interval for a visually-static screen: even when the MAD probe
/// reports no change, grab one overwrite-only liveness frame so we know the
/// capture loop is still alive. This is not evidence — it does not bind a
/// session, write `screenshot.v1`, or enqueue OCR/AX.
///
/// Tuned to 2 minutes. Dynamic activity still captures at its own cadence
/// (same_app_min_ms / visual_change_threshold), unaffected.
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
        Self::with_sessions(
            displays,
            capturer,
            frontmost,
            lock,
            idle,
            power,
            capture,
            privacy,
            SharedSessionBinder::new(idle_session_ms),
        )
    }

    pub fn with_sessions(
        displays: Arc<dyn DisplayEnumerator>,
        capturer: Arc<dyn ScreenCapturer>,
        frontmost: Arc<dyn FrontmostAppProbe>,
        lock: Arc<dyn ScreenLockProbe>,
        idle: Arc<dyn IdleProbe>,
        power: Arc<dyn DisplaySleepProbe>,
        capture: CaptureConfig,
        privacy: PrivacyConfig,
        sessions: Arc<SharedSessionBinder>,
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
            sessions,
            activity,
            stats_full: AtomicU64::new(0),
            stats_probes: AtomicU64::new(0),
            stats_skip_visual: AtomicU64::new(0),
            stats_skip_near_dup: AtomicU64::new(0),
            stats_skip_debounce: AtomicU64::new(0),
            stats_skip_gate: AtomicU64::new(0),
            stats_drop_bp: AtomicU64::new(0),
            stats_liveness: AtomicU64::new(0),
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
            liveness_captures: self.stats_liveness.load(Ordering::Relaxed),
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

    pub fn force_close_session(&mut self) -> SessionTransition {
        self.sessions.force_close()
    }

    pub fn close_idle_session(&mut self) -> SessionTransition {
        self.sessions.close_if_idle()
    }

    pub fn current_session_id(&self) -> Option<Uuid> {
        self.sessions.current_id()
    }

    pub fn session_binder(&self) -> Arc<SharedSessionBinder> {
        Arc::clone(&self.sessions)
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
                let bundle_changed =
                    prev.bundle_id != cur.bundle_id || prev.app_name != cur.app_name;
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
    /// Pause and closed-eyes emit nothing. Lock is a hard capture gate: we may
    /// persist a lock-transition fact, but it never carries app/window/URL.
    pub async fn poll_activity(&mut self) -> ActivityPoll {
        if self.paused.load(Ordering::Relaxed) {
            return ActivityPoll::default();
        }
        if self.closed_eyes.load(Ordering::Relaxed) {
            return ActivityPoll::default();
        }

        let is_locked = self.lock.is_locked().await.unwrap_or(false);
        if is_locked {
            let closed = self.sessions.force_close();
            let idle_seconds = self.idle.idle_seconds().await.unwrap_or(0.0).max(0.0);
            let sample = ActivitySample {
                frontmost: None,
                idle_seconds,
                is_locked: true,
                display_sleep_prevented: false,
            };
            let tick = self.activity.ingest_detailed(sample, chrono::Utc::now());
            let mut events = closed.events;
            if let Some(ev) = tick.focus {
                events.push(ev);
            }
            return ActivityPoll {
                events,
                upsert_sessions: closed.upserts,
            };
        }

        let frontmost = self.frontmost.frontmost().await.ok().flatten();
        if self
            .privacy
            .blocks_bundle(frontmost.as_ref().and_then(|f| f.bundle_id.as_deref()))
        {
            return ActivityPoll::default();
        }
        let idle_seconds = self.idle.idle_seconds().await.unwrap_or(0.0).max(0.0);
        let display_sleep_prevented = self.power.display_sleep_prevented().await.unwrap_or(false);
        let sample = ActivitySample {
            frontmost: frontmost.clone(),
            idle_seconds,
            is_locked: false,
            display_sleep_prevented,
        };
        let key_idle = idle_seconds >= (self.capture.idle_session_ms as f64 / 1000.0)
            && !display_sleep_prevented;

        let trans = if let Some(ref front) = frontmost {
            if !key_idle {
                self.sessions.bind(
                    Some(front.app_name.as_str()),
                    front.bundle_id.as_deref(),
                    "focus_change",
                )
            } else {
                SessionTransition::default()
            }
        } else {
            SessionTransition::default()
        };

        let tick = self.activity.ingest_detailed(sample, chrono::Utc::now());
        let sid = self.sessions.current_id();
        let bind = |mut ev: SourceEvent| {
            if let Some(id) = sid {
                ev.session_id = Some(id);
            }
            ev
        };
        let mut out = trans.events;
        if let Some(ev) = tick.window_changed {
            out.push(bind(ev));
        }
        if let Some(ev) = tick.focus {
            out.push(bind(ev));
        }
        ActivityPoll {
            events: out,
            upsert_sessions: trans.upserts,
        }
    }

    /// Run one capture decision for `reason`. Returns None if gated/skipped.
    pub async fn capture_tick(
        &mut self,
        reason: TriggerReason,
    ) -> Result<Option<CaptureOutcome>, String> {
        let locked = self.lock.is_locked().await.unwrap_or(false);
        let front = match self.frontmost.frontmost().await {
            Ok(front) => front,
            Err(_) => {
                self.stats_skip_gate.fetch_add(1, Ordering::Relaxed);
                debug!("gate: frontmost_unavailable");
                return Ok(None);
            }
        };
        let bundle = front.as_ref().and_then(|f| f.bundle_id.clone());
        let gate = PolicyGate::evaluate(
            self.paused.load(Ordering::Relaxed),
            self.closed_eyes.load(Ordering::Relaxed),
            locked,
            &self.privacy,
            bundle.as_deref(),
            front.is_some(),
        );
        if !gate.allows() {
            self.stats_skip_gate.fetch_add(1, Ordering::Relaxed);
            debug!(reason = gate.as_str(), "gate");
            return Ok(None);
        }

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
        let liveness_only = if !reason.forces_full_capture() {
            let mut visual_changed = false;
            let mut liveness_due = false;
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

                let safety_due = self
                    .last_full_capture
                    .get(&d.id.0)
                    .is_none_or(|t| now.duration_since(*t) >= DHASH_SAFETY_VALVE);

                if dist >= self.capture.visual_change_threshold {
                    // Real visual change: dHash near-duplicate check.
                    let hash = dhash(&gray, raw.width as usize, raw.height as usize);
                    probe_hashes.insert(d.id.0, hash);

                    let history = self.dhash_history.entry(d.id.0).or_default();
                    while history
                        .front()
                        .is_some_and(|(_, t)| now.duration_since(*t) > DHASH_HISTORY_TTL)
                    {
                        history.pop_front();
                    }
                    let near_dup = history
                        .iter()
                        .any(|(h, _)| hamming64(hash, *h) <= DHASH_HAMMING_THRESHOLD);

                    if near_dup {
                        skipped_near_dup = true;
                    } else {
                        visual_changed = true;
                    }
                } else if safety_due {
                    liveness_due = true;
                }
            }

            if visual_changed {
                false
            } else if liveness_due {
                true
            } else {
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
            false
        };

        if liveness_only {
            return self
                .capture_liveness(&displays, front.as_ref())
                .await
                .map(Some);
        }

        let capture_id = Uuid::new_v4();
        let app_name = front.as_ref().map(|f| f.app_name.as_str());
        let bundle_s = front.as_ref().and_then(|f| f.bundle_id.as_deref());
        let trans = self.sessions.bind(app_name, bundle_s, reason.as_str());
        let closed_session = trans
            .upserts
            .iter()
            .find(|s| matches!(s.status, lumen_types::SessionStatus::Closed))
            .cloned();
        let open_session = trans
            .upserts
            .iter()
            .find(|s| matches!(s.status, lumen_types::SessionStatus::Open))
            .cloned();
        let session_id = open_session
            .as_ref()
            .map(|s| s.id)
            .or_else(|| self.sessions.current_id())
            .unwrap_or_else(Uuid::new_v4);

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

            let window_title = front
                .as_ref()
                .and_then(|f| f.window_title.as_ref())
                .filter(|t| !t.is_empty());
            let window_title_missing_reason = if window_title.is_some() {
                None
            } else if front.is_none() {
                Some("no_frontmost")
            } else if front.as_ref().and_then(|f| f.window_id).is_none() {
                Some("no_window")
            } else {
                Some("empty_title")
            };
            let pixel_hash = probe_hashes
                .get(&d.id.0)
                .map(|h| format!("dhash:{h:016x}"))
                .unwrap_or_else(|| {
                    format!("blake3:{}", blake3::hash(&frame.png_or_jpeg_bytes).to_hex())
                });
            let payload = json!({
                "payload_version": 1,
                "reason": reason.as_str(),
                "app_name": front.as_ref().map(|f| &f.app_name),
                "bundle_id": front.as_ref().and_then(|f| f.bundle_id.as_ref()),
                "pid": front.as_ref().and_then(|f| f.pid),
                "window_id": front.as_ref().and_then(|f| f.window_id),
                "window_title": window_title,
                "window_title_missing_reason": window_title_missing_reason,
                "url": front.as_ref().and_then(|f| f.tab_url.as_ref()),
                "display_id": d.id.0,
                "display_index": index,
                "is_main": d.is_main,
                "display_origin": [d.origin_x, d.origin_y],
                "width": frame.width,
                "height": frame.height,
                "probe_distance": max_distance,
                "pixel_hash": pixel_hash,
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

        Ok(Some(CaptureOutcome::Evidence(CapturedBatch {
            capture_id,
            session_id,
            reason,
            frames,
            closed_session,
            open_session,
            session_events: trans.events,
        })))
    }

    /// Full encode for the safety valve: pixels only, no session, no events.
    async fn capture_liveness(
        &mut self,
        displays: &[DisplayInfo],
        front: Option<&FrontmostApp>,
    ) -> Result<CaptureOutcome, String> {
        let mut frames = Vec::with_capacity(displays.len());
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
            frames.push(LivenessFrame {
                display_id: d.id.0,
                display_index: index,
                is_main: d.is_main,
                width: frame.width,
                height: frame.height,
                media_type: frame.media_type,
                bytes: frame.png_or_jpeg_bytes,
            });
        }
        let now = Instant::now();
        for d in displays {
            self.last_full_capture.insert(d.id.0, now);
        }
        self.stats_liveness.fetch_add(1, Ordering::Relaxed);
        info!(displays = frames.len(), "liveness overwrite");
        Ok(CaptureOutcome::Liveness(LivenessSnapshot {
            captured_at: chrono::Utc::now(),
            app_name: front.map(|f| f.app_name.clone()),
            bundle_id: front.and_then(|f| f.bundle_id.clone()),
            frames,
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
                if prev == b && elapsed < Duration::from_millis(self.capture.same_app_min_ms) {
                    return false;
                }
            }
        }
        true
    }

    async fn select_displays(&self) -> Result<Vec<DisplayInfo>, String> {
        let mut list = self
            .displays
            .list_displays()
            .await
            .map_err(|e| e.to_string())?;
        if !self.capture.all_displays() {
            list.retain(|d| d.is_main);
            if list.is_empty() {
                // fall back to first
                list = self
                    .displays
                    .list_displays()
                    .await
                    .map_err(|e| e.to_string())?;
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

    struct FakeLock {
        locked: AtomicBool,
    }
    impl FakeLock {
        fn unlocked() -> Self {
            Self {
                locked: AtomicBool::new(false),
            }
        }
    }
    #[async_trait]
    impl ScreenLockProbe for FakeLock {
        async fn is_locked(&self) -> Result<bool, PlatformError> {
            Ok(self.locked.load(Ordering::Relaxed))
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

    fn expect_evidence(out: Option<CaptureOutcome>) -> CapturedBatch {
        match out {
            Some(CaptureOutcome::Evidence(batch)) => batch,
            Some(CaptureOutcome::Liveness(_)) => panic!("expected evidence, got liveness"),
            None => panic!("expected evidence, got none"),
        }
    }

    fn expect_liveness(out: Option<CaptureOutcome>) -> LivenessSnapshot {
        match out {
            Some(CaptureOutcome::Liveness(snap)) => snap,
            Some(CaptureOutcome::Evidence(_)) => panic!("expected liveness, got evidence"),
            None => panic!("expected liveness, got none"),
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
            Arc::new(FakeLock::unlocked()),
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
        let first = expect_evidence(o.capture_tick(TriggerReason::Interval).await.unwrap());
        assert_eq!(first.frames.len(), 2); // dual display
        let second = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn focus_forces_capture() {
        let mut o = orch(FakeCap { n: Mutex::new(10) });
        let _ = o.capture_tick(TriggerReason::Interval).await.unwrap();
        let forced = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        expect_evidence(forced);
    }

    #[tokio::test]
    async fn screenshot_payload_has_hash_and_title_reason() {
        let mut o = orch(FakeCap { n: Mutex::new(1) });
        let batch = expect_evidence(o.capture_tick(TriggerReason::FocusChange).await.unwrap());
        let payload = &batch.frames[0].0.payload;
        let hash = payload["pixel_hash"].as_str().unwrap();
        assert!(hash.starts_with("dhash:") || hash.starts_with("blake3:"));
        assert_eq!(
            payload["window_title_missing_reason"],
            serde_json::json!("no_window")
        );
    }

    #[tokio::test]
    async fn closed_eyes_blocks() {
        let mut o = orch(FakeCap { n: Mutex::new(1) });
        o.set_closed_eyes(true);
        let r = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn blocklist_blocks_screenshot_capture() {
        let mut privacy = PrivacyConfig::default();
        privacy.app_blocklist = vec!["a.b".into()];
        let mut o = CaptureOrchestrator::new(
            Arc::new(FakeDisplays),
            Arc::new(FakeCap { n: Mutex::new(1) }),
            Arc::new(FakeFront {
                app: Mutex::new(FrontmostApp {
                    app_name: "A".into(),
                    bundle_id: Some("a.b".into()),
                    window_title: Some("secret".into()),
                    ls_category_type: None,
                    tab_url: Some("https://bank.example".into()),
                    pid: None,
                    window_id: None,
                }),
            }),
            Arc::new(FakeLock::unlocked()),
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
            privacy,
        );
        let r = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        assert!(r.is_none());
        assert_eq!(o.stats().skipped_gate, 1);
    }

    struct FakeFrontNone;
    #[async_trait]
    impl FrontmostAppProbe for FakeFrontNone {
        async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn unknown_frontmost_skips_capture_when_blocklist_is_set() {
        let mut privacy = PrivacyConfig::default();
        privacy.app_blocklist = vec!["com.secret.app".into()];
        let mut o = CaptureOrchestrator::new(
            Arc::new(FakeDisplays),
            Arc::new(FakeCap { n: Mutex::new(1) }),
            Arc::new(FakeFrontNone),
            Arc::new(FakeLock::unlocked()),
            Arc::new(FakeIdle),
            Arc::new(FakePower),
            CaptureConfig {
                debounce_default_ms: 0,
                debounce_churn_ms: 0,
                same_app_min_ms: 0,
                probe_scale: 1,
                displays: "all".into(),
                ..CaptureConfig::default()
            },
            privacy,
        );
        let r = o.capture_tick(TriggerReason::FocusChange).await.unwrap();
        assert!(r.is_none());
        assert_eq!(o.stats().skipped_gate, 1);
    }

    #[tokio::test]
    async fn lock_poll_closes_session_without_app_content() {
        let lock = Arc::new(FakeLock::unlocked());
        let mut o = CaptureOrchestrator::new(
            Arc::new(FakeDisplays),
            Arc::new(FakeCap { n: Mutex::new(1) }),
            Arc::new(FakeFront {
                app: Mutex::new(FrontmostApp {
                    app_name: "Safari".into(),
                    bundle_id: Some("com.apple.Safari".into()),
                    window_title: Some("Inbox".into()),
                    ls_category_type: None,
                    tab_url: Some("https://mail.example".into()),
                    pid: None,
                    window_id: None,
                }),
            }),
            lock.clone(),
            Arc::new(FakeIdle),
            Arc::new(FakePower),
            CaptureConfig::default(),
            PrivacyConfig::default(),
        );
        let open = o.poll_activity().await;
        assert!(open
            .upsert_sessions
            .iter()
            .any(|s| matches!(s.status, lumen_types::SessionStatus::Open)));
        lock.locked.store(true, Ordering::Relaxed);
        let locked = o.poll_activity().await;
        assert!(locked
            .upsert_sessions
            .iter()
            .any(|s| matches!(s.status, lumen_types::SessionStatus::Closed)));
        let focus = locked
            .events
            .iter()
            .find(|e| e.kind == event_kind::ACTIVITY_FOCUS_V1)
            .expect("lock transition fact");
        assert_eq!(focus.payload["is_locked"], serde_json::json!(true));
        assert!(focus.payload["app_name"].is_null());
        assert!(focus.payload["url"].is_null());
    }

    /// Regression: a visually static screen used to never capture because the
    /// MAD skip fired before the safety valve. The valve must still fire, but
    /// as overwrite-only liveness — not a `screenshot.v1` evidence batch.
    #[tokio::test]
    async fn safety_valve_captures_static_screen_when_overdue() {
        // FakeCap returns the same gray value every probe -> MAD is always 0
        // below threshold after the first capture, i.e. a perfectly static frame.
        let mut o = orch(FakeCap { n: Mutex::new(10) });

        let first = expect_evidence(o.capture_tick(TriggerReason::Interval).await.unwrap());
        let session_id = first.session_id;
        let snapshot_count = o
            .session_binder()
            .current()
            .expect("evidence opens a session")
            .snapshot_count;

        let second = o.capture_tick(TriggerReason::Interval).await.unwrap();
        assert!(
            second.is_none(),
            "stable frame within safety window should skip"
        );

        for d in o.select_displays().await.unwrap() {
            o.last_full_capture
                .insert(d.id.0, Instant::now() - Duration::from_secs(121));
        }

        let live = expect_liveness(o.capture_tick(TriggerReason::Interval).await.unwrap());
        assert_eq!(live.frames.len(), 2, "liveness should capture all displays");
        assert_eq!(o.stats().liveness_captures, 1);
        assert_eq!(o.stats().full_captures, 1);
        assert_eq!(o.current_session_id(), Some(session_id));
        assert_eq!(
            o.session_binder()
                .current()
                .expect("session still open")
                .snapshot_count,
            snapshot_count,
            "liveness must not bind/touch the activity session"
        );
    }
}
