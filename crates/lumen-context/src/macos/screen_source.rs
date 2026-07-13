use std::io::Cursor;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use image::ImageFormat;
use serde_json::json;
use uuid::Uuid;

use super::ax::{
    apply_fingerprint, capture_ax_fast, lock_ax_capture, same_target, validate_target_hint,
};
use super::capture::capture_window;
use super::{is_screen_locked, MacDisplays, MacScreenCapturer};
use crate::session::{ContextSource, SourceError, SourceInput};
use crate::{
    ArtifactPayload, ArtifactRef, CaptureRequest, CapturedArtifact, ContextConfig,
    DisplayEnumerator, DisplayInfo, Rect, ScreenCapturer, ScreenshotDocument, ScreenshotFrame,
    ScreenshotKind, SourceCapture, SourceKind,
};

struct MacScreenshotSource {
    kind: SourceKind,
    ax_max_chars: usize,
    ax_timeout_seconds: f64,
    max_edge: u32,
    jpeg_quality: u8,
    capture_all_displays: bool,
    screenshot_timeout: std::time::Duration,
}

pub(crate) fn default_screen_sources(config: &ContextConfig) -> Vec<Arc<dyn ContextSource>> {
    #[cfg(target_os = "macos")]
    {
        let timeout = (config.ax_timeout_ms as f64 / 1_000.0).clamp(0.05, 5.0);
        [
            SourceKind::ScreenshotElement,
            SourceKind::ScreenshotWindow,
            SourceKind::ScreenshotDisplays,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(MacScreenshotSource::from_config(kind, config, timeout))
                as Arc<dyn ContextSource>
        })
        .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = config;
        Vec::new()
    }
}

#[async_trait]
impl ContextSource for MacScreenshotSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn dependencies(&self) -> &'static [SourceKind] {
        &[SourceKind::EditorAx]
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        _input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        if !request.privacy_policy.capture_screenshots {
            return Err(SourceError::skipped_policy(
                "screenshots_disabled",
                "screenshots are disabled by the capture privacy policy",
            ));
        }
        if is_screen_locked() {
            return Err(SourceError::skipped_policy(
                "screen_locked",
                "screen capture is disabled while the screen is locked",
            ));
        }

        let mut before = {
            let _ax_guard = lock_ax_capture().await;
            capture_ax_fast(self.ax_max_chars, self.ax_timeout_seconds)
                .map_err(SourceError::from_platform)?
        };
        apply_fingerprint(&mut before);
        validate_target_hint(&before.target, request.target_hint.as_ref())?;
        if !before.accessibility_trusted {
            return Err(SourceError::skipped_policy(
                "secure_state_unavailable",
                "screenshots require Accessibility permission to exclude secure fields",
            ));
        }
        if before.editor.as_ref().is_some_and(|editor| editor.secure) {
            return Err(SourceError::skipped_policy(
                "secure_field",
                "screenshots are disabled for secure fields",
            ));
        }
        if before
            .target
            .bundle_id
            .as_ref()
            .is_some_and(|bundle_id| request.privacy_policy.denied_bundle_ids.contains(bundle_id))
        {
            return Err(SourceError::skipped_policy(
                "bundle_denied",
                "the focused application is excluded by policy",
            ));
        }

        let displays = MacDisplays
            .list_displays()
            .await
            .map_err(SourceError::from_platform)?;
        let artifacts = match self.kind {
            SourceKind::ScreenshotDisplays => self.capture_displays(&displays).await?,
            SourceKind::ScreenshotWindow => {
                let Some(bounds) = before.target.window_bounds_global.as_ref() else {
                    return Ok(SourceCapture {
                        target: Some(before.target),
                        empty: true,
                        ..SourceCapture::default()
                    });
                };
                let window_id = before
                    .target
                    .window_id
                    .and_then(|id| u32::try_from(id).ok());
                let started = Instant::now();
                let captured = if let Some(window_id) = window_id {
                    match capture_window(window_id, self.screenshot_timeout).await {
                        Ok(mut window) => {
                            if let Some(display) = display_for_bounds(&displays, bounds) {
                                window.frame.display_id = display.id;
                            }
                            build_artifact(
                                window.frame,
                                ScreenshotKind::ActiveWindow,
                                Some(bounds.clone()),
                                None,
                                Some(window_id as u64),
                                Some(window.point_pixel_scale),
                                started.elapsed().as_millis() as u64,
                                false,
                                "screen_capture_kit_window",
                                None,
                            )
                        }
                        Err(error) => {
                            tracing::debug!(error = %error, "ScreenCaptureKit window capture fell back to display crop");
                            self.capture_crop(
                                &displays,
                                bounds,
                                ScreenshotKind::ActiveWindow,
                                Some(window_id as u64),
                                Some(error.to_string()),
                            )
                            .await?
                        }
                    }
                } else {
                    self.capture_crop(
                        &displays,
                        bounds,
                        ScreenshotKind::ActiveWindow,
                        None,
                        Some("native window id was unavailable".to_owned()),
                    )
                    .await?
                };
                vec![captured]
            }
            SourceKind::ScreenshotElement => {
                let Some(bounds) = before
                    .editor
                    .as_ref()
                    .and_then(|editor| editor.bounds_global.as_ref())
                else {
                    return Ok(SourceCapture {
                        target: Some(before.target),
                        empty: true,
                        ..SourceCapture::default()
                    });
                };
                vec![
                    self.capture_crop(
                        &displays,
                        bounds,
                        ScreenshotKind::FocusedElement,
                        None,
                        None,
                    )
                    .await?,
                ]
            }
            _ => unreachable!("MacScreenshotSource only serves screenshot sources"),
        };

        if is_screen_locked() {
            return Err(SourceError::stale(
                "screen locked while screenshots were being captured",
            ));
        }
        let mut after = {
            let _ax_guard = lock_ax_capture().await;
            capture_ax_fast(self.ax_max_chars, self.ax_timeout_seconds)
                .map_err(SourceError::from_platform)?
        };
        apply_fingerprint(&mut after);
        if !same_target(&before.target, &after.target) {
            return Err(SourceError::stale(
                "focused target changed while screenshots were being captured",
            ));
        }

        let (documents, payloads): (Vec<_>, Vec<_>) = artifacts.into_iter().unzip();
        Ok(SourceCapture {
            target: Some(before.target),
            screenshots: documents,
            artifacts: payloads,
            empty: false,
            ..SourceCapture::default()
        })
    }
}

