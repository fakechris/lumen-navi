#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use chrono::Utc;
use lumen_context::{
    CaptureId, CaptureProfile, CaptureRequest, CaptureTrigger, ContextCollector, ContextConfig,
    PrivacyPolicy, SourceKind, SourceSelection, SourceState, TriggerKind,
};
use uuid::Uuid;

#[tokio::test]
async fn captures_frontmost_target_without_requiring_editor_permission() {
    let now = Utc::now();
    let collector = ContextCollector::new(ContextConfig::default(), None).unwrap();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Editor,
            sources: SourceSelection::from_sources([
                SourceKind::Target,
                SourceKind::EditorAx,
                SourceKind::AxVisible,
            ]),
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
        .snapshot(Instant::now() + Duration::from_secs(3))
        .await;
    let target_status = &snapshot.manifest.source_status[&SourceKind::Target];
    assert_eq!(
        target_status.state,
        SourceState::Succeeded,
        "target source failed: {target_status:?}"
    );
    assert!(snapshot
        .manifest
        .target
        .as_ref()
        .and_then(|target| target.app_name.as_deref())
        .is_some_and(|name| !name.is_empty()));

    let editor_status = &snapshot.manifest.source_status[&SourceKind::EditorAx];
    assert!(editor_status.state.is_terminal());
    let visible_status = &snapshot.manifest.source_status[&SourceKind::AxVisible];
    assert!(visible_status.state.is_terminal());
}
