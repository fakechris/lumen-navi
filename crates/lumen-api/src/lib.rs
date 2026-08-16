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
    /// Live closed-eyes hard gate (mirrors `privacy.closed_eyes`).
    #[serde(default)]
    pub closed_eyes: bool,
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
    /// Persist / skip / drop counters since this daemon process started.
    #[serde(default)]
    pub observe: Option<ObserveCountersDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObserveCountersDto {
    pub persisted: u64,
    pub persist_failed: u64,
    pub skipped_gate: u64,
    pub dropped_backpressure: u64,
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
    /// Flip the live closed-eyes hard gate without restarting Observe.
    ClosedEyes {
        enabled: bool,
    },
    GetSettings,
    RecentContext {
        #[serde(default)]
        limit: Option<usize>,
    },
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
    Settings(ObserveSettingsDto),
    RecentContext {
        slots: Vec<HistorySlotDto>,
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
            closed_eyes: false,
            stored_events,
            ocr_docs,
            schema_version,
            browser: None,
            observe: None,
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
    /// Active browser tab URL when the segment's frontmost app is a scriptable
    /// browser (Safari/Chrome/Comet/…); None for non-browsers. Present since
    /// PR #24 — the segment identity includes it so each website accrues its
    /// own duration. Surfaced to the UI so the timeline can show per-site time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub is_idle: bool,
    pub is_locked: bool,
    pub category: Option<String>,
    pub productivity_level: Option<String>,
    pub event_count: i64,
    /// 'auto' (tracked) or 'manual' (user-entered retro-entry).
    #[serde(default)]
    pub source: String,
    /// Nested scene stack label (`Ghostty → herdr → writing`). Query-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_label: Option<String>,
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
    /// ms per (local hour, category) for active segments — drives the
    /// hover tooltip on the hourly bar chart ("15:00 · Development 30m").
    /// Sparse: only (hour, category) pairs with >0ms appear.
    #[serde(default)]
    pub by_hour_category: Vec<HourCategoryTotal>,
}

/// One (hour, category) cell of the hourly breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourCategoryTotal {
    pub hour: u8, // 0..24, local time
    pub category: String,
    pub ms: i64,
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
    /// In "group by site" mode: a representative page title for the domain
    /// (the title from the longest-held segment), so the UI can show
    /// "DeepSeek Platform" instead of just "deepseek.com". None in app mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One merged run of the same scene stack (capture-time leaf + optional shell).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEpisodeDto {
    pub day: String,
    pub kind: String,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub shell: Option<String>,
    pub leaf: String,
    pub label: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub segment_count: i64,
}

/// Ranked scene identity for one day (dashboard drill-down).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneRollupDto {
    pub kind: String,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub shell: Option<String>,
    pub leaf: String,
    pub label: String,
    pub ms: i64,
    pub episode_count: i64,
}

/// Scene projection for one local day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneDayDto {
    pub day: String,
    pub episodes: Vec<SceneEpisodeDto>,
    pub rollups: Vec<SceneRollupDto>,
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

/// Structured one-day behavior summary — the payload for the "roast my day"
/// LLM feature. Every field is a real number from the store; the LLM only
/// needs to be witty, not to guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayRoastSummaryDto {
    pub day: String,
    pub total_active_ms: i64,
    pub total_idle_ms: i64,
    /// 0-100 weighted productivity score (None if nothing classified).
    pub pulse_score: Option<f64>,
    pub context_switches: i64,
    pub screenshot_count: i64,
    pub ax_sample_count: i64,
    /// Aggregated behavioral input counts for the day (None if the input
    /// counter was never enabled). Content-free by construction.
    pub input_counts: Option<RoastInputCounts>,
    /// Top apps by active time with percentage of total.
    pub top_apps: Vec<RoastAppTotal>,
    /// Top websites by active time (from segment url).
    pub top_domains: Vec<RoastDomainTotal>,
    /// Top scenes (stack labels, e.g. "Ghostty → herdr → writing").
    pub top_scenes: Vec<RoastSceneTotal>,
    /// Recurring window titles — the "documents you lived in".
    pub notable_titles: Vec<RoastTitleTotal>,
    /// The single busiest local hour.
    pub busiest_hour: Option<RoastHour>,
    /// Per-hour active ms (local hour buckets 0-23, only non-zero entries).
    pub hour_histogram: Vec<RoastHour>,
    /// Total ms covered by intervals with real keyboard/mouse activity
    /// (None when no behavioral signal ran that day).
    pub user_active_ms: Option<i64>,
    /// Which signal produced the attribution: "interactions" (discrete
    /// click/submit/shortcut events), "input.stats" (periodic counters),
    /// or None (screen-only — attribution fields are 0/None).
    pub attribution: Option<String>,
    /// Non-idle segment starts that happened around real user input —
    /// switches the user plausibly drove (None without input data).
    pub switches_user: Option<i64>,
    /// The rest of the non-idle switches (programmatic / ambiguous).
    pub switches_passive: Option<i64>,
}

