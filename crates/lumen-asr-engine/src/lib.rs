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

mod download;
mod install_lock;
mod openai_http;
mod paths;
mod sensevoice;
mod wav;
mod whisper;

pub use download::{
    default_models_root, download_sensevoice_package, DownloadProgress, SENSEVOICE_ARCHIVE_NAME,
    SENSEVOICE_ARCHIVE_URL,
};
pub use install_lock::ModelInstallLock;
pub use openai_http::{OpenAiAudioAsr, OpenAiAudioConfig};
pub use paths::{
    app_models_dir, default_sensevoice_dir, default_sensevoice_dir_with_root, default_whisper_dir,
    default_whisper_dir_with_root, legacy_model_roots, lumen_models_dir,
    lumen_models_dir_with_override, scan_model_candidates, scan_model_candidates_with_root,
    sensevoice_ready, shared_sensevoice_dir, shared_whisper_dir, user_home_dir, whisper_ready,
    ModelCandidate, ENV_LUMEN_MODELS_DIR,
};
pub use sensevoice::SenseVoiceSherpaAsr;
pub use wav::{
    decode_wav_pcm_s16le, prepare_for_offline_asr, resample_linear, samples_to_wav_mono_i16,
    DecodedPcm,
};
pub use whisper::WhisperAsr;

use lumen_platform::AsrEngine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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
    /// Shared cluster models root override; empty → `LUMEN_MODELS_DIR` / default Lumen/models.
    pub models_root: PathBuf,
    /// Override model dir; empty → auto-resolve under models_root / discovery.
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
            models_root: PathBuf::new(),
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

impl EngineBuildConfig {
    fn models_root_opt(&self) -> Option<&Path> {
        if self.models_root.as_os_str().is_empty() {
            None
        } else {
            Some(self.models_root.as_path())
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
    engine_status_with_root(kind, model_dir, None)
}

pub fn engine_status_with_root(
    kind: EngineKind,
    model_dir: Option<&str>,
    models_root: Option<&Path>,
) -> EngineStatus {
    match kind {
        EngineKind::SenseVoice => {
            let configured = model_dir.map(PathBuf::from).unwrap_or_default();
            let dir = resolve_sensevoice_model_dir(&configured, models_root);
            let ready = sensevoice_ready(&dir);
            EngineStatus {
                kind: kind.as_str().into(),
                ready,
                model_dir: dir.display().to_string(),
                detail: if ready {
                    "SenseVoice model ready".into()
                } else {
                    format!(
                        "missing model*.onnx + tokens.txt under {} (shared Lumen models)",
                        dir.display()
                    )
                },
            }
        }
        EngineKind::Whisper => {
            let configured = model_dir.map(PathBuf::from).unwrap_or_default();
            let dir = resolve_whisper_model_dir(&configured, models_root);
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
    let root = cfg.models_root_opt();
    match cfg.kind {
        EngineKind::Speech => Ok(None),
        EngineKind::SenseVoice => {
            let dir = resolve_sensevoice_model_dir(&cfg.model_dir, root);
            let eng = SenseVoiceSherpaAsr::new(dir.clone())
                .with_language(sensevoice_language_from_locale(&cfg.locale))
                .with_max_audio_bytes(cfg.max_audio_bytes);
            if !eng.is_ready() {
                return Err(format!(
                    "SenseVoice model not ready under {} (set asr.model_dir, asr.models_root, or LUMEN_MODELS_DIR)",
                    dir.display()
                ));
            }
            tracing::info!(dir = %dir.display(), "ASR engine: sensevoice");
            Ok(Some(Arc::new(eng)))
        }
        EngineKind::Whisper => {
            let dir = resolve_whisper_model_dir(&cfg.model_dir, root);
            let lang = whisper_language_from_locale(&cfg.locale);
            let eng = WhisperAsr::new(dir.clone())
                .with_language(lang)
                .with_max_audio_bytes(cfg.max_audio_bytes);
            if !eng.is_ready() {
                return Err(format!(
                    "Whisper model not ready under {} (set asr.model_dir, asr.models_root, or LUMEN_MODELS_DIR)",
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

fn resolve_sensevoice_model_dir(configured: &Path, models_root: Option<&Path>) -> PathBuf {
    if !configured.as_os_str().is_empty() && sensevoice_ready(configured) {
        configured.to_path_buf()
    } else {
        default_sensevoice_dir_with_root(models_root)
    }
}

fn resolve_whisper_model_dir(configured: &Path, models_root: Option<&Path>) -> PathBuf {
    if !configured.as_os_str().is_empty() && whisper_ready(configured) {
        configured.to_path_buf()
    } else {
        default_whisper_dir_with_root(models_root)
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
        assert_eq!(
            EngineKind::parse("sensevoice"),
            Some(EngineKind::SenseVoice)
        );
        assert_eq!(EngineKind::parse("qwen"), Some(EngineKind::OpenAiAudio));
        assert_eq!(
            EngineKind::parse("qwen_asr_0.8b"),
            Some(EngineKind::OpenAiAudio)
        );
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

    #[test]
    fn invalid_selected_model_falls_back_to_ready_shared_model() {
        let root = std::env::temp_dir().join(format!(
            "lumen-navi-invalid-selected-model-{}",
            std::process::id()
        ));
        let shared = root.join("sensevoice");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("model.int8.onnx"), b"model").unwrap();
        std::fs::write(shared.join("tokens.txt"), b"tokens").unwrap();

        let selected = root.join("deleted-custom-model");
        assert_eq!(resolve_sensevoice_model_dir(&selected, Some(&root)), shared);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shared_model_contract_matches_cluster_v1() {
        let bytes = include_bytes!("../../../docs/SHARED_MODELS_CONTRACT.md");
        assert_eq!(fnv1a64(bytes), 0xc877_89f4_de20_5e71);
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }
}
