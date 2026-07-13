//! In-process Vision OCR engine — serialized, error-mapped, size-guarded.

use crate::{OcrBox, OcrEngine, OcrResult, PlatformError};
use async_trait::async_trait;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Arc, Mutex};

/// Global Vision serialization (framework is not free-threaded for heavy use).
static VISION_LOCK: Mutex<()> = Mutex::new(());

const CODE_OK: i32 = 0;
const CODE_EMPTY: i32 = 1;
const CODE_DECODE: i32 = 2;
const CODE_VISION: i32 = 3;
const CODE_UNSUPPORTED: i32 = 4;

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_ocr_is_supported() -> i32;
    fn lumen_ocr_recognize_text(
        data: *const u8,
        len: i32,
        langs: *const *const c_char,
        lang_count: i32,
        accurate: i32,
        out_text: *mut *mut c_char,
        out_err: *mut *mut c_char,
    ) -> i32;
    fn lumen_ocr_recognize_boxes_json(
        data: *const u8,
        len: i32,
        langs: *const *const c_char,
        lang_count: i32,
        out_json: *mut *mut c_char,
        out_err: *mut *mut c_char,
    ) -> i32;
    fn lumen_ocr_free(p: *mut c_char);
}

pub fn default_ocr_languages() -> Vec<String> {
    vec!["zh-Hans".into(), "en-US".into()]
}

#[derive(Clone)]
pub struct MacVisionOcr {
    max_image_bytes: usize,
}

impl Default for MacVisionOcr {
    fn default() -> Self {
        Self {
            max_image_bytes: 25 * 1024 * 1024,
        }
    }
}

impl MacVisionOcr {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_image_bytes(max_image_bytes: usize) -> Self {
        Self { max_image_bytes }
    }

    fn guard_size(&self, image: &[u8]) -> Result<(), PlatformError> {
        if image.is_empty() {
            return Err(PlatformError::Message("empty image".into()));
        }
        if image.len() > self.max_image_bytes {
            return Err(PlatformError::Message(format!(
                "image too large: {} bytes (max {})",
                image.len(),
                self.max_image_bytes
            )));
        }
        Ok(())
    }

    pub async fn recognize_text_fast(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.recognize_text_with_accuracy(image, languages, false)
            .await
    }

    async fn recognize_text_with_accuracy(
        &self,
        image: &[u8],
        languages: &[String],
        accurate: bool,
    ) -> Result<OcrResult, PlatformError> {
        self.guard_size(image)?;
        let image = image.to_vec();
        let languages = languages.to_vec();
        let max = self.max_image_bytes;
        tokio::task::spawn_blocking(move || {
            let _g = VISION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let eng = MacVisionOcr {
                max_image_bytes: max,
            };
            eng.guard_size(&image)?;
            recognize_text_sync(&image, &languages, accurate)
        })
        .await
        .map_err(|e| PlatformError::Message(format!("ocr join: {e}")))?
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
        self.recognize_text_with_accuracy(image, languages, true)
            .await
    }

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.guard_size(image)?;
        let image = image.to_vec();
        let languages = languages.to_vec();
        let max = self.max_image_bytes;
        tokio::task::spawn_blocking(move || {
            let _g = VISION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let eng = MacVisionOcr {
                max_image_bytes: max,
            };
            eng.guard_size(&image)?;
            recognize_boxes_sync(&image, &languages)
        })
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

fn take_c_string(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        lumen_ocr_free(p);
        s
    }
}

fn map_code(code: i32, err: String) -> PlatformError {
    match code {
        CODE_EMPTY => PlatformError::Message(format!("ocr empty input: {err}")),
        CODE_DECODE => PlatformError::Message(format!("ocr decode failed: {err}")),
        CODE_VISION => PlatformError::Message(format!("ocr vision error: {err}")),
        CODE_UNSUPPORTED => PlatformError::Unsupported(err),
        _ => PlatformError::Message(format!("ocr error {code}: {err}")),
    }
}

