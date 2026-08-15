//! Observe-level activity sessions (open on work, close on idle).

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lumen_types::{event_kind, ActivitySession, SessionStatus, SourceEvent, SourceKind};
use serde_json::json;
use uuid::Uuid;

/// Session row + lifecycle events produced by one bind/close.
#[derive(Debug, Default, Clone)]
pub struct SessionTransition {
    pub upserts: Vec<ActivitySession>,
    pub events: Vec<SourceEvent>,
}

impl SessionTransition {
    pub fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.events.is_empty()
    }
}

/// One process-wide session owner so activity polling and HID persist agree
/// on the open session without a 500 ms cache lag.
pub struct SharedSessionBinder {
    inner: Mutex<SessionManager>,
}

impl SharedSessionBinder {
    pub fn new(idle_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(SessionManager::new(idle_ms)),
        })
    }

    pub fn mutate<R>(&self, f: impl FnOnce(&mut SessionManager) -> R) -> R {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    fn with_inner<R>(&self, f: impl FnOnce(&mut SessionManager) -> R) -> R {
        self.mutate(f)
    }

    pub fn current(&self) -> Option<ActivitySession> {
        self.with_inner(|m| m.current().cloned())
    }

    pub fn current_id(&self) -> Option<Uuid> {
        self.with_inner(|m| m.current().map(|s| s.id))
    }

    /// True when the open session already belongs to this frontmost app.
    pub fn matches_frontmost(&self, app: Option<&str>, bundle: Option<&str>) -> bool {
        self.with_inner(|m| match m.current() {
            Some(s) => session_matches_frontmost(s, app, bundle),
            None => false,
        })
    }

    pub fn bind(
        &self,
        app: Option<&str>,
        bundle: Option<&str>,
        trigger: &str,
    ) -> SessionTransition {
        self.with_inner(|m| {
            let (_, closed) = m.touch(app, bundle, trigger);
            drain_transition(m, closed)
        })
    }

    pub fn close_if_idle(&self) -> SessionTransition {
        self.with_inner(|m| {
            let closed = m.close_if_idle();
            drain_transition(m, closed)
        })
    }

    pub fn force_close(&self) -> SessionTransition {
        self.with_inner(|m| {
            let closed = m.force_close();
            drain_transition(m, closed)
        })
    }
}

pub fn session_matches_frontmost(
    session: &ActivitySession,
    app: Option<&str>,
    bundle: Option<&str>,
) -> bool {
    match (session.primary_bundle.as_deref(), bundle) {
        (Some(prev), Some(next)) => prev == next,
        _ => session.primary_app.as_deref() == app,
    }
}

pub fn drain_transition(
    manager: &mut SessionManager,
    closed: Option<ActivitySession>,
) -> SessionTransition {
    let mut upserts = Vec::new();
    if let Some(closed) = closed {
        upserts.push(closed);
    }
    if let Some(open) = manager.current().cloned() {
        upserts.push(open);
    }
    SessionTransition {
        upserts,
        events: manager.drain_lifecycle(),
    }
}

pub struct SessionManager {
    open: Option<ActivitySession>,
    last_activity: Option<DateTime<Utc>>,
    idle_ms: u64,
    lifecycle: Vec<SourceEvent>,
}

impl SessionManager {
    pub fn new(idle_ms: u64) -> Self {
        Self {
            open: None,
            last_activity: None,
            idle_ms,
            lifecycle: Vec::new(),
        }
    }

    pub fn current(&self) -> Option<&ActivitySession> {
        self.open.as_ref()
    }

    /// Touch session; opens a new one if needed. Returns (session_id, maybe_closed_previous).
    pub fn touch(
        &mut self,
        app: Option<&str>,
        bundle: Option<&str>,
        trigger: &str,
    ) -> (Uuid, Option<ActivitySession>) {
        let now = Utc::now();
        let mut closed = None;

        if let Some(ref mut s) = self.open {
            let app_switched = match (s.primary_bundle.as_deref(), bundle) {
                (Some(prev), Some(next)) => prev != next,
                _ => s.primary_app.as_deref() != app,
            };
            if let Some(last) = self.last_activity {
                let idle = (now - last).num_milliseconds().max(0) as u64;
                if idle >= self.idle_ms || app_switched {
                    s.ended_at = Some(now);
                    s.status = SessionStatus::Closed;
                    closed = self.open.take();
                }
            } else if app_switched {
                s.ended_at = Some(now);
                s.status = SessionStatus::Closed;
                closed = self.open.take();
            }
        }
        if let Some(ref closed_session) = closed {
            self.lifecycle.push(session_event(
                event_kind::SESSION_ENDED_V1,
                closed_session,
                now,
            ));
        }

        if self.open.is_none() {
            self.open = Some(ActivitySession {
                id: Uuid::new_v4(),
                started_at: now,
                ended_at: None,
                primary_app: app.map(|s| s.to_string()),
                primary_bundle: bundle.map(|s| s.to_string()),
                trigger: trigger.to_string(),
                snapshot_count: 0,
                status: SessionStatus::Open,
            });
            if let Some(ref opened) = self.open {
                self.lifecycle
                    .push(session_event(event_kind::SESSION_STARTED_V1, opened, now));
            }
        }

        if let Some(ref mut s) = self.open {
            s.snapshot_count = s.snapshot_count.saturating_add(1);
            if let Some(a) = app {
                s.primary_app = Some(a.to_string());
            }
            if let Some(b) = bundle {
                s.primary_bundle = Some(b.to_string());
            }
        }

        self.last_activity = Some(now);
        (self.open.as_ref().unwrap().id, closed)
    }

