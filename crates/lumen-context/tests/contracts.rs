use std::collections::BTreeSet;

use lumen_context::{
    BrowserCaptureRequest, BrowserSnapshot, ContextManifest, BROWSER_CONTEXT_SCHEMA_VERSION,
    CONTEXT_SCHEMA_VERSION,
};

#[test]
fn context_snapshot_v1_fixture_roundtrips() {
    let raw = include_str!("fixtures/context_snapshot_v1.json");
    let manifest: ContextManifest = serde_json::from_str(raw).unwrap();
    assert_eq!(manifest.schema_version, CONTEXT_SCHEMA_VERSION);
    assert!(manifest.all_requested_sources_terminal());

    let encoded = serde_json::to_string(&manifest).unwrap();
    let decoded: ContextManifest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.capture_id, manifest.capture_id);
    assert_eq!(decoded.revision, manifest.revision);
    assert_eq!(decoded.source_status.len(), 1);
}

#[test]
fn browser_context_v1_fixture_is_correlated_and_stable() {
    let raw = include_str!("fixtures/browser_context_v1.json");
    let snapshot: BrowserSnapshot = serde_json::from_str(raw).unwrap();
    assert_eq!(snapshot.schema_version, BROWSER_CONTEXT_SCHEMA_VERSION);
    let request = BrowserCaptureRequest {
        request_id: snapshot.request_id,
        capture_id: snapshot.capture_id,
        target_generation: snapshot.target_generation,
        target_hint: None,
        requested_at: snapshot.captured_at - chrono::Duration::milliseconds(10),
        deadline: snapshot.captured_at + chrono::Duration::milliseconds(10),
        max_chars: 10_000,
        max_nodes: 100,
        allow_private_browsing: false,
        denied_bundle_ids: BTreeSet::new(),
        denied_domains: BTreeSet::new(),
    };

    let context = snapshot.validate_for(&request).unwrap();
    assert_eq!(context.document_id.as_deref(), Some("doc-main-1"));
    assert_eq!(context.viewport_text_blocks.len(), 1);
}