impl MacScreenshotSource {
    fn from_config(kind: SourceKind, config: &ContextConfig, timeout: f64) -> Self {
        Self {
            kind,
            ax_max_chars: config.ax_max_chars,
            ax_timeout_seconds: timeout,
            max_edge: config.screenshot_max_edge,
            jpeg_quality: config.screenshot_jpeg_quality,
            capture_all_displays: config.capture_all_displays,
            screenshot_timeout: std::time::Duration::from_millis(
                config.screenshot_timeout_ms.clamp(100, 10_000),
            ),
        }
    }

    async fn capture_displays(
        &self,
        displays: &[DisplayInfo],
    ) -> Result<Vec<(ScreenshotDocument, CapturedArtifact)>, SourceError> {
        let selected: Vec<_> = if self.capture_all_displays {
            displays.iter().collect()
        } else {
            displays
                .iter()
                .find(|display| display.is_main)
                .or_else(|| displays.first())
                .into_iter()
                .collect()
        };
        let mut captured = Vec::with_capacity(selected.len());
        for display in selected {
            let started = Instant::now();
            let frame = MacScreenCapturer
                .capture_display(display.id, self.max_edge, true, self.jpeg_quality)
                .await
                .map_err(SourceError::from_platform)?;
            let kind = if display.is_main {
                ScreenshotKind::ActiveDisplay
            } else {
                ScreenshotKind::OtherDisplay
            };
            let bounds = Rect {
                x: display.origin_x as f64,
                y: display.origin_y as f64,
                width: display.width as f64,
                height: display.height as f64,
            };
            captured.push(build_artifact(
                frame,
                kind,
                Some(bounds),
                None,
                None,
                None,
                started.elapsed().as_millis() as u64,
                false,
                "core_graphics_display",
                None,
            ));
        }
        Ok(captured)
    }

    async fn capture_crop(
        &self,
        displays: &[DisplayInfo],
        bounds: &Rect,
        kind: ScreenshotKind,
        window_id: Option<u64>,
        fallback_reason: Option<String>,
    ) -> Result<(ScreenshotDocument, CapturedArtifact), SourceError> {
        let display = display_for_bounds(displays, bounds).ok_or_else(|| {
            SourceError::failed(
                "display_for_bounds_unavailable",
                "no display intersects the requested screenshot bounds",
                false,
            )
        })?;
        let started = Instant::now();
        let full = MacScreenCapturer
            .capture_display(display.id, 0, false, self.jpeg_quality)
            .await
            .map_err(SourceError::from_platform)?;
        let scale_x = full.width as f64 / display.width.max(1) as f64;
        let scale_y = full.height as f64 / display.height.max(1) as f64;
        let x = ((bounds.x - display.origin_x as f64) * scale_x)
            .floor()
            .max(0.0) as u32;
        let y = ((bounds.y - display.origin_y as f64) * scale_y)
            .floor()
            .max(0.0) as u32;
        let width = (bounds.width * scale_x).ceil().max(1.0) as u32;
        let height = (bounds.height * scale_y).ceil().max(1.0) as u32;
        let width = width.min(full.width.saturating_sub(x));
        let height = height.min(full.height.saturating_sub(y));
        if width == 0 || height == 0 {
            return Err(SourceError::failed(
                "screenshot_crop_empty",
                "the requested screenshot crop is outside the selected display",
                false,
            ));
        }
        let decoded = image::load_from_memory(&full.png_or_jpeg_bytes).map_err(|error| {
            SourceError::failed(
                "screenshot_decode_failed",
                format!("failed to decode captured display: {error}"),
                true,
            )
        })?;
        let cropped = decoded.crop_imm(x, y, width, height);
        let mut bytes = Cursor::new(Vec::new());
        cropped
            .write_to(&mut bytes, ImageFormat::Png)
            .map_err(|error| {
                SourceError::failed(
                    "screenshot_encode_failed",
                    format!("failed to encode screenshot crop: {error}"),
                    true,
                )
            })?;
        let frame = ScreenshotFrame {
            png_or_jpeg_bytes: bytes.into_inner(),
            media_type: "image/png".to_owned(),
            width,
            height,
            display_id: display.id,
        };
        Ok(build_artifact(
            frame,
            kind,
            Some(bounds.clone()),
            Some(Rect {
                x: x as f64,
                y: y as f64,
                width: width as f64,
                height: height as f64,
            }),
            window_id,
            None,
            started.elapsed().as_millis() as u64,
            true,
            "core_graphics_crop",
            fallback_reason,
        ))
    }
}

