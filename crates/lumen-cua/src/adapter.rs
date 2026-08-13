use std::time::Duration;

use async_trait::async_trait;
use lumen_platform::{
    AxTreeSnapshot, AxTreeWalkConfig, AxTreeWalker, DisplayEnumerator, DisplayId, DisplayInfo,
    PlatformError, RawFrame, ScreenCapturer, ScreenshotFrame,
};

use crate::{CuaClient, CuaError};

/// Bound on awaiting the blocking Cua IPC task. The Cua client has its own
/// 15s socket timeout; this outer bound exists so that an exhausted tokio
/// blocking pool (or a blocking thread wedged in an OS call) can never park
/// the capture pipeline forever — we surface an error instead.
const CUA_TASK_TIMEOUT: Duration = Duration::from_secs(5);

/// Implements Navi's existing capture ports without moving policy or storage
/// into the Lumen Cua process.
#[derive(Clone)]
pub struct CuaCaptureAdapter {
    client: CuaClient,
}

impl CuaCaptureAdapter {
    pub fn new(client: CuaClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DisplayEnumerator for CuaCaptureAdapter {
    async fn list_displays(&self) -> Result<Vec<DisplayInfo>, PlatformError> {
        let client = self.client.clone();
        match tokio::time::timeout(
            CUA_TASK_TIMEOUT,
            tokio::task::spawn_blocking(move || client.list_displays()),
        )
        .await
        {
            Ok(join) => join
                .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
                .map_err(|e| PlatformError::Message(e.to_string())),
            Err(_) => Err(PlatformError::Message(format!(
                "Lumen Cua list_displays timed out after {}s",
                CUA_TASK_TIMEOUT.as_secs()
            ))),
        }
    }
}

#[async_trait]
impl ScreenCapturer for CuaCaptureAdapter {
    async fn capture_display(
        &self,
        id: DisplayId,
        max_edge: u32,
        jpeg: bool,
        jpeg_quality: u8,
    ) -> Result<ScreenshotFrame, PlatformError> {
        let client = self.client.clone();
        match tokio::time::timeout(
            CUA_TASK_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                client.capture_encoded(id, max_edge, jpeg, jpeg_quality)
            }),
        )
        .await
        {
            Ok(join) => join
                .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
                .map_err(|e| PlatformError::Message(e.to_string())),
            Err(_) => Err(PlatformError::Message(format!(
                "Lumen Cua capture_display timed out after {}s",
                CUA_TASK_TIMEOUT.as_secs()
            ))),
        }
    }

    async fn capture_display_raw(
        &self,
        id: DisplayId,
        scale_div: u32,
    ) -> Result<RawFrame, PlatformError> {
        let client = self.client.clone();
        match tokio::time::timeout(
            CUA_TASK_TIMEOUT,
            tokio::task::spawn_blocking(move || client.capture_raw(id, scale_div)),
        )
        .await
        {
            Ok(join) => join
                .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
                .map_err(|e| PlatformError::Message(e.to_string())),
            Err(_) => Err(PlatformError::Message(format!(
                "Lumen Cua capture_display_raw timed out after {}s",
                CUA_TASK_TIMEOUT.as_secs()
            ))),
        }
    }
}

/// Implements `AxTreeWalker` by forwarding to cua over the Unix socket.
/// The daemon holds NO Accessibility TCC — cua does, so the AX tree walk
/// must execute inside the cua process.
#[derive(Clone)]
pub struct CuaAxTreeAdapter {
    client: CuaClient,
}

impl CuaAxTreeAdapter {
    pub fn new(client: CuaClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl AxTreeWalker for CuaAxTreeAdapter {
    async fn walk(
        &self,
        pid: i32,
        window_id: Option<u64>,
        config: AxTreeWalkConfig,
    ) -> Result<AxTreeSnapshot, PlatformError> {
        let client = self.client.clone();
        let result = tokio::task::spawn_blocking(move || client.walk_ax_tree(pid, window_id, &config))
            .await
            .map_err(|e| PlatformError::Message(format!("Lumen Cua AX task: {e}")))?;
        match result {
            Ok(snap) => Ok(snap),
            Err(CuaError::WindowGone(id)) => Err(PlatformError::WindowGone(id)),
            Err(e) => Err(PlatformError::Message(e.to_string())),
        }
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "macos")
    }
}
