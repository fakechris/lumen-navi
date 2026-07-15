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
    #[serde(default)]
    pub asr: AsrConfig,
}

/// Microphone intake (S3). Enable flag is `sources.audio`.
///
/// Timing defaults align with the product reference path: **16 kHz mono**,
/// short continuous windows suitable for on-device ASR (same family as Lumen ASR
/// / native 16 kHz capture — dictation product stays separate).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// `continuous` | `session`
    pub mode: String,
    /// Target / preferred sample rate (16_000 product default).
    pub sample_rate: u32,
    pub channels: u16,
    /// Chunk duration before flush to store (3s product default).
    pub chunk_ms: u64,
    /// Hard cap: never emit a single chunk longer than this (ms).
    pub max_chunk_ms: u64,
    pub queue_capacity: usize,
    /// 0 = run until stop; >0 = finite chunks (smoke).
    pub ticks: u64,
    /// Session mode: close after this much silence (1.2s product default).
    pub session_silence_ms: u64,
    /// Session mode: force-close open session after this duration (10 min).
    pub max_session_ms: u64,
    /// Energy VAD threshold (RMS of float samples in [-1, 1]).
    pub vad_rms_threshold: f32,
    /// Drop chunks below VAD threshold (session mode often true).
    pub drop_silent_chunks: bool,
    /// Reject / skip chunks larger than this after WAV encode.
    pub max_audio_bytes: u64,
    /// Empty = system default input device.
    pub device: String,
    /// Enqueue `transcribe_audio` jobs after each stored chunk.
    pub enqueue_transcribe: bool,
    /// Capture system/loopback audio (ScreenCaptureKit). **Not implemented yet** —
    /// reserved flag for P1; mic path remains default.
    pub system_audio: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: "continuous".into(),
            sample_rate: 16_000,
            channels: 1,
            chunk_ms: 3_000,
            max_chunk_ms: 30_000,
            queue_capacity: 8,
            ticks: 0,
            session_silence_ms: 1_200,
            max_session_ms: 600_000,
            vad_rms_threshold: 0.01,
            drop_silent_chunks: false,
            max_audio_bytes: 8 * 1024 * 1024,
            device: String::new(),
            enqueue_transcribe: true,
            system_audio: false,
        }
    }
}

impl AudioConfig {
    pub fn is_session_mode(&self) -> bool {
        self.mode.eq_ignore_ascii_case("session")
    }

    /// Effective mic open chunk length (clamped by max_chunk_ms).
    pub fn effective_chunk_ms(&self) -> u64 {
        self.chunk_ms.clamp(200, self.max_chunk_ms.max(200))
    }
}

/// Background Observe ASR (enrichment), not dictation.
/// Dictation remains https://github.com/fakechris/lumen-asr .
///
/// Engines (patterns from lumen-asr, owned port in `lumen-asr-engine`):
/// - `sensevoice` — local sherpa-onnx SenseVoice (**default**)
/// - `whisper` — local sherpa-onnx Whisper
/// - `speech` — macOS Speech.framework
/// - `openai_audio` / `qwen` — OpenAI-compatible HTTP (e.g. Qwen ASR 0.8B)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    pub enabled: bool,
    /// `sensevoice` | `whisper` | `speech` | `openai_audio` | `qwen`
    pub engine: String,
    /// Shared Lumen cluster models root (sensevoice/whisper install + scan).
    /// Empty = `LUMEN_MODELS_DIR` or `~/Library/Application Support/Lumen/models`.
    /// All Lumen apps (navi, asr, …) should share this so models download once.
    pub models_root: String,
    /// Specific engine model directory. Empty = auto under `models_root` / discovery.
    /// User may point at any ready folder (shared, legacy, or custom).
    pub model_dir: String,
    /// BCP-47 locale (Speech / language hints), e.g. `zh-CN`, `en-US`.
    pub locale: String,
    /// If preferred engine is not ready, fall back to macOS Speech.
    pub fallback_speech: bool,
    /// OpenAI-compatible base URL (…/v1). Required for `openai_audio` / `qwen`.
    pub http_base_url: String,
    /// Bearer token for HTTP ASR (env `LUMEN_NAVI_ASR_API_KEY` overrides if set).
    pub http_api_key: String,
    /// Remote model id, e.g. `whisper-1`, `qwen3-asr-0.8b`, `qwen-audio-asr`.
    pub http_model: String,
    /// Label written into `transcript.v1.engine` for HTTP path (empty = auto).
    pub http_engine_label: String,
    pub poll_interval_ms: u64,
    pub batch_size: usize,
    pub max_attempts: u32,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
    pub timeout_ms: u64,
    pub stale_running_ms: u64,
    pub max_audio_bytes: u64,
    pub max_text_chars: u64,
    pub shutdown_drain_ms: u64,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: "sensevoice".into(),
            models_root: String::new(),
            model_dir: String::new(),
            locale: "zh-CN".into(),
            fallback_speech: true,
            http_base_url: String::new(),
            http_api_key: String::new(),
            http_model: "qwen3-asr-0.8b".into(),
            http_engine_label: String::new(),
            poll_interval_ms: 1_500,
            batch_size: 1,
            max_attempts: 5,
            retry_base_ms: 2_000,
            retry_max_ms: 60_000,
            timeout_ms: 120_000,
            stale_running_ms: 300_000,
            max_audio_bytes: 8 * 1024 * 1024,
            max_text_chars: 200_000,
            shutdown_drain_ms: 30_000,
        }
    }
}

impl AsrConfig {
    /// Normalized engine name (lowercase).
    pub fn engine_name(&self) -> &str {
        self.engine.trim()
    }

    /// Shared cluster models root if configured; `None` → engine default resolution.
    pub fn models_root_path(&self) -> Option<std::path::PathBuf> {
        let t = self.models_root.trim();
        if t.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(t))
        }
    }

    /// Effective API key: env override then config.
    pub fn effective_http_api_key(&self) -> String {
        if let Ok(k) = std::env::var("LUMEN_NAVI_ASR_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        self.http_api_key.clone()
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
            asr: AsrConfig::default(),
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
        assert_eq!(c.audio.sample_rate, 16_000);
        assert_eq!(c.audio.chunk_ms, 3_000);
        assert_eq!(c.audio.session_silence_ms, 1_200);
        assert_eq!(c.audio.max_session_ms, 600_000);
        assert!(c.audio.enqueue_transcribe);
        assert!(!c.audio.is_session_mode());
        assert!(c.asr.enabled);
        assert_eq!(c.asr.locale, "zh-CN");
        assert_eq!(c.asr.engine, "sensevoice");
        assert!(c.asr.fallback_speech);
    }

    #[test]
    fn asr_model_selection_survives_toml_roundtrip() {
        let mut config = Config::default();
        config.asr.engine = "whisper".into();
        config.asr.model_dir = "/models/custom-whisper".into();

        let encoded = toml::to_string_pretty(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded.asr.engine, "whisper");
        assert_eq!(decoded.asr.model_dir, "/models/custom-whisper");
    }
}
