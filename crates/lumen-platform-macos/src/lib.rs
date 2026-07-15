//! macOS platform ports — multi-display capture, frontmost, lock, permissions, OCR, mic, ASR.
//!
//! Observe capture and process enrichment — does **not** use cua-driver.

mod asr;
mod capture;
mod frontmost;
mod lock;
mod mic;
mod ocr;
mod permissions;

pub use asr::MacSpeechAsr;
pub use capture::{MacDisplays, MacScreenCapturer};
pub use frontmost::MacFrontmost;
pub use lock::{is_screen_locked, MacScreenLock};
pub use mic::{default_input_available, MacMicCapturer};
pub use ocr::{default_ocr_languages, MacVisionOcr};
pub use permissions::{request_screen_recording, MacPermissions};
