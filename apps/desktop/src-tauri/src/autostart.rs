//! System startup (autostart / login items) management.

#[cfg(target_os = "macos")]
pub mod macos {
    use std::path::PathBuf;
    use std::process::Command;

    pub const APP_LOGIN_ITEM_NAME: &str = "Lumen Navi";

    /// Query whether Lumen Navi is in the macOS Login Items list.
    pub fn is_autostart_enabled() -> Result<bool, String> {
        let script = format!(
            r#"tell application "System Events" to get (exists login item "{}")"#,
            APP_LOGIN_ITEM_NAME
        );
        let output = Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("osascript exec error: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("osascript failed: {stderr}"));
        }
        let stdout = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_lowercase();
        Ok(stdout == "true")
    }

    /// Add or remove Lumen Navi from macOS Login Items.
    pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
        // First delete any existing items matching this name to avoid duplicates or stale paths.
        let clean_script = format!(
            r#"tell application "System Events" to delete (every login item whose name is "{}")"#,
            APP_LOGIN_ITEM_NAME
        );
        let _ = Command::new("osascript")
            .args(["-e", &clean_script])
            .output();

        if enabled {
            let app_path = resolve_app_path()?;
            let add_script = format!(
                r#"tell application "System Events" to make login item at end with properties {{name:"{}", path:"{}", hidden:false}}"#,
                APP_LOGIN_ITEM_NAME,
                app_path.display()
            );
            let output = Command::new("osascript")
                .args(["-e", &add_script])
                .output()
                .map_err(|e| format!("osascript exec error: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("failed to create login item: {stderr}"));
            }
        }
        Ok(())
    }

    /// Resolves the .app bundle path for Lumen Navi.
    /// Prefers the enclosing .app if running from one, otherwise checks /Applications/Lumen Navi.app
    /// or ~/Applications/Lumen Navi.app before falling back to the current executable.
    pub fn resolve_app_path() -> Result<PathBuf, String> {
        if let Ok(exe) = std::env::current_exe() {
            let exe_str = exe.to_string_lossy();
            if let Some(idx) = exe_str.find(".app/") {
                let candidate = PathBuf::from(&exe_str[..idx + 4]);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }

        let main_app = PathBuf::from("/Applications/Lumen Navi.app");
        if main_app.exists() {
            return Ok(main_app);
        }

        if let Ok(home) = std::env::var("HOME") {
            let user_app = PathBuf::from(home).join("Applications/Lumen Navi.app");
            if user_app.exists() {
                return Ok(user_app);
            }
        }

        std::env::current_exe().map_err(|e| format!("current_exe error: {e}"))
    }
}

/// Query whether autostart is enabled on the current platform.
pub fn is_autostart_enabled(app: &tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        macos::is_autostart_enabled()
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().map_err(|e| e.to_string())
    }
}

/// Enable or disable autostart on the current platform.
pub fn set_autostart_enabled(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        macos::set_autostart_enabled(enabled)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        if enabled {
            manager.enable().map_err(|e| e.to_string())
        } else {
            manager.disable().map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_resolve_app_path() {
        let path = macos::resolve_app_path();
        assert!(path.is_ok(), "resolve_app_path should succeed");
    }

    #[test]
    fn test_shell_config_autostart_roundtrip() {
        let toml_str = r#"
onboarding_completed = true
onboarding_skipped = false
onboarding_step = 4
launch_observe = true
autostart = true
"#;
        let cfg: crate::shell::ShellConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.autostart);
        assert!(cfg.launch_observe);

        let default_cfg: crate::shell::ShellConfig = toml::from_str("").unwrap();
        assert!(!default_cfg.autostart);
    }
}
