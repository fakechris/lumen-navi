//! Text-selection capture (Accessibility API) + global mouse-up monitor.
//!
//! Powers the desktop selection popup (划词弹窗, PopClip-style). This is a
//! user-triggered UI surface only — Observe capture never uses it.
//!
//! Requires macOS Accessibility permission (`accessibility_trusted`).

use std::ffi::c_void;

#[cfg(target_os = "macos")]
use core_foundation::base::{TCFType, CFTypeRef};
#[cfg(target_os = "macos")]
use core_foundation::string::{CFString, CFStringRef};

/// What the Accessibility API reported for the frontmost text selection.
#[derive(Debug, Clone)]
pub struct SelectionInfo {
    pub text: String,
    /// Screen rect (x, y, w, h) in global points (top-left origin), if available.
    pub bounds: Option<(f64, f64, f64, f64)>,
    /// PID owning the focused element (used by the desktop to skip itself).
    pub pid: Option<i32>,
}

/// Trim + normalize raw AX text; returns None when effectively empty.
/// Truncates to `max_chars` (char-boundary safe).
pub fn normalize_selection(raw: &str, max_chars: usize) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max_chars.max(1)).collect())
}

/// Whether this process is Accessibility-trusted. `prompt = true` shows the
/// system dialog once (same pattern as `AXIsProcessTrustedWithOptions`).
pub fn accessibility_trusted(prompt: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        if prompt {
            use core_foundation::boolean::CFBoolean;
            use core_foundation::dictionary::CFDictionary;
            let key = CFString::new("AXTrustedCheckOptionPrompt");
            let value = CFBoolean::from(prompt);
            let pairs = [(key.as_CFType(), value.as_CFType())];
            let options = CFDictionary::from_CFType_pairs(&pairs);
            return unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) };
        }
        return unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = prompt;
        false
    }
}

/// Current mouse location in global screen points (top-left origin).
pub fn mouse_location() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            core_foundation_sys::base::CFRelease(event);
            Some((p.x, p.y))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Selected text (+ optional bounds) of the focused element in the frontmost
