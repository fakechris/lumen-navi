use lumen_platform::{DisplayInfo, PermissionState};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
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
    RequestScreenPermission,
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
    PermissionRequest { granted: bool, status: CuaStatus },
    Displays { displays: Vec<DisplayInfo> },
    EncodedFrame { frame: EncodedFrameMeta },
    RawFrame { frame: RawFrameMeta },
    Ack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuaStatus {
    pub screen_recording: PermissionState,
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
