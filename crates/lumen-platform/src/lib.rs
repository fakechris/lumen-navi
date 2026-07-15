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

// --- Microphone (Observe audio; never on screen hot path) ---

/// One PCM chunk ready for WAV packaging / storage.
#[derive(Debug, Clone)]
pub struct PcmChunk {
    /// Interleaved s16le mono samples (channels collapsed to mono when needed).
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    pub rms: f32,
    pub peak: f32,
    pub device_name: String,
}

impl PcmChunk {
    pub fn from_mono_i16(samples: Vec<i16>, sample_rate: u32, device_name: impl Into<String>) -> Self {
        let n = samples.len() as u64;
        let duration_ms = if sample_rate == 0 {
            0
        } else {
            n * 1000 / u64::from(sample_rate)
        };
        let (rms, peak) = pcm_rms_peak(&samples);
        Self {
            samples,
            sample_rate,
            channels: 1,
            duration_ms,
            rms,
            peak,
            device_name: device_name.into(),
        }
    }
}

/// Encode mono s16le PCM as a minimal RIFF/WAVE blob.
pub fn pcm_s16le_to_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let channels = channels.max(1);
    let data_bytes = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn pcm_rms_peak(samples: &[i16]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f32;
    for &s in samples {
        let f = s as f32 / 32768.0;
        let a = f.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += (f as f64) * (f as f64);
    }
    let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
    (rms, peak)
}

/// Open configuration for a mic stream.
#[derive(Debug, Clone)]
pub struct MicOpenConfig {
    pub preferred_sample_rate: u32,
    pub preferred_channels: u16,
    pub chunk_ms: u64,
    /// Empty = default device.
    pub device: String,
}

impl Default for MicOpenConfig {
    fn default() -> Self {
        Self {
            preferred_sample_rate: 16_000,
            preferred_channels: 1,
            chunk_ms: 3_000,
            device: String::new(),
        }
    }
}

// --- ASR (process plane; never on capture hot path) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrResult {
    pub text: String,
    pub confidence: f64,
    pub language: Option<String>,
    /// e.g. `speech` | `stub`
    pub engine: String,
}

/// On-device speech recognition for Observe enrichment (not dictation UI).
#[async_trait]
pub trait AsrEngine: Send + Sync {
    fn is_supported(&self) -> bool;

    /// Transcribe a WAV (or other decodeable) audio blob.
    async fn transcribe(
        &self,
        audio: &[u8],
        locale: &str,
    ) -> Result<AsrResult, PlatformError>;
}

/// Stub / unavailable ASR.
pub struct NullAsr;

#[async_trait]
impl AsrEngine for NullAsr {
    fn is_supported(&self) -> bool {
        false
    }

    async fn transcribe(
        &self,
        _audio: &[u8],
        _locale: &str,
    ) -> Result<AsrResult, PlatformError> {
        Err(PlatformError::Unsupported("ASR not available".into()))
    }
}

/// Deterministic test double.
pub struct StubAsr {
    canned: String,
}

impl StubAsr {
    pub fn new(canned: impl Into<String>) -> Self {
        Self {
            canned: canned.into(),
        }
    }
}

#[async_trait]
impl AsrEngine for StubAsr {
    fn is_supported(&self) -> bool {
        true
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        _locale: &str,
    ) -> Result<AsrResult, PlatformError> {
        if audio.is_empty() {
            return Err(PlatformError::Message("empty audio".into()));
        }
        Ok(AsrResult {
            text: self.canned.clone(),
            confidence: 1.0,
            language: Some("zh".into()),
            engine: "stub".into(),
        })
    }
}

/// Live microphone stream handle (Send). Platform keeps cpal stream off-thread.
pub struct MicStream {
    rx: std::sync::mpsc::Receiver<PcmChunk>,
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl MicStream {
    pub fn new(
        rx: std::sync::mpsc::Receiver<PcmChunk>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        join: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            rx,
            stop: Some(stop),
            join: Some(join),
        }
    }

    /// Construct a stream from a pre-filled channel (tests / fakes).
    pub fn from_receiver(rx: std::sync::mpsc::Receiver<PcmChunk>) -> Self {
        Self {
            rx,
            stop: None,
            join: None,
        }
    }

    pub fn try_recv(&self) -> Result<Option<PcmChunk>, PlatformError> {
        match self.rx.try_recv() {
            Ok(c) => Ok(Some(c)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(PlatformError::Message("mic stream disconnected".into()))
            }
        }
    }

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Option<PcmChunk>, PlatformError> {
        match self.rx.recv_timeout(timeout) {
            Ok(c) => Ok(Some(c)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(PlatformError::Message("mic stream disconnected".into()))
            }
        }
    }

    pub fn stop(mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Opens a microphone capture stream that emits fixed-duration [`PcmChunk`]s.
pub trait MicCapturer: Send + Sync {
    fn open(&self, cfg: MicOpenConfig) -> Result<MicStream, PlatformError>;
}

/// No-op mic for tests / non-macOS.
pub struct NullMic;

impl MicCapturer for NullMic {
    fn open(&self, _cfg: MicOpenConfig) -> Result<MicStream, PlatformError> {
        Err(PlatformError::Unsupported("microphone not available".into()))
    }
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

    #[test]
    fn wav_header_and_pcm_len() {
        let samples = vec![0i16, 1000, -1000, 0];
        let wav = pcm_s16le_to_wav(&samples, 16_000, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + samples.len() * 2);
        let (rms, peak) = pcm_rms_peak(&samples);
        assert!(peak > 0.0);
        assert!(rms > 0.0);
    }
}
