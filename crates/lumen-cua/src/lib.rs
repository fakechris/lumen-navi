//! Lumen Cua is a small local capability boundary for screen capture.
//!
//! The helper owns the macOS TCC identity and screen API calls. Callers own
//! policy, persistence, OCR, and all interpretation of returned frames.

mod act_driver;
mod act_driver_policy;
mod act_driver_replay;
mod adapter;
mod client;
mod focus_lock;
mod gates;
mod idle;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod peer_auth;
mod permission_host;
mod permissions;
mod probe;
mod protocol;
mod runtime;
mod server;
mod stoplines;
mod window_capture;
mod window_info;

pub use adapter::{CuaAxTreeAdapter, CuaCaptureAdapter};
pub use client::{CuaClient, CuaError};
pub use permission_host::{
    is_permission_host_request, run as run_permission_host, PERMISSION_HOST_ARG,
};
pub use protocol::{
    ActDriverInfo, ActionEffect, ActionResult, ActionRoute, CuaStatus, DeliveryMode,
    DirectCaptureError, DirectCaptureStatus, GateReport, IdleStatus, InputStep, ProbeReport,
    ACT_DRIVER_HOST_BUNDLE_ID, PROTOCOL_VERSION,
};
pub use runtime::{ensure_token_file, CuaPaths};
pub use server::serve;
pub use stoplines::{is_terminal_or_ide, refuse_replay_step};
