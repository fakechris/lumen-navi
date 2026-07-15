//! OpenAI-compatible batch audio transcription.
//!
//! Used for cloud / remote models such as Qwen ASR 0.8B (DashScope or local
//! OpenAI-compatible server), Whisper API, etc.

use crate::wav::{prepare_for_offline_asr, samples_to_wav_mono_i16};
use async_trait::async_trait;
use lumen_platform::{AsrEngine, AsrResult, PlatformError};
use reqwest::multipart::{Form, Part};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct OpenAiAudioConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    /// Optional ISO-639-1 language hint (e.g. `zh`, `en`).
    pub language: Option<String>,
    pub max_audio_bytes: usize,
    /// Engine id written into transcript.v1 (e.g. `openai_audio`, `qwen_asr`).
    pub engine_label: String,
}

impl Default for OpenAiAudioConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "whisper-1".into(),
            timeout: Duration::from_secs(120),
            language: None,
            max_audio_bytes: 8 * 1024 * 1024,
            engine_label: "openai_audio".into(),
        }
    }
}

pub struct OpenAiAudioAsr {
    client: reqwest::Client,
    config: OpenAiAudioConfig,
}

impl OpenAiAudioAsr {
    pub fn new(config: OpenAiAudioConfig) -> Result<Self, PlatformError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| PlatformError::Message(format!("http client: {e}")))?;
        Ok(Self { client, config })
    }
}

#[async_trait]
impl AsrEngine for OpenAiAudioAsr {
    fn is_supported(&self) -> bool {
        !self.config.base_url.trim().is_empty() && !self.config.model.trim().is_empty()
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        locale: &str,
    ) -> Result<AsrResult, PlatformError> {
        if audio.is_empty() {
            return Err(PlatformError::Message("empty audio".into()));
        }
        if audio.len() > self.config.max_audio_bytes {
            return Err(PlatformError::Message(format!(
                "audio too large: {} bytes (max {})",
                audio.len(),
                self.config.max_audio_bytes
            )));
        }
        if !self.is_supported() {
            return Err(PlatformError::Unsupported(
                "openai_audio base_url/model not configured".into(),
            ));
        }

        // Normalize to 16 kHz mono WAV for consistent remote behavior.
        let pcm = prepare_for_offline_asr(audio)?;
        if pcm.samples.is_empty() {
            return Err(PlatformError::Message("empty pcm after decode".into()));
        }
        let wav = samples_to_wav_mono_i16(&pcm.samples, pcm.sample_rate);

        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{base}/audio/transcriptions");

        let part = Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| PlatformError::Message(e.to_string()))?;
        let mut form = Form::new()
            .part("file", part)
            .text("model", self.config.model.clone());

        let lang = self
            .config
            .language
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| locale_to_lang_hint(locale));
        if let Some(lang) = lang {
            form = form.text("language", lang);
        }

        let mut builder = self.client.post(&url).multipart(form);
        if !self.config.api_key.is_empty() {
            builder = builder.bearer_auth(&self.config.api_key);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| PlatformError::Message(format!("http: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PlatformError::Message(format!("{status}: {body}")));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| PlatformError::Message(format!("json: {e}")))?;
        let text = v
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        Ok(AsrResult {
            text,
            confidence: 0.0,
            language: self
                .config
                .language
                .clone()
                .or_else(|| Some(locale.to_string())),
            engine: self.config.engine_label.clone(),
        })
    }
}

fn locale_to_lang_hint(locale: &str) -> Option<String> {
    let primary = locale.split(['-', '_']).next()?.to_ascii_lowercase();
    if primary.is_empty() {
        None
    } else {
        Some(primary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_hint() {
        assert_eq!(locale_to_lang_hint("zh-CN").as_deref(), Some("zh"));
        assert_eq!(locale_to_lang_hint("en_US").as_deref(), Some("en"));
    }
}
