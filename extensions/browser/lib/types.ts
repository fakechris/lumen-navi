export type ObservationKind =
  | "browser.navigation_committed.v1"
  | "browser.document_ready.v1"
  | "browser.visibility_focus_change.v1"
  | "browser.feedback.v1"
  | "browser.visit_closed.v1"
  | "browser.health.v1"
  | "browser.gap.v1";

export interface BrowserObservation {
  id: string;
  kind: ObservationKind;
  ts: string;
  visit_id: string;
  document_id?: string;
  url?: string;
  payload: Record<string, unknown>;
}

export interface BrowserArtifact {
  event_id: string;
  media_type: "text/markdown";
  body: string;
  content_hash?: string;
}

export interface QueueState {
  observations: BrowserObservation[];
  artifacts: BrowserArtifact[];
  pendingGap?: BrowserObservation;
  droppedEvents: number;
}

export interface Settings {
  installationId: string;
  daemonUrl: string;
  token: string;
  paused: boolean;
  daemonCaptureAllowed: boolean;
  daemonCaptureKnown: boolean;
  contentAllowHosts: string[];
  daemonContentAllowHosts: string[];
  excludedHosts: string[];
  daemonExcludedHosts: string[];
  batchSize: number;
  flushIntervalMs: number;
  maxQueueSize: number;
  maxQueueBytes: number;
  maxArtifactBytes: number;
  captureProfileVersion: string;
}

export interface LocalStoreStats {
  observations: number;
  artifacts: number;
  pendingSync: number;
  synced: number;
  rejected: number;
}

export interface BrowserBatch {
  installation_id: string;
  schema_version: 1;
  capture_profile_version: string;
  config_hash: string;
  observations: BrowserObservation[];
  artifacts: BrowserArtifact[];
}
