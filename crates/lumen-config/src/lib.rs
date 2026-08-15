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
#[serde(default)]
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
    pub browser: BrowserConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub asr: AsrConfig,
    #[serde(default)]
    pub ax: AxConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub assistant: AssistantConfig,
}

/// Selection-popup assistant (desktop 划词弹窗) — OpenAI-compatible chat LLM.
///
/// Text is sent to the configured endpoint **only** on explicit user action
/// (translate / ask) from the popup; never during capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantConfig {
    /// Master switch for the assistant feature.
    pub enabled: bool,
    /// Show popup automatically after mouse text selection.
    pub popup_enabled: bool,
    /// OpenAI-compatible base URL (…/v1).
    pub base_url: String,
    /// Bearer token (env `LUMEN_NAVI_LLM_API_KEY` / `OPENAI_API_KEY` overrides).
    pub api_key: String,
    /// Chat model id, e.g. `gpt-4o-mini`, `deepseek-chat`, `qwen-plus`.
    pub model: String,
    /// Translate target language, e.g. `中文`, `English`.
    pub target_lang: String,
    /// Selection text is truncated beyond this many chars before sending.
    pub max_selection_chars: usize,
    /// HTTP request timeout (connect + read).
    pub timeout_ms: u64,
    /// When AX exposes no selection (canvas editors, GPU terminals), grab it
    /// via simulated ⌘C with full pasteboard save/restore.
    pub clipboard_fallback: bool,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            popup_enabled: false,
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            target_lang: "中文".into(),
            max_selection_chars: 4_000,
            timeout_ms: 120_000,
            clipboard_fallback: true,
        }
    }
}

impl AssistantConfig {
    /// Effective API key: env override then config.
    pub fn effective_api_key(&self) -> String {
        if let Ok(k) = std::env::var("LUMEN_NAVI_LLM_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            if !k.is_empty() {
                return k;
            }
        }
        self.api_key.clone()
    }
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
    /// Drop chunks below VAD threshold — default on: storing ambient silence
    /// only floods the timeline and wastes ASR work.
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
            drop_silent_chunks: true,
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
/// Engines (shared `lumen-asr-engine` crate from lumen-suite):
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
    /// Empty = `LUMEN_MODELS_DIR`, else the platform default:
    /// `~/Library/Application Support/Lumen/models` (macOS) or
    /// `%LOCALAPPDATA%\Lumen\models` (Windows).
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
        if !t.is_empty() {
            return Some(std::path::PathBuf::from(t));
        }
        // `None` lets the shared lumen-models crate pick the cluster default,
        // which is correct on macOS. Its non-macOS fallback is `~/.lumen/models`,
        // so Windows names the Local AppData root explicitly to stay in the same
        // shared location Lumen ASR downloads into.
        #[cfg(target_os = "windows")]
        {
            if let Some(local) =
                std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())
            {
                return Some(std::path::PathBuf::from(local).join("Lumen").join("models"));
            }
        }
        None
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

/// Local browser-extension intake. Content is off until hosts are explicitly allowed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// Shared secret expected in `Authorization: Bearer ...` on loopback ingest.
    pub ingest_token: String,
    /// Hosts allowed to send Readability Markdown. Empty means metadata only.
    pub content_allow_hosts: Vec<String>,
    /// Hosts rejected before any browser observation is persisted.
    pub excluded_hosts: Vec<String>,
    pub max_batch_size: usize,
    pub max_artifact_bytes: usize,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            ingest_token: String::new(),
            content_allow_hosts: Vec::new(),
            excluded_hosts: vec![
                "mail.google.com".into(),
                "outlook.office.com".into(),
                "slack.com".into(),
                "discord.com".into(),
                "web.whatsapp.com".into(),
                "web.telegram.org".into(),
            ],
            max_batch_size: 100,
            max_artifact_bytes: 2 * 1024 * 1024,
        }
    }
}

