//! Detect whether an app is currently preventing the display from sleeping.
//!
//! When Safari plays Netflix, Zoom runs a call, or any app holds a Caffeine-
//! style assertion, it registers an IOKit power assertion of type
//! `PreventDisplaySleep` or `PreventUserIdleSystemSleep`. A pure HID-idle
//! detector then miscounts the user as AFK (they're watching a 20-min lecture
//! without touching the mouse). `IOPMCopyAssertionsByProcess` returns the live
//! assertion table keyed by pid; we scan it for those two types. This is
//! exactly Timing's "app keeps your Mac awake" heuristic.
//!
//! Read-only, no TCC permission. Runs in spawn_blocking like the idle probe.

use std::ffi::c_void;
use std::os::raw::c_char;

#[async_trait::async_trait]
impl lumen_platform::DisplaySleepProbe for MacPower {
    async fn display_sleep_prevented(&self) -> Result<bool, lumen_platform::PlatformError> {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::task::spawn_blocking(display_sleep_prevented_native),
        )
        .await
        {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) | Err(_) => Ok(false),
        }
    }
}

pub struct MacPower;

// IOKit / Power framework FFI.
//
// NOTE: `IOPMCopyAssertionsByProcess` uses the out-parameter pattern — it
// writes the result dictionary through `*AssertionsByPid` and returns an
// `IOReturn` (kern_return_t = i32) status code. Declaring it as a direct
// return was an ABI mismatch that caused the returned `IOReturn` int (0 on
// success) to be reinterpreted as a CFDictionary pointer, and the real
// dictionary was written to a garbage out-address — SIGSEGV inside CF's
// CF_IS_OBJC header check on CFDictionaryGetCount. (Apple SDK header:
// IOPMLib.h `IOPMCopyAssertionsByProcess(CFDictionaryRef *)`.)
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMCopyAssertionsByProcess(assertions_by_pid: *mut *const c_void) -> i32;
}

/// kIOReturnSuccess
const K_IO_RETURN_SUCCESS: i32 = 0;

/// True if any process currently holds a display-sleep / user-idle-sleep
/// assertion. On any error reading the table, returns false (fail-open: don't
/// suppress idle detection on probe failure).
fn display_sleep_prevented_native() -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
    #[cfg(target_os = "macos")]
    unsafe {
        scan_assertions()
    }
}

/// Assertion *values* that mean "keep the user's screen awake." The first is
/// what video playback and explicit Caffeine assertions hold; the second is
/// the broader "prevent system sleep on user idle" (calls, media,
/// `caffeinate -i`). The key name (`AssertionType` vs legacy `AssertType`)
/// is handled in `lookup_assertion_type`.
const BLOCKING_TYPES: &[&[u8]] = &[b"PreventDisplaySleep", b"PreventUserIdleSystemSleep"];

#[cfg(target_os = "macos")]
unsafe fn scan_assertions() -> bool {
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::{CFRelease, CFTypeRef};
    use core_foundation_sys::dictionary::{
        CFDictionaryGetCount, CFDictionaryGetKeysAndValues, CFDictionaryRef,
    };

    // Out-parameter call: the dictionary is written through the pointer, and
    // the return value is an IOReturn status code (0 = success).
    let mut dict_ptr: *const c_void = std::ptr::null();
    let kr = IOPMCopyAssertionsByProcess(&mut dict_ptr);
    if kr != K_IO_RETURN_SUCCESS || dict_ptr.is_null() {
        return false;
    }
    let dict = dict_ptr as CFDictionaryRef;

    let count = CFDictionaryGetCount(dict) as usize;
    if count == 0 {
        CFRelease(dict as CFTypeRef);
        return false;
    }

    let mut keys: Vec<CFTypeRef> = Vec::with_capacity(count);
    let mut vals: Vec<CFTypeRef> = Vec::with_capacity(count);
    CFDictionaryGetKeysAndValues(dict, keys.as_mut_ptr(), vals.as_mut_ptr());
    keys.set_len(count);
    vals.set_len(count);

    let mut hit = false;
    'outer: for v in &vals {
        if v.is_null() {
            continue;
        }
        let arr = *v as CFArrayRef;
        let n = CFArrayGetCount(arr) as usize;
        for i in 0..n {
            let entry = CFArrayGetValueAtIndex(arr, i as isize) as CFDictionaryRef;
            if entry.is_null() {
                continue;
            }
            if assertion_type_blocks(entry) {
                hit = true;
                break 'outer;
            }
        }
    }
    // Keys (pids) are not inspected; the vecs exist only to receive the pairs.
    drop(keys);

    CFRelease(dict as CFTypeRef);
    hit
}

