//! Act-only window screenshot. Observe screen sources must not call this.

use anyhow::{bail, Result};

use crate::protocol::EncodedFrameMeta;
use crate::window_info;

pub struct WindowShot {
    pub frame: EncodedFrameMeta,
    pub bytes: Vec<u8>,
    pub empty: bool,
    pub diagnosis: Option<String>,
}

pub fn capture_window(
    window_id: u64,
    max_edge: u32,
    jpeg: bool,
    jpeg_quality: u8,
) -> Result<WindowShot> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window_id, max_edge, jpeg, jpeg_quality);
        bail!("window capture requires macOS")
    }
    #[cfg(target_os = "macos")]
    {
        if lumen_platform_macos::is_screen_locked() {
            return Ok(empty_shot(window_id, "lock"));
        }
        if !window_info::list_windows(false)
            .iter()
            .any(|w| w.window_id == window_id)
        {
            return Ok(empty_shot(window_id, "id_gone"));
        }
        match grab_encoded(window_id, max_edge, jpeg, jpeg_quality) {
            Ok((frame, bytes, empty)) => Ok(WindowShot {
                diagnosis: empty.then(|| "unrendered".to_string()),
                frame,
                bytes,
                empty,
            }),
            Err(_) => Ok(empty_shot(window_id, "unrendered")),
        }
    }
}

fn empty_shot(_window_id: u64, diagnosis: &str) -> WindowShot {
    WindowShot {
        frame: EncodedFrameMeta {
            media_type: "image/jpeg".into(),
            width: 0,
            height: 0,
            display_id: 0,
        },
        bytes: Vec::new(),
        empty: true,
        diagnosis: Some(diagnosis.into()),
    }
}

#[cfg(target_os = "macos")]
fn grab_encoded(
    window_id: u64,
    max_edge: u32,
    jpeg: bool,
    jpeg_quality: u8,
) -> Result<(EncodedFrameMeta, Vec<u8>, bool)> {
    use core_graphics::display::{
        kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow, CGRectNull,
        CGWindowID, CGWindowListCreateImage,
    };
    use core_graphics::image::CGImage;
    use foreign_types::ForeignType;
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::imageops::FilterType;
    use image::{ColorType, ImageEncoder};

    let id = window_id as CGWindowID;
    let image = unsafe {
        let ptr = CGWindowListCreateImage(
            CGRectNull,
            kCGWindowListOptionIncludingWindow,
            id,
            kCGWindowImageBoundsIgnoreFraming,
        );
        if ptr.is_null() {
            bail!("CGWindowListCreateImage returned null");
        }
        CGImage::from_ptr(ptr)
    };

    let width = image.width() as u32;
    let height = image.height() as u32;
    if width == 0 || height == 0 {
        bail!("empty window image");
    }
    let bpp = (image.bits_per_pixel() / 8) as usize;
    let stride = image.bytes_per_row();
    let data = image.data();
    let raw = data.bytes();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let mut dark = 0u64;
    let mut total = 0u64;
    for y in 0..height as usize {
        let row = y * stride;
        for x in 0..width as usize {
            let i = row + x * bpp.max(1);
            if i + 2 >= raw.len() {
                rgba.extend_from_slice(&[0, 0, 0, 255]);
                continue;
            }
            let b = raw[i];
            let g = raw[i + 1];
            let r = raw[i + 2];
            let a = if bpp >= 4 { raw[i + 3] } else { 255 };
            if u16::from(r) + u16::from(g) + u16::from(b) < 24 {
                dark += 1;
            }
            total += 1;
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    let empty = total > 0 && dark * 20 > total * 19;

    let mut img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("rgba image failed"))?;
    if max_edge > 0 {
        let long = width.max(height);
        if long > max_edge {
            let scale = max_edge as f32 / long as f32;
            let nw = ((width as f32) * scale).round().max(1.0) as u32;
            let nh = ((height as f32) * scale).round().max(1.0) as u32;
            img = image::imageops::resize(&img, nw, nh, FilterType::Triangle);
        }
    }
    let (out_w, out_h) = img.dimensions();
    let mut bytes = Vec::new();
    let media_type = if jpeg {
        let q = jpeg_quality.clamp(1, 100);
        let mut enc = JpegEncoder::new_with_quality(&mut bytes, q);
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        enc.encode(rgb.as_raw(), out_w, out_h, ColorType::Rgb8.into())?;
        "image/jpeg"
    } else {
        let enc = PngEncoder::new(&mut bytes);
        enc.write_image(img.as_raw(), out_w, out_h, ColorType::Rgba8.into())?;
        "image/png"
    };
    Ok((
        EncodedFrameMeta {
            media_type: media_type.into(),
            width: out_w,
            height: out_h,
            display_id: 0,
        },
        bytes,
        empty,
    ))
}

pub fn dhash(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?.to_luma8();
    let small = image::imageops::resize(&img, 9, 8, image::imageops::FilterType::Triangle);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = small.get_pixel(x, y)[0];
            let right = small.get_pixel(x + 1, y)[0];
            if left > right {
                hash |= 1 << (y * 8 + x);
            }
        }
    }
    Some(hash)
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
