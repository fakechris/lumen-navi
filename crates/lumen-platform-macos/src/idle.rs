//! System-wide idle (AFK) detection via IOKit HID idle time.
//!
//! The `IOHIDSystem` registry entry publishes a `HIDIdleTime` property —
//! nanoseconds since the last HID input event (keyboard/mouse) system-wide.
//! Reading it is a single IOKit registry property fetch and needs **no TCC
//! permission**.
//!
//! This powers the time-tracking "is the user actually at the keyboard" signal,
//! replacing the old SessionManager heuristic that inferred idle from gaps
//! between capture ticks (which miscounts still-reading time as active).
//!
//! History: this used to call `CGEventSourceSecondsSinceLastEventType`. That
//! call can deadlock **forever** on a SkyLight/WindowServer-internal global
//! mutex (observed in production: 500+ threads parked in
//! `SLEventSourceSecondsSinceLastEventType` → `__psynch_mutexwait`). A
//! `spawn_blocking` thread cannot be cancelled, so each timed-out call leaked
//! a thread until the tokio blocking pool (max 512) was exhausted and the
//! whole daemon silently froze. The IOKit registry path never touches
//! SkyLight, which is why it is used here instead.

#[cfg(target_os = "macos")]
use std::ffi::c_void;

#[async_trait::async_trait]
impl lumen_platform::IdleProbe for MacIdle {
    async fn idle_seconds(&self) -> Result<f64, lumen_platform::PlatformError> {
        // IOKit registry reads are not known to wedge like the old
        // CGEventSource path did, but keep the historical belt-and-braces:
        // run off the async executor and bound the whole call so any stuck
        // OS call can never freeze the activity tracker.
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::task::spawn_blocking(idle_seconds_native),
        )
        .await
        {
            Ok(Ok(Some(secs))) => Ok(secs),
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                // Timeout, panic, or IOKit returned None — treat as "unknown", 0.0.
                Ok(0.0)
            }
        }
    }
}

pub struct MacIdle;

// IOKit entry points, declared in the same style this file used for
// CoreGraphics. `io_registry_entry_t` / `io_object_t` are mach ports (u32).
#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    /// Look up a registry entry by full path, e.g. "IOService:/IOHIDSystem".
    fn IORegistryEntryFromPath(
        main_port: u32,
        path: *const std::ffi::c_char,
    ) -> u32;
    /// Create a CF property for a registry entry. Caller must CFRelease.
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: core_foundation_sys::string::CFStringRef,
        allocator: core_foundation_sys::base::CFAllocatorRef,
        options: u32,
    ) -> core_foundation_sys::base::CFTypeRef;
    fn IOObjectRelease(object: u32) -> i32;
}

// kIOMainPortDefault == MACH_PORT_NULL (0): asks IOKit for the default main
// port. (kIOMasterPortDefault is the deprecated alias for the same constant.)
#[cfg(target_os = "macos")]
const K_IO_MAIN_PORT_DEFAULT: u32 = 0;
// NUL-terminated registry path to the HID system object.
#[cfg(target_os = "macos")]
const HID_SYSTEM_PATH: *const std::ffi::c_char =
    b"IOService:/IOHIDSystem\0".as_ptr() as *const std::ffi::c_char;

/// Seconds since the last system-wide HID input (keyboard or mouse), or `None`
/// if the IOKit lookup fails (e.g. no HIDSystem entry in a headless context).
fn idle_seconds_native() -> Option<f64> {
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
    #[cfg(target_os = "macos")]
    unsafe {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use core_foundation_sys::base::CFRelease;
        use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};

        let entry = IORegistryEntryFromPath(K_IO_MAIN_PORT_DEFAULT, HID_SYSTEM_PATH);
        if entry == 0 {
            return None;
        }
        let key = CFString::new("HIDIdleTime");
        let prop = IORegistryEntryCreateCFProperty(
            entry,
            key.as_concrete_TypeRef(),
            std::ptr::null(),
            0,
        );
        IOObjectRelease(entry);
        if prop.is_null() {
            return None;
        }

        // HIDIdleTime is nanoseconds since the last HID event, as a CFNumber.
        let mut nanos: i64 = 0;
        let ok = CFNumberGetValue(
            prop as CFNumberRef,
            kCFNumberSInt64Type,
            &mut nanos as *mut i64 as *mut c_void,
        );
        CFRelease(prop);
        if !ok || nanos < 0 {
            return None;
        }
        Some(nanos as f64 / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn idle_returns_finite_nonneg_on_real_machine() {
        // Only meaningful on a real macOS session; guard so CI without a
        // windowserver doesn't flake. In a headless context the IOHIDSystem
        // entry may be absent and idle_seconds_native() returns None — that
        // is a valid outcome, so the assertions stay inside the `if let`.
        if std::env::var_os("CI").is_some() {
            return;
        }
        if let Some(secs) = super::idle_seconds_native() {
            assert!(secs.is_finite(), "idle seconds must be finite");
            assert!(secs >= 0.0, "idle seconds must be non-negative");
        }
    }
}
