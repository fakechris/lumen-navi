//! Lumen Cua is a small local capability boundary for screen capture.
//!
//! The helper owns the macOS TCC identity and screen API calls. Callers own
//! policy, persistence, OCR, and all interpretation of returned frames.

mod adapter;
mod client;
#[cfg(target_os = "macos")]
mod peer_auth;
mod permission_host;
mod permissions;
mod protocol;
mod runtime;
mod server;

pub use adapter::{CuaAxTreeAdapter, CuaCaptureAdapter};
pub use client::{CuaClient, CuaError};
pub use permission_host::{
    is_permission_host_request, run as run_permission_host, PERMISSION_HOST_ARG,
};
pub use protocol::{CuaStatus, DirectCaptureError, DirectCaptureStatus, PROTOCOL_VERSION};
pub use runtime::{ensure_token_file, CuaPaths};
pub use server::serve;
