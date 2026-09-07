use lumen_platform::{DisplayInfo, PermissionState};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 4;
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
    /// Explicit Act: run a short keyboard/mouse replay. Observe never sends this.
    InputReplay {
        steps: Vec<InputStep>,
    },
    /// Act-only: HID idle, frontmost, and whether a focus lock is held.
    Idle,
    /// Act-only L0 probe. Read-only — never relaunches the target.
    ProbeApp {
        name_or_path: String,
    },
    /// Act-only window screenshot. Observe screen sources must not call this.
    CaptureWindow {
        window_id: u64,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    },
    /// Act only. Never starts the driver. Observe must not call this.
    ActDriverStatus,
    /// Act only. Spawn embedded cua-driver as a child of Lumen Cua if needed.
    ActDriverEnsure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStep {
    pub action: String,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub keys: Option<String>,
    #[serde(default)]
    pub nx: Option<f64>,
    #[serde(default)]
    pub ny: Option<f64>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
    /// User-provided text for `type` steps. Never recorded — only entered
    /// explicitly in the replay confirm dialog.
    #[serde(default)]
    pub text: Option<String>,
    /// Preview gates and coordinate resolution without posting events.
    #[serde(default)]
    pub dry: bool,
    /// Allow a gated, temporary activate if background delivery cannot land.
    /// Default false: never steal focus.
    #[serde(default)]
    pub allow_foreground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionEffect {
    Confirmed,
    Partial,
    Unverifiable,
    SuspectedNoop,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRoute {
    Accessibility,
    SyntheticEvents,
    GlobalInput,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Background,
    Foreground,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    Pass,
    Wait,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    pub frontmost: GateVerdict,
    pub occlusion: GateVerdict,
    pub presence: GateVerdict,
    pub focus_lock: GateVerdict,
    pub cross_space: GateVerdict,
}

impl GateReport {
    pub fn all_pass() -> Self {
        Self {
            frontmost: GateVerdict::Pass,
            occlusion: GateVerdict::Pass,
            presence: GateVerdict::Pass,
            focus_lock: GateVerdict::Pass,
            cross_space: GateVerdict::Pass,
        }
    }

    pub fn blocking_reason(&self) -> Option<&'static str> {
        if self.cross_space == GateVerdict::Refuse {
            return Some("cross_space");
        }
        if self.focus_lock == GateVerdict::Refuse {
            return Some("focus_lock");
        }
        if self.frontmost == GateVerdict::Refuse {
            return Some("frontmost");
        }
        if self.occlusion == GateVerdict::Refuse {
            return Some("occlusion");
        }
        if self.presence == GateVerdict::Refuse {
            return Some("presence");
        }
        None
    }

    pub fn needs_wait(&self) -> bool {
        self.presence == GateVerdict::Wait
            || self.occlusion == GateVerdict::Wait
            || self.frontmost == GateVerdict::Wait
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub effect: ActionEffect,
    pub route: ActionRoute,
    pub delivery: DeliveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gates: Option<GateReport>,
}

impl ActionResult {
    pub fn refused(reason: impl Into<String>, gates: Option<GateReport>) -> Self {
        Self {
            effect: ActionEffect::Refused,
            route: ActionRoute::Skipped,
            delivery: DeliveryMode::NotApplicable,
            reason: Some(reason.into()),
            gates,
        }
    }

    pub fn summary_line(&self) -> String {
        let effect = match self.effect {
            ActionEffect::Confirmed => "confirmed",
            ActionEffect::Partial => "partial",
            ActionEffect::Unverifiable => "unverifiable",
            ActionEffect::SuspectedNoop => "suspected_noop",
            ActionEffect::Refused => "refused",
        };
        match &self.reason {
            Some(reason) => format!("{effect} ({reason})"),
            None => effect.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdleStatus {
    pub hid_idle_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmost_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmost_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmost_pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmost_window_id: Option<u64>,
    #[serde(default)]
    pub screen_locked: bool,
    #[serde(default)]
    pub focus_lock_held: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub architecture: String,
    #[serde(default)]
    pub url_schemes: Vec<String>,
    #[serde(default)]
    pub applescript_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdef_path: Option<String>,
    #[serde(default)]
    pub listen_ports: Vec<u16>,
    #[serde(default)]
    pub cdp_likely: bool,
    #[serde(default)]
    pub duplicate_installs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax_editable_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

pub const ACT_DRIVER_HOST_BUNDLE_ID: &str = "com.lumenopen.cua";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActDriverInfo {
    pub present: bool,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_command: Option<String>,
    #[serde(default)]
    pub mcp_args: Vec<String>,
    pub host_bundle_id: String,
    pub embedded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    Status {
        status: CuaStatus,
    },
    Displays {
        displays: Vec<DisplayInfo>,
    },
    EncodedFrame {
        frame: EncodedFrameMeta,
    },
    RawFrame {
        frame: RawFrameMeta,
    },
    AxSnapshot {
        meta: AxSnapshotMeta,
    },
    /// Capture-time window is gone. Not a protocol failure — caller should
    /// persist a desynced marker, not retry.
    AxWindowGone {
        window_id: u64,
    },
    Ack,
    Idle {
        status: IdleStatus,
    },
    Probe {
        report: ProbeReport,
    },
    WindowFrame {
        frame: EncodedFrameMeta,
        window_id: u64,
        empty: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnosis: Option<String>,
    },
    Replay {
        effects: Vec<ActionResult>,
    },
    ActDriver {
        info: ActDriverInfo,
    },
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
    #[serde(default)]
    pub hits: Vec<lumen_platform::AxHit>,
    #[serde(default)]
    pub window_bounds: Option<lumen_platform::AxHit>,
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

    #[test]
    fn v3_input_step_defaults_act_flags() {
        let step: InputStep = serde_json::from_value(serde_json::json!({
            "action": "click",
            "bundle_id": "com.apple.TextEdit",
            "nx": 0.5,
            "ny": 0.5
        }))
        .unwrap();
        assert!(!step.dry);
        assert!(!step.allow_foreground);
        assert_eq!(step.action, "click");
    }

    #[test]
    fn action_result_refused_serializes_snake_case() {
        let result = ActionResult::refused("cross_space", Some(GateReport::all_pass()));
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["effect"], "refused");
        assert_eq!(encoded["route"], "skipped");
        assert_eq!(encoded["delivery"], "not_applicable");
        assert_eq!(encoded["reason"], "cross_space");
    }

    #[test]
    fn act_driver_status_command_round_trips() {
        let request = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "request_id": "t",
            "token": "0".repeat(64),
            "command": "act_driver_status"
        });
        let parsed = serde_json::from_value::<RequestEnvelope>(request).unwrap();
        assert!(matches!(parsed.command, Command::ActDriverStatus));
    }
}
