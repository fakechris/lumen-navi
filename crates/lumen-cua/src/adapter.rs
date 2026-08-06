use async_trait::async_trait;
use lumen_platform::{
    DisplayEnumerator, DisplayId, DisplayInfo, PlatformError, RawFrame, ScreenCapturer,
    ScreenshotFrame,
};

use crate::CuaClient;

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
        tokio::task::spawn_blocking(move || client.list_displays())
            .await
            .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
            .map_err(|e| PlatformError::Message(e.to_string()))
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
        tokio::task::spawn_blocking(move || {
            client.capture_encoded(id, max_edge, jpeg, jpeg_quality)
        })
        .await
        .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
        .map_err(|e| PlatformError::Message(e.to_string()))
    }

    async fn capture_display_raw(
        &self,
        id: DisplayId,
        scale_div: u32,
    ) -> Result<RawFrame, PlatformError> {
        let client = self.client.clone();
        tokio::task::spawn_blocking(move || client.capture_raw(id, scale_div))
            .await
            .map_err(|e| PlatformError::Message(format!("Lumen Cua task: {e}")))?
            .map_err(|e| PlatformError::Message(e.to_string()))
    }
}
