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
}

/// Read a CFString attribute of an AX element (e.g. `kAXTitleAttribute`,
/// `AXRole`). Returns `None` on any AX error or when the value is not a string.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
pub unsafe fn ax_string_attr(element: AxUIElementRef, name: &str) -> Option<String> {
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
