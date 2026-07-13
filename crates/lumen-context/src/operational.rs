//! Narrow operational contracts used by the existing continuous capture and
//! durable OCR pipelines while they migrate behind [`crate::ContextCollector`].

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

#[derive(Debug, Clone)]
pub struct ScreenshotFrame {
    pub png_or_jpeg_bytes: Vec<u8>,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub display_id: DisplayId,
}

#[derive(Debug, Clone)]
pub struct RawFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: usize,
    pub display_id: DisplayId,
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
    async fn capture_display(
        &self,
        id: DisplayId,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    ) -> Result<ScreenshotFrame, PlatformError>;

    async fn capture_display_raw(
        &self,
        id: DisplayId,
        scale_div: u32,
    ) -> Result<RawFrame, PlatformError>;
}

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
    pub mode: String,
    pub boxes: Vec<OcrBox>,
}

#[async_trait]
pub trait OcrEngine: Send + Sync {
    fn is_supported(&self) -> bool;

    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError>;

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError>;
}

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

pub fn bgra_to_gray(frame: &RawFrame) -> Vec<u8> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = y * frame.bytes_per_row;
        for x in 0..width {
            let index = row + x * 4;
            if index + 2 >= frame.bgra.len() {
                break;
            }
            let blue = frame.bgra[index] as u32;
            let green = frame.bgra[index + 1] as u32;
            let red = frame.bgra[index + 2] as u32;
            out.push(((red * 77 + green * 150 + blue * 29) >> 8) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_distance_identical_is_zero() {
        let gray = vec![10_u8, 20, 30];
        assert!((gray_distance(&gray, &gray) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn bgra_to_gray_has_one_byte_per_pixel() {
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
