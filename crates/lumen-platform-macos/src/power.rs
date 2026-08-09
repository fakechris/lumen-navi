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

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    /// Returns a CFDictionary (retained) mapping pid (CFNumber) → CFArray of
    /// assertion CFDictionary, or NULL on failure. Caller must CFRelease.
    fn IOPMCopyAssertionsByProcess() -> *const c_void;
}

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

/// Assertion types that mean "keep the user's screen awake." The first is what
/// video playback and explicit Caffeine assertions hold; the second is the
/// broader "prevent system sleep on user idle" (calls, media, `caffeinate -i`).
const BLOCKING_TYPES: &[&[u8]] = &[b"PreventDisplaySleep", b"PreventUserIdleSystemSleep"];

#[cfg(target_os = "macos")]
unsafe fn scan_assertions() -> bool {
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::{CFRelease, CFTypeRef};
    use core_foundation_sys::dictionary::{
        CFDictionaryGetCount, CFDictionaryGetKeysAndValues, CFDictionaryRef,
    };

    let dict = IOPMCopyAssertionsByProcess();
    if dict.is_null() {
        return false;
    }
    let dict = dict as CFDictionaryRef;

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

/// Read one assertion dict's `AssertionType` value; true if it's a blocking type.
#[cfg(target_os = "macos")]
unsafe fn assertion_type_blocks(
    entry: core_foundation_sys::dictionary::CFDictionaryRef,
) -> bool {
    use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease, CFTypeRef, Boolean};
    use core_foundation_sys::dictionary::CFDictionaryGetValueIfPresent;
    use core_foundation_sys::string::{
        CFStringCreateWithCString, CFStringGetCString, CFStringGetCStringPtr, CFStringRef,
        kCFStringEncodingUTF8,
    };

    let key_bytes = c"AssertionType".as_ptr();
    let cf_key = CFStringCreateWithCString(kCFAllocatorDefault, key_bytes, kCFStringEncodingUTF8);
    if cf_key.is_null() {
        return false;
    }

    let mut out: CFTypeRef = std::ptr::null();
    let found: Boolean = CFDictionaryGetValueIfPresent(entry, cf_key as *const c_void, &mut out);
    CFRelease(cf_key as CFTypeRef);
    // core-foundation-sys Boolean is u8; nonzero == true.
    if found == 0 || out.is_null() {
        return false;
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
            return false;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        BLOCKING_TYPES.iter().any(|t| *t == &buf[..len])
    } else {
        let bytes = std::ffi::CStr::from_ptr(ptr).to_bytes();
        BLOCKING_TYPES.iter().any(|t| *t == bytes)
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
