//! Main-display screenshot via CoreGraphics.

use async_trait::async_trait;
use lumen_platform::{PlatformError, ScreenCapturer, ScreenshotFrame};
use tracing::debug;

pub struct MacScreenCapturer {
    /// Longest edge after optional downscale (0 = no downscale).
    pub max_edge: u32,
}

impl Default for MacScreenCapturer {
    fn default() -> Self {
        Self { max_edge: 1920 }
    }
}

impl MacScreenCapturer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_edge(max_edge: u32) -> Self {
        Self { max_edge }
    }
}

#[async_trait]
impl ScreenCapturer for MacScreenCapturer {
    async fn capture_main_display(&self) -> Result<ScreenshotFrame, PlatformError> {
        let max_edge = self.max_edge;
        // CG calls are blocking; keep them off the async runtime.
        tokio::task::spawn_blocking(move || capture_main_display_sync(max_edge))
            .await
            .map_err(|e| PlatformError::Message(format!("capture join: {e}")))?
    }
}

fn capture_main_display_sync(max_edge: u32) -> Result<ScreenshotFrame, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::CGDisplay;
        use image::codecs::png::PngEncoder;
        use image::{ColorType, ImageEncoder};

        let display = CGDisplay::main();
        let display_id = display.id;
        let cg_image = display.image().ok_or_else(|| {
            PlatformError::PermissionDenied(
                "CGDisplayCreateImage returned null — grant Screen Recording to this process \
                 (System Settings → Privacy & Security → Screen Recording)"
                    .into(),
            )
        })?;

        let width = cg_image.width() as u32;
        let height = cg_image.height() as u32;
        if width == 0 || height == 0 {
            return Err(PlatformError::Message("empty display image".into()));
        }

        let bpp = (cg_image.bits_per_pixel() / 8) as usize;
        if bpp < 3 {
            return Err(PlatformError::Message(format!(
                "unsupported bits_per_pixel={}",
                cg_image.bits_per_pixel()
            )));
        }
        let stride = cg_image.bytes_per_row();
        let cf_data = cg_image.data();
        let raw = cf_data.bytes();

        // CoreGraphics main-display images are typically BGRA (32-bit).
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            let row = y * stride;
            for x in 0..width as usize {
                let i = row + x * bpp;
                if i + 2 >= raw.len() {
                    break;
                }
                let b = raw[i];
                let g = raw[i + 1];
                let r = raw[i + 2];
                let a = if bpp >= 4 { raw[i + 3] } else { 255 };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }

        let mut img = image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
            PlatformError::Message("failed to build RGBA buffer from CGImage".into())
        })?;

        if max_edge > 0 {
            let long = width.max(height);
            if long > max_edge {
                let scale = max_edge as f32 / long as f32;
                let nw = ((width as f32) * scale).round().max(1.0) as u32;
                let nh = ((height as f32) * scale).round().max(1.0) as u32;
                img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
                debug!(width, height, nw, nh, "downscaled screenshot");
            }
        }

        let (out_w, out_h) = img.dimensions();
        let mut png_bytes = Vec::new();
        {
            let encoder = PngEncoder::new(&mut png_bytes);
            encoder
                .write_image(img.as_raw(), out_w, out_h, ColorType::Rgba8.into())
                .map_err(|e| PlatformError::Message(format!("png encode: {e}")))?;
        }

        Ok(ScreenshotFrame {
            png_bytes,
            width: out_w,
            height: out_h,
            display_id: Some(display_id),
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = max_edge;
        Err(PlatformError::Unsupported(
            "MacScreenCapturer requires macOS".into(),
        ))
    }
}