/// Day-aggregated input counts (sum of all input.stats.v1 events for the day).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoastInputCounts {
    pub key_delete: u64,
    pub key_tab: u64,
    pub key_esc: u64,
    pub key_enter: u64,
    pub key_arrow: u64,
    pub key_space: u64,
    pub combo_copy: u64,
    pub combo_paste: u64,
    pub combo_cut: u64,
    pub combo_undo: u64,
    pub combo_selectall: u64,
    pub combo_find: u64,
    pub combo_close: u64,
    pub combo_new: u64,
    pub combo_save: u64,
    pub mouse_left: u64,
    pub mouse_right: u64,
    pub mouse_other: u64,
    pub mouse_double: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastAppTotal {
    pub app: String,
    pub ms: i64,
    /// Percent of total_active_ms, 0-100 with one decimal.
    pub pct: f64,
    pub category: Option<String>,
    /// Overlap of this app's foreground time with intervals where the user
    /// actually typed/clicked (input.stats.v1). 0 when input counting is off.
    pub user_active_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastDomainTotal {
    pub domain: String,
    pub ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastSceneTotal {
    pub label: String,
    pub ms: i64,
    pub episode_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastTitleTotal {
    /// App that owned the window.
    pub app: String,
    pub title: String,
    /// Number of foreground episodes (NOT "times the user opened it").
    pub seen_count: i64,
    /// Total foreground dwell from activity segments.
    pub dwell_ms: i64,
    /// Dwell overlap with real keyboard/mouse activity.
    pub user_active_ms: i64,
    /// Discrete mouse clicks on this title (observe_interactions).
    pub clicks: i64,
    /// Enter-key submits (messages sent, forms confirmed).
    pub submits: i64,
    /// Cmd/Ctrl shortcut invocations.
    pub shortcuts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastHour {
    /// Local hour 0-23.
    pub hour: u8,
    pub active_ms: i64,
    pub top_app: Option<String>,
}

/// AI chat thread summary (one conversation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiThreadDto {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

/// One persisted chat turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMessageDto {
    pub id: String,
    pub thread_id: String,
    /// "user" | "assistant"
    pub role: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub created_at: String,
}

/// An archived roast for one day (there can be several per day).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastRecordDto {
    pub id: String,
    /// "YYYY-MM-DD" the roast is about.
    pub day: String,
    pub model: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub created_at: String,
}

/// Calendar index entry: which days have roasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoastIndexDto {
    pub day: String,
    pub count: i64,
}

/// Live Observe settings snapshot for agents (get-then-replace later).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveSettingsDto {
    pub paused: bool,
    pub closed_eyes: bool,
    pub app_blocklist: Vec<String>,
    pub sources: ObserveSourcesDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveSourcesDto {
    pub screen: bool,
    pub audio: bool,
    pub browser: bool,
}

/// One 10-minute History card (deterministic fold of activity segments).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySlotDto {
    pub slot_start: DateTime<Utc>,
    pub slot_end: DateTime<Utc>,
    pub title: String,
    pub body: String,
    pub apps: Vec<HistorySlotAppDto>,
    pub scenes: Vec<HistorySlotSceneDto>,
    pub titles: Vec<String>,
    pub urls: Vec<String>,
    pub active_ms: i64,
    #[serde(default)]
    pub narrative_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySlotAppDto {
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub ms: i64,
    pub pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySlotSceneDto {
    pub label: String,
    pub ms: i64,
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
