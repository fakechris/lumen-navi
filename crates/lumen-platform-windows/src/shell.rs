//! Shell integration: open a path or `ms-settings:` URI.

use lumen_platform::PlatformError;

/// Hand a path or URI to the shell (`ShellExecuteW` "open").
///
/// Used for the data folder and the privacy Settings deep links. Unlike
/// `cmd /c start`, this neither flashes a console window nor re-parses the
/// argument as a command line.
pub fn open_uri(target: &str) -> Result<(), PlatformError> {
    #[cfg(target_os = "windows")]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let verb = HSTRING::from("open");
        let file = HSTRING::from(target);
        // ShellExecuteW signals failure with a "HINSTANCE" of 32 or less.
        let result = unsafe {
            ShellExecuteW(None, &verb, &file, None, None, SW_SHOWNORMAL)
        };
        if result.0 as usize <= 32 {
            return Err(PlatformError::Message(format!(
                "ShellExecuteW failed for {target}"
            )));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target;
        Err(PlatformError::Unsupported("open_uri requires Windows".into()))
    }
}

/// Deep link into the Windows privacy Settings page for a capability.
pub fn privacy_settings_uri(kind: &str) -> Option<&'static str> {
    match kind {
        "microphone" => Some("ms-settings:privacy-microphone"),
        "speech" => Some("ms-settings:privacy-speech"),
        // Windows does not gate desktop screen capture, so there is no
        // per-app pane to send the user to; the privacy root is the closest
        // honest destination.
        "screen" => Some("ms-settings:privacy"),
        // No Accessibility-trust analogue.
        _ => None,
    }
}
