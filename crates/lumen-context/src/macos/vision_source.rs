use std::cmp::Reverse;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::MacVisionOcr;
use crate::session::{ContextSource, SourceError, SourceInput};
use crate::{
    ArtifactPayload, ArtifactRef, CaptureRequest, CapturedArtifact, ContextConfig, HelperVisionOcr,
    OcrDocument, OcrEngine, OcrRegion, Rect, ScreenshotKind, SourceCapture, SourceKind,
};

struct MacVisionSource {
    kind: SourceKind,
    languages: Vec<String>,
    engine: Arc<dyn OcrEngine>,
}

#[derive(Default)]
struct OcrScheduleState {
    running: bool,
    next_sequence: u64,
    waiters: Vec<OcrWaiter>,
}

struct OcrWaiter {
    priority: u8,
    sequence: u64,
    ready: oneshot::Sender<()>,
}

#[derive(Default)]
struct OcrScheduler {
    state: std::sync::Mutex<OcrScheduleState>,
}

struct OcrPermit {
    scheduler: Arc<OcrScheduler>,
}

impl OcrScheduler {
    async fn acquire(self: &Arc<Self>, priority: u8) -> OcrPermit {
        let receiver = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.running {
                state.running = true;
                None
            } else {
                let sequence = state.next_sequence;
                state.next_sequence = state.next_sequence.wrapping_add(1);
                let (ready, receiver) = oneshot::channel();
                state.waiters.push(OcrWaiter {
                    priority,
                    sequence,
                    ready,
                });
                Some(receiver)
            }
        };
        if let Some(receiver) = receiver {
            let _ = receiver.await;
        }
        OcrPermit {
            scheduler: self.clone(),
        }
    }

    fn release(&self) {
        loop {
            let next = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(index) = state
                    .waiters
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, waiter)| (waiter.priority, Reverse(waiter.sequence)))
                    .map(|(index, _)| index)
                else {
                    state.running = false;
                    return;
                };
                state.waiters.swap_remove(index).ready
            };
            if next.send(()).is_ok() {
                return;
            }
        }
    }
}

impl Drop for OcrPermit {
    fn drop(&mut self) {
        self.scheduler.release();
    }
}

fn ocr_scheduler() -> &'static Arc<OcrScheduler> {
    static SCHEDULER: std::sync::OnceLock<Arc<OcrScheduler>> = std::sync::OnceLock::new();
    SCHEDULER.get_or_init(|| Arc::new(OcrScheduler::default()))
}

fn ocr_priority(request: &CaptureRequest, kind: SourceKind) -> u8 {
    let trigger = match request.trigger.kind {
        crate::TriggerKind::DictationHotkey | crate::TriggerKind::Manual => 3,
        crate::TriggerKind::FocusChange | crate::TriggerKind::TitleChange => 2,
        crate::TriggerKind::Interval => 1,
        crate::TriggerKind::Test => 3,
    };
    let source = match kind {
        SourceKind::OcrElement => 3,
        SourceKind::OcrWindow => 2,
        SourceKind::OcrDisplays => 1,
        _ => 0,
    };
    trigger * 10 + source
}

pub(crate) fn default_vision_sources(config: &ContextConfig) -> Vec<Arc<dyn ContextSource>> {
    #[cfg(target_os = "macos")]
    {
        [
            SourceKind::OcrElement,
            SourceKind::OcrWindow,
            SourceKind::OcrDisplays,
        ]
        .into_iter()
        .map(|kind| {
            let engine: Arc<dyn OcrEngine> = match &config.ocr_helper_path {
                Some(path) => Arc::new(HelperVisionOcr::new(
                    path.clone(),
                    std::time::Duration::from_millis(config.ocr_helper_timeout_ms),
                    config.ocr_max_image_bytes,
                )),
                None => Arc::new(MacVisionOcr::with_max_image_bytes(
                    config.ocr_max_image_bytes,
                )),
            };
            Arc::new(MacVisionSource {
                kind,
                languages: config.ocr_languages.clone(),
                engine,
            }) as Arc<dyn ContextSource>
        })
        .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = config;
        Vec::new()
    }
}

