//! Windows capability probes.
//!
//! Windows has no TCC. Screen capture is ungated for a desktop process, the
//! microphone is gated by the privacy Consent Store (and, for packaged builds,
//! by the declared MSIX capability), and there is no Accessibility equivalent
//! for the features navi gates on macOS.

use async_trait::async_trait;
use lumen_platform::{PermissionProbe, PermissionState, PermissionStatus, PlatformError};
use lumen_platform_cpal::default_input_available;

pub struct WinPermissions;

#[async_trait]
impl PermissionProbe for WinPermissions {
    async fn status(&self) -> Result<PermissionStatus, PlatformError> {
        Ok(PermissionStatus {
            // A desktop process may read the screen without a prompt.
            screen_recording: PermissionState::Granted,
            microphone: mic_state(),
            // No Windows analogue of the macOS Accessibility gate. The
            // features navi puts behind it (AX text selection) are not
            // implemented here, so `Restricted` — "the system will not let you
            // have this" — is the honest answer, not `Granted`.
            accessibility: PermissionState::Restricted,
        })
    }
}

/// No-op: Windows never prompts a desktop process for screen access.
pub fn request_screen_recording() -> bool {
    true
}

fn mic_state() -> PermissionState {
    #[cfg(target_os = "windows")]
    {
        // Packaged (MSIX/Store) builds get the authoritative answer without
        // prompting, and it survives a Settings change made while running.
        if let Some(state) = app_capability_mic_state() {
            return state;
        }
    }
    // Unpackaged builds have no queryable grant, so device enumeration is the
    // best signal: WASAPI hides input devices from a process denied by the
    // privacy setting.
    if default_input_available() {
        PermissionState::Granted
    } else {
        PermissionState::NotDetermined
    }
}

#[cfg(target_os = "windows")]
fn app_capability_mic_state() -> Option<PermissionState> {
    use windows::core::HSTRING;
    use windows::Security::Authorization::AppCapabilityAccess::{
        AppCapability, AppCapabilityAccessStatus,
    };

    let capability = AppCapability::Create(&HSTRING::from("Microphone")).ok()?;
    let status = capability.CheckAccess().ok()?;
    match status {
        AppCapabilityAccessStatus::Allowed => Some(PermissionState::Granted),
        AppCapabilityAccessStatus::DeniedByUser => Some(PermissionState::Denied),
        AppCapabilityAccessStatus::DeniedBySystem => Some(PermissionState::Restricted),
        AppCapabilityAccessStatus::UserPromptRequired => Some(PermissionState::NotDetermined),
        // Unpackaged build: it cannot declare a capability, so this says
        // nothing. Fall through to the device probe.
        _ => None,
    }
}
