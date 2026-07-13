//! OS capability ports for Observe capture (no `#[cfg]` in consumers).

use async_trait::async_trait;
pub use lumen_context::{
    bgra_to_gray, gray_distance, DisplayEnumerator, DisplayId, DisplayInfo, FrontmostApp,
    FrontmostAppProbe, NullFrontmost, NullOcr, NullScreenLock, OcrBox, OcrEngine, OcrResult,
    PlatformError, RawFrame, ScreenCapturer, ScreenLockProbe, ScreenshotFrame,
};
use serde::{Deserialize, Serialize};

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

#[async_trait]
pub trait PermissionProbe: Send + Sync {
    async fn status(&self) -> Result<PermissionStatus, PlatformError>;
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
    pub fn from_mono_i16(
        samples: Vec<i16>,
        sample_rate: u32,
        device_name: impl Into<String>,
    ) -> Self {
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
            chunk_ms: 5_000,
            device: String::new(),
        }
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
        Err(PlatformError::Unsupported(
            "microphone not available".into(),
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

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