#[async_trait]
impl ContextSource for MacVisionSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn dependencies(&self) -> &'static [SourceKind] {
        match self.kind {
            SourceKind::OcrElement => &[SourceKind::ScreenshotElement],
            SourceKind::OcrWindow => &[SourceKind::ScreenshotWindow],
            SourceKind::OcrDisplays => &[SourceKind::ScreenshotDisplays],
            _ => &[],
        }
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        if !self.engine.is_supported() {
            return Err(SourceError::from_platform(
                crate::PlatformError::Unsupported(
                    "Apple Vision text recognition is unavailable".to_owned(),
                ),
            ));
        }
        let screenshot_kind = match self.kind {
            SourceKind::OcrElement => ScreenshotKind::FocusedElement,
            SourceKind::OcrWindow => ScreenshotKind::ActiveWindow,
            SourceKind::OcrDisplays => ScreenshotKind::ActiveDisplay,
            _ => unreachable!("MacVisionSource only serves OCR sources"),
        };
        let screenshots: Vec<_> = input
            .screenshots
            .iter()
            .filter(|screenshot| {
                screenshot.kind == screenshot_kind
                    || (self.kind == SourceKind::OcrDisplays
                        && screenshot.kind == ScreenshotKind::OtherDisplay)
            })
            .cloned()
            .collect();
        if screenshots.is_empty() {
            return Ok(SourceCapture {
                target: input.target,
                empty: true,
                ..SourceCapture::default()
            });
        }

        let mut documents = Vec::with_capacity(screenshots.len());
        for screenshot in &screenshots {
            let artifact = input
                .artifacts
                .iter()
                .find(|artifact| artifact.descriptor.artifact_id == screenshot.artifact_id)
                .ok_or_else(|| {
                    SourceError::failed(
                        "ocr_screenshot_payload_missing",
                        "screenshot payload is missing for OCR",
                        false,
                    )
                })?;
            let image = match &artifact.payload {
                ArtifactPayload::Bytes { bytes, .. } => bytes.as_ref(),
                ArtifactPayload::File { .. } => {
                    return Err(SourceError::failed(
                        "ocr_file_payload_unsupported",
                        "Vision source cannot read an unimported file payload",
                        false,
                    ));
                }
            };
            let queued = Instant::now();
            let _permit = ocr_scheduler()
                .acquire(ocr_priority(request, self.kind))
                .await;
            let queue_wait_ms = queued.elapsed().as_millis() as u64;
            let started = Instant::now();
            let boxes_result = match self.engine.recognize_boxes(image, &self.languages).await {
                Ok(result) => result,
                Err(error) if is_retryable_vision_error(&error) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    self.engine
                        .recognize_boxes(image, &self.languages)
                        .await
                        .map_err(ocr_source_error)?
                }
                Err(error) => return Err(ocr_source_error(error)),
            };
            let text = boxes_result.text.clone();
            let confidence = boxes_result.confidence;
            let boxes: Vec<_> = boxes_result
                .boxes
                .into_iter()
                .map(|region| OcrRegion {
                    text: region.text,
                    confidence: region.confidence,
                    normalized_box: Rect {
                        x: region.x,
                        y: region.y,
                        width: region.w,
                        height: region.h,
                    },
                    pixel_box: None,
                })
                .collect();
            documents.push(OcrDocument {
                document_id: Uuid::new_v4(),
                screenshot_artifact_id: screenshot.artifact_id,
                engine: "apple_vision".to_owned(),
                engine_version: None,
                binary_hash: None,
                mode: boxes_result.mode,
                languages: boxes_result.languages,
                custom_words: Vec::new(),
                language_correction: None,
                text,
                confidence,
                reading_order: (0..boxes.len()).collect(),
                boxes,
                duration_ms: started.elapsed().as_millis() as u64,
                queue_wait_ms,
                captured_at: screenshot.captured_at,
                completed_at: Utc::now(),
            });
        }

        let serialized = serde_json::to_vec(&documents).map_err(|error| {
            SourceError::failed(
                "ocr_document_serialize_failed",
                format!("failed to serialize OCR documents: {error}"),
                false,
            )
        })?;
        let artifact_id = Uuid::new_v4();
        let hash = blake3::hash(&serialized).to_hex().to_string();
        let artifact = CapturedArtifact {
            descriptor: ArtifactRef {
                artifact_id,
                source: self.kind,
                kind: "ocr_documents_v1".to_owned(),
                content_hash: hash,
                media_type: "application/json".to_owned(),
                bytes: serialized.len() as u64,
                metadata: json!({ "documents": documents.len() }),
            },
            payload: ArtifactPayload::Bytes {
                media_type: "application/json".to_owned(),
                bytes: Bytes::from(serialized),
            },
        };
        let empty = documents
            .iter()
            .all(|document| document.text.trim().is_empty());
        Ok(SourceCapture {
            target: input.target,
            ocr_documents: documents,
            artifacts: vec![artifact],
            empty,
            ..SourceCapture::default()
        })
    }
}

