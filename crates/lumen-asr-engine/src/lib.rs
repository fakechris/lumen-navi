//! Observe ASR engines for continuous voice enrichment.
//!
//! Patterns adapted from [lumen-asr](https://github.com/fakechris/lumen-asr)
//! (separate product — do not merge monorepos). Implements Navi's
//! [`lumen_platform::AsrEngine`] (WAV blob in → text out).
//!
//! | Engine id | Backend |
//! |-----------|---------|
//! | `sensevoice` | Local sherpa-onnx SenseVoice (default) |
//! | `whisper` | Local sherpa-onnx Whisper |
//! | `openai_audio` / `qwen` | OpenAI-compatible HTTP (`/audio/transcriptions`) |

mod openai_http;
mod paths;
mod sensevoice;
mod wav;
mod whisper;

pub use openai_http::{OpenAiAudioAsr, OpenAiAudioConfig};
pub use paths::{
    app_models_dir, default_sensevoice_dir, default_whisper_dir, sensevoice_ready, whisper_ready,
};
pub use sensevoice::SenseVoiceSherpaAsr;
pub use wav::{
    decode_wav_pcm_s16le, prepare_for_offline_asr, resample_linear, samples_to_wav_mono_i16,
    DecodedPcm,
};
pub use whisper::WhisperAsr;

use lumen_platform::AsrEngine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Engine selector (config / UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// Local SenseVoice (sherpa-onnx). Product default for continuous Observe.
    SenseVoice,
    /// Local Whisper (sherpa-onnx).
    Whisper,
    /// macOS Speech.framework (platform-macos; built outside this crate).
    Speech,
    /// OpenAI-compatible HTTP ASR (Whisper API, Qwen ASR 0.8B, etc.).
    OpenAiAudio,
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SenseVoice => "sensevoice",
            Self::Whisper => "whisper",
            Self::Speech => "speech",
            Self::OpenAiAudio => "openai_audio",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sensevoice" | "sensevoice_sherpa" | "sherpa" | "local_sensevoice" => {
                Some(Self::SenseVoice)
            }
            "whisper" | "local_whisper" => Some(Self::Whisper),
            "speech" | "macos_speech" | "apple" => Some(Self::Speech),
            "openai_audio" | "openai" | "http" | "cloud" => Some(Self::OpenAiAudio),
            // Qwen ASR family → OpenAI-compatible HTTP path
            "qwen" | "qwen_asr" | "qwen-asr" | "qwen_asr_0.8b" | "qwen3-asr" => {
                Some(Self::OpenAiAudio)
            }
            _ => None,
        }
    }
}

impl Default for EngineKind {
    fn default() -> Self {
        Self::SenseVoice
    }
}

/// Build inputs for engines owned by this crate (not Speech).
#[derive(Debug, Clone)]
pub struct EngineBuildConfig {
    pub kind: EngineKind,
    /// Override model dir; empty → auto-resolve for SenseVoice/Whisper.
    pub model_dir: PathBuf,
    pub locale: String,
    pub max_audio_bytes: usize,
    pub http_base_url: String,
    pub http_api_key: String,
    pub http_model: String,
    pub http_timeout_ms: u64,
    /// Label stored in transcript.v1 for HTTP engines.
    pub http_engine_label: String,
}

