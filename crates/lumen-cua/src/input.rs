//! Explicit Act input. Observe never calls this.

use anyhow::{bail, Context, Result};
use std::thread;
use std::time::Duration;

use crate::protocol::InputStep;

pub fn replay(steps: &[InputStep]) -> Result<()> {
    if steps.is_empty() {
        bail!("empty replay");
    }
    if steps.len() > 16 {
        bail!("replay too long");
    }
    for step in steps {
        run_step(step)?;
        let wait = step.wait_ms.unwrap_or(180).min(2_000);
        thread::sleep(Duration::from_millis(wait));
    }
    Ok(())
}

fn run_step(step: &InputStep) -> Result<()> {
    match step.action.as_str() {
        "focus" | "activate" => {
            let bundle = step
                .bundle_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("focus needs bundle_id")?;
            activate_bundle(bundle)?;
        }
        "click" => {
            if let Some(bundle) = step.bundle_id.as_deref().filter(|s| !s.is_empty()) {
                let _ = activate_bundle(bundle);
                thread::sleep(Duration::from_millis(120));
            }
            if let (Some(nx), Some(ny)) = (step.nx, step.ny) {
                let (x, y) = resolve_point(step, nx, ny)?;
                click_at(x, y)?;
            } else {
                bail!("click needs relative nx/ny");
            }
        }
        "shortcut" | "submit" | "key" => {
            let keys = step.keys.as_deref().filter(|s| !s.is_empty()).context("key step needs keys")?;
            if let Some(bundle) = step.bundle_id.as_deref().filter(|s| !s.is_empty()) {
                let _ = activate_bundle(bundle);
                thread::sleep(Duration::from_millis(80));
            }
            key_combo(keys)?;
        }
        "type" => {
            // No recorded text — skip rather than invent.
            tracing::info!("skip type step (no recorded text)");
        }
        other => bail!("unsupported replay action {other}"),
    }
    Ok(())
}

fn resolve_point(step: &InputStep, nx: f64, ny: f64) -> Result<(f64, f64)> {
    let nx = nx.clamp(0.02, 0.98);
    let ny = ny.clamp(0.02, 0.98);
    if let Some(frame) = window_frame(step.bundle_id.as_deref(), step.window.as_deref()) {
        Ok((frame.0 + nx * frame.2, frame.1 + ny * frame.3))
    } else {
        bail!("could not resolve window frame for click")
    }
}

#[cfg(target_os = "macos")]
fn activate_bundle(bundle_id: &str) -> Result<()> {
    let app = running_app(bundle_id).context("app not running")?;
    let ok = unsafe {
        app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(0))
    };
    if !ok {
        bail!("activate {bundle_id} failed");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn running_app(bundle_id: &str) -> Option<objc2::rc::Retained<objc2_app_kit::NSRunningApplication>> {
    use objc2_app_kit::NSWorkspace;
    let ws = NSWorkspace::sharedWorkspace();
    let apps = ws.runningApplications();
    for app in apps {
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
    use lumen_platform_macos::ax::{
        ax_point_attr, ax_size_attr, ax_string_attr, AxUIElementRef, ReleaseGuard,
        AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    };
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

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

#[cfg(target_os = "macos")]
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
fn key_combo(spec: &str) -> Result<()> {
    let lower = spec.to_ascii_lowercase();
    let mut flags: u64 = 0;
    if lower.contains("command") || lower.contains("cmd") {
        flags |= 0x0010_0000; // kCGEventFlagMaskCommand
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
    let code = keycode(key).with_context(|| format!("unknown key {key}"))?;
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
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
    fn CFRelease(cf: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
mod _core_foundation_sys_shim {}
