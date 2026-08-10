//! Foreground window / owning process.

use async_trait::async_trait;
use lumen_platform::{FrontmostApp, FrontmostAppProbe, PlatformError};

pub struct WinFrontmost;

#[async_trait]
impl FrontmostAppProbe for WinFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        tokio::task::spawn_blocking(frontmost_sync)
            .await
            .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }
}

fn frontmost_sync() -> Result<Option<FrontmostApp>, PlatformError> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        };
        use windows::core::PWSTR;

        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                // No foreground window: lock screen, UAC prompt, or a desktop
                // switch. Not an error — the capture path treats it as unknown.
                return Ok(None);
            }

            let window_title = {
                let len = GetWindowTextLengthW(hwnd);
                if len <= 0 {
                    None
                } else {
                    let mut buf = vec![0u16; len as usize + 1];
                    let copied = GetWindowTextW(hwnd, &mut buf);
                    if copied <= 0 {
                        None
                    } else {
                        Some(String::from_utf16_lossy(&buf[..copied as usize]))
                    }
                }
            };

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return Ok(Some(FrontmostApp {
                    app_name: "unknown".into(),
                    bundle_id: None,
                    window_title,
                    ls_category_type: None,
                    tab_url: None,
                }));
            }

            // LIMITED_INFORMATION is the right-sized ask: it works without
            // elevation for same-integrity processes and still returns the
            // image path.
            let exe_path = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => {
                    let mut buf = vec![0u16; MAX_PATH as usize];
                    let mut len = buf.len() as u32;
                    let ok = QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_FORMAT(0),
                        PWSTR(buf.as_mut_ptr()),
                        &mut len,
                    );
                    let _ = CloseHandle(handle);
                    if ok.is_ok() && len > 0 {
                        Some(String::from_utf16_lossy(&buf[..len as usize]))
                    } else {
                        None
                    }
                }
                // Elevated or protected process (Task Manager, some installers).
                Err(_) => None,
            };

            let file_name = exe_path.as_deref().and_then(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            });
            let app_name = file_name
                .as_deref()
                .map(|n| n.trim_end_matches(".exe").to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "unknown".into());

            Ok(Some(FrontmostApp {
                app_name,
                // Windows has no bundle id. The lowercased executable name is
                // the stable per-app key privacy rules can match on, matching
                // how macOS rules match a bundle id.
                bundle_id: file_name.map(|n| n.to_ascii_lowercase()),
                window_title,
                ls_category_type: None,
                tab_url: None,
            }))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(PlatformError::Unsupported(
            "frontmost requires Windows".into(),
        ))
    }
}
