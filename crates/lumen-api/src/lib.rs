//! Versioned local control plane schema.
//!
//! Transport (UDS / loopback HTTP) is chosen by the daemon. Chrome and desktop
//! UI must speak this schema so the core does not grow ad-hoc endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// API schema version advertised by the daemon.
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub api_version: u32,
    pub product: String,
    pub sources: Vec<SourceStatus>,
    pub paused: bool,
    pub stored_events: usize,
    /// Indexed OCR documents (`ocr_docs` / FTS).
    #[serde(default)]
    pub ocr_docs: usize,
    /// Store schema version.
    #[serde(default)]
    pub schema_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub id: String,
    pub enabled: bool,
    pub running: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlRequest {
    Health,
    Pause { source: Option<String> },
    Resume { source: Option<String> },
    ListEvents { limit: usize },
    Wipe,
    Permissions,
    /// Full-text search over OCR (`ocr_docs` / FTS5).
    SearchOcr {
        query: String,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Rebuild `ocr_docs` from all `derived` ocr.v1 rows.
    ReindexOcr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "ok", rename_all = "snake_case")]
pub enum ControlResponse {
    Health(HealthResponse),
    Ack,
    Events { events: Vec<EventSummary> },
    OcrSearch {
        query: String,
        hits: Vec<OcrSearchHitDto>,
    },
    Reindex {
        indexed: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: Uuid,
    pub source: String,
    pub kind: String,
    pub ts: DateTime<Utc>,
}

/// Wire format for one OCR search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrSearchHitDto {
    pub event_id: Uuid,
    pub session_id: Option<Uuid>,
    pub event_ts: Option<DateTime<Utc>>,
    pub confidence: f64,
    pub snippet: String,
    pub text_preview: String,
}

impl HealthResponse {
    pub fn scaffold(
        sources: Vec<SourceStatus>,
        stored_events: usize,
        paused: bool,
        ocr_docs: usize,
        schema_version: i64,
    ) -> Self {
        Self {
            api_version: API_VERSION,
            product: "lumen-navi".into(),
            sources,
            paused,
            stored_events,
            ocr_docs,
            schema_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_roundtrip() {
        let h = HealthResponse::scaffold(
            vec![SourceStatus {
                id: "screen".into(),
                enabled: true,
                running: false,
                last_error: None,
            }],
            0,
            false,
            0,
            4,
        );
        let s = serde_json::to_string(&h).unwrap();
        let back: HealthResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.api_version, API_VERSION);
        assert_eq!(back.product, "lumen-navi");
        assert_eq!(back.schema_version, 4);
    }

    #[test]
    fn search_ocr_request_roundtrip() {
        let req = ControlRequest::SearchOcr {
            query: "hello".into(),
            limit: Some(10),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("search_ocr"));
        let back: ControlRequest = serde_json::from_str(&s).unwrap();
        match back {
            ControlRequest::SearchOcr { query, limit } => {
                assert_eq!(query, "hello");
                assert_eq!(limit, Some(10));
            }
            _ => panic!("wrong variant"),
        }
    }
}
