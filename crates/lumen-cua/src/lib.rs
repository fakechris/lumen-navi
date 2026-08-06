//! Lumen Cua is a small local capability boundary for screen capture.
//!
//! The helper owns the macOS TCC identity and screen API calls. Callers own
//! policy, persistence, OCR, and all interpretation of returned frames.

mod adapter;
mod client;
#[cfg(target_os = "macos")]
mod peer_auth;
mod protocol;
mod runtime;
mod server;

pub use adapter::CuaCaptureAdapter;
pub use client::{CuaClient, CuaError};
pub use protocol::{CuaStatus, PROTOCOL_VERSION};
pub use runtime::{ensure_token_file, CuaPaths};
pub use server::serve;
