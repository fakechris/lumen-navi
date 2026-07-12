//! macOS platform ports — screen capture, frontmost app, permissions.
//!
//! Capture uses CoreGraphics `CGDisplayCreateImage` (not cua-driver; observe ≠ act).
//! Screen Recording TCC must be granted for non-empty frames on modern macOS.

mod capture;
mod frontmost;
mod permissions;

pub use capture::MacScreenCapturer;
pub use frontmost::MacFrontmost;
pub use permissions::{request_screen_recording, MacPermissions};
