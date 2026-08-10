//! Opening paths and system-settings deep links.

use std::path::Path;

use lumen_platform::PlatformError;

/// Reveal a folder (or file) in the OS file manager.
pub fn open_path(path: &Path) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        spawn_open(path.as_os_str())
    }
    #[cfg(target_os = "windows")]
    {
        lumen_platform_windows::open_uri(&path.display().to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        spawn_xdg_open(path.as_os_str())
    }
}

/// Hand a URL or OS settings URI to the default handler.
pub fn open_uri(uri: &str) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        spawn_open(std::ffi::OsStr::new(uri))
    }
    #[cfg(target_os = "windows")]
    {
        lumen_platform_windows::open_uri(uri)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        spawn_xdg_open(std::ffi::OsStr::new(uri))
    }
}

/// Deep link to the privacy settings page for `kind`
/// (`screen` | `microphone` | `speech` | `accessibility`), when the OS has one.
pub fn privacy_settings_uri(kind: &str) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        match kind {
            "screen" => Some(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture",
            ),
            "microphone" => Some(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone",
            ),
            "speech" => Some(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_SpeechRecognition",
            ),
            "accessibility" => Some(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
            ),
            _ => None,
        }
    }
    #[cfg(target_os = "windows")]
    {
        lumen_platform_windows::privacy_settings_uri(kind)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = kind;
        None
    }
}

#[cfg(target_os = "macos")]
fn spawn_open(target: &std::ffi::OsStr) -> Result<(), PlatformError> {
    std::process::Command::new("open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| PlatformError::Message(format!("open: {e}")))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn spawn_xdg_open(target: &std::ffi::OsStr) -> Result<(), PlatformError> {
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| PlatformError::Message(format!("xdg-open: {e}")))
}
