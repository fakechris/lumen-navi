//! macOS platform ports — multi-display capture, frontmost, lock, permissions, OCR, ASR.
//!
//! Observe capture and process enrichment — does **not** use cua-driver.
//!
//! The crate is empty off macOS. `lumen-platform-host` only pulls it into the
//! dependency graph for macOS targets, so there is no reason to carry
//! always-failing stubs — and pretending to build them hid the fact that they
//! did not.

#![cfg(target_os = "macos")]

mod asr;
pub mod ax;
mod capture;
mod clipboard;
mod frontmost;
mod idle;
mod lock;
mod ocr;
mod power;
mod permissions;
mod selection;

pub use asr::MacSpeechAsr;
pub use capture::{MacDisplays, MacScreenCapturer};
pub use clipboard::clipboard_grab_selection;
pub use frontmost::MacFrontmost;
pub use idle::MacIdle;
pub use lock::{is_screen_locked, MacScreenLock};
pub use ocr::{default_ocr_languages, MacVisionOcr};
pub use power::MacPower;
pub use permissions::{
    accessibility_permission_state, microphone_permission_state, request_microphone_access,
    request_screen_recording, screen_recording_access_granted, MacPermissions,
};
pub use selection::{
    accessibility_trusted, focused_element_pid, focused_selection, maybe_selection, mouse_location,
    normalize_selection, start_mouse_up_monitor, MouseUp, SelectionInfo,
};
