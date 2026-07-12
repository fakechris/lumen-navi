//! Shared domain types for Lumen Navi.

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
    Other(String),
}

/// Why a full screenshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerReason {
    Interval,
    FocusChange,
    TitleChange,
    Manual,
    SessionOpen,
}

impl TriggerReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interval => "interval",
            Self::FocusChange => "focus_change",
            Self::TitleChange => "title_change",
            Self::Manual => "manual",
            Self::SessionOpen => "session_open",
        }
    }

    /// Focus/manual always attempt full capture after gates (skip visual threshold).
    pub fn forces_full_capture(self) -> bool {
        !matches!(self, Self::Interval)
    }

    pub fn is_churn(self) -> bool {
        matches!(self, Self::FocusChange | Self::TitleChange)
    }
}

pub mod event_kind {
    pub const SCREENSHOT_V1: &str = "screenshot.v1";
    pub const ACTIVITY_SESSION_V1: &str = "activity.session.v1";
    pub const ACTIVITY_FOCUS_V1: &str = "activity.focus.v1";
    pub const AUDIO_CHUNK_V1: &str = "audio_chunk.v1";
    pub const AUDIO_SESSION_V1: &str = "audio_session.v1";
    pub const VIDEO_SEGMENT_V1: &str = "video_segment.v1";
    pub const PAGE_VISIT_V1: &str = "page_visit.v1";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: Uuid,
    pub media_type: String,
    pub path: String,
    pub bytes: Option<u64>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEvent {
    pub id: Uuid,
    pub source: SourceKind,
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

    pub fn with_session(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

/// Lightweight activity session (Observe-level grouping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySession {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub primary_app: Option<String>,
    pub primary_bundle: Option<String>,
    pub trigger: String,
    pub snapshot_count: u32,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Open,
    Closed,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "open" {
            Self::Open
        } else {
            Self::Closed
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
            json!({ "url": "https://example.com" }),
        );
        let encoded = serde_json::to_string(&event).unwrap();
        let decoded: SourceEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.kind, "page_visit");
    }

    #[test]
    fn focus_forces_full_capture() {
        assert!(TriggerReason::FocusChange.forces_full_capture());
        assert!(!TriggerReason::Interval.forces_full_capture());
    }
}
