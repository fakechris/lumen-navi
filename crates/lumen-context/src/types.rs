use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CONTEXT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaptureId(pub Uuid);

impl CaptureId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CaptureId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CaptureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProfile {
    Metadata,
    Editor,
    Visible,
    Vision,
    FullLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Target,
    EditorAx,
    AxVisible,
    Browser,
    ScreenshotElement,
    ScreenshotWindow,
    ScreenshotDisplays,
    OcrElement,
    OcrWindow,
    OcrDisplays,
    VisibleTextFusion,
}

impl SourceKind {
    pub const ALL: [Self; 11] = [
        Self::Target,
        Self::EditorAx,
        Self::AxVisible,
        Self::Browser,
        Self::ScreenshotElement,
        Self::ScreenshotWindow,
        Self::ScreenshotDisplays,
        Self::OcrElement,
        Self::OcrWindow,
        Self::OcrDisplays,
        Self::VisibleTextFusion,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSelection {
    pub requested: BTreeSet<SourceKind>,
}

impl SourceSelection {
    pub fn none() -> Self {
        Self {
            requested: BTreeSet::new(),
        }
    }

    pub fn from_sources(sources: impl IntoIterator<Item = SourceKind>) -> Self {
        Self {
            requested: sources.into_iter().collect(),
        }
    }

    pub fn full_local() -> Self {
        Self::from_sources(SourceKind::ALL)
    }

    pub fn contains(&self, source: SourceKind) -> bool {
        self.requested.contains(&source)
    }
}

impl Default for SourceSelection {
    fn default() -> Self {
        Self::full_local()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    DictationHotkey,
    Manual,
    FocusChange,
    TitleChange,
    Interval,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTrigger {
    pub kind: TriggerKind,
    pub pressed_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TargetHint {
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub pid: Option<i32>,
    pub window_id: Option<u64>,
    pub focused_element_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyPolicy {
    pub capture_raw_text: bool,
    pub capture_screenshots: bool,
    pub allow_private_browsing: bool,
    pub denied_bundle_ids: BTreeSet<String>,
    pub denied_domains: BTreeSet<String>,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            capture_raw_text: true,
            capture_screenshots: true,
            allow_private_browsing: false,
            denied_bundle_ids: BTreeSet::new(),
            denied_domains: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub capture_id: CaptureId,
    pub consumer_session_id: Uuid,
    pub target_generation: u64,
    pub profile: CaptureProfile,
    pub sources: SourceSelection,
    pub trigger: CaptureTrigger,
    pub requested_at: DateTime<Utc>,
    pub target_hint: Option<TargetHint>,
    pub privacy_policy: PrivacyPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    pub ax_max_chars: usize,
    pub ax_max_nodes: usize,
    pub ax_max_depth: usize,
    pub ax_timeout_ms: u64,
    pub screenshot_max_edge: u32,
    pub screenshot_jpeg_quality: u8,
    pub screenshot_timeout_ms: u64,
    pub capture_all_displays: bool,
    pub ocr_languages: Vec<String>,
    pub ocr_max_image_bytes: usize,
    pub ocr_helper_path: Option<PathBuf>,
    pub ocr_helper_timeout_ms: u64,
    pub browser_timeout_ms: u64,
    pub browser_max_chars: usize,
    pub browser_max_nodes: usize,
    pub max_payload_bytes_per_capture: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            ax_max_chars: 200_000,
            ax_max_nodes: 5_000,
            ax_max_depth: 32,
            ax_timeout_ms: 1_000,
            screenshot_max_edge: 2_560,
            screenshot_jpeg_quality: 85,
            screenshot_timeout_ms: 3_000,
            capture_all_displays: true,
            ocr_languages: vec!["zh-Hans".to_owned(), "en-US".to_owned()],
            ocr_max_image_bytes: 25 * 1024 * 1024,
            ocr_helper_path: None,
            ocr_helper_timeout_ms: 5_000,
            browser_timeout_ms: 500,
            browser_max_chars: 200_000,
            browser_max_nodes: 5_000,
            max_payload_bytes_per_capture: 128 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    NotRequested,
    Queued,
    Running,
    Succeeded,
    Empty,
    SkippedPolicy,
    Denied,
    Unsupported,
    Unavailable,
    Timeout,
    Stale,
    Cancelled,
    Failed,
}

impl SourceState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Empty
                | Self::SkippedPolicy
                | Self::Denied
                | Self::Unsupported
                | Self::Unavailable
                | Self::Timeout
                | Self::Stale
                | Self::Cancelled
                | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: SourceKind,
    pub state: SourceState,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub queue_wait_ms: Option<u64>,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub retryable: bool,
    pub target_generation: u64,
    pub truncated_nodes: u64,
    pub truncated_chars: u64,
}

impl SourceStatus {
    pub fn new(source: SourceKind, state: SourceState, target_generation: u64) -> Self {
        Self {
            source,
            state,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            queue_wait_ms: None,
            reason_code: None,
            message: None,
            retryable: false,
            target_generation,
            truncated_nodes: 0,
            truncated_chars: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetContext {
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub pid: Option<i32>,
    pub process_start_time: Option<DateTime<Utc>>,
    pub window_id: Option<u64>,
    pub window_title: Option<String>,
    pub window_bounds_global: Option<Rect>,
    pub display_id: Option<u32>,
    pub document_url: Option<String>,
    pub focused_element_fingerprint: Option<String>,
    pub captured_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemContext {
    pub os_version: Option<String>,
    pub os_build: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub input_source: Option<String>,
    pub accessibility_permission: Option<String>,
    pub screen_recording_permission: Option<String>,
    pub browser_permission: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextRange {
    pub location: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorContext {
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub title: Option<String>,
    pub label: Option<String>,
    pub placeholder: Option<String>,
    pub ax_identifier: Option<String>,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub editable: Option<bool>,
    pub secure: bool,
    pub multiline: Option<bool>,
    pub rich_text: Option<bool>,
    pub bounds_global: Option<Rect>,
    pub selection_range: Option<TextRange>,
    pub selected_text: Option<String>,
    pub cursor_prefix: Option<String>,
    pub cursor_suffix: Option<String>,
    pub full_field_text: Option<String>,
    pub value_length: Option<usize>,
    pub ancestor_path: Vec<String>,
    pub nearby_before: Option<String>,
    pub nearby_after: Option<String>,
    pub captured_at: Option<DateTime<Utc>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AxNode {
    pub stable_path: String,
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub title: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    pub placeholder: Option<String>,
    pub bounds_global: Option<Rect>,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub selected: Option<bool>,
    pub depth: usize,
    pub sibling_index: usize,
    pub visible_on_screen: Option<bool>,
    pub children: Vec<AxNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AxVisibleContext {
    pub roots: Vec<AxNode>,
    pub captured_at: Option<DateTime<Utc>>,
    pub visited_nodes: usize,
    pub hidden_nodes: usize,
    pub truncated_nodes: usize,
    pub truncated_chars: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserElementContext {
    pub tag: Option<String>,
    pub input_type: Option<String>,
    pub role: Option<String>,
    pub aria_label: Option<String>,
    pub name: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub placeholder: Option<String>,
    pub value: Option<String>,
    pub selection_start: Option<usize>,
    pub selection_end: Option<usize>,
    pub contenteditable: Option<bool>,
    pub disabled: Option<bool>,
    pub readonly: Option<bool>,
    pub secure: bool,
    pub bounding_rect: Option<Rect>,
    pub coordinate_space: Option<String>,
    pub labels: Vec<String>,
    pub ancestor_path: Vec<String>,
    pub sibling_before: Option<String>,
    pub sibling_after: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserViewportContext {
    pub width: f64,
    pub height: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub device_pixel_ratio: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserContext {
    pub browser: Option<String>,
    pub browser_bundle_id: Option<String>,
    pub browser_pid: Option<i32>,
    pub profile: Option<String>,
    pub incognito: bool,
    pub window_id: Option<i64>,
    pub tab_id: Option<i64>,
    pub frame_id: Option<i64>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub origin: Option<String>,
    pub domain: Option<String>,
    pub navigation_id: Option<String>,
    pub document_id: Option<String>,
    pub page_language: Option<String>,
    pub selection_text: Option<String>,
    pub focused_element: Option<BrowserElementContext>,
    pub nearby_before: Option<String>,
    pub nearby_after: Option<String>,
    pub viewport: Option<BrowserViewportContext>,
    pub viewport_text_blocks: Vec<VisibleTextBlock>,
    pub captured_at: Option<DateTime<Utc>>,
    pub permission_scope: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotKind {
    FocusedElement,
    ActiveWindow,
    ActiveDisplay,
    OtherDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotDocument {
    pub artifact_id: Uuid,
    pub kind: ScreenshotKind,
    pub display_id: Option<u32>,
    pub window_id: Option<u64>,
    pub global_bounds: Option<Rect>,
    pub pixel_bounds: Option<Rect>,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub color_space: Option<String>,
    pub media_type: String,
    pub content_hash: String,
    pub captured_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub occluded: Option<bool>,
    pub cropped: bool,
    #[serde(default)]
    pub capture_method: Option<String>,
    #[serde(default)]
    pub capture_fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrRegion {
    pub text: String,
    pub confidence: f64,
    pub normalized_box: Rect,
    pub pixel_box: Option<Rect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrDocument {
    pub document_id: Uuid,
    pub screenshot_artifact_id: Uuid,
    pub engine: String,
    pub engine_version: Option<String>,
    pub binary_hash: Option<String>,
    pub mode: String,
    pub languages: Vec<String>,
    pub custom_words: Vec<String>,
    pub language_correction: Option<bool>,
    pub text: String,
    pub confidence: f64,
    pub boxes: Vec<OcrRegion>,
    pub reading_order: Vec<usize>,
    pub duration_ms: u64,
    #[serde(default)]
    pub queue_wait_ms: u64,
    pub captured_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibleTextBlock {
    pub text: String,
    pub source_refs: Vec<String>,
    pub global_bounds: Option<Rect>,
    pub coordinate_space: Option<String>,
    pub semantic_role: Option<String>,
    pub order: usize,
    pub confidence: Option<f64>,
    pub duplicate_group_id: Option<String>,
    pub conflict_group_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VisibleTextDocument {
    pub blocks: Vec<VisibleTextBlock>,
    pub generated_at: Option<DateTime<Utc>>,
    pub policy_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub artifact_id: Uuid,
    pub source: SourceKind,
    pub kind: String,
    pub content_hash: String,
    pub media_type: String,
    pub bytes: u64,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum ArtifactPayload {
    Bytes {
        media_type: String,
        bytes: Bytes,
    },
    File {
        media_type: String,
        path: PathBuf,
        delete_after_import: bool,
    },
}

#[derive(Debug, Clone)]
pub struct CapturedArtifact {
    pub descriptor: ArtifactRef,
    pub payload: ArtifactPayload,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrivacyContext {
    pub raw_text_allowed: bool,
    pub screenshots_allowed: bool,
    pub applied_gates: Vec<String>,
    pub policy_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureDiagnostics {
    pub total_duration_ms: Option<u64>,
    pub payload_bytes: u64,
    pub warnings: Vec<String>,
    pub counters: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextManifest {
    pub schema_version: u32,
    pub capture_id: CaptureId,
    pub consumer_session_id: Uuid,
    pub revision: u64,
    pub profile: CaptureProfile,
    pub trigger: CaptureTrigger,
    pub requested_at: DateTime<Utc>,
    pub frozen_at: DateTime<Utc>,
    pub target_generation: u64,
    pub target: Option<TargetContext>,
    pub system: Option<SystemContext>,
    pub editor: Option<EditorContext>,
    pub ax_visible: Option<AxVisibleContext>,
    pub browser: Option<BrowserContext>,
    pub screenshots: Vec<ScreenshotDocument>,
    pub ocr_documents: Vec<OcrDocument>,
    pub visible_text_fused: Option<VisibleTextDocument>,
    pub artifacts: Vec<ArtifactRef>,
    pub source_status: BTreeMap<SourceKind, SourceStatus>,
    pub privacy: PrivacyContext,
    pub diagnostics: CaptureDiagnostics,
}

impl ContextManifest {
    pub fn all_requested_sources_terminal(&self) -> bool {
        self.source_status
            .values()
            .all(|status| status.state.is_terminal())
    }
}

#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    pub manifest: ContextManifest,
    pub payloads: Vec<CapturedArtifact>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SourceCapture {
    pub target: Option<TargetContext>,
    pub system: Option<SystemContext>,
    pub editor: Option<EditorContext>,
    pub ax_visible: Option<AxVisibleContext>,
    pub browser: Option<BrowserContext>,
    pub screenshots: Vec<ScreenshotDocument>,
    pub ocr_documents: Vec<OcrDocument>,
    pub visible_text_fused: Option<VisibleTextDocument>,
    pub artifacts: Vec<CapturedArtifact>,
    pub empty: bool,
    pub truncated_nodes: u64,
    pub truncated_chars: u64,
}

impl SourceCapture {
    pub(crate) fn browser(browser: BrowserContext) -> Self {
        Self {
            browser: Some(browser),
            ..Self::default()
        }
    }
}
