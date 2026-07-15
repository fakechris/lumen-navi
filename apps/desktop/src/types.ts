export type TabId = "overview" | "search" | "activity" | "settings";

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
}

export interface Permissions {
  screen_recording: string;
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
  ocr: boolean;
  asr: boolean;
  paused: boolean;
  api_bind: string;
  audio_chunk_ms: number;
  asr_locale: string;
  system_audio: boolean;
}

export interface SourcesUpdate {
  screen?: boolean;
  audio?: boolean;
  ocr?: boolean;
  asr?: boolean;
  paused?: boolean;
  system_audio?: boolean;
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
