use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

use crate::{OcrEngine, OcrResult, PlatformError};

const MAX_HEADER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperOcrMode {
    Text,
    Boxes,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelperRequest {
    pub request_id: Uuid,
    pub mode: HelperOcrMode,
    pub languages: Vec<String>,
    pub image_len: usize,
    pub max_image_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelperResponse {
    pub request_id: Uuid,
    pub result: Option<OcrResult>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone)]
pub struct HelperVisionOcr {
    executable: PathBuf,
    timeout: Duration,
    max_image_bytes: usize,
}

impl HelperVisionOcr {
    pub fn new(executable: PathBuf, timeout: Duration, max_image_bytes: usize) -> Self {
        Self {
            executable,
            timeout,
            max_image_bytes,
        }
    }

    async fn recognize(
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
        let request = HelperRequest {
            request_id: Uuid::new_v4(),
            mode,
            languages: languages.to_vec(),
            image_len: image.len(),
            max_image_bytes: self.max_image_bytes,
        };
        let header = serde_json::to_vec(&request)
            .map_err(|error| PlatformError::Message(format!("encode OCR request: {error}")))?;
        let mut command = Command::new(&self.executable);
        command
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| PlatformError::Message(format!("start OCR helper failed: {error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Message("OCR helper stdin is unavailable".to_owned()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Message("OCR helper stdout is unavailable".to_owned()))?;
        let image = image.to_vec();
        let exchange = async move {
            stdin
                .write_all(&(header.len() as u32).to_be_bytes())
                .await?;
            stdin.write_all(&header).await?;
            stdin.write_all(&image).await?;
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
        let response = match tokio::time::timeout(self.timeout, exchange).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                return Err(PlatformError::Message(format!(
                    "OCR helper protocol failed: {error}"
                )));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(PlatformError::Message("OCR helper timed out".to_owned()));
            }
        };
        let status = child.wait().await.map_err(|error| {
            PlatformError::Message(format!("wait for OCR helper failed: {error}"))
        })?;
        if !status.success() {
            return Err(PlatformError::Message(format!(
                "OCR helper exited with status {status}"
            )));
        }
        let response: HelperResponse = serde_json::from_slice(&response).map_err(|error| {
            PlatformError::Message(format!("decode OCR helper response: {error}"))
        })?;
        if response.request_id != request.request_id {
            return Err(PlatformError::Message(
                "OCR helper response id mismatch".to_owned(),
            ));
        }
        if let Some(result) = response.result {
            return Ok(result);
        }
        let message = response
            .error_message
            .unwrap_or_else(|| "OCR helper returned no result".to_owned());
        if response.error_kind.as_deref() == Some("unsupported") {
            Err(PlatformError::Unsupported(message))
        } else {
            Err(PlatformError::Message(message))
        }
    }
}

#[async_trait]
impl OcrEngine for HelperVisionOcr {
    fn is_supported(&self) -> bool {
        self.executable.is_file()
    }

    async fn recognize_text(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.recognize(HelperOcrMode::Text, image, languages).await
    }

    async fn recognize_boxes(
        &self,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        self.recognize(HelperOcrMode::Boxes, image, languages).await
    }
}

pub async fn run_vision_ocr_helper_stdio() -> Result<(), String> {
    use std::io::{Read, Write};

    let mut stdin = std::io::stdin().lock();
    let mut length = [0_u8; 4];
    stdin
        .read_exact(&mut length)
        .map_err(|error| error.to_string())?;
    let header_len = u32::from_be_bytes(length) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err("invalid OCR helper request length".to_owned());
    }
    let mut header = vec![0_u8; header_len];
    stdin
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    let request: HelperRequest =
        serde_json::from_slice(&header).map_err(|error| error.to_string())?;
    if request.image_len == 0 || request.image_len > request.max_image_bytes {
        return Err("invalid OCR helper image length".to_owned());
    }
    let mut image = vec![0_u8; request.image_len];
    stdin
        .read_exact(&mut image)
        .map_err(|error| error.to_string())?;

    let engine = crate::macos::MacVisionOcr::with_max_image_bytes(request.max_image_bytes);
    let result = match request.mode {
        HelperOcrMode::Text => engine.recognize_text(&image, &request.languages).await,
        HelperOcrMode::Boxes => engine.recognize_boxes(&image, &request.languages).await,
    };
    let response = match result {
        Ok(result) => HelperResponse {
            request_id: request.request_id,
            result: Some(result),
            error_kind: None,
            error_message: None,
        },
        Err(error) => HelperResponse {
            request_id: request.request_id,
            result: None,
            error_kind: Some(
                match &error {
                    PlatformError::Unsupported(_) => "unsupported",
                    PlatformError::PermissionDenied(_) => "denied",
                    PlatformError::Message(_) => "failed",
                }
                .to_owned(),
            ),
            error_message: Some(error.to_string()),
        },
    };
    let encoded = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&(encoded.len() as u32).to_be_bytes())
        .and_then(|_| stdout.write_all(&encoded))
        .and_then(|_| stdout.flush())
        .map_err(|error| error.to_string())
}
