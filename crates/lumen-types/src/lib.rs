//! Shared domain types for Lumen Navi.
//!
//! The core abstraction is a multi-source [`SourceEvent`]: every intake adapter
//! normalizes into this envelope so storage and processing stay source-agnostic.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// High-level origin of an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Screen,
    Video,
    Audio,
    Browser,
    CodingAgent,
    /// Escape hatch for adapters not yet given a dedicated variant.
    Other(String),
}

/// Reference to a blob on disk (or future object store).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: Uuid,
    /// Logical media type, e.g. `image/png`, `audio/wav`, `video/mp4`.
    pub media_type: String,
    /// Relative path under the store's blob root, or absolute path in early prototypes.
    pub path: String,
    pub bytes: Option<u64>,
    pub content_hash: Option<String>,
}

/// Unified intake event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEvent {
    pub id: Uuid,
    pub source: SourceKind,
    /// Finer-grained type inside a source, e.g. `screenshot`, `page_visit`.
    pub kind: String,
    pub ts: DateTime<Utc>,
    pub session_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub artifacts: Vec<ArtifactRef>,
}

impl SourceEvent {
    pub fn new(source: SourceKind, kind: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            source,
            kind: kind.into(),
            ts: Utc::now(),
            session_id: None,
            payload,
            artifacts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_roundtrips_json() {
        let event = SourceEvent::new(
            SourceKind::Browser,
            "page_visit",
            json!({ "url": "https://example.com", "title": "Example" }),
        );
        let encoded = serde_json::to_string(&event).expect("serialize");
        let decoded: SourceEvent = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.source, SourceKind::Browser);
        assert_eq!(decoded.kind, "page_visit");
        assert_eq!(decoded.payload["url"], "https://example.com");
    }
}
