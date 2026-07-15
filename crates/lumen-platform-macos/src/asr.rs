//! On-device Observe ASR via Speech.framework (file-based recognition).

use async_trait::async_trait;
use lumen_platform::{AsrEngine, AsrResult, PlatformError};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::debug;

/// Serialize Speech recognition (framework is not free-threaded for heavy use).
static SPEECH_LOCK: Mutex<()> = Mutex::new(());

const CODE_OK: i32 = 0;
const CODE_EMPTY: i32 = 1;
const CODE_AUTH: i32 = 2;
const CODE_UNAVAILABLE: i32 = 3;
const CODE_UNSUPPORTED: i32 = 5;

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_asr_is_supported() -> i32;
    fn lumen_asr_transcribe_file(
        path: *const c_char,
        locale: *const c_char,
        out_text: *mut *mut c_char,
        out_err: *mut *mut c_char,
    ) -> i32;
    fn lumen_asr_free(p: *mut c_char);
}

#[derive(Clone)]
pub struct MacSpeechAsr {
    max_audio_bytes: usize,
}

impl Default for MacSpeechAsr {
    fn default() -> Self {
        Self {
            max_audio_bytes: 8 * 1024 * 1024,
        }
    }
}

impl MacSpeechAsr {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_audio_bytes(max_audio_bytes: usize) -> Self {
        Self { max_audio_bytes }
    }
}

#[async_trait]
impl AsrEngine for MacSpeechAsr {
    fn is_supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            unsafe { lumen_asr_is_supported() != 0 }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        locale: &str,
    ) -> Result<AsrResult, PlatformError> {
        if audio.is_empty() {
            return Err(PlatformError::Message("empty audio".into()));
        }
        if audio.len() > self.max_audio_bytes {
            return Err(PlatformError::Message(format!(
                "audio too large: {} bytes (max {})",
                audio.len(),
                self.max_audio_bytes
            )));
        }
        if !self.is_supported() {
            return Err(PlatformError::Unsupported("Speech ASR not available".into()));
        }

        let locale = locale.to_string();
        let audio = audio.to_vec();
        let max = self.max_audio_bytes;
        tokio::task::spawn_blocking(move || {
            let _guard = SPEECH_LOCK
                .lock()
                .map_err(|_| PlatformError::Message("speech lock poisoned".into()))?;
            transcribe_blocking(&audio, &locale, max)
        })
        .await
        .map_err(|e| PlatformError::Message(format!("asr join: {e}")))?
    }
}

fn transcribe_blocking(
    audio: &[u8],
    locale: &str,
    _max: usize,
) -> Result<AsrResult, PlatformError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (audio, locale);
        return Err(PlatformError::Unsupported("macOS only".into()));
    }
    #[cfg(target_os = "macos")]
    {
        let tmp = write_temp_wav(audio)?;
        let path_c = CString::new(tmp.to_string_lossy().as_bytes())
            .map_err(|e| PlatformError::Message(format!("path: {e}")))?;
        let locale_c = CString::new(locale)
            .map_err(|e| PlatformError::Message(format!("locale: {e}")))?;

        let mut out_text: *mut c_char = std::ptr::null_mut();
        let mut out_err: *mut c_char = std::ptr::null_mut();
        let code = unsafe {
            lumen_asr_transcribe_file(
                path_c.as_ptr(),
                locale_c.as_ptr(),
                &mut out_text,
                &mut out_err,
            )
        };
        let text = take_cstr(out_text);
        let err = take_cstr(out_err);
        let _ = std::fs::remove_file(&tmp);

        match code {
            CODE_OK => {
                debug!(chars = text.chars().count(), locale, "speech asr ok");
                Ok(AsrResult {
                    text,
                    confidence: 0.0, // Speech framework does not always expose conf
                    language: Some(locale.to_string()),
                    engine: "speech".into(),
                })
            }
            CODE_EMPTY => Err(PlatformError::Message(err.if_empty("empty audio"))),
            CODE_AUTH => Err(PlatformError::PermissionDenied(
                err.if_empty("speech recognition not authorized"),
            )),
            CODE_UNAVAILABLE => Err(PlatformError::Unsupported(
                err.if_empty("speech recognizer unavailable"),
            )),
            CODE_UNSUPPORTED => Err(PlatformError::Unsupported(
                err.if_empty("speech not supported"),
            )),
            _ => Err(PlatformError::Message(err.if_empty("speech recognition failed"))),
        }
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}

fn take_cstr(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        lumen_asr_free(p);
        s
    }
}

fn write_temp_wav(audio: &[u8]) -> Result<PathBuf, PlatformError> {
    let dir = std::env::temp_dir().join("lumen-navi-asr");
    std::fs::create_dir_all(&dir).map_err(PlatformError::from_io)?;
    let path = dir.join(format!("{}.wav", uuid_simple()));
    std::fs::write(&path, audio).map_err(PlatformError::from_io)?;
    Ok(path)
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}")
}

trait FromIo {
    fn from_io(e: std::io::Error) -> Self;
}

impl FromIo for PlatformError {
    fn from_io(e: std::io::Error) -> Self {
        PlatformError::Message(format!("io: {e}"))
    }
}
