//! OS capability ports for Observe capture (no `#[cfg]` in consumers).

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisplayId(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub is_main: bool,
}

/// Encoded (or encode-ready RGBA) frame for archival.
#[derive(Debug, Clone)]
pub struct ScreenshotFrame {
    pub png_or_jpeg_bytes: Vec<u8>,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub display_id: DisplayId,
}

/// Raw BGRA frame for visual probing (not stored).
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: usize,
    pub display_id: DisplayId,
}

#[async_trait]
pub trait PermissionProbe: Send + Sync {
    async fn status(&self) -> Result<PermissionStatus, PlatformError>;
}

#[async_trait]
pub trait FrontmostAppProbe: Send + Sync {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError>;
}

#[async_trait]
pub trait ScreenLockProbe: Send + Sync {
    async fn is_locked(&self) -> Result<bool, PlatformError>;
}

#[async_trait]
pub trait DisplayEnumerator: Send + Sync {
    async fn list_displays(&self) -> Result<Vec<DisplayInfo>, PlatformError>;
}

#[async_trait]
pub trait ScreenCapturer: Send + Sync {
    /// Full archival capture for one display (already scaled/encoded).
    async fn capture_display(
        &self,
        id: DisplayId,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    ) -> Result<ScreenshotFrame, PlatformError>;

    /// Downscaled raw BGRA for visual probe (`scale_div` ≥ 1, e.g. 6).
    async fn capture_display_raw(
        &self,
        id: DisplayId,
        scale_div: u32,
    ) -> Result<RawFrame, PlatformError>;
}

// --- OCR (process plane; never called from capture hot path) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub confidence: f64,
    pub languages: Vec<String>,
    /// `accurate` | `fast`
    pub mode: String,
    pub boxes: Vec<OcrBox>,
}

/// On-device OCR. Implementations must be safe to call off the capture thread.
#[async_trait]
pub trait OcrEngine: Send + Sync {
    fn is_supported(&self) -> bool;

    /// Quality path: best-effort full text.
    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError>;

    /// Layout path: text regions with normalized boxes.
    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError>;
}

/// Stub OCR for tests / non-macOS.
pub struct NullOcr;

#[async_trait]
impl OcrEngine for NullOcr {
    fn is_supported(&self) -> bool {
        false
    }

    async fn recognize_text(
        &self,
        _image: &[u8],
        _languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        Err(PlatformError::Unsupported("OCR not available".into()))
    }

    async fn recognize_boxes(
        &self,
        _image: &[u8],
        _languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        Err(PlatformError::Unsupported("OCR not available".into()))
    }
}

// --- Null stubs (tests / non-macOS) ---

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

pub struct NullFrontmost;
#[async_trait]
impl FrontmostAppProbe for NullFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        Ok(None)
    }
}

pub struct NullScreenLock;
#[async_trait]
impl ScreenLockProbe for NullScreenLock {
    async fn is_locked(&self) -> Result<bool, PlatformError> {
        Ok(false)
    }
}

/// Mean absolute difference of grayscale planes in [0, 1].
pub fn gray_distance(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 1.0;
    }
    let mut sum = 0u64;
    for (x, y) in a.iter().zip(b.iter()) {
        sum += (*x as i16 - *y as i16).unsigned_abs() as u64;
    }
    (sum as f64) / (a.len() as f64) / 255.0
}

/// BGRA → grayscale (BT.601).
pub fn bgra_to_gray(frame: &RawFrame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = y * frame.bytes_per_row;
        for x in 0..w {
            let i = row + x * 4;
            if i + 2 >= frame.bgra.len() {
                break;
            }
            let b = frame.bgra[i] as u32;
            let g = frame.bgra[i + 1] as u32;
            let r = frame.bgra[i + 2] as u32;
            // (R*77 + G*150 + B*29) >> 8
            let yv = (r * 77 + g * 150 + b * 29) >> 8;
            out.push(yv as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_distance_identical_is_zero() {
        let g = vec![10u8, 20, 30];
        assert!((gray_distance(&g, &g) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn bgra_to_gray_len() {
        let frame = RawFrame {
            bgra: vec![0, 0, 255, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
            bytes_per_row: 8,
            display_id: DisplayId(1),
        };
        assert_eq!(bgra_to_gray(&frame).len(), 2);
    }
}
