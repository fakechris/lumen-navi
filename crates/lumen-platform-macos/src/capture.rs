//! Multi-display capture via CoreGraphics.

use async_trait::async_trait;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder};
use lumen_platform::{
    DisplayEnumerator, DisplayId, DisplayInfo, PlatformError, RawFrame, ScreenCapturer,
    ScreenshotFrame,
};
use tracing::debug;

pub struct MacDisplays;

#[async_trait]
impl DisplayEnumerator for MacDisplays {
    async fn list_displays(&self) -> Result<Vec<DisplayInfo>, PlatformError> {
        tokio::task::spawn_blocking(list_displays_sync)
            .await
            .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }
}

pub struct MacScreenCapturer;

impl Default for MacScreenCapturer {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ScreenCapturer for MacScreenCapturer {
    async fn capture_display(
        &self,
        id: DisplayId,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    ) -> Result<ScreenshotFrame, PlatformError> {
        tokio::task::spawn_blocking(move || {
            capture_display_encoded(id, max_edge, jpeg, jpeg_quality)
        })
        .await
        .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }

    async fn capture_display_raw(
        &self,
        id: DisplayId,
        scale_div: u32,
    ) -> Result<RawFrame, PlatformError> {
        tokio::task::spawn_blocking(move || capture_display_raw_sync(id, scale_div.max(1)))
            .await
            .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }
}

fn list_displays_sync() -> Result<Vec<DisplayInfo>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::CGDisplay;

