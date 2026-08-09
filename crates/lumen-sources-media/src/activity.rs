//! Activity-event accumulator (ActivityWatch-style heartbeat/pulsetime model).
//!
//! The frontmost-app probe runs every `focus_poll_ms` (default 500ms). Writing
//! one DB row per poll would flood the store; but writing only on app-change
//! loses duration for the common case of sitting in one app/tab reading for
//! minutes (the same reason the screenshot path's visual-debounce drops frames
//! on static pages).
//!
//! Resolution: emit a lightweight `activity.focus.v1` event on every poll, but
//! let the storage projection (`project_activity_event`) merge consecutive
//! identical events into a single `activity_segments` row. This keeps the
//! raw-event stream faithful to reality while the projection gives clean
//! intervals. The accumulator here just decides *which* polls produce an event
//! worth persisting — enough to reconstruct continuous activity without
//! per-500ms write amplification:
//!
//! - always emit on state change (app / title / idle boundary),
//! - otherwise emit a heartbeat at most every `heartbeat_interval` so a long,
//!   unchanging run still gets a trailing row the projection can close.
//!
//! This is independent of the screenshot capture path's debounce, so reading a
//! static page still accrues activity time.

use std::time::Duration;

use chrono::Utc;
use lumen_platform::FrontmostApp;
use lumen_types::{event_kind, SourceEvent, SourceKind};
use serde_json::json;
use uuid::Uuid;

/// What the accumulator needs to know about the current moment.
#[derive(Debug, Clone)]
pub struct ActivitySample {
    pub frontmost: Option<FrontmostApp>,
    pub idle_seconds: f64,
    pub is_locked: bool,
    /// True when some app holds a power assertion that prevents the display
    /// from sleeping (video playback, video/voice call, presentation, Caffeine).
    /// When set, HID silence is NOT treated as idle — the user is likely
    /// watching a lecture or on a call without touching the mouse. Timing's
    /// "app keeps your Mac awake" heuristic.
    pub display_sleep_prevented: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivityKey {
    app_name: Option<String>,
    bundle_id: Option<String>,
    window_title: Option<String>,
    /// Active browser tab URL. Part of the segment identity so navigating
    /// github.com → gmail.com within one Safari window opens a new segment,
    /// each accruing its own duration → per-website time tracking.
    tab_url: Option<String>,
    is_idle: bool,
    is_locked: bool,
}

impl ActivityKey {
    fn from(sample: &ActivitySample, idle_threshold: f64) -> Self {
        let is_idle = sample.is_locked
            || (sample.idle_seconds >= idle_threshold && !sample.display_sleep_prevented);
        Self {
            app_name: sample.frontmost.as_ref().map(|f| f.app_name.clone()),
            bundle_id: sample.frontmost.as_ref().and_then(|f| f.bundle_id.clone()),
            window_title: sample.frontmost.as_ref().and_then(|f| f.window_title.clone()),
            tab_url: sample.frontmost.as_ref().and_then(|f| f.tab_url.clone()),
            is_idle,
            is_locked: sample.is_locked,
        }
    }
}

pub struct ActivityAccumulator {
    idle_threshold: f64,
    heartbeat: Duration,
    last_key: Option<ActivityKey>,
    last_emit: Option<chrono::DateTime<Utc>>,
}

impl ActivityAccumulator {
    /// `idle_threshold`: seconds of HID silence that count as AFK.
    /// `heartbeat`: emit a trailing event at least this often during a steady run.
    pub fn new(idle_threshold: Duration, heartbeat: Duration) -> Self {
        Self {
            idle_threshold: idle_threshold.as_secs_f64(),
            heartbeat,
            last_key: None,
            last_emit: None,
        }
    }