fn display_for_bounds<'a>(displays: &'a [DisplayInfo], bounds: &Rect) -> Option<&'a DisplayInfo> {
    displays.iter().max_by(|left, right| {
        intersection_area(left, bounds).total_cmp(&intersection_area(right, bounds))
    })
}

fn intersection_area(display: &DisplayInfo, bounds: &Rect) -> f64 {
    let left = bounds.x.max(display.origin_x as f64);
    let top = bounds.y.max(display.origin_y as f64);
    let right = (bounds.x + bounds.width).min(display.origin_x as f64 + display.width as f64);
    let bottom = (bounds.y + bounds.height).min(display.origin_y as f64 + display.height as f64);
    (right - left).max(0.0) * (bottom - top).max(0.0)
}

#[allow(clippy::too_many_arguments)]
fn build_artifact(
    frame: ScreenshotFrame,
    kind: ScreenshotKind,
    global_bounds: Option<Rect>,
    pixel_bounds: Option<Rect>,
    window_id: Option<u64>,
    scale_override: Option<f64>,
    duration_ms: u64,
    cropped: bool,
    capture_method: &str,
    capture_fallback_reason: Option<String>,
) -> (ScreenshotDocument, CapturedArtifact) {
    let artifact_id = Uuid::new_v4();
    let hash = blake3::hash(&frame.png_or_jpeg_bytes).to_hex().to_string();
    let bytes = frame.png_or_jpeg_bytes.len() as u64;
    let scale = scale_override.unwrap_or_else(|| {
        global_bounds
            .as_ref()
            .filter(|bounds| bounds.width > 0.0)
            .map_or(1.0, |bounds| frame.width as f64 / bounds.width)
    });
    let document = ScreenshotDocument {
        artifact_id,
        kind,
        display_id: Some(frame.display_id.0),
        window_id,
        global_bounds,
        pixel_bounds,
        scale,
        width: frame.width,
        height: frame.height,
        color_space: None,
        media_type: frame.media_type.clone(),
        content_hash: hash.clone(),
        captured_at: Utc::now(),
        duration_ms,
        occluded: None,
        cropped,
        capture_method: Some(capture_method.to_owned()),
        capture_fallback_reason,
    };
    let descriptor = ArtifactRef {
        artifact_id,
        source: match kind {
            ScreenshotKind::FocusedElement => SourceKind::ScreenshotElement,
            ScreenshotKind::ActiveWindow => SourceKind::ScreenshotWindow,
            ScreenshotKind::ActiveDisplay | ScreenshotKind::OtherDisplay => {
                SourceKind::ScreenshotDisplays
            }
        },
        kind: format!("{kind:?}").to_ascii_lowercase(),
        content_hash: hash,
        media_type: frame.media_type.clone(),
        bytes,
        metadata: json!({
            "display_id": frame.display_id.0,
            "width": frame.width,
            "height": frame.height,
            "cropped": cropped,
            "capture_method": capture_method,
        }),
    };
    let payload = CapturedArtifact {
        descriptor,
        payload: ArtifactPayload::Bytes {
            media_type: frame.media_type,
            bytes: Bytes::from(frame.png_or_jpeg_bytes),
        },
    };
    (document, payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DisplayId;

    #[test]
    fn selects_display_with_largest_intersection() {
        let displays = vec![
            DisplayInfo {
                id: DisplayId(1),
                width: 100,
                height: 100,
                origin_x: 0,
                origin_y: 0,
                is_main: true,
            },
            DisplayInfo {
                id: DisplayId(2),
                width: 100,
                height: 100,
                origin_x: 100,
                origin_y: 0,
                is_main: false,
            },
        ];
        let bounds = Rect {
            x: 80.0,
            y: 10.0,
            width: 80.0,
            height: 50.0,
        };
        assert_eq!(display_for_bounds(&displays, &bounds).unwrap().id.0, 2);
    }
}