impl Default for EngineBuildConfig {
    fn default() -> Self {
        Self {
            kind: EngineKind::SenseVoice,
            model_dir: PathBuf::new(),
            locale: "zh-CN".into(),
            max_audio_bytes: 8 * 1024 * 1024,
            http_base_url: String::new(),
            http_api_key: String::new(),
            http_model: "whisper-1".into(),
            http_timeout_ms: 120_000,
            http_engine_label: "openai_audio".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineStatus {
    pub kind: String,
    pub ready: bool,
    pub model_dir: String,
    pub detail: String,
}

/// Status probe for settings UI / logs (does not load models).
pub fn engine_status(kind: EngineKind, model_dir: Option<&str>) -> EngineStatus {
    match kind {
        EngineKind::SenseVoice => {
            let dir = if let Some(d) = model_dir.filter(|s| !s.is_empty()) {
                PathBuf::from(d)
            } else {
                default_sensevoice_dir()
            };
            let ready = sensevoice_ready(&dir);
            EngineStatus {
                kind: kind.as_str().into(),
                ready,
                model_dir: dir.display().to_string(),
                detail: if ready {
                    "SenseVoice model ready".into()
                } else {
                    "missing model*.onnx + tokens.txt (see docs/AUDIO_PRODUCT.md)".into()
                },
            }
        }
        EngineKind::Whisper => {
            let dir = if let Some(d) = model_dir.filter(|s| !s.is_empty()) {
                PathBuf::from(d)
            } else {
                default_whisper_dir()
            };
            let ready = whisper_ready(&dir);
            EngineStatus {
                kind: kind.as_str().into(),
                ready,
                model_dir: dir.display().to_string(),
                detail: if ready {
                    "Whisper model ready".into()
                } else {
                    "missing encoder/decoder/tokens onnx layout".into()
                },
            }
        }
        EngineKind::Speech => EngineStatus {
            kind: kind.as_str().into(),
            ready: cfg!(target_os = "macos"),
            model_dir: String::new(),
            detail: "macOS Speech.framework (platform)".into(),
        },
        EngineKind::OpenAiAudio => EngineStatus {
            kind: kind.as_str().into(),
            ready: true, // network; validated at first request
            model_dir: String::new(),
            detail: "OpenAI-compatible HTTP (Qwen ASR / Whisper API)".into(),
        },
    }
}

/// Build a local/HTTP engine. Returns `None` for [`EngineKind::Speech`]
/// (caller uses `MacSpeechAsr`).
pub fn build_engine(cfg: &EngineBuildConfig) -> Result<Option<Arc<dyn AsrEngine>>, String> {
    match cfg.kind {
        EngineKind::Speech => Ok(None),
        EngineKind::SenseVoice => {
            let dir = if cfg.model_dir.as_os_str().is_empty() {
                default_sensevoice_dir()
            } else {
                cfg.model_dir.clone()
            };
            let eng = SenseVoiceSherpaAsr::new(dir.clone())
                .with_language(sensevoice_language_from_locale(&cfg.locale))
                .with_max_audio_bytes(cfg.max_audio_bytes);
            if !eng.is_ready() {
                return Err(format!(
                    "SenseVoice model not ready under {} (set asr.model_dir or LUMEN_SENSEVOICE_DIR)",
                    dir.display()
                ));
            }
            tracing::info!(dir = %dir.display(), "ASR engine: sensevoice");
            Ok(Some(Arc::new(eng)))
        }
        EngineKind::Whisper => {
            let dir = if cfg.model_dir.as_os_str().is_empty() {
                default_whisper_dir()
            } else {
                cfg.model_dir.clone()
            };
            let lang = whisper_language_from_locale(&cfg.locale);
            let eng = WhisperAsr::new(dir.clone())
                .with_language(lang)
                .with_max_audio_bytes(cfg.max_audio_bytes);
            if !eng.is_ready() {
                return Err(format!(
                    "Whisper model not ready under {} (set asr.model_dir or LUMEN_WHISPER_DIR)",
                    dir.display()
                ));
            }
            tracing::info!(dir = %dir.display(), "ASR engine: whisper");
            Ok(Some(Arc::new(eng)))
        }
        EngineKind::OpenAiAudio => {
            let base = cfg.http_base_url.trim();
            if base.is_empty() {
                return Err(
                    "openai_audio/qwen requires asr.http_base_url (OpenAI-compatible endpoint)"
                        .into(),
                );
            }
            let model = if cfg.http_model.trim().is_empty() {
                "whisper-1".into()
            } else {
                cfg.http_model.clone()
            };
            let label = if cfg.http_engine_label.trim().is_empty() {
                guess_http_label(base, &model)
            } else {
                cfg.http_engine_label.clone()
            };
            let http = OpenAiAudioConfig {
                base_url: base.to_string(),
                api_key: cfg.http_api_key.clone(),
                model: model.clone(),
                timeout: Duration::from_millis(cfg.http_timeout_ms.max(5_000)),
                language: locale_to_lang_hint(&cfg.locale),
                max_audio_bytes: cfg.max_audio_bytes,
                engine_label: label,
            };
            let eng = OpenAiAudioAsr::new(http).map_err(|e| e.to_string())?;
            tracing::info!(base = %base, model = %model, "ASR engine: openai_audio");
            Ok(Some(Arc::new(eng)))
        }
    }
}

fn sensevoice_language_from_locale(locale: &str) -> String {
    let primary = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("auto")
        .to_ascii_lowercase();
    match primary.as_str() {
        "zh" | "yue" | "ja" | "ko" | "en" => primary,
        _ => "auto".into(),
    }
}

fn whisper_language_from_locale(locale: &str) -> String {
    locale
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

fn locale_to_lang_hint(locale: &str) -> Option<String> {
    let primary = locale.split(['-', '_']).next()?.to_ascii_lowercase();
    if primary.is_empty() {
        None
    } else {
        Some(primary)
    }
}

fn guess_http_label(base: &str, model: &str) -> String {
    let b = base.to_ascii_lowercase();
    let m = model.to_ascii_lowercase();
    if b.contains("dashscope") || m.contains("qwen") {
        "qwen_asr".into()
    } else {
        "openai_audio".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_engines() {
        assert_eq!(EngineKind::parse("sensevoice"), Some(EngineKind::SenseVoice));
        assert_eq!(EngineKind::parse("qwen"), Some(EngineKind::OpenAiAudio));
        assert_eq!(EngineKind::parse("qwen_asr_0.8b"), Some(EngineKind::OpenAiAudio));
        assert_eq!(EngineKind::parse("speech"), Some(EngineKind::Speech));
        assert_eq!(EngineKind::parse("whisper"), Some(EngineKind::Whisper));
        assert_eq!(EngineKind::parse("nope"), None);
    }

    #[test]
    fn speech_build_is_none() {
        let mut cfg = EngineBuildConfig::default();
        cfg.kind = EngineKind::Speech;
        assert!(build_engine(&cfg).unwrap().is_none());
    }

    #[test]
    fn openai_requires_url() {
        let mut cfg = EngineBuildConfig::default();
        cfg.kind = EngineKind::OpenAiAudio;
        cfg.http_base_url = String::new();
        assert!(build_engine(&cfg).is_err());
    }
}
