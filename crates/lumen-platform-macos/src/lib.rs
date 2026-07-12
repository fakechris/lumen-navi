//! macOS platform ports — multi-display capture, frontmost, lock, permissions.
//!
//! Observe plane only — does **not** use cua-driver.

mod capture;
mod frontmost;
mod lock;
mod permissions;

pub use capture::{MacDisplays, MacScreenCapturer};
pub use frontmost::MacFrontmost;
pub use lock::{is_screen_locked, MacScreenLock};
pub use permissions::{request_screen_recording, MacPermissions};