fn is_retryable_vision_error(error: &crate::PlatformError) -> bool {
    matches!(error, crate::PlatformError::Message(message) if message.contains("ocr vision error"))
}

fn ocr_source_error(error: crate::PlatformError) -> SourceError {
    match error {
        crate::PlatformError::Message(message) if message.contains("ocr vision error") => {
            SourceError::failed("ocr_vision_failed", message, true)
        }
        crate::PlatformError::Message(message) if message.contains("ocr decode failed") => {
            SourceError::failed("ocr_image_decode_failed", message, false)
        }
        crate::PlatformError::Message(message) if message.contains("OCR helper timed out") => {
            SourceError::failed("ocr_helper_timeout", message, true)
        }
        crate::PlatformError::Message(message)
            if message.contains("OCR helper protocol failed") =>
        {
            SourceError::failed("ocr_helper_protocol_failed", message, true)
        }
        crate::PlatformError::Message(message) if message.contains("start OCR helper failed") => {
            SourceError::failed("ocr_helper_start_failed", message, true)
        }
        crate::PlatformError::Message(message) if message.contains("OCR helper exited") => {
            SourceError::failed("ocr_helper_exit_failed", message, true)
        }
        crate::PlatformError::Message(message)
            if message.contains("decode OCR helper response") =>
        {
            SourceError::failed("ocr_helper_response_invalid", message, true)
        }
        crate::PlatformError::Message(message) if message.contains("ocr join") => {
            SourceError::failed("ocr_worker_failed", message, true)
        }
        other => SourceError::from_platform(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scheduler_runs_higher_priority_waiter_first() {
        let scheduler = Arc::new(OcrScheduler::default());
        let blocker = scheduler.acquire(1).await;
        let (completed, mut completions) = tokio::sync::mpsc::unbounded_channel();

        let low_scheduler = scheduler.clone();
        let low_completed = completed.clone();
        let low = tokio::spawn(async move {
            let _permit = low_scheduler.acquire(1).await;
            low_completed.send("low").unwrap();
        });
        tokio::task::yield_now().await;

        let high_scheduler = scheduler.clone();
        let high = tokio::spawn(async move {
            let _permit = high_scheduler.acquire(99).await;
            completed.send("high").unwrap();
        });
        tokio::task::yield_now().await;

        drop(blocker);
        assert_eq!(completions.recv().await, Some("high"));
        assert_eq!(completions.recv().await, Some("low"));
        high.await.unwrap();
        low.await.unwrap();
    }

    #[test]
    fn only_vision_runtime_errors_are_retried() {
        assert!(is_retryable_vision_error(&crate::PlatformError::Message(
            "ocr vision error: transient".to_owned()
        )));
        assert!(!is_retryable_vision_error(&crate::PlatformError::Message(
            "ocr decode failed: invalid image".to_owned()
        )));
        assert!(!is_retryable_vision_error(&crate::PlatformError::Message(
            "OCR helper timed out".to_owned()
        )));
    }
}
