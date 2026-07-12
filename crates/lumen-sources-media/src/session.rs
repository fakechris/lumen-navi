//! Observe-level activity sessions (open on work, close on idle).

use chrono::{DateTime, Utc};
use lumen_types::{ActivitySession, SessionStatus};
use uuid::Uuid;

pub struct SessionManager {
    open: Option<ActivitySession>,
    last_activity: Option<DateTime<Utc>>,
    idle_ms: u64,
}

impl SessionManager {
    pub fn new(idle_ms: u64) -> Self {
        Self {
            open: None,
            last_activity: None,
            idle_ms,
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
            if let Some(last) = self.last_activity {
                let idle = (now - last).num_milliseconds().max(0) as u64;
                if idle >= self.idle_ms {
                    s.ended_at = Some(now);
                    s.status = SessionStatus::Closed;
                    closed = self.open.take();
                }
            }
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
            return Some(s);
        }
        None
    }

    pub fn force_close(&mut self) -> Option<ActivitySession> {
        let now = Utc::now();
        if let Some(mut s) = self.open.take() {
            s.ended_at = Some(now);
            s.status = SessionStatus::Closed;
            return Some(s);
        }
        None
    }
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
}
