//! Rolling restart budget shared by the daemon supervisor and health monitor.
//!
//! Intentional stops / config reloads do not consume a slot. Crash recoveries
//! share one 10-minute window so a wedged pipeline cannot boot-loop.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(10 * 60);
const MAX_RESTARTS: usize = 3;

#[derive(Debug)]
pub struct RestartBudget {
    events: Mutex<VecDeque<Instant>>,
}

impl Default for RestartBudget {
    fn default() -> Self {
        Self {
            events: Mutex::new(VecDeque::new()),
        }
    }
}

impl RestartBudget {
    /// Record a crash restart if the rolling window still has room.
    pub fn try_acquire(&self) -> bool {
        let Ok(mut events) = self.events.lock() else {
            return false;
        };
        prune(&mut events, Instant::now());
        if events.len() >= MAX_RESTARTS {
            return false;
        }
        events.push_back(Instant::now());
        true
    }

    pub fn used(&self) -> usize {
        let Ok(mut events) = self.events.lock() else {
            return MAX_RESTARTS;
        };
        prune(&mut events, Instant::now());
        events.len()
    }

    pub fn remaining(&self) -> usize {
        MAX_RESTARTS.saturating_sub(self.used())
    }
}

fn prune(events: &mut VecDeque<Instant>, now: Instant) {
    while events
        .front()
        .is_some_and(|t| now.duration_since(*t) >= WINDOW)
    {
        events.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_three_then_refuses() {
        let budget = RestartBudget::default();
        assert!(budget.try_acquire());
        assert!(budget.try_acquire());
        assert!(budget.try_acquire());
        assert!(!budget.try_acquire());
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn slots_free_after_window() {
        let budget = RestartBudget::default();
        {
            let mut events = budget.events.lock().unwrap();
            let stale = Instant::now() - WINDOW - Duration::from_secs(1);
            events.push_back(stale);
            events.push_back(stale);
            events.push_back(stale);
        }
        assert_eq!(budget.used(), 0);
        assert!(budget.try_acquire());
    }
}
