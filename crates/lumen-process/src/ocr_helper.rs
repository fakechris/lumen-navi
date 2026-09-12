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
    fallback: Option<Arc<dyn OcrEngine>>,
}

impl OutOfProcessOcrEngine {
    pub fn new(
        executable: PathBuf,
        args: Vec<String>,
        timeout: Duration,
        max_image_bytes: usize,
        fallback: Option<Arc<dyn OcrEngine>>,
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
        let exchange = async {
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
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((response, status))
        };

        let (response_bytes, status) = match tokio::time::timeout(self.timeout, exchange).await {
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
        // A missing executable is a retryable startup failure, not permission
        // to discard pending jobs or enter the native engine in this process.
        true
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

impl OutOfProcessOcrEngine {
    async fn recognize(
        &self,
        mode: HelperOcrMode,
        image: &[u8],
        languages: &[String],
    ) -> Result<OcrResult, PlatformError> {
        match self.recognize_out_of_process(mode, image, languages).await {
            Ok(result) => Ok(result),
            Err(err) => {
                if let Some(fallback) = &self.fallback {
                    warn!(error = %err, "diagnostic OCR fallback enabled: native crash isolation is disabled");
                    match mode {
                        HelperOcrMode::Text => fallback.recognize_text(image, languages).await,
                        HelperOcrMode::Boxes => fallback.recognize_boxes(image, languages).await,
                    }
                } else {
                    // Keep transport/native failures retryable, including broken
                    // pipes and malformed native responses. Do not skip the job.
                    Err(PlatformError::Message(format!("OCR helper failure: {err}")))
                }
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

    let request: HelperOcrRequest =
        serde_json::from_slice(&header_buf).map_err(|e| format!("decode request JSON: {e}"))?;

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

    let encoded =
        serde_json::to_vec(&response).map_err(|e| format!("encode response JSON: {e}"))?;

    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&(encoded.len() as u32).to_be_bytes())
        .and_then(|_| stdout.write_all(&encoded))
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("write response to stdout: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    };

    fn fixture() -> PathBuf {
        static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().unwrap();
                let binary = dir
                    .path()
                    .join(format!("ocr-fixture{}", std::env::consts::EXE_SUFFIX));
                let source =
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr_helper.rs");
                let status = std::process::Command::new("rustc")
                    .arg("--edition=2021")
                    .arg(source)
                    .arg("-o")
                    .arg(&binary)
                    .status()
                    .unwrap();
                assert!(status.success());
                (dir, binary)
            })
            .1
            .clone()
    }

    struct Fallback(AtomicUsize);
    #[async_trait]
    impl OcrEngine for Fallback {
        fn is_supported(&self) -> bool {
            true
        }
        async fn recognize_text(&self, _: &[u8], _: &[String]) -> Result<OcrResult, PlatformError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(OcrResult {
                text: "diagnostic".into(),
                confidence: 1.0,
                languages: vec![],
                mode: "test".into(),
                boxes: vec![],
            })
        }
        async fn recognize_boxes(
            &self,
            i: &[u8],
            l: &[String],
        ) -> Result<OcrResult, PlatformError> {
            self.recognize_text(i, l).await
        }
    }

    fn engine(mode: &str, pid: &std::path::Path) -> OutOfProcessOcrEngine {
        OutOfProcessOcrEngine::new(
            fixture(),
            vec![mode.into(), pid.display().to_string()],
            Duration::from_secs(1),
            1024,
            None,
        )
    }

    #[cfg(unix)]
    fn assert_reaped(pid_file: &std::path::Path) {
        let pid = std::fs::read_to_string(pid_file).unwrap();
        let status = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "helper {pid} still exists");
    }

    #[tokio::test]
    async fn helper_failures_are_retryable_and_children_are_reaped() {
        let dir = tempfile::tempdir().unwrap();
        for mode in [
            "crash",
            "hang",
            "exit_hang",
            "bad_json",
            "bad_length",
            "truncated",
            "wrong_id",
            "native_error",
        ] {
            for boxes in [false, true] {
                let pid = dir.path().join("pid");
                let e = engine(mode, &pid);
                let result = if boxes {
                    e.recognize_boxes(b"x", &[]).await
                } else {
                    e.recognize_text(b"x", &[]).await
                };
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("OCR helper failure:"),
                    "{mode}"
                );
                #[cfg(unix)]
                assert_reaped(&pid);
            }
        }
    }

    #[tokio::test]
    async fn helper_success_and_explicit_diagnostic_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        let e = engine("success", &pid);
        assert_eq!(e.recognize_text(b"x", &[]).await.unwrap().text, "fixture");
        assert_eq!(e.recognize_boxes(b"x", &[]).await.unwrap().text, "fixture");
        let fallback = Arc::new(Fallback(AtomicUsize::new(0)));
        let mut e = engine("crash", &pid);
        e.fallback = Some(fallback.clone());
        assert_eq!(
            e.recognize_text(b"x", &[]).await.unwrap().text,
            "diagnostic"
        );
        e.recognize_boxes(b"x", &[]).await.unwrap();
        assert_eq!(fallback.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn missing_executable_keeps_jobs_retryable() {
        let e = OutOfProcessOcrEngine::new(
            PathBuf::new(),
            vec![],
            Duration::from_millis(10),
            1024,
            None,
        );
        assert!(e.is_supported());
        assert!(e
            .recognize_text(b"x", &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("OCR helper failure:"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killed_helper_does_not_stop_event_persistence() {
        use lumen_store::{EventStore, SqliteStore};
        use lumen_types::{event_kind, SourceEvent, SourceKind};
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        let store = SqliteStore::open(&dir.path().join("store")).unwrap();
        let mut e = engine("hang", &pid);
        e.timeout = Duration::from_secs(10);
        let task = tokio::spawn(async move { e.recognize_text(b"x", &[]).await });
        for _ in 0..100 {
            if pid.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let child_pid = std::fs::read_to_string(&pid).unwrap();
        assert!(std::process::Command::new("kill")
            .args(["-KILL", child_pid.trim()])
            .status()
            .unwrap()
            .success());
        assert!(task.await.unwrap().is_err());
        assert_reaped(&pid);
        let event = SourceEvent::new(
            SourceKind::Screen,
            event_kind::SCREENSHOT_V1,
            serde_json::json!({"synthetic":true}),
        );
        store
            .put_and_append(event, "image/jpeg", b"synthetic")
            .unwrap();
        assert_eq!(store.len().await.unwrap(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canceling_helper_also_kills_child() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        let mut e = engine("hang", &pid);
        e.timeout = Duration::from_secs(60);
        let task = tokio::spawn(async move { e.recognize_text(b"x", &[]).await });
        for _ in 0..100 {
            if pid.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(pid.exists());
        task.abort();
        let _ = task.await;
        // Tokio's child-drop reaper runs asynchronously after cancellation.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_reaped(&pid);
    }
}
