//! Pure Act gates. Runtime fills [`GateInput`]; this module does not touch TCC.

use crate::protocol::{GateReport, GateVerdict};

/// User is "busy" if HID idle is below this many seconds.
pub const PRESENCE_BUSY_SECS: f64 = 2.0;
/// How long a Wait presence may be polled before it becomes Refuse.
pub const PRESENCE_WAIT_MAX_SECS: f64 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct GateInput {
    pub target_pid: i32,
    pub frontmost_pid: Option<i32>,
    pub target_on_current_space: bool,
    /// PID of the topmost window under the intended click, if known.
    pub hit_top_pid: Option<i32>,
    pub hid_idle_seconds: f64,
    pub focus_lock_held_by_other: bool,
    /// When false, frontmost/occlusion/presence are not required (postToPid).
    pub allow_foreground: bool,
}

pub fn evaluate(input: &GateInput) -> GateReport {
    let cross_space = if input.target_on_current_space {
        GateVerdict::Pass
    } else {
        GateVerdict::Refuse
    };

    if !input.allow_foreground {
        return GateReport {
            frontmost: GateVerdict::Pass,
            occlusion: GateVerdict::Pass,
            presence: GateVerdict::Pass,
            focus_lock: if input.focus_lock_held_by_other {
                GateVerdict::Refuse
            } else {
                GateVerdict::Pass
            },
            cross_space,
        };
    }

    let frontmost = match input.frontmost_pid {
        Some(pid) if pid == input.target_pid => GateVerdict::Pass,
        _ => GateVerdict::Refuse,
    };
    let occlusion = match input.hit_top_pid {
        Some(pid) if pid == input.target_pid => GateVerdict::Pass,
        Some(_) => GateVerdict::Refuse,
        None => GateVerdict::Pass,
    };
    let presence = if input.hid_idle_seconds >= PRESENCE_BUSY_SECS {
        GateVerdict::Pass
    } else {
        GateVerdict::Wait
    };
    let focus_lock = if input.focus_lock_held_by_other {
        GateVerdict::Refuse
    } else {
        GateVerdict::Pass
    };

    GateReport {
        frontmost,
        occlusion,
        presence,
        focus_lock,
        cross_space,
    }
}

/// After waiting for presence, a still-busy user is a hard refuse.
pub fn presence_after_wait(hid_idle_seconds: f64) -> GateVerdict {
    if hid_idle_seconds >= PRESENCE_BUSY_SECS {
        GateVerdict::Pass
    } else {
        GateVerdict::Refuse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> GateInput {
        GateInput {
            target_pid: 42,
            frontmost_pid: Some(42),
            target_on_current_space: true,
            hit_top_pid: Some(42),
            hid_idle_seconds: 10.0,
            focus_lock_held_by_other: false,
            allow_foreground: true,
        }
    }

    #[test]
    fn background_delivery_skips_frontmost_and_presence() {
        let mut input = base();
        input.allow_foreground = false;
        input.frontmost_pid = Some(7);
        input.hid_idle_seconds = 0.1;
        input.hit_top_pid = Some(7);
        let report = evaluate(&input);
        assert_eq!(report.frontmost, GateVerdict::Pass);
        assert_eq!(report.presence, GateVerdict::Pass);
        assert_eq!(report.occlusion, GateVerdict::Pass);
        assert!(report.blocking_reason().is_none());
    }

    #[test]
    fn background_still_refuses_other_space() {
        let mut input = base();
        input.allow_foreground = false;
        input.target_on_current_space = false;
        let report = evaluate(&input);
        assert_eq!(report.blocking_reason(), Some("cross_space"));
    }

    #[test]
    fn foreground_refuses_when_user_is_frontmost_elsewhere() {
        let mut input = base();
        input.frontmost_pid = Some(99);
        let report = evaluate(&input);
        assert_eq!(report.blocking_reason(), Some("frontmost"));
    }

    #[test]
    fn foreground_refuses_occluded_click() {
        let mut input = base();
        input.hit_top_pid = Some(99);
        let report = evaluate(&input);
        assert_eq!(report.blocking_reason(), Some("occlusion"));
    }

    #[test]
    fn presence_waits_then_refuses_if_still_busy() {
        let mut input = base();
        input.hid_idle_seconds = 0.5;
        let report = evaluate(&input);
        assert_eq!(report.presence, GateVerdict::Wait);
        assert!(report.needs_wait());
        assert!(report.blocking_reason().is_none());
        assert_eq!(presence_after_wait(0.4), GateVerdict::Refuse);
        assert_eq!(presence_after_wait(3.0), GateVerdict::Pass);
    }

    #[test]
    fn focus_lock_blocks_both_modes() {
        let mut input = base();
        input.focus_lock_held_by_other = true;
        assert_eq!(evaluate(&input).blocking_reason(), Some("focus_lock"));
        input.allow_foreground = false;
        assert_eq!(evaluate(&input).blocking_reason(), Some("focus_lock"));
    }
}
