use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use lumen_context::{
    BrowserCaptureError, BrowserCaptureRequest, BrowserSnapshot, BrowserSnapshotProvider,
    CaptureId, CaptureProfile, CaptureRequest, CaptureTrigger, ContextCollector, ContextConfig,
    PrivacyPolicy, SourceKind, SourceSelection, SourceState, TriggerKind,
};
use uuid::Uuid;

struct SlowProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl BrowserSnapshotProvider for SlowProvider {
    async fn capture(
        &self,
        _request: BrowserCaptureRequest,
    ) -> Result<BrowserSnapshot, BrowserCaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        Err(BrowserCaptureError::Unavailable("fixture".to_owned()))
    }
}

fn request(capture_raw_text: bool) -> CaptureRequest {
    let now = Utc::now();
    CaptureRequest {
        capture_id: CaptureId::new(),
        consumer_session_id: Uuid::new_v4(),
        target_generation: 1,
        profile: CaptureProfile::Visible,
        sources: SourceSelection::from_sources([SourceKind::Browser]),
        trigger: CaptureTrigger {
            kind: TriggerKind::Test,
            pressed_at: now,
            released_at: None,
        },
        requested_at: now,
        target_hint: None,
        privacy_policy: PrivacyPolicy {
            capture_raw_text,
            ..PrivacyPolicy::default()
        },
    }
}

#[tokio::test]
async fn provider_deadline_becomes_explicit_timeout() {
    let provider = Arc::new(SlowProvider {
        calls: AtomicUsize::new(0),
    });
    let collector = ContextCollector::new(
        ContextConfig {
            browser_timeout_ms: 10,
            ..ContextConfig::default()
        },
        Some(provider.clone()),
    )
    .unwrap();
    let session = collector.begin(request(true)).unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(1))
        .await;

    let status = &snapshot.manifest.source_status[&SourceKind::Browser];
    assert_eq!(status.state, SourceState::Timeout);
    assert_eq!(status.reason_code.as_deref(), Some("browser_timeout"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn raw_text_policy_blocks_provider_before_dom_capture() {
    let provider = Arc::new(SlowProvider {
        calls: AtomicUsize::new(0),
    });
    let collector =
        ContextCollector::new(ContextConfig::default(), Some(provider.clone())).unwrap();
    let session = collector.begin(request(false)).unwrap();
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(1))
        .await;

    let status = &snapshot.manifest.source_status[&SourceKind::Browser];
    assert_eq!(status.state, SourceState::SkippedPolicy);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(snapshot
        .manifest
        .privacy
        .applied_gates
        .iter()
        .any(|gate| gate == "browser_raw_text_disabled"));
}
