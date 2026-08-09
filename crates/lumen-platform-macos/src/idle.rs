//! System-wide idle (AFK) detection via CoreGraphics.
//!
//! `CGEventSourceSecondsSinceLastEventType` returns seconds since the last HID
//! input event (keyboard/mouse) system-wide. It is a single read-only call and
//! needs **no TCC permission** (unlike CGEventTap for content-level events) —
//! ActivityWatch's `aw-watcher-afk` uses exactly this call.
//!
//! This powers the time-tracking "is the user actually at the keyboard" signal,
//! replacing the old SessionManager heuristic that inferred idle from gaps
//! between capture ticks (which miscounts still-reading time as active).

use std::ffi::c_void;

#[async_trait::async_trait]
impl lumen_platform::IdleProbe for MacIdle {
    async fn idle_seconds(&self) -> Result<f64, lumen_platform::PlatformError> {
        // CGEventSource calls can block on internal CoreGraphics locks (notably
        // when the frontmost app is loginwindow / no HID session is attached).
        // Run them off the async executor and bound the whole call so a stuck
        // CG call can never freeze the activity tracker.
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::task::spawn_blocking(idle_seconds_native),
        )
        .await
        {
            Ok(Ok(Some(secs))) => Ok(secs),
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                // Timeout, panic, or CG returned None — treat as "unknown", 0.0.
                Ok(0.0)
            }
        }
    }
}

pub struct MacIdle;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// `CFRunLoopSourceRef` / `CFAllocator`-shaped handle — we pass null and
    /// treat it as opaque.
    fn CGEventSourceCreate(state_id: u32) -> *const c_void;
    /// Seconds since the last event of `eventType` from the given source.
    fn CGEventSourceSecondsSinceLastEventType(source: *const c_void, event_type: u32) -> f64;
}

// kCGEventSourceStateHIDSystemState = 1 (combines all HID input sessions).
const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: u32 = 1;
// kCGAnyInputEventType = 0xFFFFFFFF (any keyboard/mouse event).
const K_CG_ANY_INPUT_EVENT_TYPE: u32 = 0xFFFF_FFFF;

/// Seconds since the last system-wide HID input (keyboard or mouse), or `None`
/// if the CoreGraphics call fails (e.g. running in a headless context).
fn idle_seconds_native() -> Option<f64> {
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
    #[cfg(target_os = "macos")]
    unsafe {
        use core_foundation_sys::base::CFRelease;

        let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE);
        if source.is_null() {
            return None;
        }
        let secs = CGEventSourceSecondsSinceLastEventType(source, K_CG_ANY_INPUT_EVENT_TYPE);
        CFRelease(source);

        // CG returns -1.0 on error.
        if secs < 0.0 {
            return None;
        }
        Some(secs)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn idle_returns_finite_nonneg_on_real_machine() {
        // Only meaningful on a real macOS session; guard so CI without a
        // windowserver doesn't flake.
        if std::env::var_os("CI").is_some() {
            return;
        }
        if let Some(secs) = super::idle_seconds_native() {
            assert!(secs.is_finite(), "idle seconds must be finite");
            assert!(secs >= 0.0, "idle seconds must be non-negative");
        }
    }
}
