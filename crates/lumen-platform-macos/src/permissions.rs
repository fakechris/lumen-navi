//! Screen / mic / accessibility permission probes.

use async_trait::async_trait;
use lumen_platform::{PermissionProbe, PermissionState, PermissionStatus, PlatformError};

/// macOS permission probe.
pub struct MacPermissions;

#[async_trait]
impl PermissionProbe for MacPermissions {
    async fn status(&self) -> Result<PermissionStatus, PlatformError> {
        Ok(PermissionStatus {
            screen_recording: screen_recording_state(),
            microphone: microphone_permission_state(),
            accessibility: accessibility_permission_state(),
        })
    }
}

pub fn microphone_permission_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        mic_state_from_av_status(unsafe { lumen_microphone_authorization_status() })
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

fn mic_state_from_av_status(status: i32) -> PermissionState {
    match status {
        3 => PermissionState::Granted,
        2 => PermissionState::Denied,
        1 => PermissionState::Restricted,
        _ => PermissionState::NotDetermined,
    }
}

/// Ask macOS for microphone consent through AVFoundation and wait for the
/// user's answer. Unlike probing a CoreAudio stream, this API creates the TCC
/// record that System Settings displays.
pub fn request_microphone_access() -> Result<bool, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        match unsafe { lumen_request_microphone_access() } {
            3 => Ok(true),
            1 | 2 => Ok(false),
            -1 => Err(PlatformError::Message(
                "microphone permission request timed out".into(),
            )),
            status => Err(PlatformError::Message(format!(
                "unexpected microphone authorization status: {status}"
            ))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(PlatformError::Unsupported(
            "microphone permission request requires macOS".into(),
        ))
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

/// Whether this process currently has access to window contents during screen capture.
pub fn screen_recording_access_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightScreenCaptureAccess() }
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
        if screen_recording_access_granted() {
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

pub fn accessibility_permission_state() -> PermissionState {
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

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_microphone_authorization_status() -> i32;
    fn lumen_request_microphone_access() -> i32;
}

#[cfg(test)]
mod tests {
    use super::mic_state_from_av_status;
    use lumen_platform::PermissionState;

    #[test]
    fn maps_avfoundation_microphone_authorization_states() {
        assert_eq!(mic_state_from_av_status(0), PermissionState::NotDetermined);
        assert_eq!(mic_state_from_av_status(1), PermissionState::Restricted);
        assert_eq!(mic_state_from_av_status(2), PermissionState::Denied);
        assert_eq!(mic_state_from_av_status(3), PermissionState::Granted);
        assert_eq!(mic_state_from_av_status(-1), PermissionState::NotDetermined);
    }
}
