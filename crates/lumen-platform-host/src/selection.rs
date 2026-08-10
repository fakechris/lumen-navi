//! Text-selection port for the desktop 划词弹窗.
//!
//! Implemented on macOS via Accessibility; a no-op on Windows until UI
//! Automation lands. `supported()` lets the shell say so instead of running a
//! monitor that can never fire.

pub use lumen_platform::{normalize_selection, MouseUp, SelectionInfo};

#[cfg(target_os = "macos")]
use lumen_platform_macos as backend;
#[cfg(target_os = "windows")]
use lumen_platform_windows as backend;

/// Whether this OS backend can read another app's selected text.
pub const fn supported() -> bool {
    crate::capabilities().text_selection
}

/// Whether the process holds whatever trust the OS requires to read a
/// selection. `prompt = true` may show the system dialog once.
pub fn accessibility_trusted(prompt: bool) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::accessibility_trusted(prompt)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = prompt;
        false
    }
}

pub fn focused_selection() -> Option<SelectionInfo> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::focused_selection()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

pub fn focused_element_pid() -> Option<i32> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::focused_element_pid()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Copy-key fallback for apps that expose no accessible selection.
pub fn clipboard_grab_selection() -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::clipboard_grab_selection()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

pub fn mouse_location() -> Option<(f64, f64)> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::mouse_location()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Fire `callback` on every global left-mouse-up. `Err` when the OS backend
/// cannot install the monitor (missing trust, or not implemented).
pub fn start_mouse_up_monitor<F>(callback: F) -> Result<(), String>
where
    F: Fn(MouseUp) + Send + 'static,
{
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        backend::start_mouse_up_monitor(callback)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = callback;
        Err("selection monitor is not implemented on this platform".into())
    }
}
