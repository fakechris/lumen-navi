//! On-device OCR via `Windows.Media.Ocr`.
//!
//! Runs entirely offline against the language packs installed on the machine.
//! Recognition is serialized like the macOS Vision path: the engine is cheap
//! to create but heavy to run, and Observe must never let OCR starve capture.

use async_trait::async_trait;
#[cfg(target_os = "windows")]
use lumen_platform::OcrBox;
use lumen_platform::{OcrEngine, OcrResult, PlatformError};
use std::sync::Mutex;

/// Serialize recognition so a burst of queued frames cannot saturate the box.
static OCR_LOCK: Mutex<()> = Mutex::new(());

/// Preferred recognizer languages, matching the macOS default pair. Windows
/// only recognizes languages whose OCR pack is installed; unavailable entries
/// fall back to the user profile languages.
pub fn default_ocr_languages() -> Vec<String> {
    vec!["zh-Hans".into(), "en-US".into()]
}

#[derive(Clone)]
pub struct WinOcr {
    max_image_bytes: usize,
}

impl Default for WinOcr {
    fn default() -> Self {
        Self {
            max_image_bytes: 25 * 1024 * 1024,
        }
    }
}

impl WinOcr {
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
}

#[async_trait]
impl OcrEngine for WinOcr {
    fn is_supported(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            engine_supported()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.recognize(image, languages, false).await
    }

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.recognize(image, languages, true).await
    }
}

impl WinOcr {
    async fn recognize(
        &self,
        image: &[u8],
        languages: &[String],
        want_boxes: bool,
    ) -> Result<OcrResult, PlatformError> {
        self.guard_size(image)?;
        let image = image.to_vec();
        let languages = languages.to_vec();
        tokio::task::spawn_blocking(move || {
            let _guard = OCR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            recognize_sync(&image, &languages, want_boxes)
        })
        .await
        .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }
}

#[cfg(not(target_os = "windows"))]
fn recognize_sync(
    _image: &[u8],
    _languages: &[String],
    _want_boxes: bool,
) -> Result<OcrResult, PlatformError> {
    Err(PlatformError::Unsupported("OCR requires Windows".into()))
}

#[cfg(target_os = "windows")]
fn engine_supported() -> bool {
    use windows::Media::Ocr::OcrEngine as WinRtOcrEngine;

    WinRtOcrEngine::AvailableRecognizerLanguages()
        .map(|langs| langs.Size().unwrap_or(0) > 0)
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn recognize_sync(
    image: &[u8],
    languages: &[String],
    want_boxes: bool,
) -> Result<OcrResult, PlatformError> {
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine as WinRtOcrEngine;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

    let map = |e: windows::core::Error| PlatformError::Message(format!("winrt ocr: {e}"));

    let engine = create_engine(languages)?;

    let stream = InMemoryRandomAccessStream::new().map_err(map)?;
    let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0).map_err(map)?)
        .map_err(map)?;
    writer.WriteBytes(image).map_err(map)?;
    writer.StoreAsync().map_err(map)?.get().map_err(map)?;
    writer.FlushAsync().map_err(map)?.get().map_err(map)?;
    // Detach before dropping the writer, otherwise it closes the stream.
    let _ = writer.DetachStream();
    stream.Seek(0).map_err(map)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(map)?
        .get()
        .map_err(|e| PlatformError::Message(format!("decode image: {e}")))?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(map)?
        .get()
        .map_err(map)?;

    let width = bitmap.PixelWidth().unwrap_or(0).max(1) as f64;
    let height = bitmap.PixelHeight().unwrap_or(0).max(1) as f64;
    let max_dim = WinRtOcrEngine::MaxImageDimension().unwrap_or(u32::MAX);
    if width as u32 > max_dim || height as u32 > max_dim {
        return Err(PlatformError::Message(format!(
            "image {}x{} exceeds OCR max dimension {max_dim}",
            width as u32, height as u32
        )));
    }

    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(map)?
        .get()
        .map_err(map)?;

    let text = result.Text().map_err(map)?.to_string_lossy();
    let mut boxes = Vec::new();
    if want_boxes {
        for line in result.Lines().map_err(map)?.into_iter() {
            let line_text = line.Text().map_err(map)?.to_string_lossy();
            if line_text.trim().is_empty() {
                continue;
            }
            // WinRT gives rects per word only; the union is the line box.
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for word in line.Words().map_err(map)?.into_iter() {
                let r = word.BoundingRect().map_err(map)?;
                min_x = min_x.min(r.X as f64);
                min_y = min_y.min(r.Y as f64);
                max_x = max_x.max((r.X + r.Width) as f64);
                max_y = max_y.max((r.Y + r.Height) as f64);
            }
            if min_x > max_x || min_y > max_y {
                continue;
            }
            // Ports use Vision's convention: normalized, bottom-left origin.
            boxes.push(OcrBox {
                x: min_x / width,
                y: 1.0 - (max_y / height),
                w: (max_x - min_x) / width,
                h: (max_y - min_y) / height,
                text: line_text,
                confidence: 0.0,
            });
        }
    }

    Ok(OcrResult {
        text,
        // Windows.Media.Ocr exposes no confidence score. 0.0 means "no signal",
        // not "bad recognition" — consumers hide the value rather than rank on it.
        confidence: 0.0,
        languages: languages.to_vec(),
        mode: if want_boxes { "fast" } else { "accurate" }.into(),
        boxes,
    })
}

/// Best available recognizer: first configured language that has an installed
/// OCR pack, else the user profile languages.
#[cfg(target_os = "windows")]
fn create_engine(languages: &[String]) -> Result<windows::Media::Ocr::OcrEngine, PlatformError> {
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Media::Ocr::OcrEngine as WinRtOcrEngine;

    for tag in languages {
        let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag.as_str())) else {
            continue;
        };
        if let Ok(engine) = WinRtOcrEngine::TryCreateFromLanguage(&language) {
            return Ok(engine);
        }
    }
    WinRtOcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        PlatformError::Unsupported(format!(
            "no Windows OCR language pack installed \
             (Settings → Time & language → Language & region → \
             add a language with the optional OCR feature): {e}"
        ))
    })
}
