#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use chrono::Utc;
use lumen_context::{
    CaptureId, CaptureProfile, CaptureRequest, CaptureTrigger, ContextCollector, ContextConfig,
    PrivacyPolicy, SourceKind, SourceSelection, SourceState, TriggerKind,
};
use uuid::Uuid;

static REAL_CAPTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn display_capture_is_real_or_explicitly_gated() {
    let _guard = REAL_CAPTURE_LOCK.lock().await;
    let now = Utc::now();
    let config = ContextConfig {
        screenshot_max_edge: 256,
        capture_all_displays: false,
        ..ContextConfig::default()
    };
    let collector = ContextCollector::new(config, None).unwrap();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Vision,
            sources: SourceSelection::from_sources([SourceKind::ScreenshotDisplays]),
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy::default(),
        })
        .unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(4))
        .await;
    let status = &snapshot.manifest.source_status[&SourceKind::ScreenshotDisplays];
    assert!(status.state.is_terminal());
    match status.state {
        SourceState::Succeeded => {
            assert_eq!(snapshot.manifest.screenshots.len(), 1);
            assert_eq!(snapshot.payloads.len(), 1);
            assert!(snapshot.manifest.screenshots[0].width <= 256);
            assert!(!snapshot.manifest.screenshots[0].content_hash.is_empty());
        }
        SourceState::Denied | SourceState::SkippedPolicy => {}
        SourceState::Stale => {
            assert_eq!(
                status.reason_code.as_deref(),
                Some("target_generation_stale")
            );
            assert!(snapshot.manifest.screenshots.is_empty());
            assert!(snapshot.payloads.is_empty());
        }
        unexpected => panic!("unexpected screenshot status: {unexpected:?} ({status:?})"),
    }
}

#[tokio::test]
async fn active_window_prefers_screen_capture_kit() {
    let _guard = REAL_CAPTURE_LOCK.lock().await;
    let now = Utc::now();
    let collector = ContextCollector::new(ContextConfig::default(), None).unwrap();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Vision,
            sources: SourceSelection::from_sources([SourceKind::ScreenshotWindow]),
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy::default(),
        })
        .unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(5))
        .await;
    let status = &snapshot.manifest.source_status[&SourceKind::ScreenshotWindow];
    match status.state {
        SourceState::Succeeded => {
            assert_eq!(snapshot.manifest.screenshots.len(), 1);
            let screenshot = &snapshot.manifest.screenshots[0];
            match screenshot.capture_method.as_deref() {
                Some("screen_capture_kit_window") => {
                    assert!(!screenshot.cropped);
                    assert!(screenshot.capture_fallback_reason.is_none());
                }
                Some("core_graphics_crop") => {
                    assert!(screenshot.cropped);
                    assert!(screenshot.capture_fallback_reason.is_some());
                }
                unexpected => panic!("unexpected window capture method: {unexpected:?}"),
            }
            assert!(screenshot.window_id.is_some());
        }
        SourceState::Empty | SourceState::Denied | SourceState::SkippedPolicy => {}
        SourceState::Stale => {
            assert_eq!(
                status.reason_code.as_deref(),
                Some("target_generation_stale")
            );
            assert!(snapshot.manifest.screenshots.is_empty());
            assert!(snapshot.payloads.is_empty());
        }
        unexpected => panic!("unexpected window status: {unexpected:?} ({status:?})"),
    }
}

#[tokio::test]
async fn vision_ocr_is_real_or_explicitly_gated() {
    let _guard = REAL_CAPTURE_LOCK.lock().await;
    let now = Utc::now();
    let config = ContextConfig {
        screenshot_max_edge: 256,
        capture_all_displays: false,
        ocr_helper_path: Some(env!("CARGO_BIN_EXE_lumen-context-ocr-helper").into()),
        ..ContextConfig::default()
    };
    let collector = ContextCollector::new(config, None).unwrap();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Vision,
            sources: SourceSelection::from_sources([SourceKind::OcrDisplays]),
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy::default(),
        })
        .unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(20))
        .await;
    let status = &snapshot.manifest.source_status[&SourceKind::OcrDisplays];
    assert!(status.state.is_terminal());
    match status.state {
        SourceState::Succeeded | SourceState::Empty => {
            assert_eq!(snapshot.manifest.ocr_documents.len(), 1);
            assert_eq!(snapshot.manifest.screenshots.len(), 1);
            assert_eq!(snapshot.payloads.len(), 2);
            assert_eq!(snapshot.manifest.ocr_documents[0].engine, "apple_vision");
        }
        SourceState::Denied | SourceState::SkippedPolicy | SourceState::Unsupported => {}
        SourceState::Stale => {
            assert_eq!(
                status.reason_code.as_deref(),
                Some("dependency_unavailable")
            );
            assert!(snapshot.manifest.ocr_documents.is_empty());
        }
        unexpected => panic!("unexpected OCR status: {unexpected:?} ({status:?})"),
    }
}

#[tokio::test]
async fn ocr_inherits_screenshot_policy_gate_without_capturing() {
    let _guard = REAL_CAPTURE_LOCK.lock().await;
    let now = Utc::now();
    let collector = ContextCollector::new(ContextConfig::default(), None).unwrap();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Vision,
            sources: SourceSelection::from_sources([SourceKind::OcrDisplays]),
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy {
                capture_screenshots: false,
                ..PrivacyPolicy::default()
            },
        })
        .unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(2))
        .await;
    assert_eq!(
        snapshot.manifest.source_status[&SourceKind::ScreenshotDisplays].state,
        SourceState::SkippedPolicy
    );
    assert_eq!(
        snapshot.manifest.source_status[&SourceKind::OcrDisplays].state,
        SourceState::SkippedPolicy
    );
    assert!(snapshot.manifest.screenshots.is_empty());
    assert!(snapshot.manifest.ocr_documents.is_empty());
    assert!(snapshot.payloads.is_empty());
    assert!(snapshot
        .manifest
        .privacy
        .applied_gates
        .iter()
        .any(|gate| gate == "screenshots_disabled"));
    assert!(snapshot.manifest.diagnostics.total_duration_ms.is_some());
}
