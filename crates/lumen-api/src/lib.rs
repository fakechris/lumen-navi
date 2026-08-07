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
    /// Browser loopback intake status when configured.
    #[serde(default)]
    pub browser: Option<BrowserHealthResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserHealthResponse {
    pub enabled: bool,
    pub configured: bool,
    pub paused: bool,
    pub accepted_events: u64,
    pub duplicate_events: u64,
    pub rejected_batches: u64,
    pub last_ingest_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserIngestResponse {
    pub schema_version: u32,
    pub accepted: usize,
    pub duplicates: usize,
    #[serde(default)]
    pub rejected_artifacts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserPolicyResponse {
    pub schema_version: u32,
    pub capture_allowed: bool,
    pub content_allow_hosts: Vec<String>,
    pub excluded_hosts: Vec<String>,
    pub max_batch_size: usize,
    pub max_artifact_bytes: usize,
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
    Pause {
        source: Option<String>,
    },
    Resume {
        source: Option<String>,
    },
    ListEvents {
        limit: usize,
    },
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
    Events {
        events: Vec<EventSummary>,
    },
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
            browser: None,
        }
    }
}

// --- Activity / time-tracking ---

/// One continuous activity segment (folded from heartbeats by the projection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySegmentDto {
    pub seg_id: String,
    pub day: String,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub is_idle: bool,
    pub is_locked: bool,
    pub category: Option<String>,
    pub productivity_level: Option<String>,
    pub event_count: i64,
}

/// Aggregated stats for one day (the dashboard's `stats` view payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStatsDto {
    pub day: String,
    /// Active time (excludes idle segments).
    pub total_active_ms: i64,
    pub total_idle_ms: i64,
    /// 0–100 weighted average over classified segments; `None` when nothing
    /// classified (uncategorized time is excluded from the denominator).
    pub pulse_score: Option<f64>,
    pub context_switches: i64,
    /// ms per category (active only).
    pub by_category: Vec<CategoryTotal>,
    /// ms per app (active only), sorted descending.
    pub top_apps: Vec<AppTotal>,
    /// ms per local hour bucket [0..24] (active only).
    pub by_hour: [i64; 24],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryTotal {
    pub category: String,
    pub ms: i64,
    pub productivity_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTotal {
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub ms: i64,
    pub category: Option<String>,
    pub productivity_level: Option<String>,
    pub segment_count: i64,
}

/// One day's roll-up inside a range query (the weekly view payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayRollupDto {
    pub day: String,
    pub total_active_ms: i64,
    pub total_idle_ms: i64,
    pub pulse_score: Option<f64>,
    pub context_switches: i64,
    /// ms per category (active only), top entries sorted desc.
    pub by_category: Vec<CategoryTotal>,
}

/// Aggregated stats over a date range (e.g. the last 7 days).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeStatsDto {
    pub days: Vec<DayRollupDto>,
    /// Range-wide totals.
    pub total_active_ms: i64,
    pub total_idle_ms: i64,
    pub pulse_score: Option<f64>,
    /// ms per app across the whole range, sorted desc (the week's top apps).
    pub top_apps: Vec<AppTotal>,
    /// ms per category across the whole range, sorted desc.
    pub by_category: Vec<CategoryTotal>,
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