    pub fn close_if_idle(&mut self) -> Option<ActivitySession> {
        let now = Utc::now();
        let last = self.last_activity?;
        let idle = (now - last).num_milliseconds().max(0) as u64;
        if idle < self.idle_ms {
            return None;
        }
        if let Some(mut s) = self.open.take() {
            s.ended_at = Some(now);
            s.status = SessionStatus::Closed;
            self.lifecycle
                .push(session_event(event_kind::SESSION_ENDED_V1, &s, now));
            return Some(s);
        }
        None
    }

    pub fn force_close(&mut self) -> Option<ActivitySession> {
        let now = Utc::now();
        if let Some(mut s) = self.open.take() {
            s.ended_at = Some(now);
            s.status = SessionStatus::Closed;
            self.lifecycle
                .push(session_event(event_kind::SESSION_ENDED_V1, &s, now));
            return Some(s);
        }
        None
    }

    pub fn drain_lifecycle(&mut self) -> Vec<SourceEvent> {
        std::mem::take(&mut self.lifecycle)
    }
}

fn session_event(kind: &str, session: &ActivitySession, ts: DateTime<Utc>) -> SourceEvent {
    let payload = json!({
        "payload_version": 1,
        "application_session_id": session.id,
        "app_name": session.primary_app,
        "bundle_id": session.primary_bundle,
        "trigger": session.trigger,
        "snapshot_count": session.snapshot_count,
    });
    let mut event = SourceEvent::new(SourceKind::Activity, kind, payload).with_session(session.id);
    event.ts = ts;
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_increments() {
        let mut m = SessionManager::new(60_000);
        let (id1, closed) = m.touch(Some("Safari"), Some("com.apple.Safari"), "focus_change");
        assert!(closed.is_none());
        let (id2, _) = m.touch(Some("Safari"), Some("com.apple.Safari"), "interval");
        assert_eq!(id1, id2);
        assert_eq!(m.current().unwrap().snapshot_count, 2);
    }

    #[test]
    fn app_switch_closes_and_emits_lifecycle() {
        let mut m = SessionManager::new(60_000);
        let (safari, closed) = m.touch(Some("Safari"), Some("com.apple.Safari"), "focus_change");
        assert!(closed.is_none());
        let (term, closed) = m.touch(
            Some("Ghostty"),
            Some("com.mitchellh.ghostty"),
            "focus_change",
        );
        assert!(closed.is_some());
        assert_ne!(safari, term);
        let evs = m.drain_lifecycle();
        let kinds: Vec<_> = evs.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "session.started.v1",
                "session.ended.v1",
                "session.started.v1"
            ]
        );
    }

    #[test]
    fn missing_bundle_still_switches_on_app_name() {
        let mut m = SessionManager::new(60_000);
        m.touch(Some("Safari"), None, "focus_change");
        let (_, closed) = m.touch(Some("Mail"), None, "focus_change");
        assert!(closed.is_some());
        assert_eq!(closed.unwrap().primary_app.as_deref(), Some("Safari"));
    }

    #[test]
    fn idle_close_requires_no_fresh_touch() {
        let mut m = SessionManager::new(10);
        m.touch(Some("Safari"), Some("com.apple.Safari"), "focus_change");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let closed = m.close_if_idle();
        assert!(closed.is_some());
        assert_eq!(m.current(), None);
        let kinds: Vec<_> = m.drain_lifecycle().into_iter().map(|e| e.kind).collect();
        assert!(kinds.iter().any(|k| k == "session.ended.v1"));
    }

    #[test]
    fn binder_opens_once_and_switches_on_bundle() {
        let binder = SharedSessionBinder::new(60_000);
        assert!(binder.current_id().is_none());
        let first = binder.bind(Some("Safari"), Some("com.apple.Safari"), "interaction");
        assert_eq!(first.upserts.len(), 1);
        assert_eq!(
            first
                .events
                .iter()
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>(),
            ["session.started.v1"]
        );
        let safari = binder.current_id().unwrap();
        assert!(binder.matches_frontmost(Some("Safari"), Some("com.apple.Safari")));
        let same = binder.bind(Some("Safari"), Some("com.apple.Safari"), "focus_change");
        assert_eq!(binder.current_id(), Some(safari));
        assert!(same.events.iter().all(|e| e.kind != "session.started.v1"));
        let switched = binder.bind(Some("Mail"), Some("com.apple.mail"), "interaction");
        assert_ne!(binder.current_id(), Some(safari));
        let kinds: Vec<_> = switched.events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["session.ended.v1", "session.started.v1"]);
        assert!(!binder.matches_frontmost(Some("Safari"), Some("com.apple.Safari")));
    }
}
