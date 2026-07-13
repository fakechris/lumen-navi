//! Versioned, local-only context capture contracts and orchestration.
//!
//! This crate is intentionally a leaf module: it does not know about product
//! databases, application lifecycle, retention, prompts, or model inference.

mod browser;
#[cfg(unix)]
mod browser_host;
#[cfg(unix)]
mod browser_native;
mod fusion;
mod helper;
pub mod macos;
mod operational;
mod readiness;
mod session;
mod types;
mod vault;

pub use browser::{
    BrowserCaptureError, BrowserCaptureRequest, BrowserFrameStatus, BrowserSnapshot,
    BrowserSnapshotProvider, BROWSER_CONTEXT_SCHEMA_VERSION,
};
#[cfg(unix)]
pub use browser_host::{run_native_browser_host, run_native_browser_host_with_config};
#[cfg(unix)]
pub use browser_native::{NativeBrowserBridgeConfig, NativeBrowserProvider};
pub use helper::{run_vision_ocr_helper_stdio, HelperVisionOcr};
pub use operational::*;
pub use readiness::{
    build_readiness_report, CapabilityObservation, ReadinessAccumulator, ReadinessReport,
    ReadinessSample, ReadinessSourceSample,
};
pub use session::{CaptureSession, CaptureStartError, ContextCollector, ContextInitError};
pub use types::*;
pub use vault::{ContextSealer, ContextVaultError, SealedContextEnvelope};

#[doc(hidden)]
pub mod helper_protocol {
    pub use crate::helper::{
        HelperOcrMode as Mode, HelperRequest as Request, HelperResponse as Response,
    };
}
