//! macOS platform ports.
//!
//! Phase S0: stubs that compile on all hosts and return `Unsupported` /
//! `NotDetermined` so the daemon can boot headlessly. Real ScreenCaptureKit /
//! AVFoundation / Accessibility wiring lands in S2–S3.

use async_trait::async_trait;
use lumen_platform::{
    FrontmostApp, FrontmostAppProbe, PermissionProbe, PermissionState, PermissionStatus,
    PlatformError, ScreenCapturer, ScreenshotFrame,
};

/// macOS permission probe (stub).
pub struct MacPermissions;

#[async_trait]
impl PermissionProbe for MacPermissions {
    async fn status(&self) -> Result<PermissionStatus, PlatformError> {
        // Real TCC queries land with signed binary + S2 screen work.
        Ok(PermissionStatus {
            screen_recording: PermissionState::NotDetermined,
            microphone: PermissionState::NotDetermined,
            accessibility: PermissionState::NotDetermined,
        })
    }
}

/// macOS frontmost app probe (stub).
pub struct MacFrontmost;

#[async_trait]
impl FrontmostAppProbe for MacFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        Ok(None)
    }
}

/// macOS screen capturer (stub).
pub struct MacScreenCapturer;

#[async_trait]
impl ScreenCapturer for MacScreenCapturer {
    async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError> {
        Err(PlatformError::Unsupported(
            "MacScreenCapturer not implemented yet (Phase S2)".into(),
        ))
    }
}