/// app. Returns None when there is no selection or AX is unavailable.
pub fn focused_selection() -> Option<SelectionInfo> {
    #[cfg(target_os = "macos")]
    {
        unsafe { focused_selection_native() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Pid owning the currently focused AX element — cheap probe used by the
/// desktop's fast-dismiss path (clicks on our own windows must not hide the
/// popup).
pub fn focused_element_pid() -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        unsafe { focused_element_pid_native() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// What a left-mouse-up means for selection tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseUp {
    /// Drag distance since mouse-down exceeded a few px, or a multi-click —
    /// a selection is plausible and worth querying (with retries).
    pub maybe_selection: bool,
}

/// Whether a mouse-up could plausibly have produced a selection.
/// `down_pos`/`up_pos` in global points; `click_state` 1 = single click.
pub fn maybe_selection(
    down_pos: Option<(f64, f64)>,
    up_pos: (f64, f64),
    click_state: i64,
) -> bool {
    if click_state > 1 {
        return true; // double/triple click selects word/line
    }
    match down_pos {
        Some((dx, dy)) => {
            let dist2 = (up_pos.0 - dx).powi(2) + (up_pos.1 - dy).powi(2);
            dist2 > 5.0 * 5.0 // drag → possible selection
        }
        None => true, // no down info → be conservative
    }
}

/// Spawn a background thread firing `callback` on every global left-mouse-up.
/// Listen-only tap; re-enables itself when macOS disables the tap by timeout.
/// Fails (Err) when the tap cannot be created (e.g. no Accessibility trust).
pub fn start_mouse_up_monitor<F>(callback: F) -> Result<(), String>
where
    F: Fn(MouseUp) + Send + 'static,
{
    #[cfg(target_os = "macos")]
    {
        return start_mouse_up_monitor_native(callback);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = callback;
        Err("mouse-up monitor is only implemented on macOS".into())
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
type AXUIElementRef = *const c_void;
#[cfg(target_os = "macos")]
type AXValueRef = *const c_void;
/// `AXError` — 0 == kAXErrorSuccess.
#[cfg(target_os = "macos")]
type AXError = i32;
/// `AXValueType` is a CFIndex enum.
#[cfg(target_os = "macos")]
type AXValueType = i64;

#[cfg(target_os = "macos")]
const K_AX_VALUE_TYPE_CGRECT: AXValueType = 3;
#[cfg(target_os = "macos")]
const K_AX_VALUE_TYPE_CF_RANGE: AXValueType = 4;

#[cfg(target_os = "macos")]
#[repr(C)]
struct CFRange {
    location: isize,
    length: isize,
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
        -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(the_type: AXValueType, value_ptr: *const c_void) -> AXValueRef;
    fn AXValueGetType(value: AXValueRef) -> AXValueType;
    fn AXValueGetValue(value: AXValueRef, the_type: AXValueType, value_ptr: *mut c_void) -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> core_graphics::geometry::CGPoint;
    fn CGEventTapEnable(tap: *const c_void, enable: bool);
}

#[cfg(target_os = "macos")]
struct ReleaseGuard(*const c_void);
#[cfg(target_os = "macos")]
impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { core_foundation_sys::base::CFRelease(self.0) };
        }
    }
}

/// Read a CFString attribute of an AX element (helper for role/diagnostics).
#[cfg(target_os = "macos")]
unsafe fn ax_string_attr(element: AXUIElementRef, name: &str) -> Option<String> {
    use core_foundation_sys::base::{CFGetTypeID, CFRelease};
    use core_foundation_sys::string::CFStringGetTypeID;

    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) != 0
        || value.is_null()
    {
        return None;
    }
    if CFGetTypeID(value) != CFStringGetTypeID() {
        CFRelease(value);
        return None;
    }
    let s = CFString::wrap_under_get_rule(value as CFStringRef).to_string();
    CFRelease(value);
    Some(s)
}

/// Non-empty AXSelectedText of an element, with the AX error logged.
#[cfg(target_os = "macos")]
unsafe fn selected_text_of(element: AXUIElementRef) -> Option<String> {
    use core_foundation_sys::base::{CFGetTypeID, CFRelease};
    use core_foundation_sys::string::CFStringGetTypeID;

    let attr = CFString::new("AXSelectedText");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != 0 || value.is_null() {
        let role = ax_string_attr(element, "AXRole").unwrap_or_else(|| "?".into());
        let subrole = ax_string_attr(element, "AXSubrole").unwrap_or_default();
        tracing::debug!(err, role, subrole, "AXSelectedText unavailable");
        return None;
    }
    if CFGetTypeID(value) != CFStringGetTypeID() {
        CFRelease(value);
        return None;
    }
    let text = CFString::wrap_under_get_rule(value as CFStringRef).to_string();
    CFRelease(value);
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// Parent of an AX element (retained), or None.
#[cfg(target_os = "macos")]
unsafe fn ax_parent(element: AXUIElementRef) -> Option<AXUIElementRef> {
    let attr = CFString::new("AXParent");
    let mut value: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) != 0
        || value.is_null()
    {
        return None;
    }
    Some(value as AXUIElementRef)
}

/// WebKit/Safari path: AXSelectedText is kAXErrorNoValue on AXWebArea, but
/// the selection is vended as an AXTextMarkerRange — pull the string via
/// AXStringForTextMarkerRange (same API VoiceOver-style clients use).
#[cfg(target_os = "macos")]
unsafe fn selected_text_via_markers(element: AXUIElementRef) -> Option<String> {
    use core_foundation_sys::base::{CFGetTypeID, CFRelease};
    use core_foundation_sys::string::CFStringGetTypeID;

    let attr_range = CFString::new("AXSelectedTextMarkerRange");
    let mut range: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr_range.as_concrete_TypeRef(), &mut range) != 0
        || range.is_null()
    {
        return None;
    }
    let _range_guard = ReleaseGuard(range as *const c_void);

    let attr_string = CFString::new("AXStringForTextMarkerRange");
    let mut out: CFTypeRef = std::ptr::null();
    if AXUIElementCopyParameterizedAttributeValue(
        element,
        attr_string.as_concrete_TypeRef(),
        range,
        &mut out,
    ) != 0
        || out.is_null()
    {
        return None;
    }
    if CFGetTypeID(out) != CFStringGetTypeID() {
        CFRelease(out);
        return None;
    }
    let text = CFString::wrap_under_get_rule(out as CFStringRef).to_string();
    CFRelease(out);
    if text.is_empty() {
        return None;
    }
    tracing::debug!(chars = text.chars().count(), "selection via AXTextMarkerRange");
    Some(text)
}

/// Chromium/Electron build their accessibility tree lazily; nudge the app to
/// create it up front (same trick PopClip-style tools use). Once per pid.
#[cfg(target_os = "macos")]
unsafe fn activate_ax_for_pid(pid: i32) {
    use core_foundation::boolean::CFBoolean;
    use std::sync::atomic::{AtomicI32, Ordering};

    static ACTIVATED_PID: AtomicI32 = AtomicI32::new(-1);
    if ACTIVATED_PID.swap(pid, Ordering::SeqCst) == pid {
        return;
    }
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return;
    }
    let _guard = ReleaseGuard(app as *const c_void);
    let on = CFBoolean::from(true);
    let manual = CFString::new("AXManualAccessibility");
    let e1 = AXUIElementSetAttributeValue(
        app,
        manual.as_concrete_TypeRef(),
        on.as_concrete_TypeRef() as CFTypeRef,
    );
    let enhanced = CFString::new("AXEnhancedUserInterface");
    let e2 = AXUIElementSetAttributeValue(
        app,
        enhanced.as_concrete_TypeRef(),
        on.as_concrete_TypeRef() as CFTypeRef,
    );
    tracing::debug!(pid, err_manual = e1, err_enhanced = e2, "AX activation nudge");
}

