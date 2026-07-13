use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::session::{ContextSource, SourceError, SourceInput};
use crate::{
    AxVisibleContext, CaptureRequest, ContextConfig, EditorContext, PlatformError, SourceCapture,
    SourceKind, TargetContext,
};

static AX_CAPTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn lock_ax_capture() -> tokio::sync::MutexGuard<'static, ()> {
    AX_CAPTURE_LOCK.lock().await
}

#[cfg(target_os = "macos")]
extern "C" {
    fn lumen_context_capture_ax_fast(
        max_chars: u64,
        timeout_seconds: f64,
        out_json: *mut *mut c_char,
        out_error: *mut *mut c_char,
    ) -> i32;
    fn lumen_context_capture_ax_visible(
        max_nodes: u64,
        max_depth: u64,
        max_chars: u64,
        timeout_seconds: f64,
        out_json: *mut *mut c_char,
        out_error: *mut *mut c_char,
    ) -> i32;
    fn lumen_context_ax_free(value: *mut c_char);
}

#[derive(Debug, Deserialize)]
pub(crate) struct AxFastEnvelope {
    pub(crate) target: TargetContext,
    pub(crate) editor: Option<EditorContext>,
    pub(crate) accessibility_trusted: bool,
    pub(crate) fingerprint_material: Option<String>,
}

struct MacAxSource {
    kind: SourceKind,
    max_chars: usize,
    timeout_seconds: f64,
}

struct MacAxVisibleSource {
    max_nodes: usize,
    max_depth: usize,
    max_chars: usize,
    timeout_seconds: f64,
}

pub(crate) fn default_ax_sources(config: &ContextConfig) -> Vec<Arc<dyn ContextSource>> {
    #[cfg(target_os = "macos")]
    {
        let timeout_seconds = (config.ax_timeout_ms as f64 / 1_000.0).clamp(0.05, 5.0);
        vec![
            Arc::new(MacAxSource {
                kind: SourceKind::Target,
                max_chars: config.ax_max_chars,
                timeout_seconds,
            }),
            Arc::new(MacAxSource {
                kind: SourceKind::EditorAx,
                max_chars: config.ax_max_chars,
                timeout_seconds,
            }),
            Arc::new(MacAxVisibleSource {
                max_nodes: config.ax_max_nodes,
                max_depth: config.ax_max_depth,
                max_chars: config.ax_max_chars,
                timeout_seconds,
            }),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = config;
        Vec::new()
    }
}

#[async_trait]
impl ContextSource for MacAxVisibleSource {
    fn kind(&self) -> SourceKind {
        SourceKind::AxVisible
    }

    fn dependencies(&self) -> &'static [SourceKind] {
        &[SourceKind::EditorAx]
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        _input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        let _ax_guard = lock_ax_capture().await;
        let max_nodes = self.max_nodes;
        let max_depth = self.max_depth;
        let max_chars = self.max_chars;
        let timeout_seconds = self.timeout_seconds;
        let mut before =
            tokio::task::spawn_blocking(move || capture_ax_fast(max_chars, timeout_seconds))
                .await
                .map_err(|error| {
                    SourceError::failed(
                        "ax_visible_preflight_join_failed",
                        format!("AX preflight task failed: {error}"),
                        true,
                    )
                })?
                .map_err(SourceError::from_platform)?;
        apply_fingerprint(&mut before);
        validate_target_hint(&before.target, request.target_hint.as_ref())?;

        let visible = tokio::task::spawn_blocking(move || {
            capture_ax_visible(max_nodes, max_depth, max_chars, timeout_seconds)
        })
        .await
        .map_err(|error| {
            SourceError::failed(
                "ax_visible_join_failed",
                format!("AX visible task failed: {error}"),
                true,
            )
        })?
        .map_err(classify_ax_visible_error)?;
        let mut after =
            tokio::task::spawn_blocking(move || capture_ax_fast(max_chars, timeout_seconds))
                .await
                .map_err(|error| {
                    SourceError::failed(
                        "ax_visible_postflight_join_failed",
                        format!("AX postflight task failed: {error}"),
                        true,
                    )
                })?
                .map_err(SourceError::from_platform)?;
        apply_fingerprint(&mut after);
        if !same_target(&before.target, &after.target) {
            return Err(SourceError::stale(
                "focused target changed while AX visible tree was captured",
            ));
        }
        let empty = visible.roots.is_empty();
        Ok(SourceCapture {
            truncated_nodes: visible.truncated_nodes as u64,
            truncated_chars: visible.truncated_chars as u64,
            ax_visible: Some(visible),
            empty,
            ..SourceCapture::default()
        })
    }
}

#[async_trait]
impl ContextSource for MacAxSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn dependencies(&self) -> &'static [SourceKind] {
        match self.kind {
            SourceKind::EditorAx => &[SourceKind::Target],
            _ => &[],
        }
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        _input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        let _ax_guard = lock_ax_capture().await;
        let max_chars = self.max_chars;
        let timeout_seconds = self.timeout_seconds;
        let kind = self.kind;
        let envelope =
            tokio::task::spawn_blocking(move || capture_ax_fast(max_chars, timeout_seconds))
                .await
                .map_err(|error| {
                    SourceError::failed(
                        "ax_join_failed",
                        format!("AX capture task failed: {error}"),
                        true,
                    )
                })?
                .map_err(SourceError::from_platform)?;

        let mut target = envelope.target;
        if let Some(material) = envelope.fingerprint_material {
            target.focused_element_fingerprint =
                Some(blake3::hash(material.as_bytes()).to_hex().to_string());
        }
        validate_target_hint(&target, request.target_hint.as_ref())?;

        match kind {
            SourceKind::Target => Ok(SourceCapture {
                target: Some(target),
                empty: false,
                ..SourceCapture::default()
            }),
            SourceKind::EditorAx => {
                if !envelope.accessibility_trusted {
                    return Err(SourceError::from_platform(PlatformError::PermissionDenied(
                        "Accessibility permission is not granted".to_owned(),
                    )));
                }
                let empty = envelope.editor.is_none();
                Ok(SourceCapture {
                    target: Some(target),
                    editor: envelope.editor,
                    empty,
                    ..SourceCapture::default()
                })
            }
            _ => unreachable!("MacAxSource only serves target and editor_ax"),
        }
    }
}

