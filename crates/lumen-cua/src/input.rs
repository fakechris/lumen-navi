//! Explicit Act input. Observe never calls this.
//!
//! Default delivery is background (`CGEventPostToPid`). Activate is opt-in.

use anyhow::{bail, Result};
use std::thread;
use std::time::{Duration, Instant};

use crate::focus_lock::FocusGuard;
use crate::gates::{self, GateInput};
use crate::protocol::{
    ActionEffect, ActionResult, ActionRoute, DeliveryMode, GateVerdict, InputStep,
};
use crate::stoplines;
use crate::window_capture;
use crate::window_info;

pub fn replay(steps: &[InputStep]) -> Result<Vec<ActionResult>> {
    if steps.is_empty() {
        bail!("empty replay");
    }
    if steps.len() > 16 {
        bail!("replay too long");
    }
    let mut effects = Vec::with_capacity(steps.len());
    for step in steps {
        effects.push(run_step(step)?);
        let wait = step.wait_ms.unwrap_or(180).min(2_000);
        thread::sleep(Duration::from_millis(wait));
    }
    Ok(effects)
}

fn run_step(step: &InputStep) -> Result<ActionResult> {
    let app = step
        .bundle_id
        .as_deref()
        .or(step.window.as_deref())
        .unwrap_or("");
    if let Some(reason) = stoplines::refuse_replay_step(
        &step.action,
        app,
        step.bundle_id.as_deref(),
        step.keys.as_deref(),
        &[
            step.target.as_deref().unwrap_or(""),
            step.window.as_deref().unwrap_or(""),
        ],
    ) {
        return Ok(ActionResult::refused(reason, None));
    }

    let pid = step
        .bundle_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(pid_for_bundle);

    match step.action.as_str() {
        "focus" | "activate" => run_focus(step, pid),
        "click" => run_click(step, pid),
        "shortcut" | "submit" | "key" => run_key(step, pid),
        "type" => run_type(step, pid),
        other => Ok(ActionResult::refused(
            format!("unsupported replay action {other}"),
            None,
        )),
    }
}

fn run_focus(step: &InputStep, pid: Option<i32>) -> Result<ActionResult> {
    let Some(bundle) = step.bundle_id.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(ActionResult::refused("focus needs bundle_id", None));
    };
    let Some(pid) = pid else {
        return Ok(ActionResult::refused("app not running", None));
    };
    let idle = crate::idle::snapshot();
    let on_space = window_on_space(pid, step.window.as_deref());
    if idle.frontmost_pid == Some(pid) && on_space {
        return Ok(ActionResult {
            effect: ActionEffect::Confirmed,
            route: ActionRoute::Skipped,
            delivery: DeliveryMode::NotApplicable,
            reason: Some("already_frontmost".into()),
            gates: None,
        });
    }
    if !step.allow_foreground {
        return Ok(ActionResult::refused("would_activate", None));
    }
    if step.dry {
        return Ok(dry_result(step, pid, None));
    }
    let gates = wait_and_evaluate(step, pid, None)?;
    if let Some(reason) = gates.blocking_reason() {
        return Ok(ActionResult::refused(reason, Some(gates)));
    }
    let Some(_lock) = FocusGuard::try_acquire() else {
        return Ok(ActionResult::refused("focus_lock_held", Some(gates)));
    };
    activate_bundle(bundle)?;
    Ok(ActionResult {
        effect: ActionEffect::Unverifiable,
        route: ActionRoute::GlobalInput,
        delivery: DeliveryMode::Foreground,
        reason: None,
        gates: Some(gates),
    })
}

