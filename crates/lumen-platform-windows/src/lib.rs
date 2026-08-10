//! Windows platform ports — multi-monitor capture, foreground app, session
//! lock, permissions, OCR.
//!
//! Every module compiles on non-Windows hosts with `Unsupported` stubs so the
//! whole workspace still type-checks from a Mac.

mod capture;
mod dpi;
mod frontmost;
mod lock;
mod ocr;
mod permissions;
mod selection;
mod shell;

pub use capture::{WinDisplays, WinScreenCapturer};
pub use dpi::ensure_process_dpi_aware;
pub use frontmost::WinFrontmost;
pub use lock::{is_screen_locked, WinScreenLock};
pub use ocr::{default_ocr_languages, WinOcr};
pub use permissions::{request_screen_recording, WinPermissions};
pub use selection::{
    accessibility_trusted, clipboard_grab_selection, focused_element_pid, focused_selection,
    mouse_location, start_mouse_up_monitor,
};
pub use shell::{open_uri, privacy_settings_uri};
