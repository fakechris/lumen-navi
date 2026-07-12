//! Screen lock detection.

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
