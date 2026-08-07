//! SQLite schema for meta/navi.db

pub const SCHEMA_VERSION: i64 = 7;

pub const MIGRATE_V1: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY NOT NULL,
  source TEXT NOT NULL,
  kind TEXT NOT NULL,
  ts TEXT NOT NULL,
  session_id TEXT,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);

CREATE TABLE IF NOT EXISTS artifacts (
  id TEXT PRIMARY KEY NOT NULL,
  event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  media_type TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER,
  content_hash TEXT,
  ordinal INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_artifacts_event ON artifacts(event_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_hash ON artifacts(content_hash);

CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY NOT NULL,
  event_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_event ON jobs(event_id);

CREATE TABLE IF NOT EXISTS derived (
  id TEXT PRIMARY KEY NOT NULL,
  event_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_derived_event ON derived(event_id);

CREATE TABLE IF NOT EXISTS kv (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL
);
"#;

pub const MIGRATE_V2: &str = r#"
CREATE TABLE IF NOT EXISTS activity_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  primary_app TEXT,
  primary_bundle TEXT,
  trigger TEXT NOT NULL,
  snapshot_count INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activity_sessions_status ON activity_sessions(status);
CREATE INDEX IF NOT EXISTS idx_activity_sessions_started ON activity_sessions(started_at);
"#;

/// OCR / job robustness: backoff, uniqueness, reclaim support.
pub const MIGRATE_V3: &str = r#"
ALTER TABLE jobs ADD COLUMN available_at TEXT;
ALTER TABLE jobs ADD COLUMN created_at TEXT;

CREATE INDEX IF NOT EXISTS idx_jobs_claim
  ON jobs(kind, status, available_at, updated_at);

-- At most one open OCR job per event (pending/running/failed still countable via unique open).
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_open_ocr
  ON jobs(event_id, kind)
  WHERE status IN ('pending', 'running');

-- One derived document per (event, kind) — OCR idempotent.
CREATE UNIQUE INDEX IF NOT EXISTS idx_derived_event_kind
  ON derived(event_id, kind);
"#;

/// OCR search documents + FTS5.
pub const MIGRATE_V4: &str = r#"
CREATE TABLE IF NOT EXISTS ocr_docs (
  id INTEGER PRIMARY KEY,
  event_id TEXT NOT NULL UNIQUE,
  text TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 0,
  session_id TEXT,
  event_ts TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ocr_docs_session ON ocr_docs(session_id);
CREATE INDEX IF NOT EXISTS idx_ocr_docs_event_ts ON ocr_docs(event_ts);
"#;

/// Rebuildable browser visit projection derived from append-only browser events.
pub const MIGRATE_V5: &str = r#"
CREATE TABLE IF NOT EXISTS browser_visits (
  visit_id TEXT PRIMARY KEY NOT NULL,
  document_id TEXT,
  url TEXT,
  opened_at TEXT,
  document_ready_at TEXT,
  closed_at TEXT,
  active_ms INTEGER,
  visible_ms INTEGER,
  max_scroll_ratio REAL,
  close_reason TEXT,
  extraction_status TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_browser_visits_opened ON browser_visits(opened_at);
CREATE INDEX IF NOT EXISTS idx_browser_visits_closed ON browser_visits(closed_at);
"#;

/// Complete the neutral browser visit projection without changing raw events.
pub const MIGRATE_V6: &str = r#"
ALTER TABLE browser_visits ADD COLUMN content_id TEXT;
ALTER TABLE browser_visits ADD COLUMN first_visible_at TEXT;
ALTER TABLE browser_visits ADD COLUMN last_visible_at TEXT;
ALTER TABLE browser_visits ADD COLUMN background_ms INTEGER;
ALTER TABLE browser_visits ADD COLUMN revisit_index INTEGER;
ALTER TABLE browser_visits ADD COLUMN opener_tab_id INTEGER;
ALTER TABLE browser_visits ADD COLUMN referrer TEXT;
ALTER TABLE browser_visits ADD COLUMN transition TEXT;
ALTER TABLE browser_visits ADD COLUMN snapshot_hashes TEXT NOT NULL DEFAULT '[]';

CREATE INDEX IF NOT EXISTS idx_browser_visits_content ON browser_visits(content_id);
"#;

/// Rebuildable activity-segment projection derived from append-only
/// `activity.focus.v1` events (the time-tracking layer). One row per
/// continuous run of the same app+title+idle state, with duration derived
/// from the first/last event ts in the run.
pub const MIGRATE_V7: &str = r#"
CREATE TABLE IF NOT EXISTS activity_segments (
  seg_id TEXT PRIMARY KEY NOT NULL,
  day TEXT NOT NULL,
  app_name TEXT,
  bundle_id TEXT,
  window_title TEXT,
  url TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  is_idle INTEGER NOT NULL DEFAULT 0,
  is_locked INTEGER NOT NULL DEFAULT 0,
  category TEXT,
  project TEXT,
  productivity_level TEXT,
  event_count INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activity_segments_day ON activity_segments(day);
CREATE INDEX IF NOT EXISTS idx_activity_segments_started ON activity_segments(started_at);
CREATE INDEX IF NOT EXISTS idx_activity_segments_app ON activity_segments(app_name);
CREATE INDEX IF NOT EXISTS idx_activity_segments_category ON activity_segments(category);
"#;
