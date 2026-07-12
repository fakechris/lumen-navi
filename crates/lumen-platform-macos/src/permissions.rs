//! Screen / mic / accessibility permission probes.

use async_trait::async_trait;
use lumen_platform::{
    PermissionProbe, PermissionState, PermissionStatus, PlatformError,
};

/// macOS permission probe.
pub struct MacPermissions;

#[async_trait]
impl PermissionProbe for MacPermissions {
    async fn status(&self) -> Result<PermissionStatus, PlatformError> {
        Ok(PermissionStatus {
            screen_recording: screen_recording_state(),
            microphone: PermissionState::NotDetermined,
            accessibility: accessibility_state(),
        })
    }
}

/// Request Screen Recording access (may show system prompt once).
pub fn request_screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGRequestScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn screen_recording_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        // CGPreflight returns whether the process may capture without prompting.
        if unsafe { CGPreflightScreenCaptureAccess() } {
            PermissionState::Granted
        } else {
            // Distinguish denied vs not-determined is imperfect without private APIs;
            // treat preflight false as NotDetermined until a capture fails.
            PermissionState::NotDetermined
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

fn accessibility_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        // AXIsProcessTrusted — optional for intake; used later for window titles.
        let trusted = unsafe { AXIsProcessTrusted() };
        if trusted {
            PermissionState::Granted
        } else {
            PermissionState::NotDetermined
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}