fn run_click(step: &InputStep, pid: Option<i32>) -> Result<ActionResult> {
    let Some(pid) = pid else {
        return Ok(ActionResult::refused("click needs running app", None));
    };
    let (Some(nx), Some(ny)) = (step.nx, step.ny) else {
        return Ok(ActionResult::refused("click needs relative nx/ny", None));
    };
    let windows = window_info::windows_for_pid(pid);
    if windows.len() > 1 && step.window.as_deref().unwrap_or("").is_empty() {
        return Ok(ActionResult::refused("ambiguous_window", None));
    }
    let Some((x, y, window_id)) = resolve_click_point(step, pid, nx, ny) else {
        return Ok(ActionResult::refused(
            "could not resolve window frame for click",
            None,
        ));
    };
    if !window_info::window_on_current_space(window_id) {
        return Ok(ActionResult::refused("cross_space", None));
    }
    if step.dry {
        return Ok(dry_result(step, pid, Some((x, y))));
    }

    let before = window_id
        .checked_sub(0)
        .and_then(|_| window_capture::capture_window(window_id, 320, true, 50).ok());
    let before_hash = before
        .as_ref()
        .and_then(|s| window_capture::dhash(&s.bytes));

    let gates = wait_and_evaluate(step, pid, Some((x, y)))?;
    if let Some(reason) = gates.blocking_reason() {
        return Ok(ActionResult::refused(reason, Some(gates)));
    }

    let posted = click_to_pid(pid, x, y);
    thread::sleep(Duration::from_millis(300));

    let after = window_capture::capture_window(window_id, 320, true, 50).ok();
    let after_hash = after.as_ref().and_then(|s| window_capture::dhash(&s.bytes));
    let effect = hash_effect(before_hash, after_hash);
    let reason = if !posted {
        Some("post_to_pid_failed".into())
    } else if effect == ActionEffect::SuspectedNoop {
        Some("background_noop".into())
    } else {
        None
    };
    Ok(ActionResult {
        effect: if !posted {
            ActionEffect::SuspectedNoop
        } else {
            effect
        },
        route: ActionRoute::SyntheticEvents,
        delivery: DeliveryMode::Background,
        reason,
        gates: Some(gates),
    })
}

fn run_key(step: &InputStep, pid: Option<i32>) -> Result<ActionResult> {
    let keys = step
        .keys
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("key step needs keys"))
        .ok();
    let Some(keys) = keys else {
        return Ok(ActionResult::refused("key step needs keys", None));
    };
    let Some(pid) = pid else {
        return Ok(ActionResult::refused("key needs running app", None));
    };
    let windows = window_info::windows_for_pid(pid);
    if windows.len() > 1 && step.window.as_deref().unwrap_or("").is_empty() {
        return Ok(ActionResult::refused("ambiguous_window", None));
    }
    if step.dry {
        return Ok(dry_result(step, pid, None));
    }
    let gates = wait_and_evaluate(step, pid, None)?;
    if let Some(reason) = gates.blocking_reason() {
        return Ok(ActionResult::refused(reason, Some(gates)));
    }
    let window_id =
        window_info::choose_window(&windows, step.window.as_deref()).map(|w| w.window_id);
    let before_hash = window_id.and_then(window_dhash);
    let posted = key_combo_to_pid(pid, keys).is_ok();
    thread::sleep(Duration::from_millis(300));
    let after_hash = window_id.and_then(window_dhash);
    let effect = if posted {
        hash_effect(before_hash, after_hash)
    } else {
        ActionEffect::SuspectedNoop
    };
    Ok(ActionResult {
        effect,
        route: ActionRoute::SyntheticEvents,
        delivery: DeliveryMode::Background,
        reason: if posted {
            None
        } else {
            Some("post_to_pid_failed".into())
        },
        gates: Some(gates),
    })
}

