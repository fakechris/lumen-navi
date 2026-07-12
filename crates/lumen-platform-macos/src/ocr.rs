//! In-process Vision OCR engine.

use async_trait::async_trait;
use lumen_platform::{OcrBox, OcrEngine, OcrResult, PlatformError};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_ocr_is_supported() -> i32;
    fn lumen_ocr_recognize_text(
        data: *const u8,
        len: i32,
        langs: *const *const c_char,
        lang_count: i32,
        accurate: i32,
    ) -> *mut c_char;
    fn lumen_ocr_recognize_boxes_json(
        data: *const u8,
        len: i32,
        langs: *const *const c_char,
        lang_count: i32,
    ) -> *mut c_char;
    fn lumen_ocr_free(p: *mut c_char);
}

/// Default product languages (Chinese UI + English).
pub fn default_ocr_languages() -> Vec<String> {
    vec!["zh-Hans".into(), "en-US".into()]
}

pub struct MacVisionOcr {
    /// Serialize Vision calls (GPU / framework thrash).
    lock: Mutex<()>,
}

impl Default for MacVisionOcr {
    fn default() -> Self {
        Self {
            lock: Mutex::new(()),
        }
    }
}

impl MacVisionOcr {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OcrEngine for MacVisionOcr {
    fn is_supported(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            unsafe { lumen_ocr_is_supported() == 1 }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        let image = image.to_vec();
        let languages = languages.to_vec();
        let this_supported = self.is_supported();
        if !this_supported {
            return Err(PlatformError::Unsupported(
                "Vision OCR requires macOS 10.15+".into(),
            ));
        }
        // Hold mutex across blocking work via owned clone of lock pattern:
        tokio::task::spawn_blocking(move || recognize_text_sync(&image, &languages, true))
            .await
            .map_err(|e| PlatformError::Message(format!("ocr join: {e}")))?
    }

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        let image = image.to_vec();
        let languages = languages.to_vec();
        if !self.is_supported() {
            return Err(PlatformError::Unsupported(
                "Vision OCR requires macOS 10.15+".into(),
            ));
        }
        tokio::task::spawn_blocking(move || recognize_boxes_sync(&image, &languages))
            .await
            .map_err(|e| PlatformError::Message(format!("ocr join: {e}")))?
    }
}

fn with_lang_ptrs<T>(languages: &[String], f: impl FnOnce(&[*const c_char]) -> T) -> T {
    let cstrings: Vec<CString> = languages
        .iter()
        .filter_map(|s| CString::new(s.as_str()).ok())
        .collect();
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    f(&ptrs)
}

fn recognize_text_sync(
    image: &[u8],
    languages: &[String],
    accurate: bool,
) -> Result<OcrResult, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        if image.is_empty() {
            return Ok(OcrResult {
                text: String::new(),
                confidence: 0.0,
                languages: languages.to_vec(),
                mode: if accurate { "accurate" } else { "fast" }.into(),
                boxes: vec![],
            });
        }
        let raw = with_lang_ptrs(languages, |ptrs| unsafe {
            let p = lumen_ocr_recognize_text(
                image.as_ptr(),
                image.len() as i32,
                ptrs.as_ptr(),
                ptrs.len() as i32,
                if accurate { 1 } else { 0 },
            );
            if p.is_null() {
                return String::new();
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            lumen_ocr_free(p);
            s
        });
        let (text, confidence) = split_text_conf(&raw);
        Ok(OcrResult {
            text,
            confidence,
            languages: languages.to_vec(),
            mode: if accurate { "accurate" } else { "fast" }.into(),
            boxes: vec![],
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, languages, accurate);
        Err(PlatformError::Unsupported("OCR requires macOS".into()))
    }
}

fn recognize_boxes_sync(image: &[u8], languages: &[String]) -> Result<OcrResult, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        if image.is_empty() {
            return Ok(OcrResult {
                text: String::new(),
                confidence: 0.0,
                languages: languages.to_vec(),
                mode: "fast".into(),
                boxes: vec![],
            });
        }
        let json = with_lang_ptrs(languages, |ptrs| unsafe {
            let p = lumen_ocr_recognize_boxes_json(
                image.as_ptr(),
                image.len() as i32,
                ptrs.as_ptr(),
                ptrs.len() as i32,
            );
            if p.is_null() {
                return "[]".to_string();
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            lumen_ocr_free(p);
            s
        });
        let boxes: Vec<OcrBox> = serde_json::from_str(&json).unwrap_or_default();
        let mut conf_sum = 0.0;
        let mut conf_n = 0usize;
        let mut lines = Vec::new();
        // Reading order: top-to-bottom (Vision y is bottom-left origin → larger y is higher)
        let mut ordered = boxes.clone();
        ordered.sort_by(|a, b| {
            let row = (a.y - b.y).abs();
            let tol = a.h.max(b.h) * 0.6;
            let tol = if tol < 0.012 { 0.012 } else { tol };
            if row > tol {
                b.y.total_cmp(&a.y)
            } else {
                a.x.total_cmp(&b.x)
            }
        });
        for b in &ordered {
            let t = b.text.trim();
            if t.is_empty() {
                continue;
            }
            lines.push(t.to_string());
            if b.confidence > 0.0 {
                conf_sum += b.confidence;
                conf_n += 1;
            }
        }
        Ok(OcrResult {
            text: lines.join("\n"),
            confidence: if conf_n > 0 {
                conf_sum / conf_n as f64
            } else {
                0.0
            },
            languages: languages.to_vec(),
            mode: "fast".into(),
            boxes,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, languages);
        Err(PlatformError::Unsupported("OCR requires macOS".into()))
    }
}

fn split_text_conf(raw: &str) -> (String, f64) {
    if let Some(idx) = raw.rfind("\n---\n") {
        let text = raw[..idx].to_string();
        let conf: f64 = raw[idx + 5..].trim().parse().unwrap_or(0.0);
        (text, conf)
    } else {
        (raw.to_string(), 0.0)
    }
}

// Silence unused lock field warning until we use it inside spawn_blocking with Arc.
impl MacVisionOcr {
    pub fn run_serialized<R>(&self, f: impl FnOnce() -> R) -> R {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        f()
    }
}