    /// Ingest a sample; returns an event to persist if this poll should leave a
    /// row (state change or heartbeat due).
    pub fn ingest(&mut self, sample: ActivitySample, now: chrono::DateTime<Utc>) -> Option<SourceEvent> {
        let key = ActivityKey::from(&sample, self.idle_threshold);

        let emit = match (&self.last_key, self.last_emit) {
            (None, _) => true,
            (Some(prev), _) if *prev != key => true,
            (Some(_), Some(last)) => now.signed_duration_since(last).to_std().ok()? >= self.heartbeat,
            (Some(_), None) => true,
        };

        if !emit {
            return None;
        }

        self.last_key = Some(key.clone());
        self.last_emit = Some(now);

        Some(make_event(&sample, &key, now))
    }
}

fn make_event(sample: &ActivitySample, key: &ActivityKey, ts: chrono::DateTime<Utc>) -> SourceEvent {
    let ls_category_type = sample
        .frontmost
        .as_ref()
        .and_then(|f| f.ls_category_type.clone());
    let payload = json!({
        "payload_version": 1,
        "app_name": key.app_name,
        "bundle_id": key.bundle_id,
        "window_title": key.window_title,
        "url": key.tab_url,
        "ls_category_type": ls_category_type,
        "idle_seconds": sample.idle_seconds,
        "is_idle": key.is_idle,
        "is_locked": key.is_locked,
    });
    let mut event = SourceEvent::new(SourceKind::Activity, event_kind::ACTIVITY_FOCUS_V1, payload);
    event.id = Uuid::new_v4();
    event.ts = ts;
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(app: &str, title: Option<&str>, idle: f64) -> ActivitySample {
        ActivitySample {
            frontmost: Some(FrontmostApp {
                app_name: app.into(),
                bundle_id: Some(format!("com.{app}")),
                window_title: title.map(str::to_string),
                ls_category_type: None,
                tab_url: None,
            }),
            idle_seconds: idle,
            is_locked: false,
            display_sleep_prevented: false,
        }
    }

    #[test]
    fn emits_on_first_sample() {
        let mut acc = ActivityAccumulator::new(Duration::from_secs(180), Duration::from_secs(5));
        let now = Utc::now();
        let e = acc.ingest(sample("Safari", Some("Hello"), 1.0), now);
        assert!(e.is_some());
    }

    #[test]
    fn emits_on_app_change() {
        let mut acc = ActivityAccumulator::new(Duration::from_secs(180), Duration::from_secs(60));
        let mut now = Utc::now();
        acc.ingest(sample("Safari", Some("Hello"), 1.0), now);
        now += chrono::Duration::seconds(1);
        // Same app+title → no heartbeat within 60s.
        assert!(acc.ingest(sample("Safari", Some("Hello"), 1.0), now).is_none());
        // App change → emit.
        now += chrono::Duration::seconds(1);
        let e = acc.ingest(sample("Mail", None, 1.0), now);
        assert!(e.is_some());
    }

    #[test]
    fn emits_on_idle_boundary() {
        let mut acc = ActivityAccumulator::new(Duration::from_secs(180), Duration::from_secs(60));
        let mut now = Utc::now();
        acc.ingest(sample("Safari", Some("Hello"), 1.0), now);
        now += chrono::Duration::seconds(1);
        // Still active, same app → no emit.
        assert!(acc.ingest(sample("Safari", Some("Hello"), 1.0), now).is_none());
        // Cross idle threshold → emit (idle boundary).
        now += chrono::Duration::seconds(1);
        let e = acc.ingest(sample("Safari", Some("Hello"), 200.0), now).unwrap();
        assert_eq!(e.payload["is_idle"], serde_json::json!(true));
    }

    /// When an app holds a display-sleep power assertion (video playback, call),
    /// HID silence must NOT count as idle — the user is watching, not away.
    /// This is the Timing "app keeps your Mac awake" suppression.
    #[test]
    fn display_sleep_prevention_suppresses_idle() {
        let mut acc = ActivityAccumulator::new(Duration::from_secs(180), Duration::from_secs(60));
        let mut now = Utc::now();
        acc.ingest(sample("Safari", Some("Netflix"), 1.0), now);

        // Long idle, no assertion → goes idle (boundary emit).
        now += chrono::Duration::seconds(1);
        let e = acc.ingest(sample("Safari", Some("Netflix"), 200.0), now).unwrap();
        assert_eq!(e.payload["is_idle"], serde_json::json!(true));

        // Now a power assertion is active (video playing). Same long idle, but
        // display_sleep_prevented → flips back to non-idle (boundary emit again).
        now += chrono::Duration::seconds(1);
        let mut s = sample("Safari", Some("Netflix"), 200.0);
        s.display_sleep_prevented = true;
        let e = acc.ingest(s, now).unwrap();
        assert_eq!(e.payload["is_idle"], serde_json::json!(false));
    }


    #[test]
    fn heartbeat_emits_on_steady_run() {
        let mut acc = ActivityAccumulator::new(Duration::from_secs(180), Duration::from_secs(5));
        let mut now = Utc::now();
        acc.ingest(sample("Safari", Some("Hello"), 1.0), now);
        // Within heartbeat window → no emit.
        now += chrono::Duration::seconds(3);
        assert!(acc.ingest(sample("Safari", Some("Hello"), 1.0), now).is_none());
        // Past heartbeat → emit even though state unchanged.
        now += chrono::Duration::seconds(3);
        assert!(acc.ingest(sample("Safari", Some("Hello"), 1.0), now).is_some());
    }
}