impl BrowserConfig {
    pub fn effective_ingest_token(&self) -> String {
        std::env::var("LUMEN_NAVI_BROWSER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.ingest_token.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesConfig {
    pub screen: bool,
    pub audio: bool,
    pub video: bool,
    pub browser: bool,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            screen: true,
            audio: true,
            video: false,
            browser: false,
        }
    }
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

/// AX (Accessibility) tree capture — deep text extraction from app UI for
/// recall/search. Runs alongside OCR as an async worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AxConfig {
    pub enabled: bool,
    pub max_depth: u32,
    pub max_nodes: u32,
    pub walk_timeout_ms: u64,
    pub element_timeout_ms: u64,
    pub max_text_chars: u64,
    pub poll_interval_ms: u64,
    pub batch_size: usize,
    pub max_attempts: u32,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
    pub stale_running_ms: u64,
    pub shutdown_drain_ms: u64,
}

impl Default for AxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 30,
            max_nodes: 3000,
            walk_timeout_ms: 200,
            element_timeout_ms: 150,
            max_text_chars: 50_000,
            poll_interval_ms: 1_500,
            batch_size: 2,
            max_attempts: 3,
            retry_base_ms: 2_000,
            retry_max_ms: 60_000,
            stale_running_ms: 300_000,
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
            browser: BrowserConfig::default(),
            audio: AudioConfig::default(),
            asr: AsrConfig::default(),
            assistant: AssistantConfig::default(),
            ax: AxConfig::default(),
            input: InputConfig::default(),
        }
    }
}

/// Input event counting for the roast feature. Records only behavioral keys
/// (Delete/Tab/Esc/arrows) + shortcut combos (Cmd+C/V/X/Z) + click counts —
/// never keystroke content. Requires Input Monitoring TCC (opt-in).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    pub enabled: bool,
    /// How often to flush counters as an `input.stats.v1` event (seconds).
    pub flush_interval_s: u64,
    /// Persist discrete click / shortcut / text / drag events (Observe).
    pub observe_interactions: bool,
    /// Include coalesced typed text on `keyboard.text_input.v1`.
    pub record_text: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            flush_interval_s: 300, // 5 minutes
            observe_interactions: true,
            record_text: true,
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
        assert!(c.audio.drop_silent_chunks);
        assert!(!c.audio.is_session_mode());
        assert!(c.asr.enabled);
        assert_eq!(c.asr.locale, "zh-CN");
        assert_eq!(c.asr.engine, "sensevoice");
        assert!(c.asr.fallback_speech);
        assert!(!c.assistant.enabled);
        assert!(!c.assistant.popup_enabled);
        assert_eq!(c.assistant.model, "gpt-4o-mini");
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

    #[test]
    fn assistant_config_survives_toml_roundtrip() {
        let mut config = Config::default();
        config.assistant.enabled = true;
        config.assistant.popup_enabled = true;
        config.assistant.base_url = "https://api.deepseek.com/v1".into();
        config.assistant.model = "deepseek-chat".into();
        config.assistant.target_lang = "English".into();

        let encoded = toml::to_string_pretty(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();

        assert!(decoded.assistant.enabled);
        assert!(decoded.assistant.popup_enabled);
        assert_eq!(decoded.assistant.base_url, "https://api.deepseek.com/v1");
        assert_eq!(decoded.assistant.model, "deepseek-chat");
        assert_eq!(decoded.assistant.target_lang, "English");
    }

    #[test]
    fn browser_ingest_is_disabled_until_a_token_is_configured() {
        let config = Config::default();
        assert!(!config.sources.browser);
        assert!(config.browser.effective_ingest_token().is_empty());
        assert!(config.browser.content_allow_hosts.is_empty());
    }

    #[test]
    fn browser_privacy_lists_survive_toml_roundtrip() {
        let mut config = Config::default();
        config.sources.browser = true;
        config.browser.ingest_token = "fixture-token".into();
        config.browser.content_allow_hosts = vec!["example.test".into()];
        config.browser.excluded_hosts = vec!["private.example.test".into()];

        let encoded = toml::to_string_pretty(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();

        assert!(decoded.sources.browser);
        assert_eq!(decoded.browser.ingest_token, "fixture-token");
        assert_eq!(decoded.browser.content_allow_hosts, vec!["example.test"]);
        assert_eq!(decoded.browser.excluded_hosts, vec!["private.example.test"]);
    }
}
