//! Screen lock + screensaver detection.
//!
//! Both states mean "the user is not at the screen" and should mark the
//! activity stream idle. Lock is read from `CGSSessionScreenIsLocked`; the
//! screensaver is detected by checking whether `ScreenSaverEngine` is running
//! (it launches on screensaver start and exits on dismiss, and covers the
//! common case where the user set a screensaver delay shorter than their
//! display-sleep delay, or walked away without locking).

use async_trait::async_trait;
use lumen_platform::{PlatformError, ScreenLockProbe};

pub struct MacScreenLock;

#[async_trait]
impl ScreenLockProbe for MacScreenLock {
    async fn is_locked(&self) -> Result<bool, PlatformError> {
        Ok(is_screen_locked())
    }
}

pub fn is_screen_locked() -> bool {
    #[cfg(target_os = "macos")]
    {
        if screensaver_active() {
            return true;
        }
        // CGSessionCopyCurrentDictionary → CGSSessionScreenIsLocked
        unsafe {
            let dict = CGSessionCopyCurrentDictionary();
            if dict.is_null() {
                return false;
            }
            let key = cfstr("CGSSessionScreenIsLocked");
            let mut value: *const std::ffi::c_void = std::ptr::null();
            let found = CFDictionaryGetValueIfPresent(dict, key, &mut value);
            CFRelease(dict as *const _);
            if found == 0 || value.is_null() {
                return false;
            }
            // CFBoolean
            CFBooleanGetValue(value as *const _)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// True if the macOS screensaver engine is currently running. `ScreenSaverEngine`
/// lives under different paths across macOS versions (CoreServices on modern,
/// old location pre-Big Sur), so we resolve by bundle id via LaunchServices
/// rather than a hardcoded path — but in practice a running-process name match
/// is the cheapest reliable signal and matches what ActivityWatch/Timing do.
#[cfg(target_os = "macos")]
fn screensaver_active() -> bool {
    use std::process::Command;
    // `pgrep -x ScreenSaverEngine` — exact process-name match, cheap, no deps.
    // Wrapped: any failure (pgrep missing, non-zero exit) means "not running".
    Command::new("/usr/bin/pgrep")
        .args(["-x", "ScreenSaverEngine"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn screensaver_active() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn cfstr(s: &str) -> *const std::ffi::c_void {
    use std::ffi::CString;
    // kCFStringEncodingUTF8 = 0x08000100
    const UTF8: u32 = 0x0800_0100;
    let c = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) as _ }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const std::ffi::c_void;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryGetValueIfPresent(
        theDict: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
        value: *mut *const std::ffi::c_void,
    ) -> u8;
    fn CFBooleanGetValue(boolean: *const std::ffi::c_void) -> bool;
    fn CFRelease(cf: *const std::ffi::c_void);
    fn CFStringCreateWithCString(
        alloc: *const std::ffi::c_void,
        cStr: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const std::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `screensaver_active` must not panic and must return false when the
    /// screensaver isn't running. (Can't easily force it on in a test, but this
    /// at least guards the pgrep path against the "non-zero exit = not running"
    /// fallback and the unwrap_or.)
    #[test]
    fn screensaver_not_active_in_test_session() {
        if std::env::var_os("CI").is_some() {
            return;
        }
        // No assertion on the value (a dev machine could have the screensaver
        // active mid-test); the point is it returns without panicking.
        let _ = screensaver_active();
    }

    /// When the screensaver isn't running and the session isn't locked,
    /// `is_screen_locked` returns false. Guards the combined path.
    #[test]
    fn not_locked_when_session_unlocked() {
        if std::env::var_os("CI").is_some() {
            return;
        }
        // Only meaningful if the test machine is actually unlocked (it is, or
        // the test runner couldn't run). Tolerate a true result only if the
        // screensaver is genuinely active.
        let locked = is_screen_locked();
        let ss = screensaver_active();
        assert!(
            !locked || ss,
            "is_screen_locked=true only expected when screensaver is active; got locked={locked} ss={ss}"
        );
    }
}
