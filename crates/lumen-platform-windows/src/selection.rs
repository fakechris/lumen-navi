//! Text-selection port — not implemented on Windows.
//!
//! The macOS backend reads the selection through the Accessibility API. The
//! Windows equivalent is UI Automation (`TextPattern` / `ValuePattern`) plus a
//! low-level mouse hook, which is a separate milestone. Until then every entry
//! point reports "unsupported" so the desktop shell can disable the 划词弹窗
//! and say why, instead of running a monitor that never fires.

use lumen_platform::SelectionInfo;

/// Windows has no Accessibility trust gate, and the selection features that
/// gate depends on are not implemented — always false.
pub fn accessibility_trusted(_prompt: bool) -> bool {
    false
}

pub fn focused_selection() -> Option<SelectionInfo> {
    None
}

pub fn focused_element_pid() -> Option<i32> {
    None
}

pub fn clipboard_grab_selection() -> Option<String> {
    None
}

/// Cursor position in physical screen pixels.
pub fn mouse_location() -> Option<(f64, f64)> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point).ok()? };
        Some((point.x as f64, point.y as f64))
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn start_mouse_up_monitor<F>(_callback: F) -> Result<(), String>
where
    F: Fn(lumen_platform::MouseUp) + Send + 'static,
{
    Err("selection monitor is not implemented on Windows yet".into())
}
