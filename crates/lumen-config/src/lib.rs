//! Daemon and intake configuration — media-first Observe defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    pub sources: SourcesConfig,
    pub capture: CaptureConfig,
    pub privacy: PrivacyConfig,
    pub retention: RetentionConfig,
    #[serde(default)]
    pub ocr: OcrConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub audio: AudioConfig,
}

/// Microphone intake (S3). Enable flag is `sources.audio`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// `continuous` | `session`
    pub mode: String,
    /// Preferred sample rate; device may negotiate another rate.
    pub sample_rate: u32,
    pub channels: u16,
    /// Chunk duration before flush to store.
    pub chunk_ms: u64,
    pub queue_capacity: usize,
    /// 0 = run until stop; >0 = finite chunks (smoke).
    pub ticks: u64,
    /// Session mode: close after this much silence.
    pub session_silence_ms: u64,
    /// Energy VAD threshold (RMS of float samples in [-1, 1]).
    pub vad_rms_threshold: f32,
    /// Drop chunks below VAD threshold (useful in session mode).
    pub drop_silent_chunks: bool,
    /// Empty = system default input device.
    pub device: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: "continuous".into(),
            sample_rate: 16_000,
            channels: 1,
            chunk_ms: 5_000,
            queue_capacity: 8,
            ticks: 0,
            session_silence_ms: 2_500,
            vad_rms_threshold: 0.008,
            drop_silent_chunks: false,
            device: String::new(),
        }
    }
}

impl AudioConfig {
    pub fn is_session_mode(&self) -> bool {
        self.mode.eq_ignore_ascii_case("session")
    }
}

/// Local control API (loopback HTTP).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Serve control plane while daemon is running.
    pub enabled: bool,
    /// Bind address. Default loopback only.
    pub bind: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: "127.0.0.1:7420".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesConfig {
    pub screen: bool,
    pub audio: bool,
    pub video: bool,
    pub browser: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub screen_interval_ms: u64,
    pub screen_dedup_window_ms: u64,
    pub screen_max_edge: u32,
    pub screen_ticks: u64,
    pub probe_scale: u32,
    pub visual_change_threshold: f64,
    pub debounce_default_ms: u64,
    pub debounce_churn_ms: u64,
    pub same_app_min_ms: u64,
    pub idle_session_ms: u64,
    pub queue_capacity: usize,
    pub focus_poll_ms: u64,
    pub displays: String,
    pub encode: String,
    pub jpeg_quality: u8,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            screen_interval_ms: 3_000,
            screen_dedup_window_ms: 5_000,
            screen_max_edge: 1920,
            screen_ticks: 0,
            probe_scale: 6,
            visual_change_threshold: 0.05,
            debounce_default_ms: 1_000,
            debounce_churn_ms: 3_000,
            same_app_min_ms: 10_000,
            idle_session_ms: 300_000,
            queue_capacity: 8,
            focus_poll_ms: 500,
            displays: "all".into(),
            encode: "jpeg".into(),
            jpeg_quality: 75,
        }
    }
}

impl CaptureConfig {
    pub fn all_displays(&self) -> bool {
        !self.displays.eq_ignore_ascii_case("main")
    }

    pub fn use_jpeg(&self) -> bool {
        self.encode.eq_ignore_ascii_case("jpeg")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub paused: bool,
    pub closed_eyes: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            paused: false,
            closed_eyes: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    pub max_blob_mb: u64,
    pub wipe_on_request: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    pub enabled: bool,
    pub languages: Vec<String>,
    pub poll_interval_ms: u64,
    pub batch_size: usize,
    pub include_boxes: bool,
    /// Only run layout OCR when accurate text is empty (default true — cheaper).
    pub boxes_when_empty_only: bool,
    pub max_attempts: u32,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
    pub timeout_ms: u64,
    pub stale_running_ms: u64,
    pub max_image_bytes: u64,
    pub max_text_chars: u64,
    pub shutdown_drain_ms: u64,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            languages: vec!["zh-Hans".into(), "en-US".into()],
            poll_interval_ms: 1_500,
            batch_size: 2,
            include_boxes: true,
            boxes_when_empty_only: true,
            max_attempts: 5,
            retry_base_ms: 2_000,
            retry_max_ms: 60_000,
            timeout_ms: 90_000,
            stale_running_ms: 300_000,
            max_image_bytes: 25 * 1024 * 1024,
            max_text_chars: 500_000,
            shutdown_drain_ms: 30_000,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            sources: SourcesConfig {
                screen: true,
                audio: true,
                video: false,
                browser: false,
            },
            capture: CaptureConfig::default(),
            privacy: PrivacyConfig::default(),
            retention: RetentionConfig {
                max_blob_mb: 20_480,
                wipe_on_request: true,
            },
            ocr: OcrConfig::default(),
            api: ApiConfig::default(),
            audio: AudioConfig::default(),
        }
    }
}

impl Config {
    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_product_observe() {
        let c = Config::default();
        assert!(c.sources.screen);
        assert!(!c.privacy.closed_eyes);
        assert_eq!(c.capture.probe_scale, 6);
        assert!(c.ocr.enabled);
        assert_eq!(c.ocr.batch_size, 2);
        assert!(c.ocr.boxes_when_empty_only);
        assert!(c.api.enabled);
        assert_eq!(c.api.bind, "127.0.0.1:7420");
        assert!(c.sources.audio);
        assert_eq!(c.audio.chunk_ms, 5_000);
        assert!(!c.audio.is_session_mode());
    }
}
