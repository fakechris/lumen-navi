//! Shared macOS Accessibility (AX) FFI plumbing.
//!
//! Both the selection popup (`selection.rs`) and the frontmost-app probe
//! (`frontmost.rs`) need the same set of `AXUIElement*` calls. The type
//! aliases, extern block, and small helpers live here so neither file has to
//! re-declare them, and so the frontmost probe can read window titles without
//! pulling in the selection-only business logic.
//!
//! Requires macOS Accessibility permission (see `selection::accessibility_trusted`).

use std::ffi::c_void;

#[cfg(target_os = "macos")]
use core_foundation::base::{TCFType, CFTypeRef};
#[cfg(target_os = "macos")]
use core_foundation::string::{CFString, CFStringRef};

pub type AxUIElementRef = *const c_void;
pub type AxValueRef = *const c_void;
/// `AXError` — 0 == kAXErrorSuccess.
pub type AxError = i32;
/// `AXValueType` is a CFIndex enum.
pub type AxValueType = i64;

pub const K_AX_VALUE_TYPE_CGRECT: AxValueType = 3;
pub const K_AX_VALUE_TYPE_CF_RANGE: AxValueType = 4;

#[repr(C)]
pub struct CFRange {
    pub location: isize,
    pub length: isize,
}

/// RAII release guard for a CF/AX object pointer. Calls `CFRelease` on drop
/// when the pointer is non-null.
pub struct ReleaseGuard(pub *const c_void);
impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { core_foundation_sys::base::CFRelease(self.0) };
        }
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
        -> bool;
    pub fn AXUIElementCreateSystemWide() -> AxUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AxUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AxError;
    pub fn AXUIElementCopyParameterizedAttributeValue(
        element: AxUIElementRef,
        attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AxError;
    pub fn AXUIElementGetPid(element: AxUIElementRef, pid: *mut i32) -> AxError;
    pub fn AXUIElementCreateApplication(pid: i32) -> AxUIElementRef;
    pub fn AXUIElementSetAttributeValue(
        element: AxUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AxError;
    pub fn AXValueCreate(the_type: AxValueType, value_ptr: *const c_void) -> AxValueRef;
    pub fn AXValueGetType(value: AxValueRef) -> AxValueType;
    pub fn AXValueGetValue(value: AxValueRef, the_type: AxValueType, value_ptr: *mut c_void) -> bool;
    /// Set the messaging timeout for an AXUIElement. Bounds how long a single
    /// AX IPC call can block — without this, a hung app stalls the caller
    /// indefinitely. screenpipe applies 0.2s on every walk root.
    pub fn AXUIElementSetMessagingTimeout(element: AxUIElementRef, timeout: f64) -> AxError;
}

/// Read a CFString attribute of an AX element (e.g. `kAXTitleAttribute`,
/// `AXRole`). Returns `None` on any AX error or when the value is not a string.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
pub unsafe fn ax_string_attr(element: AxUIElementRef, name: &str) -> Option<String> {
    use core_foundation_sys::base::CFGetTypeID;
    use core_foundation_sys::string::CFStringGetTypeID;

    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) != 0
        || value.is_null()
    {
        return None;
    }
    // AXUIElementCopyAttributeValue follows Create rule (+1 retain).
    // Use wrap_under_create_rule so Drop releases exactly once.
    // After extracting the String, the CFString (and its backing CFTypeRef)
    // is released by Drop — no manual CFRelease needed.
    if CFGetTypeID(value) != CFStringGetTypeID() {
        core_foundation_sys::base::CFRelease(value);
        return None;
    }
    let cf_str = core_foundation::string::CFString::wrap_under_create_rule(value as core_foundation::string::CFStringRef);
    Some(cf_str.to_string())
}

/// Read a non-string CFType attribute as a retained `CFTypeRef` (caller must
/// release). Useful when you need to inspect the type before converting.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
pub unsafe fn ax_attr(element: AxUIElementRef, name: &str) -> Option<CFTypeRef> {
    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) != 0
        || value.is_null()
    {
        return None;
    }
    Some(value)
}

/// Title of the focused window of the application owning `pid`, via
/// `AXUIElementCreateApplication` → `kAXFocusedWindowAttribute` →
/// `kAXTitleAttribute`. Same permission path as the selection popup.
///
/// Returns `None` when Accessibility is not granted, the app has no window,
/// or the window has no title.
pub fn focused_window_title(pid: i32) -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let _app_guard = ReleaseGuard(app as *const c_void);

        let mut focused_window: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(
            app,
            CFString::new("AXFocusedWindow").as_concrete_TypeRef(),
            &mut focused_window,
        ) != 0
            || focused_window.is_null()
        {
            return None;
        }
        let _win_guard = ReleaseGuard(focused_window);

        ax_string_attr(focused_window as AxUIElementRef, "AXTitle").filter(|s| !s.is_empty())
    }
}

/// Force-enable AX tree materialization for a Chromium/Electron app. Writes
/// `AXEnhancedUserInterface=true` + `AXManualAccessibility=true` on the app's
/// root AXUIElement. Without this poke, Electron apps (Slack/VS Code/Notion/
/// Discord) return an opaque single-node tree.
///
/// **Process-global TTL cache** (60s): re-poking forces Chromium to
/// synchronously rebuild its AX tree, which can commit a pending
/// composition/autocomplete buffer into the focused field ("phantom text"
/// bug, screenpipe issue). The cache ensures we poke at most once per 60s per
/// pid across the whole process (walker + popup share it).
///
/// Returns `true` when the poke actually fired (caller should sleep ~150ms for
/// the tree to materialize); `false` when cached (no sleep needed).
pub fn ensure_enhanced_ax_for_pid(pid: i32) -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        false
    }
    #[cfg(target_os = "macos")]
    {
        use std::collections::HashMap;
        use std::sync::{LazyLock, Mutex};
        use std::time::{Duration, Instant};

        /// 60s — long enough that the tree stays materialized across typical
        /// poll intervals, short enough to re-poke after a background→foreground
        /// cycle. Matches screenpipe's `DEFAULT_TTL`.
        const TTL: Duration = Duration::from_secs(60);
        static CACHE: LazyLock<Mutex<HashMap<i32, Instant>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));

        // Check + update the cache under the lock.
        let needs_poke = {
            let mut cache = CACHE.lock().unwrap();
            let now = Instant::now();
            match cache.get(&pid) {
                Some(t) if now.duration_since(*t) < TTL => false,
                _ => {
                    cache.insert(pid, now);
                    // Evict stale entries to bound memory.
                    cache.retain(|_, t| now.duration_since(*t) < TTL * 5);
                    true
                }
            }
        };
        if !needs_poke {
            return false;
        }

        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                return false;
            }
            let _guard = ReleaseGuard(app as *const c_void);
            let on = core_foundation::boolean::CFBoolean::from(true);
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
            tracing::debug!(pid, err_manual = e1, err_enhanced = e2, "AX enhanced-mode poke");
        }
        true
    }
}
