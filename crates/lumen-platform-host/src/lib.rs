//! Picks the `lumen-platform` backend for the build target.
//!
//! This is the only crate allowed to know which OS it is compiled for.
//! Consumers (`lumen-daemon`, the desktop shell) ask for a port and get the
//! implementation that exists here, so adding a platform never means adding
//! `#[cfg]` to product code.

use std::sync::Arc;

use lumen_platform::{
    AsrEngine, DisplayEnumerator, DisplaySleepProbe, FrontmostAppProbe, IdleProbe, MicCapturer,
    OcrEngine, PermissionProbe, ScreenCapturer, ScreenLockProbe,
};

pub mod selection;
pub mod shell;

#[cfg(target_os = "macos")]
use lumen_platform_macos as backend;
#[cfg(target_os = "windows")]
use lumen_platform_windows as backend;

/// What the current OS backend can actually do. Drives UI copy so the shell
/// never offers a macOS-only remedy on Windows (or the reverse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCapabilities {
    /// `macos` | `windows` | `other`
    pub os: &'static str,
    pub screen_capture: bool,
    pub microphone: bool,
    pub ocr: bool,
    /// A built-in OS speech recognizer usable as the `speech` ASR engine.
    pub system_speech_asr: bool,
    /// Reading the selected text of another app (划词弹窗).
    pub text_selection: bool,
    /// The OS gates screen capture behind a permission the user must grant.
    pub screen_permission_gate: bool,
    /// The OS has an Accessibility-style trust gate.
    pub accessibility_gate: bool,
}

pub const fn capabilities() -> HostCapabilities {
    #[cfg(target_os = "macos")]
    {
        HostCapabilities {
            os: "macos",
            screen_capture: true,
            microphone: true,
            ocr: true,
            system_speech_asr: true,
            text_selection: true,
            screen_permission_gate: true,
            accessibility_gate: true,
        }
    }
    #[cfg(target_os = "windows")]
    {
        HostCapabilities {
            os: "windows",
            screen_capture: true,
            microphone: true,
            ocr: true,
            // No shippable built-in recognizer; local SenseVoice/Whisper or a
            // cloud engine covers ASR instead.
            system_speech_asr: false,
            // UI Automation selection capture is a later milestone.
            text_selection: false,
            screen_permission_gate: false,
            accessibility_gate: false,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        HostCapabilities {
            os: "other",
            screen_capture: false,
            microphone: true,
            ocr: false,
            system_speech_asr: false,
            text_selection: false,
            screen_permission_gate: false,
            accessibility_gate: false,
        }
    }
}

pub fn displays() -> Arc<dyn DisplayEnumerator> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacDisplays)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinDisplays)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(lumen_platform::NullDisplays)
    }
}

pub fn screen_capturer() -> Arc<dyn ScreenCapturer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacScreenCapturer)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinScreenCapturer)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(lumen_platform::NullCapturer)
    }
}

pub fn idle() -> Arc<dyn IdleProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacIdle)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(lumen_platform::NullIdle)
    }
}

pub fn display_sleep() -> Arc<dyn DisplaySleepProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacPower)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(lumen_platform::NullDisplaySleep)
    }
}

pub fn frontmost() -> Arc<dyn FrontmostAppProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacFrontmost)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinFrontmost)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(lumen_platform::NullFrontmost)
    }
}

pub fn screen_lock() -> Arc<dyn ScreenLockProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacScreenLock)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinScreenLock)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(lumen_platform::NullScreenLock)
    }
}

pub fn permissions() -> Arc<dyn PermissionProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacPermissions)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinPermissions)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(lumen_platform::NullPermissions)
    }
}

pub fn ocr(max_image_bytes: usize) -> Arc<dyn OcrEngine> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacVisionOcr::with_max_image_bytes(max_image_bytes))
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(backend::WinOcr::with_max_image_bytes(max_image_bytes))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = max_image_bytes;
        Arc::new(lumen_platform::NullOcr)
    }
}

pub fn default_ocr_languages() -> Vec<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::default_ocr_languages()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// cpal covers CoreAudio and WASAPI, so every target shares one mic backend.
pub fn mic() -> Arc<dyn MicCapturer> {
    Arc::new(lumen_platform_cpal::CpalMicCapturer)
}

/// The OS's built-in speech recognizer, or a `NullAsr` that reports
/// `is_supported() == false` where there is none.
pub fn system_speech_asr(max_audio_bytes: usize) -> Arc<dyn AsrEngine> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(backend::MacSpeechAsr::with_max_audio_bytes(max_audio_bytes))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = max_audio_bytes;
        Arc::new(lumen_platform::NullAsr)
    }
}

/// Fail-closed screen lock probe used as a hard capture gate.
pub fn is_screen_locked() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::is_screen_locked()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Ask the OS for screen capture access. No-op where capture is ungated.
pub fn request_screen_recording() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::request_screen_recording()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

pub fn request_microphone_access() -> Result<bool, lumen_platform::PlatformError> {
    #[cfg(target_os = "macos")]
    {
        backend::request_microphone_access()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(lumen_platform_cpal::default_input_available())
    }
}