#[cfg(target_os = "macos")]
unsafe fn focused_element_pid_native() -> Option<i32> {
    let system_wide = AXUIElementCreateSystemWide();
    if system_wide.is_null() {
        return None;
    }
    let _sys_guard = ReleaseGuard(system_wide as *const c_void);
    let attr = CFString::new("AXFocusedUIElement");
    let mut focused: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(system_wide, attr.as_concrete_TypeRef(), &mut focused) != 0
        || focused.is_null()
    {
        return None;
    }
    let _focused_guard = ReleaseGuard(focused as *const c_void);
    let mut pid: i32 = 0;
    if AXUIElementGetPid(focused as AXUIElementRef, &mut pid) == 0 {
        Some(pid)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
unsafe fn focused_selection_native() -> Option<SelectionInfo> {
    let system_wide = AXUIElementCreateSystemWide();
    if system_wide.is_null() {
        tracing::debug!("AXUIElementCreateSystemWide returned null");
        return None;
    }
    let _sys_guard = ReleaseGuard(system_wide as *const c_void);

    let attr_focused = CFString::new("AXFocusedUIElement");
    let mut focused: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(
        system_wide,
        attr_focused.as_concrete_TypeRef(),
        &mut focused,
    );
    if err != 0 || focused.is_null() {
        tracing::debug!(err, "AXFocusedUIElement unavailable");
        return None;
    }
    let mut current = focused as AXUIElementRef;
    let mut current_guard = ReleaseGuard(focused as *const c_void);

    // Owner pid (desktop excludes its own windows with this).
    let mut raw_pid: i32 = 0;
    let pid = if AXUIElementGetPid(current, &mut raw_pid) == 0 {
        Some(raw_pid)
    } else {
        None
    };

    if let Some(p) = pid {
        activate_ax_for_pid(p);
    }

    // Try the focused element, then walk up a few parents — some apps
    // (Safari web content among them) report focus on a container while the
    // selection lives on an ancestor/nearby element.
    for depth in 0..=3u8 {
        let text = selected_text_of(current).or_else(|| unsafe {
            selected_text_via_markers(current)
        });
        if let Some(text) = text {
            let bounds = selection_bounds(current);
            drop(current_guard);
            return Some(SelectionInfo { text, bounds, pid });
        }
        if depth == 3 {
            break;
        }
        match ax_parent(current) {
            Some(parent) => {
                // Replacing the guard drops (releases) the previous element.
                current_guard = ReleaseGuard(parent as *const c_void);
                current = parent;
            }
            None => break,
        }
    }
    None
}

/// Best-effort screen rect of the selection via kAXBoundsForRange.
#[cfg(target_os = "macos")]
unsafe fn selection_bounds(focused: AXUIElementRef) -> Option<(f64, f64, f64, f64)> {
    use core_foundation_sys::base::CFRelease;

    let attr_range = CFString::new("AXSelectedTextRange");
    let mut range_value: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(focused, attr_range.as_concrete_TypeRef(), &mut range_value)
        != 0
        || range_value.is_null()
    {
        return None;
    }
    if AXValueGetType(range_value as AXValueRef) != K_AX_VALUE_TYPE_CF_RANGE {
        CFRelease(range_value);
        return None;
    }
    let mut range = CFRange {
        location: 0,
        length: 0,
    };
    if !AXValueGetValue(
        range_value as AXValueRef,
        K_AX_VALUE_TYPE_CF_RANGE,
        &mut range as *mut CFRange as *mut c_void,
    ) {
        CFRelease(range_value);
        return None;
    }
    // Fresh AXValue as the parameterized-attribute argument.
    let param = AXValueCreate(
        K_AX_VALUE_TYPE_CF_RANGE,
        &range as *const CFRange as *const c_void,
    );
    CFRelease(range_value);
    if param.is_null() {
        return None;
    }

    let attr_bounds = CFString::new("AXBoundsForRange");
    let mut bounds_value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyParameterizedAttributeValue(
        focused,
        attr_bounds.as_concrete_TypeRef(),
        param as CFTypeRef,
        &mut bounds_value,
    );
    CFRelease(param as *const c_void);
    if err != 0 || bounds_value.is_null() {
        return None;
    }
    let mut rect = core_graphics::geometry::CGRect::new(
        &core_graphics::geometry::CGPoint::new(0.0, 0.0),
        &core_graphics::geometry::CGSize::new(0.0, 0.0),
    );
    let ok = AXValueGetType(bounds_value as AXValueRef) == K_AX_VALUE_TYPE_CGRECT
        && AXValueGetValue(
            bounds_value as AXValueRef,
            K_AX_VALUE_TYPE_CGRECT,
            &mut rect as *mut core_graphics::geometry::CGRect as *mut c_void,
        );
    CFRelease(bounds_value);
    if ok {
        Some((
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn start_mouse_up_monitor_native<F>(callback: F) -> Result<(), String>
where
    F: Fn(MouseUp) + Send + 'static,
{
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    };
    use std::sync::{mpsc, Arc, Mutex};

    // Handshake so tap-creation failures inside the thread become a
    // synchronous Err for the caller (thread spawn alone proves nothing).
    let (tx, rx) = mpsc::channel::<Result<(), String>>();

    std::thread::Builder::new()
        .name("lumen-selection-monitor".into())
        .spawn(move || {
            // Slot so the tap callback can re-enable its own tap after macOS
            // disables it (TapDisabledByTimeout). Raw port as usize for Send.
            let tap_slot: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
            let tap_slot_cb = Arc::clone(&tap_slot);
            // Mouse-down position for drag detection (plain clicks can't
            // create a selection → callers may dismiss instantly).
            let down_pos: Arc<Mutex<Option<(f64, f64)>>> = Arc::new(Mutex::new(None));
            let down_pos_cb = Arc::clone(&down_pos);

            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![CGEventType::LeftMouseDown, CGEventType::LeftMouseUp],
                move |_proxy, etype, event| {
                    match etype {
                        CGEventType::LeftMouseDown => {
                            let p = event.location();
                            if let Ok(mut guard) = down_pos_cb.lock() {
                                *guard = Some((p.x, p.y));
                            }
                        }
                        CGEventType::LeftMouseUp => {
                            let p = event.location();
                            // kCGMouseEventClickState = 23
                            let clicks = event.get_integer_value_field(23);
                            let down = down_pos_cb.lock().ok().and_then(|g| *g);
                            callback(MouseUp {
                                maybe_selection: maybe_selection(down, (p.x, p.y), clicks),
                            });
                        }
                        CGEventType::TapDisabledByTimeout
                        | CGEventType::TapDisabledByUserInput => {
                            if let Ok(guard) = tap_slot_cb.lock() {
                                if let Some(port) = *guard {
                                    unsafe { CGEventTapEnable(port as *const c_void, true) };
                                }
                            }
                        }
                        _ => {}
                    }
                    None
                },
            );
            let tap = match tap {
                Ok(t) => t,
                Err(()) => {
                    let _ = tx.send(Err(
                        "CGEventTapCreate failed (Accessibility not granted?)".into()
                    ));
                    return;
                }
            };
            if let Ok(mut guard) = tap_slot.lock() {
                *guard = Some(tap.mach_port.as_concrete_TypeRef() as *const c_void as usize);
            }

            let run_loop = CFRunLoop::get_current();
            unsafe {
                match tap.mach_port.create_runloop_source(0) {
                    Ok(source) => run_loop.add_source(&source, kCFRunLoopCommonModes),
                    Err(()) => {
                        let _ = tx.send(Err("CFMachPortCreateRunLoopSource failed".into()));
                        return;
                    }
                }
            }
            tap.enable();
            let _ = tx.send(Ok(()));
            tracing::info!("selection mouse-up monitor started");
            CFRunLoop::run_current();
        })
        .map_err(|e| format!("spawn selection monitor thread: {e}"))?;

    match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(result) => result,
        Err(e) => Err(format!("selection monitor handshake timeout: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_rejects_empty() {
        assert_eq!(normalize_selection("  hello  ", 100), Some("hello".into()));
        assert_eq!(normalize_selection("   ", 100), None);
        assert_eq!(normalize_selection("", 100), None);
    }

    #[test]
    fn normalize_truncates_on_char_boundary() {
        let s = "你好世界abc";
        assert_eq!(normalize_selection(s, 4), Some("你好世界".into()));
        assert_eq!(normalize_selection(s, 100), Some(s.into()));
        // max_chars clamped to >= 1
        assert_eq!(normalize_selection(s, 0), Some("你".into()));
    }

    #[test]
    fn maybe_selection_drag_and_multiclick() {
        // plain click (no movement) → no selection
        assert!(!maybe_selection(Some((100.0, 100.0)), (102.0, 101.0), 1));
        // drag → selection plausible
        assert!(maybe_selection(Some((100.0, 100.0)), (160.0, 100.0), 1));
        // double click without movement → word selection
        assert!(maybe_selection(Some((100.0, 100.0)), (100.0, 100.0), 2));
        // no down info → conservative
        assert!(maybe_selection(None, (100.0, 100.0), 1));
    }
}
