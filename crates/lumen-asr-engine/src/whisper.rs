//! Whisper offline ASR via sherpa-onnx (same pattern as lumen-asr).

use crate::paths::{whisper_decoder_path, whisper_encoder_path, whisper_tokens_path};
use crate::wav::prepare_for_offline_asr;
use async_trait::async_trait;
use lumen_platform::{AsrEngine, AsrResult, PlatformError};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "sherpa")]
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineWhisperModelConfig};

struct WhisperInner {
    model_dir: PathBuf,
    language: String,
    max_audio_bytes: usize,
    #[cfg(feature = "sherpa")]
    recognizer: Mutex<Option<OfflineRecognizer>>,
}

pub struct WhisperAsr {
    inner: Arc<WhisperInner>,
}

impl Clone for WhisperAsr {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl WhisperAsr {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(WhisperInner {
                model_dir: model_dir.into(),
                language: "en".into(),
                max_audio_bytes: 8 * 1024 * 1024,
                #[cfg(feature = "sherpa")]
                recognizer: Mutex::new(None),
            }),
        }
    }

    pub fn with_language(self, language: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(WhisperInner {
                model_dir: self.inner.model_dir.clone(),
                language: language.into(),
                max_audio_bytes: self.inner.max_audio_bytes,
                #[cfg(feature = "sherpa")]
                recognizer: Mutex::new(None),
            }),
        }
    }

    pub fn with_max_audio_bytes(self, max_audio_bytes: usize) -> Self {
        Self {
            inner: Arc::new(WhisperInner {
                model_dir: self.inner.model_dir.clone(),
                language: self.inner.language.clone(),
                max_audio_bytes,
                #[cfg(feature = "sherpa")]
                recognizer: Mutex::new(None),
            }),
        }
    }

    pub fn model_dir(&self) -> &Path {
        &self.inner.model_dir
    }

    pub fn is_ready(&self) -> bool {
        whisper_encoder_path(&self.inner.model_dir).is_some()
            && whisper_decoder_path(&self.inner.model_dir).is_some()
            && whisper_tokens_path(&self.inner.model_dir).is_some()
    }
}

#[cfg(feature = "sherpa")]
impl WhisperInner {
    fn ensure_recognizer(&self) -> Result<(), PlatformError> {
        let mut guard = self.recognizer.lock();
        if guard.is_some() {
            return Ok(());
        }
        let encoder = whisper_encoder_path(&self.model_dir).ok_or_else(|| {
            PlatformError::Message(format!(
                "Whisper encoder not found under {}",
                self.model_dir.display()
            ))
        })?;
        let decoder = whisper_decoder_path(&self.model_dir).ok_or_else(|| {
            PlatformError::Message(format!(
                "Whisper decoder not found under {}",
                self.model_dir.display()
            ))
        })?;
        let tokens = whisper_tokens_path(&self.model_dir).ok_or_else(|| {
            PlatformError::Message(format!(
                "Whisper tokens not found under {}",
                self.model_dir.display()
            ))
        })?;

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.whisper = OfflineWhisperModelConfig {
            encoder: Some(encoder.display().to_string()),
            decoder: Some(decoder.display().to_string()),
            language: Some(self.language.clone()),
            task: Some("transcribe".into()),
            tail_paddings: 0,
            enable_token_timestamps: false,
            enable_segment_timestamps: false,
        };
        config.model_config.tokens = Some(tokens.display().to_string());
        config.model_config.num_threads = 2;
        config.model_config.provider = Some("cpu".into());

        tracing::info!(encoder = %encoder.display(), "creating Whisper OfflineRecognizer");
        let rec = OfflineRecognizer::create(&config).ok_or_else(|| {
            PlatformError::Message(format!(
                "failed to create Whisper recognizer under {}",
                self.model_dir.display()
            ))
        })?;
        *guard = Some(rec);
        Ok(())
    }

    fn decode_sync(&self, samples: &[f32], sample_rate: u32) -> Result<String, PlatformError> {
        self.ensure_recognizer()?;
        let guard = self.recognizer.lock();
        let recognizer = guard
            .as_ref()
            .ok_or_else(|| PlatformError::Message("whisper recognizer missing".into()))?;

        let stream = recognizer.create_stream();
        stream.accept_waveform(sample_rate as i32, samples);
        recognizer.decode(&stream);
        let text = stream
            .get_result()
            .map(|r| r.text)
            .unwrap_or_default()
            .trim()
            .to_string();
        Ok(text)
    }
}

#[async_trait]
impl AsrEngine for WhisperAsr {
    fn is_supported(&self) -> bool {
        #[cfg(feature = "sherpa")]
        {
            self.is_ready()
        }
        #[cfg(not(feature = "sherpa"))]
        {
            false
        }
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        _locale: &str,
    ) -> Result<AsrResult, PlatformError> {
        if audio.is_empty() {
            return Err(PlatformError::Message("empty audio".into()));
        }
        if audio.len() > self.inner.max_audio_bytes {
            return Err(PlatformError::Message(format!(
                "audio too large: {} bytes (max {})",
                audio.len(),
                self.inner.max_audio_bytes
            )));
        }

        #[cfg(not(feature = "sherpa"))]
        {
            return Err(PlatformError::Unsupported(
                "build with feature `sherpa` for Whisper".into(),
            ));
        }

        #[cfg(feature = "sherpa")]
        {
            let pcm = prepare_for_offline_asr(audio)?;
            if pcm.samples.is_empty() {
                return Err(PlatformError::Message("empty pcm after decode".into()));
            }
            let inner = Arc::clone(&self.inner);
            let samples = pcm.samples;
            let sr = pcm.sample_rate;
            let text = tokio::task::spawn_blocking(move || inner.decode_sync(&samples, sr))
                .await
                .map_err(|e| PlatformError::Message(format!("asr join: {e}")))??;

            Ok(AsrResult {
                text,
                confidence: 0.0,
                language: Some(self.inner.language.clone()),
                engine: "whisper".into(),
            })
        }
    }
}
