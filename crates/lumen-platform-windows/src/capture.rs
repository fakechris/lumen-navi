//! Multi-monitor capture via GDI.
//!
//! GDI `BitBlt` off the virtual-screen DC works on every supported Windows
//! build, needs no permission prompt and no graphics device. Windows Graphics
//! Capture would be faster and would include per-window capture, but it also
//! drags in a D3D device and WinRT interop; that is a later optimisation, not
//! a requirement for Observe's low-rate screenshot cadence.

use async_trait::async_trait;
#[cfg(target_os = "windows")]
use image::codecs::jpeg::JpegEncoder;
#[cfg(target_os = "windows")]
use image::codecs::png::PngEncoder;
#[cfg(target_os = "windows")]
use image::{ColorType, ImageEncoder};
use lumen_platform::{
    DisplayEnumerator, DisplayId, DisplayInfo, PlatformError, RawFrame, ScreenCapturer,
    ScreenshotFrame,
};
#[cfg(target_os = "windows")]
use tracing::debug;

#[cfg(target_os = "windows")]
use crate::dpi::ensure_process_dpi_aware;

pub struct WinDisplays;

#[async_trait]
impl DisplayEnumerator for WinDisplays {
    async fn list_displays(&self) -> Result<Vec<DisplayInfo>, PlatformError> {
        tokio::task::spawn_blocking(list_displays_sync)
            .await
            .map_err(|e| PlatformError::Message(format!("join: {e}")))?
    }
}

pub struct WinScreenCapturer;

impl Default for WinScreenCapturer {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ScreenCapturer for WinScreenCapturer {
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

/// Stable per-monitor id derived from the GDI device name (`\\.\DISPLAY1`).
///
/// `HMONITOR` is a process-lifetime handle, so it cannot be persisted with an
/// event. Hashing the device name keeps stored `display_id`s comparable across
/// daemon restarts for an unchanged monitor arrangement.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn display_id_for_device(name: &str) -> DisplayId {
    // FNV-1a/32.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    DisplayId(hash)
}

fn list_displays_sync() -> Result<Vec<DisplayInfo>, PlatformError> {
    #[cfg(target_os = "windows")]
    {
        ensure_process_dpi_aware();
        let monitors = enumerate_monitors()?;
        let mut out: Vec<DisplayInfo> = monitors
            .iter()
            .map(|m| DisplayInfo {
                id: m.id,
                width: (m.right - m.left).max(1) as u32,
                height: (m.bottom - m.top).max(1) as u32,
                origin_x: m.left,
                origin_y: m.top,
                is_main: m.is_main,
            })
            .collect();
        if out.is_empty() {
            return Err(PlatformError::Message(
                "EnumDisplayMonitors returned no monitors".into(),
            ));
        }
        out.sort_by_key(|d| !d.is_main);
        Ok(out)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(PlatformError::Unsupported(
            "list_displays requires Windows".into(),
        ))
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy)]
struct MonitorRect {
    id: DisplayId,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    is_main: bool,
}

#[cfg(target_os = "windows")]
fn enumerate_monitors() -> Result<Vec<MonitorRect>, PlatformError> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    unsafe extern "system" fn enum_proc(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<MonitorRect>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let ok = unsafe {
            GetMonitorInfoW(
                monitor,
                std::ptr::addr_of_mut!(info.monitorInfo) as *mut MONITORINFO,
            )
        };
        if ok.as_bool() {
            let device = String::from_utf16_lossy(&info.szDevice);
            let device = device.trim_end_matches('\0').to_string();
            let r = info.monitorInfo.rcMonitor;
            out.push(MonitorRect {
                id: display_id_for_device(&device),
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
                is_main: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        TRUE
    }

    let mut monitors: Vec<MonitorRect> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(std::ptr::addr_of_mut!(monitors) as isize),
        )
    };
    if !ok.as_bool() {
        return Err(PlatformError::Message("EnumDisplayMonitors failed".into()));
    }
    Ok(monitors)
}

#[cfg(target_os = "windows")]
fn monitor_for(id: DisplayId) -> Result<MonitorRect, PlatformError> {
    let monitors = enumerate_monitors()?;
    monitors
        .iter()
        .find(|m| m.id == id)
        .copied()
        // A monitor that vanished mid-session (unplugged, RDP resize) must not
        // silently capture a different screen — the caller retries after a
        // fresh `list_displays`.
        .ok_or_else(|| PlatformError::Message(format!("display {} is gone", id.0)))
}

/// Blit one monitor into tightly packed, top-down BGRA.
#[cfg(target_os = "windows")]
fn grab_monitor_bgra(id: DisplayId) -> Result<(Vec<u8>, u32, u32), PlatformError> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, ROP_CODE, SRCCOPY,
    };

    ensure_process_dpi_aware();
    let m = monitor_for(id)?;
    let width = (m.right - m.left).max(1);
    let height = (m.bottom - m.top).max(1);

    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(PlatformError::Message("GetDC(screen) failed".into()));
        }
        // Every early return past this point must release the DCs, so the work
        // runs in a closure and cleanup happens once.
        let result = (|| -> Result<(Vec<u8>, u32, u32), PlatformError> {
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.is_invalid() {
                return Err(PlatformError::Message("CreateCompatibleDC failed".into()));
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(mem_dc);
                return Err(PlatformError::Message(
                    "CreateCompatibleBitmap failed".into(),
                ));
            }
            let previous = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

            let blit = BitBlt(
                mem_dc,
                0,
                0,
                width,
                height,
                Some(screen_dc),
                m.left,
                m.top,
                ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
            );

            let pixels = blit.map_err(|e| PlatformError::Message(format!("BitBlt: {e}"))).and_then(
                |()| {
                    let mut info = BITMAPINFO::default();
                    info.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        // Negative height requests a top-down DIB, matching the
                        // row order every consumer here expects.
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    };
                    let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
                    let copied = GetDIBits(
                        mem_dc,
                        bitmap,
                        0,
                        height as u32,
                        Some(buf.as_mut_ptr().cast()),
                        &mut info,
                        DIB_RGB_COLORS,
                    );
                    if copied == 0 {
                        return Err(PlatformError::Message("GetDIBits copied 0 rows".into()));
                    }
                    // GDI leaves the fourth byte undefined for screen blits.
                    for px in buf.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                    Ok((buf, width as u32, height as u32))
                },
            );

            SelectObject(mem_dc, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            pixels
        })();

        ReleaseDC(None, screen_dc);
        result
    }
}