        let ids = CGDisplay::active_displays().map_err(|e| {
            PlatformError::Message(format!("CGGetActiveDisplayList failed: {e:?}"))
        })?;
        let main_id = CGDisplay::main().id;
        let mut out = Vec::with_capacity(ids.len().max(1));
        if ids.is_empty() {
            let main = CGDisplay::main();
            let b = main.bounds();
            return Ok(vec![DisplayInfo {
                id: DisplayId(main.id),
                width: b.size.width.max(1.0) as u32,
                height: b.size.height.max(1.0) as u32,
                origin_x: b.origin.x as i32,
                origin_y: b.origin.y as i32,
                is_main: true,
            }]);
        }
        for id in ids {
            let d = CGDisplay::new(id);
            let b = d.bounds();
            out.push(DisplayInfo {
                id: DisplayId(id),
                width: b.size.width.max(1.0) as u32,
                height: b.size.height.max(1.0) as u32,
                origin_x: b.origin.x as i32,
                origin_y: b.origin.y as i32,
                is_main: id == main_id,
            });
        }
        out.sort_by_key(|d| !d.is_main);
        Ok(out)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(PlatformError::Unsupported(
            "list_displays requires macOS".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
fn cg_image_for_display(id: DisplayId) -> Result<core_graphics::image::CGImage, PlatformError> {
    use core_graphics::display::CGDisplay;

    let display = CGDisplay::new(id.0);
    display.image().ok_or_else(|| {
        PlatformError::PermissionDenied(
            "CGDisplayCreateImage null — grant Screen Recording \
             (System Settings → Privacy & Security → Screen Recording)"
                .into(),
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn cg_image_for_display(_id: DisplayId) -> Result<(), PlatformError> {
    Err(PlatformError::Unsupported("capture requires macOS".into()))
}

#[cfg(target_os = "macos")]
fn rgba_from_cg(image: &core_graphics::image::CGImage) -> Result<(Vec<u8>, u32, u32), PlatformError> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    if width == 0 || height == 0 {
        return Err(PlatformError::Message("empty display image".into()));
    }
    let bpp = (image.bits_per_pixel() / 8) as usize;
    if bpp < 3 {
        return Err(PlatformError::Message(format!(
            "unsupported bpp={}",
            image.bits_per_pixel()
        )));
    }
    let stride = image.bytes_per_row();
    let data = image.data();
    let raw = data.bytes();
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
    Ok((rgba, width, height))
}

#[cfg(target_os = "macos")]
fn bgra_from_cg(image: &core_graphics::image::CGImage) -> Result<RawFrame, PlatformError> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    let bpp = (image.bits_per_pixel() / 8) as usize;
    let stride = image.bytes_per_row();
    let data = image.data();
    let raw = data.bytes();
    // Copy tightly packed BGRA for simpler gray convert
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = y * stride;
        for x in 0..width as usize {
            let i = row + x * bpp;
            if i + 2 >= raw.len() {
                bgra.extend_from_slice(&[0, 0, 0, 255]);
                continue;
            }
            let b = raw[i];
            let g = raw[i + 1];
            let r = raw[i + 2];
            let a = if bpp >= 4 { raw[i + 3] } else { 255 };
            bgra.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(RawFrame {
        bgra,
        width,
        height,
        bytes_per_row: (width as usize) * 4,
        display_id: DisplayId(0), // filled by caller
    })
}

fn capture_display_raw_sync(id: DisplayId, scale_div: u32) -> Result<RawFrame, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        use image::imageops::FilterType;

        let cg = cg_image_for_display(id)?;
        let mut frame = bgra_from_cg(&cg)?;
        frame.display_id = id;
        if scale_div <= 1 {
            return Ok(frame);
        }
        // Convert to image, downscale, back to BGRA-ish for gray (store as gray in B plane only via RGBA→we rebuild BGRA)
        let rgba: Vec<u8> = frame
            .bgra
            .chunks_exact(4)
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect();
        let img = image::RgbaImage::from_raw(frame.width, frame.height, rgba)
            .ok_or_else(|| PlatformError::Message("rgba rebuild failed".into()))?;
        let nw = (frame.width / scale_div).max(1);
        let nh = (frame.height / scale_div).max(1);
        let small = image::imageops::resize(&img, nw, nh, FilterType::Triangle);
        let mut bgra = Vec::with_capacity((nw * nh * 4) as usize);
        for p in small.pixels() {
            bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        Ok(RawFrame {
            bgra,
            width: nw,
            height: nh,
            bytes_per_row: (nw as usize) * 4,
            display_id: id,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, scale_div);
        Err(PlatformError::Unsupported("capture requires macOS".into()))
    }
}

fn capture_display_encoded(
    id: DisplayId,
    max_edge: u32,
    jpeg: bool,
    jpeg_quality: u8,
) -> Result<ScreenshotFrame, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        use image::imageops::FilterType;

        let cg = cg_image_for_display(id)?;
        let (rgba, width, height) = rgba_from_cg(&cg)?;
        let mut img = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| PlatformError::Message("rgba image failed".into()))?;

        if max_edge > 0 {
            let long = width.max(height);
            if long > max_edge {
                let scale = max_edge as f32 / long as f32;
                let nw = ((width as f32) * scale).round().max(1.0) as u32;
                let nh = ((height as f32) * scale).round().max(1.0) as u32;
                img = image::imageops::resize(&img, nw, nh, FilterType::Triangle);
                debug!(width, height, nw, nh, "downscaled capture");
            }
        }

        let (out_w, out_h) = img.dimensions();
        let mut bytes = Vec::new();
        let media_type = if jpeg {
            let q = jpeg_quality.clamp(1, 100);
            let mut enc = JpegEncoder::new_with_quality(&mut bytes, q);
            // JPEG encoder wants RGB
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            enc.encode(rgb.as_raw(), out_w, out_h, ColorType::Rgb8.into())
                .map_err(|e| PlatformError::Message(format!("jpeg: {e}")))?;
            "image/jpeg".to_string()
        } else {
            let enc = PngEncoder::new(&mut bytes);
            enc.write_image(img.as_raw(), out_w, out_h, ColorType::Rgba8.into())
                .map_err(|e| PlatformError::Message(format!("png: {e}")))?;
            "image/png".to_string()
        };

        Ok(ScreenshotFrame {
            png_or_jpeg_bytes: bytes,
            media_type,
            width: out_w,
            height: out_h,
            display_id: id,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, max_edge, jpeg, jpeg_quality);
        Err(PlatformError::Unsupported("capture requires macOS".into()))
    }
}