fn recognize_text_sync(
    image: &[u8],
    languages: &[String],
    accurate: bool,
) -> Result<OcrResult, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let (code, text, err) = with_lang_ptrs(languages, |ptrs| unsafe {
            let mut out: *mut c_char = std::ptr::null_mut();
            let mut e: *mut c_char = std::ptr::null_mut();
            let code = lumen_ocr_recognize_text(
                image.as_ptr(),
                image.len() as i32,
                ptrs.as_ptr(),
                ptrs.len() as i32,
                if accurate { 1 } else { 0 },
                &mut out,
                &mut e,
            );
            (code, take_c_string(out), take_c_string(e))
        });
        if code != CODE_OK {
            return Err(map_code(code, err));
        }
        let (text, confidence) = split_text_conf(&text);
        Ok(OcrResult {
            text: normalize_text(&text),
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
        let (code, json, err) = with_lang_ptrs(languages, |ptrs| unsafe {
            let mut out: *mut c_char = std::ptr::null_mut();
            let mut e: *mut c_char = std::ptr::null_mut();
            let code = lumen_ocr_recognize_boxes_json(
                image.as_ptr(),
                image.len() as i32,
                ptrs.as_ptr(),
                ptrs.len() as i32,
                &mut out,
                &mut e,
            );
            (code, take_c_string(out), take_c_string(e))
        });
        if code != CODE_OK {
            return Err(map_code(code, err));
        }
        let boxes: Vec<OcrBox> = serde_json::from_str(&json).unwrap_or_default();
        let text = reading_order_text(&boxes);
        let confidence = mean_box_conf(&boxes);
        Ok(OcrResult {
            text: normalize_text(&text),
            confidence,
            languages: languages.to_vec(),
            mode: "accurate_layout".into(),
            boxes,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, languages);
        Err(PlatformError::Unsupported("OCR requires macOS".into()))
    }
}

fn reading_order_text(boxes: &[OcrBox]) -> String {
    struct Line<'a> {
        anchor_y: f64,
        max_height: f64,
        boxes: Vec<&'a OcrBox>,
    }

    let mut ordered: Vec<&OcrBox> = boxes.iter().filter(|b| !b.text.trim().is_empty()).collect();
    ordered.sort_by(|left, right| {
        box_center_y(right)
            .total_cmp(&box_center_y(left))
            .then_with(|| left.x.total_cmp(&right.x))
    });
    let mut lines: Vec<Line<'_>> = Vec::new();
    for region in ordered {
        let center_y = box_center_y(region);
        let matching = lines.last_mut().filter(|line| {
            let tolerance = (line.max_height.max(region.h) * 0.6).max(0.012);
            (line.anchor_y - center_y).abs() <= tolerance
        });
        if let Some(line) = matching {
            let count = line.boxes.len() as f64;
            line.anchor_y = (line.anchor_y * count + center_y) / (count + 1.0);
            line.max_height = line.max_height.max(region.h);
            line.boxes.push(region);
        } else {
            lines.push(Line {
                anchor_y: center_y,
                max_height: region.h,
                boxes: vec![region],
            });
        }
    }
    lines
        .iter_mut()
        .flat_map(|line| {
            line.boxes.sort_by(|left, right| left.x.total_cmp(&right.x));
            line.boxes.iter().copied()
        })
        .map(|region| region.text.trim())
        .collect::<Vec<_>>()
        .join("\n")
}

fn box_center_y(region: &OcrBox) -> f64 {
    region.y + region.h / 2.0
}

fn mean_box_conf(boxes: &[OcrBox]) -> f64 {
    let mut s = 0.0;
    let mut n = 0usize;
    for b in boxes {
        if b.confidence > 0.0 {
            s += b.confidence;
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        s / n as f64
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

fn normalize_text(s: &str) -> String {
    // Collapse runaway blank lines; trim ends.
    let mut out = String::with_capacity(s.len());
    let mut blank = 0u32;
    for line in s.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    out.trim().to_string()
}

// Keep Arc unused warning away when cloning engine.
#[allow(dead_code)]
fn _arc_marker(e: MacVisionOcr) -> Arc<MacVisionOcr> {
    Arc::new(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(text: &str, x: f64, y: f64, h: f64) -> OcrBox {
        OcrBox {
            text: text.to_owned(),
            confidence: 1.0,
            x,
            y,
            w: 0.1,
            h,
        }
    }

    #[test]
    fn reading_order_is_total_and_deterministic_across_mixed_heights() {
        let boxes = vec![
            region("right", 0.7, 0.80, 0.02),
            region("lower", 0.1, 0.74, 0.10),
            region("left", 0.1, 0.81, 0.02),
            region("bottom", 0.2, 0.20, 0.03),
        ];
        let first = reading_order_text(&boxes);
        for _ in 0..100 {
            assert_eq!(reading_order_text(&boxes), first);
        }
        assert!(first.contains("left"));
        assert!(first.contains("right"));
        assert!(first.contains("lower"));
        assert!(first.ends_with("bottom"));
    }

    #[test]
    fn reading_order_sorts_boxes_left_to_right_within_a_line() {
        let boxes = vec![
            region("right", 0.7, 0.8, 0.02),
            region("left", 0.1, 0.8, 0.02),
        ];
        assert_eq!(reading_order_text(&boxes), "left\nright");
    }
}
