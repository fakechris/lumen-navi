//! Act idle snapshot. HID idle uses IOKit (no TCC); frontmost uses CGWindowList.

use crate::protocol::IdleStatus;

pub fn snapshot() -> IdleStatus {
    #[cfg(target_os = "macos")]
    {
        let hid = hid_idle_seconds_blocking();
        let front = lumen_platform_macos::frontmost_app();
        let locked = lumen_platform_macos::is_screen_locked();
        IdleStatus {
            hid_idle_seconds: hid,
            frontmost_app: front.as_ref().map(|a| a.app_name.clone()),
            frontmost_bundle_id: front.as_ref().and_then(|a| a.bundle_id.clone()),
            frontmost_pid: front.as_ref().and_then(|a| a.pid),
            frontmost_window_id: front.as_ref().and_then(|a| a.window_id),
            screen_locked: locked,
            focus_lock_held: crate::focus_lock::is_held(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        IdleStatus {
            hid_idle_seconds: 0.0,
            frontmost_app: None,
            frontmost_bundle_id: None,
            frontmost_pid: None,
            frontmost_window_id: None,
            screen_locked: false,
            focus_lock_held: false,
        }
    }
}

#[cfg(target_os = "macos")]
fn hid_idle_seconds_blocking() -> f64 {
    // MacIdle::idle_seconds is async (spawn_blocking + timeout). Cua's
    // execute() already runs on the server runtime, but a direct IOKit read
    // is cheaper and avoids nesting runtimes. Duplicate the registry path
    // used by lumen-platform-macos::idle so we never call the SkyLight API.
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::CFRelease;
    use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
    use std::ffi::c_void;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IORegistryEntryFromPath(main_port: u32, path: *const std::ffi::c_char) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: core_foundation_sys::string::CFStringRef,
            allocator: core_foundation_sys::base::CFAllocatorRef,
            options: u32,
        ) -> core_foundation_sys::base::CFTypeRef;
        fn IOObjectRelease(object: u32) -> i32;
    }

    const PATH: *const std::ffi::c_char =
        b"IOService:/IOHIDSystem\0".as_ptr() as *const std::ffi::c_char;
    unsafe {
        let entry = IORegistryEntryFromPath(0, PATH);
        if entry == 0 {
            return 0.0;
        }
        let key = CFString::new("HIDIdleTime");
        let prop =
            IORegistryEntryCreateCFProperty(entry, key.as_concrete_TypeRef(), std::ptr::null(), 0);
        IOObjectRelease(entry);
        if prop.is_null() {
            return 0.0;
        }
        let mut nanos: i64 = 0;
        let ok = CFNumberGetValue(
            prop as CFNumberRef,
            kCFNumberSInt64Type,
            &mut nanos as *mut i64 as *mut c_void,
        );
        CFRelease(prop);
        if !ok || nanos < 0 {
            return 0.0;
        }
        nanos as f64 / 1_000_000_000.0
    }
}
