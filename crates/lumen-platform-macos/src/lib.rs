//! Compatibility facade for macOS platform ports during context migration.
//!
//! Observe capture and process OCR — does **not** use cua-driver.

mod mic;
mod permissions;

pub use lumen_context::macos::{
    default_ocr_languages, is_screen_locked, MacDisplays, MacFrontmost, MacScreenCapturer,
    MacScreenLock, MacVisionOcr,
};
pub use mic::{default_input_available, MacMicCapturer};
pub use permissions::{request_screen_recording, MacPermissions};
