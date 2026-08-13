use lumen_platform::{DisplayInfo, PermissionState};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 3;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RequestEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub token: String,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum Command {
    Status,
    ListDisplays,
    CaptureEncoded {
        display_id: u32,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    },
    CaptureRaw {
        display_id: u32,
        scale_div: u32,
    },
    Shutdown,
    /// Walk the AX tree of the focused window of `pid`'s app. Returns a flat
    /// text blob (in the response payload as UTF-8 bytes) + metadata in the
    /// result header. This runs inside cua (which holds the Accessibility TCC).
    AxWalk {
        pid: i32,
        /// Capture-time `kCGWindowNumber`. Absent on old clients → focused window.
        #[serde(default)]
        window_id: Option<u64>,
        max_depth: u32,
        max_nodes: u32,
        walk_timeout_ms: u64,
        element_timeout_ms: u64,
        max_text_length: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResponseEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponseResult>,
    pub payload_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResponseResult {
    Status { status: CuaStatus },
    Displays { displays: Vec<DisplayInfo> },
    EncodedFrame { frame: EncodedFrameMeta },
    RawFrame { frame: RawFrameMeta },
    AxSnapshot { meta: AxSnapshotMeta },
    /// Capture-time window is gone. Not a protocol failure — caller should
    /// persist a desynced marker, not retry.
    AxWindowGone { window_id: u64 },
    Ack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuaStatus {
    pub screen_recording: PermissionState,
    #[serde(default)]
    pub screen_recording_capturable: Option<bool>,
    #[serde(default)]
    pub direct_capture_status: DirectCaptureStatus,
    #[serde(default)]
    pub direct_capture_error: Option<DirectCaptureError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectCaptureStatus {
    NotChecked,
    BlockedByScreenRecording,
    Ready,
    Unavailable,
    TimedOut,
    ProbeFailed,
}

impl Default for DirectCaptureStatus {
    fn default() -> Self {
        Self::NotChecked
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectCaptureError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EncodedFrameMeta {
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub display_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawFrameMeta {
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: usize,
    pub display_id: u32,
}

/// Metadata for an AX tree walk result. The actual flattened text is sent as
/// the binary payload (UTF-8); these fields carry the structured metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AxSnapshotMeta {
    pub node_count: usize,
    pub content_hash: String,
    pub walk_duration_ms: u64,
    pub truncated: bool,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub document_path: Option<String>,
    pub browser_url: Option<String>,
}

impl ResponseEnvelope {
    pub(crate) fn success(request_id: String, result: ResponseResult, payload_len: usize) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            ok: true,
            error: None,
            result: Some(result),
            payload_len,
        }
    }

    pub(crate) fn failure(request_id: String, error: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            ok: false,
            error: Some(error.into()),
            result: None,
            payload_len: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn daemon_protocol_does_not_accept_permission_prompt_commands() {
        let request = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": "test-request",
            "token": "0".repeat(64),
            "command": "request_screen_permission"
        });

        assert!(serde_json::from_value::<RequestEnvelope>(request).is_err());
    }

    #[test]
    fn status_distinguishes_tcc_from_live_capture_readiness() {
        let status = CuaStatus {
            screen_recording: PermissionState::Granted,
            screen_recording_capturable: None,
            direct_capture_status: DirectCaptureStatus::NotChecked,
            direct_capture_error: None,
        };

        let encoded = serde_json::to_value(status).unwrap();
        assert_eq!(encoded["screen_recording"], "granted");
        assert_eq!(encoded["screen_recording_capturable"], Value::Null);
        assert_eq!(encoded["direct_capture_status"], "not_checked");
        assert_eq!(encoded["direct_capture_error"], Value::Null);
    }

    #[test]
    fn protocol_v1_status_defaults_new_capture_fields_for_migration() {
        let status: CuaStatus = serde_json::from_value(serde_json::json!({
            "screen_recording": "granted"
        }))
        .unwrap();

        assert_eq!(status.screen_recording_capturable, None);
        assert_eq!(
            status.direct_capture_status,
            DirectCaptureStatus::NotChecked
        );
        assert_eq!(status.direct_capture_error, None);
    }
}