/// Read one assertion dict's type value; true if it's a blocking type. Tries
/// both `AssertionType` (modern) and `AssertType` (legacy alias) keys — older
/// macOS releases and some assertion creators use the short form.
#[cfg(target_os = "macos")]
unsafe fn assertion_type_blocks(
    entry: core_foundation_sys::dictionary::CFDictionaryRef,
) -> bool {
    // Try both known key names; whichever resolves gives the type string.
    lookup_assertion_type(entry, c"AssertionType".as_ptr())
        .or_else(|| lookup_assertion_type(entry, c"AssertType".as_ptr()))
        .is_some_and(|matches_blocking| matches_blocking)
}

/// Look up `key` in the assertion dict, read its CFString value into bytes,
/// and return `Some(true)` if it's a blocking type, `Some(false)` if it's a
/// known non-blocking type, `None` if the key/value is absent or unreadable.
#[cfg(target_os = "macos")]
unsafe fn lookup_assertion_type(
    entry: core_foundation_sys::dictionary::CFDictionaryRef,
    key_bytes: *const c_char,
) -> Option<bool> {
    use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease, CFTypeRef, Boolean};
    use core_foundation_sys::dictionary::CFDictionaryGetValueIfPresent;
    use core_foundation_sys::string::{
        CFStringCreateWithCString, CFStringGetCString, CFStringGetCStringPtr, CFStringRef,
        kCFStringEncodingUTF8,
    };

    let cf_key = CFStringCreateWithCString(kCFAllocatorDefault, key_bytes, kCFStringEncodingUTF8);
    if cf_key.is_null() {
        return None;
    }
    let mut out: CFTypeRef = std::ptr::null();
    let found: Boolean = CFDictionaryGetValueIfPresent(entry, cf_key as *const c_void, &mut out);
    CFRelease(cf_key as CFTypeRef);
    // core-foundation-sys Boolean is u8; nonzero == true.
    if found == 0 || out.is_null() {
        return None;
    }

    let val_ref = out as CFStringRef;
    let ptr = CFStringGetCStringPtr(val_ref, kCFStringEncodingUTF8);
    if ptr.is_null() {
        // Fallback: copy into a buffer (some CFStrings don't expose a direct ptr).
        let mut buf = [0u8; 64];
        let ok: Boolean = CFStringGetCString(
            val_ref,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as isize,
            kCFStringEncodingUTF8,
        );
        if ok == 0 {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(BLOCKING_TYPES.iter().any(|t| *t == &buf[..len]))
    } else {
        let bytes = std::ffi::CStr::from_ptr(ptr).to_bytes();
        Some(BLOCKING_TYPES.iter().any(|t| *t == bytes))
    }
}

#[cfg(test)]
mod tests {
    // Only meaningful on a real macOS session.
    #[test]
    fn power_probe_returns_finite_bool_on_real_machine() {
        if std::env::var_os("CI").is_some() {
            return;
        }
        // Just assert it doesn't panic / hang; the value depends on whether any
        // app currently holds an assertion (e.g. this test running while music
        // plays → true; otherwise false).
        let _ = super::display_sleep_prevented_native();
    }
}