fn run_type(step: &InputStep, pid: Option<i32>) -> Result<ActionResult> {
    let text = step
        .text
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("type 步没有用户提供文本（回放确认时未填写）"));
    let text = match text {
        Ok(t) => t,
        Err(e) => return Ok(ActionResult::refused(e.to_string(), None)),
    };
    let Some(pid) = pid else {
        return Ok(ActionResult::refused("type needs running app", None));
    };
    if step.dry {
        return Ok(dry_result(step, pid, None));
    }
    let gates = wait_and_evaluate(step, pid, None)?;
    if let Some(reason) = gates.blocking_reason() {
        return Ok(ActionResult::refused(reason, Some(gates)));
    }
    #[cfg(target_os = "macos")]
    {
        match lumen_platform_macos::inject::inject_text(
            pid,
            text,
            lumen_platform_macos::inject::InjectMode::Replace,
        ) {
            Ok(()) => {
                return Ok(ActionResult {
                    effect: ActionEffect::Confirmed,
                    route: ActionRoute::Accessibility,
                    delivery: DeliveryMode::Background,
                    reason: Some("ax_set_value".into()),
                    gates: Some(gates),
                });
            }
            Err(err) => {
                return Ok(ActionResult {
                    effect: ActionEffect::SuspectedNoop,
                    route: ActionRoute::Accessibility,
                    delivery: DeliveryMode::Background,
                    reason: Some(err),
                    gates: Some(gates),
                });
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (text, pid, gates);
        Ok(ActionResult::refused("type requires macOS", None))
    }
}

fn dry_result(step: &InputStep, pid: i32, point: Option<(f64, f64)>) -> ActionResult {
    let report = evaluate_now(step, pid, point);
    ActionResult {
        effect: ActionEffect::Unverifiable,
        route: ActionRoute::Skipped,
        delivery: if step.allow_foreground {
            DeliveryMode::Foreground
        } else {
            DeliveryMode::Background
        },
        reason: Some("dry_run".into()),
        gates: Some(report),
    }
}

fn wait_and_evaluate(
    step: &InputStep,
    pid: i32,
    point: Option<(f64, f64)>,
) -> Result<crate::protocol::GateReport> {
    let started = Instant::now();
    loop {
        let mut report = evaluate_now(step, pid, point);
        if report.presence == GateVerdict::Wait
            && started.elapsed() < Duration::from_secs_f64(gates::PRESENCE_WAIT_MAX_SECS)
        {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        if report.presence == GateVerdict::Wait {
            report.presence = gates::presence_after_wait(crate::idle::snapshot().hid_idle_seconds);
        }
        return Ok(report);
    }
}

fn evaluate_now(
    step: &InputStep,
    pid: i32,
    point: Option<(f64, f64)>,
) -> crate::protocol::GateReport {
    let idle = crate::idle::snapshot();
    let hit = point.and_then(|(x, y)| window_info::top_pid_at(x, y));
    let on_space = window_on_space(pid, step.window.as_deref());
    gates::evaluate(&GateInput {
        target_pid: pid,
        frontmost_pid: idle.frontmost_pid,
        target_on_current_space: on_space,
        hit_top_pid: hit,
        hid_idle_seconds: idle.hid_idle_seconds,
        focus_lock_held_by_other: idle.focus_lock_held,
        allow_foreground: step.allow_foreground,
    })
}

fn window_on_space(pid: i32, title: Option<&str>) -> bool {
    let windows = window_info::windows_for_pid(pid);
    let Some(chosen) = window_info::choose_window(&windows, title) else {
        return !windows.is_empty()
            && windows
                .iter()
                .any(|w| window_info::window_on_current_space(w.window_id));
    };
    window_info::window_on_current_space(chosen.window_id)
}

fn resolve_click_point(step: &InputStep, pid: i32, nx: f64, ny: f64) -> Option<(f64, f64, u64)> {
    let nx = nx.clamp(0.02, 0.98);
    let ny = ny.clamp(0.02, 0.98);
    if let Some(frame) = window_frame(step.bundle_id.as_deref(), step.window.as_deref()) {
        let x = frame.0 + nx * frame.2;
        let y = frame.1 + ny * frame.3;
        let windows = window_info::windows_for_pid(pid);
        let id =
            window_info::choose_window(&windows, step.window.as_deref()).map(|w| w.window_id)?;
        return Some((x, y, id));
    }
    None
}

fn window_dhash(window_id: u64) -> Option<u64> {
    let shot = window_capture::capture_window(window_id, 320, true, 50).ok()?;
    window_capture::dhash(&shot.bytes)
}

fn hash_effect(before: Option<u64>, after: Option<u64>) -> ActionEffect {
    match (before, after) {
        (Some(a), Some(b)) if window_capture::hamming(a, b) >= 6 => ActionEffect::Partial,
        (Some(a), Some(b)) if a == b => ActionEffect::SuspectedNoop,
        _ => ActionEffect::Unverifiable,
    }
}

/// PostToPid create-success is not delivery. Steal focus only when the
/// window hash is unchanged or missing, and the step opted into foreground.
#[cfg_attr(not(test), allow(dead_code))]
fn escalate_background_noop(effect: ActionEffect, allow_foreground: bool) -> bool {
    allow_foreground
        && matches!(
            effect,
            ActionEffect::SuspectedNoop | ActionEffect::Unverifiable
        )
}

#[cfg(target_os = "macos")]
fn activate_bundle(bundle_id: &str) -> Result<()> {
    let app = running_app(bundle_id).ok_or_else(|| anyhow::anyhow!("app not running"))?;
    let ok = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(0));
    if !ok {
        bail!("activate {bundle_id} failed");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn running_app(
    bundle_id: &str,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSRunningApplication>> {
    use objc2_app_kit::NSWorkspace;
    let ws = NSWorkspace::sharedWorkspace();
    for app in ws.runningApplications() {
        let id = app.bundleIdentifier().map(|s| s.to_string());
        if id.as_deref() == Some(bundle_id) {
            return Some(app);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn activate_bundle(_: &str) -> Result<()> {
    bail!("input replay requires macOS")
}

#[cfg(target_os = "macos")]
fn window_frame(bundle_id: Option<&str>, title: Option<&str>) -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use lumen_platform_macos::ax::{
        ax_point_attr, ax_size_attr, ax_string_attr, AXUIElementCopyAttributeValue,
        AXUIElementCreateApplication, AxUIElementRef, ReleaseGuard,
    };

    let pid = pid_for_bundle(bundle_id?)?;
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let _g = ReleaseGuard(app as *const std::ffi::c_void);
        let wins_attr = CFString::new("AXWindows");
        let mut wins: core_foundation::base::CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app, wins_attr.as_concrete_TypeRef(), &mut wins) != 0
            || wins.is_null()
        {
            return None;
        }
        let _wins_g = ReleaseGuard(wins);
        let arr = wins as core_foundation_sys::array::CFArrayRef;
        let count = core_foundation_sys::array::CFArrayGetCount(arr);
        let want = title.unwrap_or("");
        let mut chosen: Option<AxUIElementRef> = None;
        for i in 0..count {
            let v = core_foundation_sys::array::CFArrayGetValueAtIndex(arr, i);
            if v.is_null() {
                continue;
            }
            let el = v as AxUIElementRef;
            let t = ax_string_attr(el, "AXTitle").unwrap_or_default();
            if want.is_empty() || t == want || t.contains(want) || want.contains(&t) {
                chosen = Some(el);
                if t == want {
                    break;
                }
            }
        }
        let el = chosen?;
        let (x, y) = ax_point_attr(el, "AXPosition")?;
        let (w, h) = ax_size_attr(el, "AXSize")?;
        Some((x, y, w, h))
    }
}

#[cfg(not(target_os = "macos"))]
fn window_frame(_: Option<&str>, _: Option<&str>) -> Option<(f64, f64, f64, f64)> {
    None
}

#[cfg(target_os = "macos")]
fn pid_for_bundle(bundle_id: &str) -> Option<i32> {
    Some(running_app(bundle_id)?.processIdentifier())
}

#[cfg(not(target_os = "macos"))]
fn pid_for_bundle(_: &str) -> Option<i32> {
    None
}

fn click_to_pid(pid: i32, x: f64, y: f64) -> bool {
    #[cfg(target_os = "macos")]
    {
        post_mouse_to_pid(pid, x, y)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, x, y);
        false
    }
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn click_at(x: f64, y: f64) -> Result<()> {
    unsafe {
        let pt = CGPoint { x, y };
        let src = CGEventSourceCreate(0);
        let down = CGEventCreateMouseEvent(src, 1, pt, 0);
        let up = CGEventCreateMouseEvent(src, 2, pt, 0);
        if down.is_null() || up.is_null() {
            bail!("CGEventCreateMouseEvent failed");
        }
        CGEventPost(0, down);
        CGEventPost(0, up);
        CFRelease(down);
        CFRelease(up);
        if !src.is_null() {
            CFRelease(src);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn click_at(_: f64, _: f64) -> Result<()> {
    bail!("click requires macOS")
}

#[cfg(target_os = "macos")]
fn post_mouse_to_pid(pid: i32, x: f64, y: f64) -> bool {
    unsafe {
        let pt = CGPoint { x, y };
        let src = CGEventSourceCreate(0);
        let down = CGEventCreateMouseEvent(src, 1, pt, 0);
        let up = CGEventCreateMouseEvent(src, 2, pt, 0);
        if down.is_null() || up.is_null() {
            return false;
        }
        CGEventPostToPid(pid, down);
        CGEventPostToPid(pid, up);
        CFRelease(down);
        CFRelease(up);
        if !src.is_null() {
            CFRelease(src);
        }
        true
    }
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn key_combo(spec: &str) -> Result<()> {
    let (code, flags) = parse_combo(spec)?;
    unsafe {
        let src = CGEventSourceCreate(0);
        let down = CGEventCreateKeyboardEvent(src, code, true);
        let up = CGEventCreateKeyboardEvent(src, code, false);
        if down.is_null() || up.is_null() {
            bail!("CGEventCreateKeyboardEvent failed");
        }
        if flags != 0 {
            CGEventSetFlags(down, flags);
            CGEventSetFlags(up, flags);
        }
        CGEventPost(0, down);
        CGEventPost(0, up);
        CFRelease(down);
        CFRelease(up);
        if !src.is_null() {
            CFRelease(src);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn key_combo(_: &str) -> Result<()> {
    bail!("keys require macOS")
}

fn key_combo_to_pid(pid: i32, spec: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let (code, flags) = parse_combo(spec)?;
        unsafe {
            let src = CGEventSourceCreate(0);
            let down = CGEventCreateKeyboardEvent(src, code, true);
            let up = CGEventCreateKeyboardEvent(src, code, false);
            if down.is_null() || up.is_null() {
                bail!("CGEventCreateKeyboardEvent failed");
            }
            if flags != 0 {
                CGEventSetFlags(down, flags);
                CGEventSetFlags(up, flags);
            }
            CGEventPostToPid(pid, down);
            CGEventPostToPid(pid, up);
            CFRelease(down);
            CFRelease(up);
            if !src.is_null() {
                CFRelease(src);
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, spec);
        bail!("keys require macOS")
    }
}

fn parse_combo(spec: &str) -> Result<(u16, u64)> {
    let lower = spec.to_ascii_lowercase();
    let mut flags: u64 = 0;
    if lower.contains("command") || lower.contains("cmd") {
        flags |= 0x0010_0000;
    }
    if lower.contains("shift") {
        flags |= 0x0002_0000;
    }
    if lower.contains("option") || lower.contains("alt") {
        flags |= 0x0008_0000;
    }
    if lower.contains("control") || lower.contains("ctrl") {
        flags |= 0x0004_0000;
    }
    let key = lower.rsplit('+').next().unwrap_or("").trim();
    let code = keycode(key).ok_or_else(|| anyhow::anyhow!("unknown key {key}"))?;
    Ok((code, flags))
}

fn keycode(name: &str) -> Option<u16> {
    Some(match name {
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "return" | "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" => 0x33,
        "escape" | "esc" => 0x35,
        "n" => 0x2D,
        "w" => 0x0D,
        "t" => 0x11,
        "q" => 0x0C,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state_id: u32) -> *mut std::ffi::c_void;
    fn CGEventCreateMouseEvent(
        source: *mut std::ffi::c_void,
        mouse_type: u32,
        point: CGPoint,
        button: u32,
    ) -> *mut std::ffi::c_void;
    fn CGEventCreateKeyboardEvent(
        source: *mut std::ffi::c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut std::ffi::c_void;
    fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
    #[allow(dead_code)]
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
    fn CGEventPostToPid(pid: i32, event: *mut std::ffi::c_void);
    fn CFRelease(cf: *mut std::ffi::c_void);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_effect_treats_unchanged_pixels_as_noop() {
        assert_eq!(hash_effect(Some(1), Some(1)), ActionEffect::SuspectedNoop);
        assert_eq!(hash_effect(Some(0), Some(u64::MAX)), ActionEffect::Partial);
        assert_eq!(hash_effect(None, Some(1)), ActionEffect::Unverifiable);
        assert_eq!(hash_effect(None, None), ActionEffect::Unverifiable);
    }

    #[test]
    fn foreground_escalates_on_noop_not_on_partial() {
        assert!(escalate_background_noop(ActionEffect::SuspectedNoop, true));
        assert!(escalate_background_noop(ActionEffect::Unverifiable, true));
        assert!(!escalate_background_noop(ActionEffect::Partial, true));
        assert!(!escalate_background_noop(
            ActionEffect::SuspectedNoop,
            false
        ));
        assert!(!escalate_background_noop(ActionEffect::Unverifiable, false));
    }
}
