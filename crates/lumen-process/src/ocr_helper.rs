//! Out-of-process OCR engine adapter — isolates Apple Vision/Metal memory allocations.
//!
//! macOS Apple Vision framework caches CoreML models and Metal textures in
//! process-global singletons that are never purged during the lifetime of a
//! long-running daemon. Running OCR in an isolated helper child process ensures
//! that 100% of GPU/Metal/Vision memory is immediately reclaimed by the OS
//! upon helper termination.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lumen_platform::{OcrEngine, OcrResult, PlatformError};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::warn;
use uuid::Uuid;

const MAX_HEADER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HelperOcrMode {
    Text,
    Boxes,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelperOcrRequest {
    pub request_id: Uuid,
    pub mode: HelperOcrMode,
    pub languages: Vec<String>,
    pub image_len: usize,
    pub max_image_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelperOcrResponse {
    pub request_id: Uuid,
    pub result: Option<OcrResult>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone)]
pub struct OutOfProcessOcrEngine {
    executable: PathBuf,
    args: Vec<String>,
    timeout: Duration,
    max_image_bytes: usize,
    fallback: Arc<dyn OcrEngine>,
}

impl OutOfProcessOcrEngine {
    pub fn new(
        executable: PathBuf,
        args: Vec<String>,
        timeout: Duration,
        max_image_bytes: usize,
        fallback: Arc<dyn OcrEngine>,
    ) -> Self {
        Self {
            executable,
            args,
            timeout,
            max_image_bytes,
            fallback,
        }
    }

    async fn recognize_out_of_process(
        &self,
        mode: HelperOcrMode,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        if image.is_empty() {
            return Err(PlatformError::Message("empty OCR image".to_owned()));
        }
        if image.len() > self.max_image_bytes {
            return Err(PlatformError::Message(format!(
                "OCR image exceeds helper limit: {} > {}",
                image.len(),
                self.max_image_bytes
            )));
        }

        let request = HelperOcrRequest {
            request_id: Uuid::new_v4(),
            mode,
            languages: languages.to_vec(),
            image_len: image.len(),
            max_image_bytes: self.max_image_bytes,
        };

        let header = serde_json::to_vec(&request)
            .map_err(|e| PlatformError::Message(format!("encode OCR helper request: {e}")))?;

        let mut command = Command::new(&self.executable);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| PlatformError::Message(format!("spawn OCR helper failed: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Message("OCR helper stdin unavailable".to_owned()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Message("OCR helper stdout unavailable".to_owned()))?;

        let image_vec = image.to_vec();
        let exchange = async move {
            stdin
                .write_all(&(header.len() as u32).to_be_bytes())
                .await?;
            stdin.write_all(&header).await?;
            stdin.write_all(&image_vec).await?;
            stdin.shutdown().await?;

            let response_len = stdout.read_u32().await? as usize;
            if response_len == 0 || response_len > MAX_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid OCR helper response length",
                ));
            }
            let mut response = vec![0_u8; response_len];
            stdout.read_exact(&mut response).await?;
            Ok::<_, std::io::Error>(response)
        };

        let response_bytes = match tokio::time::timeout(self.timeout, exchange).await {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                let _ = child.kill().await;
                return Err(PlatformError::Message(format!(
                    "OCR helper IPC error: {err}"
                )));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(PlatformError::Message("OCR helper timed out".to_owned()));
            }
        };

        let status = child
            .wait()
            .await
            .map_err(|e| PlatformError::Message(format!("wait OCR helper: {e}")))?;
        if !status.success() {
            return Err(PlatformError::Message(format!(
                "OCR helper exited with status {status}"
            )));
        }

        let resp: HelperOcrResponse = serde_json::from_slice(&response_bytes)
            .map_err(|e| PlatformError::Message(format!("decode OCR response: {e}")))?;

        if resp.request_id != request.request_id {
            return Err(PlatformError::Message(
                "OCR helper response ID mismatch".to_owned(),
            ));
        }

        if let Some(result) = resp.result {
            return Ok(result);
        }

        let msg = resp
            .error_message
            .unwrap_or_else(|| "OCR helper returned empty result".to_owned());
        if resp.error_kind.as_deref() == Some("unsupported") {
            Err(PlatformError::Unsupported(msg))
        } else {
            Err(PlatformError::Message(msg))
        }
    }
}

#[async_trait]
impl OcrEngine for OutOfProcessOcrEngine {
    fn is_supported(&self) -> bool {
        self.executable.is_file() || self.fallback.is_supported()
    }

    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        match self
            .recognize_out_of_process(HelperOcrMode::Text, image, languages)
            .await
        {
            Ok(res) => Ok(res),
            Err(err) => {
                warn!(
                    error = %err,
                    "out-of-process OCR helper failed; falling back to in-process OCR engine"
                );
                self.fallback.recognize_text(image, languages).await
            }
        }
    }

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        match self
            .recognize_out_of_process(HelperOcrMode::Boxes, image, languages)
            .await
        {
            Ok(res) => Ok(res),
            Err(err) => {
                warn!(
                    error = %err,
                    "out-of-process OCR helper boxes failed; falling back to in-process OCR engine"
                );
                self.fallback.recognize_boxes(image, languages).await
            }
        }
    }
}

/// Helper entry point when running as a child process (e.g. `lumen-daemon ocr-helper --stdio`).
pub async fn run_ocr_helper_stdio(engine: Arc<dyn OcrEngine>) -> Result<(), String> {
    use std::io::{Read, Write};

    let mut stdin = std::io::stdin().lock();
    let mut len_buf = [0_u8; 4];
    stdin
        .read_exact(&mut len_buf)
        .map_err(|e| format!("read header length: {e}"))?;

    let header_len = u32::from_be_bytes(len_buf) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err("invalid OCR helper header length".to_owned());
    }

    let mut header_buf = vec![0_u8; header_len];
    stdin
        .read_exact(&mut header_buf)
        .map_err(|e| format!("read header body: {e}"))?;

    let request: HelperOcrRequest = serde_json::from_slice(&header_buf)
        .map_err(|e| format!("decode request JSON: {e}"))?;

    if request.image_len == 0 || request.image_len > request.max_image_bytes {
        return Err("invalid OCR image length in helper request".to_owned());
    }

    let mut image = vec![0_u8; request.image_len];
    stdin
        .read_exact(&mut image)
        .map_err(|e| format!("read image body: {e}"))?;

    let result = match request.mode {
        HelperOcrMode::Text => engine.recognize_text(&image, &request.languages).await,
        HelperOcrMode::Boxes => engine.recognize_boxes(&image, &request.languages).await,
    };

    let response = match result {
        Ok(res) => HelperOcrResponse {
            request_id: request.request_id,
            result: Some(res),
            error_kind: None,
            error_message: None,
        },
        Err(err) => HelperOcrResponse {
            request_id: request.request_id,
            result: None,
            error_kind: Some(
                match &err {
                    PlatformError::Unsupported(_) => "unsupported",
                    PlatformError::PermissionDenied(_) => "denied",
                    PlatformError::WindowGone(_) => "window_gone",
                    PlatformError::Message(_) => "failed",
                }
                .to_owned(),
            ),
            error_message: Some(err.to_string()),
        },
    };

    let encoded = serde_json::to_vec(&response)
        .map_err(|e| format!("encode response JSON: {e}"))?;

    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&(encoded.len() as u32).to_be_bytes())
        .and_then(|_| stdout.write_all(&encoded))
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("write response to stdout: {e}"))
}