pub(crate) fn capture_ax_fast(
    max_chars: usize,
    timeout_seconds: f64,
) -> Result<AxFastEnvelope, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let mut json_ptr = std::ptr::null_mut();
        let mut error_ptr = std::ptr::null_mut();
        let code = unsafe {
            lumen_context_capture_ax_fast(
                max_chars.min(u64::MAX as usize) as u64,
                timeout_seconds,
                &mut json_ptr,
                &mut error_ptr,
            )
        };
        let json = take_bridge_string(json_ptr);
        let error = take_bridge_string(error_ptr);
        if code != 0 {
            return Err(PlatformError::Message(if error.is_empty() {
                format!("AX bridge failed with code {code}")
            } else {
                error
            }));
        }
        serde_json::from_str(&json)
            .map_err(|error| PlatformError::Message(format!("invalid AX bridge JSON: {error}")))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (max_chars, timeout_seconds);
        Err(PlatformError::Unsupported(
            "Accessibility capture requires macOS".to_owned(),
        ))
    }
}

pub(crate) fn apply_fingerprint(envelope: &mut AxFastEnvelope) {
    if let Some(material) = envelope.fingerprint_material.as_deref() {
        envelope.target.focused_element_fingerprint =
            Some(blake3::hash(material.as_bytes()).to_hex().to_string());
    }
}

pub(crate) fn same_target(left: &TargetContext, right: &TargetContext) -> bool {
    left.pid == right.pid
        && left.window_id == right.window_id
        && left.bundle_id == right.bundle_id
        && left.focused_element_fingerprint == right.focused_element_fingerprint
}

pub(crate) fn validate_target_hint(
    target: &TargetContext,
    hint: Option<&crate::TargetHint>,
) -> Result<(), SourceError> {
    let Some(hint) = hint else {
        return Ok(());
    };
    let matches = hint
        .app_name
        .as_ref()
        .is_none_or(|expected| target.app_name.as_ref() == Some(expected))
        && hint
            .bundle_id
            .as_ref()
            .is_none_or(|expected| target.bundle_id.as_ref() == Some(expected))
        && hint.pid.is_none_or(|expected| target.pid == Some(expected))
        && hint
            .window_id
            .is_none_or(|expected| target.window_id == Some(expected))
        && hint
            .focused_element_fingerprint
            .as_ref()
            .is_none_or(|expected| target.focused_element_fingerprint.as_ref() == Some(expected));
    if matches {
        Ok(())
    } else {
        Err(SourceError::stale(
            "captured target does not match the requested target generation",
        ))
    }
}

fn capture_ax_visible(
    max_nodes: usize,
    max_depth: usize,
    max_chars: usize,
    timeout_seconds: f64,
) -> Result<AxVisibleContext, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let mut json_ptr = std::ptr::null_mut();
        let mut error_ptr = std::ptr::null_mut();
        let code = unsafe {
            lumen_context_capture_ax_visible(
                max_nodes.min(u64::MAX as usize) as u64,
                max_depth.min(u64::MAX as usize) as u64,
                max_chars.min(u64::MAX as usize) as u64,
                timeout_seconds,
                &mut json_ptr,
                &mut error_ptr,
            )
        };
        let json = take_bridge_string(json_ptr);
        let error = take_bridge_string(error_ptr);
        if code == 2 {
            return Err(PlatformError::PermissionDenied(error));
        }
        if code != 0 {
            return Err(PlatformError::Message(if error.is_empty() {
                format!("AX visible bridge failed with code {code}")
            } else {
                error
            }));
        }
        serde_json::from_str(&json).map_err(|error| {
            PlatformError::Message(format!("invalid AX visible bridge JSON: {error}"))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (max_nodes, max_depth, max_chars, timeout_seconds);
        Err(PlatformError::Unsupported(
            "Accessibility capture requires macOS".to_owned(),
        ))
    }
}

fn classify_ax_visible_error(error: PlatformError) -> SourceError {
    match error {
        PlatformError::PermissionDenied(_) | PlatformError::Unsupported(_) => {
            SourceError::from_platform(error)
        }
        PlatformError::Message(message) => {
            let code = if message.contains("invalid AX visible bridge JSON") {
                "ax_visible_decode_failed"
            } else if message.to_ascii_lowercase().contains("timed out") {
                "ax_visible_timeout"
            } else if message.contains("AX visible bridge failed") {
                "ax_visible_bridge_failed"
            } else {
                "ax_visible_capture_failed"
            };
            SourceError::failed(code, "AX visible capture failed", true)
        }
    }
}

#[cfg(target_os = "macos")]
fn take_bridge_string(pointer: *mut c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    unsafe {
        let value = CStr::from_ptr(pointer).to_string_lossy().into_owned();
        lumen_context_ax_free(pointer);
        value
    }
}
