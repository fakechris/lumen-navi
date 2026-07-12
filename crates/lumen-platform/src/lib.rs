//! Platform capability ports.
//!
//! Implementations live in `lumen-platform-macos` (and future OS crates).
//! Core intake/store/process must depend on these traits only — no `#[cfg]` soup.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{0}")]
    Message(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionStatus {
    pub screen_recording: PermissionState,
    pub microphone: PermissionState,
    pub accessibility: PermissionState,
}

impl PermissionStatus {
    pub fn can_capture_screen(&self) -> bool {
        self.screen_recording == PermissionState::Granted
    }

    pub fn can_record_mic(&self) -> bool {
        self.microphone == PermissionState::Granted
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontmostApp {
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScreenshotFrame {
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub display_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub pcm_s16le: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Query OS permission state (does not prompt unless the impl chooses to).
#[async_trait]
pub trait PermissionProbe: Send + Sync {
    async fn status(&self) -> Result<PermissionStatus, PlatformError>;
}

/// Frontmost application / window metadata (cheap signal).
#[async_trait]
pub trait FrontmostAppProbe: Send + Sync {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError>;
}

/// Full-display or main-display screenshot.
#[async_trait]
pub trait ScreenCapturer: Send + Sync {
    async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError>;
}

/// Microphone (system audio is a separate future port).
#[async_trait]
pub trait AudioCapturer: Send + Sync {
    async fn start(&mut self) -> Result<(), PlatformError>;
    async fn stop(&mut self) -> Result<(), PlatformError>;
    async fn next_chunk(&mut self) -> Result<Option<AudioChunk>, PlatformError>;
}

/// Stub probe for headless tests and non-macOS scaffolding.
pub struct NullPermissions;

#[async_trait]
impl PermissionProbe for NullPermissions {
    async fn status(&self) -> Result<PermissionStatus, PlatformError> {
        Ok(PermissionStatus {
            screen_recording: PermissionState::NotDetermined,
            microphone: PermissionState::NotDetermined,
            accessibility: PermissionState::NotDetermined,
        })
    }
}

/// Stub frontmost probe.
pub struct NullFrontmost;

#[async_trait]
impl FrontmostAppProbe for NullFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        Ok(None)
    }
}
