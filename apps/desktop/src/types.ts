export type TabId = "overview" | "dashboard" | "search" | "activity" | "settings";

export interface ActivitySegment {
  seg_id: string;
  day: string;
  app_name: string | null;
  bundle_id: string | null;
  window_title: string | null;
  /** Browser tab URL when the segment's frontmost app is a scriptable browser. */
  url?: string | null;
  started_at: string;
  ended_at: string | null;
  duration_ms: number;
  is_idle: boolean;
  is_locked: boolean;
  category: string | null;
  productivity_level: string | null;
  event_count: number;
  source?: string;
}

export interface CategoryTotal {
  category: string;
  ms: number;
  productivity_level: string | null;
}

export interface AppTotal {
  app_name: string;
  bundle_id: string | null;
  ms: number;
  category: string | null;
  productivity_level: string | null;
  segment_count: number;
  /** In site mode: a representative page title for the domain. */
  title?: string | null;
}

export interface HourCategoryTotal {
  hour: number; // 0..24, local
  category: string;
  ms: number;
}

export interface DayStats {
  day: string;
  total_active_ms: number;
  total_idle_ms: number;
  pulse_score: number | null;
  context_switches: number;
  by_category: CategoryTotal[];
  top_apps: AppTotal[];
  by_hour: number[];
  by_hour_category?: HourCategoryTotal[];
}

export interface DayRollup {
  day: string;
  total_active_ms: number;
  total_idle_ms: number;
  pulse_score: number | null;
  context_switches: number;
  by_category: CategoryTotal[];
}

export interface RangeStats {
  days: DayRollup[];
  total_active_ms: number;
  total_idle_ms: number;
  pulse_score: number | null;
  top_apps: AppTotal[];
  by_category: CategoryTotal[];
}

export type MatchField = "bundle_id" | "app_name" | "domain" | "url" | "title";
export type ProductivityLevel = "productive" | "neutral" | "distracting";

export interface CategoryRule {
  field: MatchField;
  value: string;
  category: string;
  level?: ProductivityLevel | null;
}

export interface SourceStatus {
  id: string;
  enabled: boolean;
  running: boolean;
  last_error: string | null;
}

export interface Health {
  api_version: number;
  product: string;
  sources: SourceStatus[];
  paused: boolean;
  stored_events: number;
  ocr_docs: number;
  schema_version: number;
  browser: BrowserHealth | null;
}

export interface BrowserHealth {
  enabled: boolean;
  configured: boolean;
  paused: boolean;
  accepted_events: number;
  duplicate_events: number;
  rejected_batches: number;
  last_ingest_at: string | null;
}

export interface BrowserPairing {
  enabled: boolean;
  configured: boolean;
  endpoint: string;
  token: string;
}

export interface Permissions {
  screen_recording: string;
  screen_capture_ready: boolean | null;
  direct_capture_status: string;
  direct_capture_error: string | null;
  microphone: string;
  accessibility: string;
}

export interface SearchHit {
  event_id: string;
  session_id: string | null;
  event_ts: string | null;
  confidence: number;
  snippet: string;
  text_preview: string;
}

export interface EventSummary {
  id: string;
  source: string;
  kind: string;
  ts: string;
}

export interface ConfigSummary {
  data_dir: string;
  config_path: string;
  screen: boolean;
  audio: boolean;
  browser: boolean;
  ocr: boolean;
  asr: boolean;
  paused: boolean;
  api_bind: string;
  audio_chunk_ms: number;
  asr_locale: string;
  asr_engine: string;
  asr_model_dir: string;
  asr_http_base_url: string;
  asr_http_model: string;
  asr_fallback_speech: boolean;
  system_audio: boolean;
}

export interface SourcesUpdate {
  screen?: boolean;
  audio?: boolean;
  browser?: boolean;
  ocr?: boolean;
  asr?: boolean;
  paused?: boolean;
  system_audio?: boolean;
  asr_engine?: string;
  asr_model_dir?: string;
  asr_http_base_url?: string;
  asr_http_model?: string;
  asr_locale?: string;
  asr_fallback_speech?: boolean;
}

export interface TimelineItem {
  id: string;
  source: string;
  kind: string;
  ts: string;
  session_id: string | null;
  app_name: string | null;
  window_title: string | null;
  text_preview: string | null;
  text_kind: string | null;
  media_type: string | null;
  has_image: boolean;
  artifact_bytes: number | null;
}

export interface ObserveStatus {
  running: boolean;
  pid: number | null;
}

export interface OnboardingState {
  needs_onboarding: boolean;
  completed: boolean;
  skipped: boolean;
  step: number;
  launch_observe: boolean;
}

export interface AsrModelCandidate {
  engine: string;
  path: string;
  label: string;
  ready: boolean;
  source: string;
}

export interface AsrModelStatus {
  sensevoice_ready: boolean;
  sensevoice_dir: string;
  whisper_ready: boolean;
  whisper_dir: string;
  /** Shared Lumen cluster models root (navi/asr/future apps). */
  models_root: string;
  active_engine: string;
  active_model_dir: string;
  candidates: AsrModelCandidate[];
  download_url: string;
}

export interface AsrDownloadProgress {
  phase: string;
  message: string;
  bytes: number;
  total: number | null;
  percent: number | null;
}

/** Selection-popup assistant config (navi.toml `[assistant]`). */
export interface AssistantConfig {
  enabled: boolean;
  popup_enabled: boolean;
  base_url: string;
  model: string;
  target_lang: string;
  max_selection_chars: number;
  /** Key is never echoed back — only whether one is configured. */
  api_key_set: boolean;
  accessibility_trusted: boolean;
  clipboard_fallback: boolean;
}

export interface AssistantUpdate {
  enabled?: boolean;
  popup_enabled?: boolean;
  base_url?: string;
  model?: string;
  target_lang?: string;
  /** undefined = keep, "" = clear, value = set. */
  api_key?: string;
  clipboard_fallback?: boolean;
}

export type AssistantAction = "translate" | "ask";