fn capture_display_raw_sync(id: DisplayId, scale_div: u32) -> Result<RawFrame, PlatformError> {
    #[cfg(target_os = "windows")]
    {
        use image::imageops::FilterType;

        let (bgra, width, height) = grab_monitor_bgra(id)?;
        if scale_div <= 1 {
            return Ok(RawFrame {
                bgra,
                width,
                height,
                bytes_per_row: (width as usize) * 4,
                display_id: id,
            });
        }
        let rgba: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect();
        let img = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| PlatformError::Message("rgba rebuild failed".into()))?;
        let nw = (width / scale_div).max(1);
        let nh = (height / scale_div).max(1);
        let small = image::imageops::resize(&img, nw, nh, FilterType::Triangle);
        let mut out = Vec::with_capacity((nw * nh * 4) as usize);
        for p in small.pixels() {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        Ok(RawFrame {
            bgra: out,
            width: nw,
            height: nh,
            bytes_per_row: (nw as usize) * 4,
            display_id: id,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (id, scale_div);
        Err(PlatformError::Unsupported("capture requires Windows".into()))
    }
}

fn capture_display_encoded(
    id: DisplayId,
    max_edge: u32,
    jpeg: bool,
    jpeg_quality: u8,
) -> Result<ScreenshotFrame, PlatformError> {
    #[cfg(target_os = "windows")]
    {
        use image::imageops::FilterType;

        let (bgra, width, height) = grab_monitor_bgra(id)?;
        let rgba: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect();
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
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (id, max_edge, jpeg, jpeg_quality);
        Err(PlatformError::Unsupported("capture requires Windows".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_are_stable_and_distinct() {
        let a = display_id_for_device(r"\\.\DISPLAY1");
        let b = display_id_for_device(r"\\.\DISPLAY2");
        assert_eq!(a, display_id_for_device(r"\\.\DISPLAY1"));
        assert_ne!(a, b);
    }
}
