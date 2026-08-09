//! Durable SQLite + blob store.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lumen_api::{ActivitySegmentDto, AppTotal, CategoryTotal, DayRollupDto, DayStatsDto, RangeStatsDto};
use lumen_types::{event_kind, ActivitySession, ArtifactRef, SourceEvent, SourceKind};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::blob::BlobStore;
use crate::categorization::{ActivityFields, CategoryRule};
use crate::schema::{
    MIGRATE_V1, MIGRATE_V2, MIGRATE_V3, MIGRATE_V4, MIGRATE_V5, MIGRATE_V6, MIGRATE_V7,
    MIGRATE_V8, SCHEMA_VERSION,
};
use crate::{EventStore, JobRecord, JobStatus, StoreError};

/// One OCR search hit (FTS).
#[derive(Debug, Clone)]
pub struct OcrSearchHit {
    pub event_id: Uuid,
    pub session_id: Option<Uuid>,
    pub event_ts: Option<DateTime<Utc>>,
    pub confidence: f64,
    pub snippet: String,
    pub text_preview: String,
}

/// Timeline list filters (product UI / control API).
#[derive(Debug, Clone, Default)]
pub struct TimelineQuery {
    pub limit: usize,
    /// Substring match on kind (e.g. `screenshot`, `audio_chunk`). Empty = all.
    pub kind_contains: String,
    /// Case-insensitive match against payload app_name / text preview.
    pub app_contains: String,
    /// Only events at or after this timestamp (RFC3339).
    pub since: Option<DateTime<Utc>>,
    /// Only events at or before this timestamp.
    pub until: Option<DateTime<Utc>>,
}

/// One row for timeline UI (enriched preview, no full blobs).
#[derive(Debug, Clone)]
pub struct TimelineItem {
    pub id: Uuid,
    pub source: String,
    pub kind: String,
    pub ts: DateTime<Utc>,
    pub session_id: Option<Uuid>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    /// From ocr.v1 or transcript.v1 when present.
    pub text_preview: Option<String>,
    pub text_kind: Option<String>,
    pub media_type: Option<String>,
    /// Relative blob path under data_dir (for thumbnail fetch).
    pub artifact_path: Option<String>,
    pub artifact_bytes: Option<u64>,
}

/// One event of a session with one derived doc body (see
/// [`SqliteStore::list_session_derived`]).
#[derive(Debug, Clone)]
pub struct SessionDerivedRow {
    pub event_id: Uuid,
    pub ts: DateTime<Utc>,
    pub payload: serde_json::Value,
    /// Derived doc body JSON (e.g. a `transcript.v1` record).
    pub body: String,
}

/// Artifact bytes accepted at an intake boundary before content-addressed storage.
#[derive(Debug, Clone)]
pub struct ArtifactInput {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// One event and its artifacts for idempotent batch persistence.
#[derive(Debug, Clone)]
pub struct EventWithArtifacts {
    pub event: SourceEvent,
    pub artifacts: Vec<ArtifactInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotentAppendOutcome {
    pub accepted: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobLimitedAppendOutcome {
    Appended(IdempotentAppendOutcome),
    LimitExceeded,
}

#[derive(Debug, Clone)]
pub struct CursorEvent {
    pub cursor: i64,
    pub event: SourceEvent,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrowserVisitProjection {
    pub visit_id: Uuid,
    pub document_id: Option<String>,
    pub content_id: Option<String>,
    pub url: Option<String>,
    pub opened_at: Option<DateTime<Utc>>,
    pub document_ready_at: Option<DateTime<Utc>>,
    pub first_visible_at: Option<DateTime<Utc>>,
    pub last_visible_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub active_ms: Option<i64>,
    pub visible_ms: Option<i64>,
    pub background_ms: Option<i64>,
    pub max_scroll_ratio: Option<f64>,
    pub revisit_index: Option<i64>,
    pub opener_tab_id: Option<i64>,
    pub referrer: Option<String>,
    pub transition: Option<String>,
    pub close_reason: Option<String>,
    pub extraction_status: Option<String>,
    pub snapshot_hashes: Vec<String>,
}

/// On-disk store: `$data_dir/meta/navi.db` + `$data_dir/blobs/...`.
pub struct SqliteStore {
    data_dir: PathBuf,
    conn: Mutex<Connection>,
    blobs: BlobStore,
    blob_intake: Mutex<()>,
}

impl SqliteStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let meta_dir = data_dir.join("meta");
        std::fs::create_dir_all(&meta_dir).map_err(StoreError::io)?;
        let db_path = meta_dir.join("navi.db");

        let conn = Connection::open(&db_path).map_err(StoreError::db)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(StoreError::db)?;
        migrate(&conn)?;

        let blobs = BlobStore::open(&data_dir)?;
        Ok(Self {
            data_dir,
            conn: Mutex::new(conn),
            blobs,
            blob_intake: Mutex::new(()),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Put bytes into CA store, then attach as a new artifact on a clone of the event and append.
    pub fn put_and_append(
        &self,
        mut event: SourceEvent,
        media_type: impl Into<String>,
        bytes: &[u8],
    ) -> Result<SourceEvent, StoreError> {
        let _blob_guard = self
            .blob_intake
            .lock()
            .map_err(|_| StoreError::Other("blob intake lock poisoned".into()))?;
        let artifact = self.blobs.put_bytes(media_type, bytes)?;
        event.artifacts.push(artifact);
        self.append_sync(std::slice::from_ref(&event))?;
        Ok(event)
    }

    /// Persist a single event with no artifact bytes (metadata-only events,
    /// e.g. `activity.focus.v1` heartbeats). Projects into derived tables via
    /// the same insert path as screenshot events.
    pub fn append_event(&self, event: SourceEvent) -> Result<(), StoreError> {
        self.append_sync(std::slice::from_ref(&event))
    }

    /// Persist a replay-safe batch. Existing event ids are counted as duplicates;
    /// their payload and artifacts are left untouched.
    pub fn append_idempotent_with_artifacts(
        &self,
        records: Vec<EventWithArtifacts>,
    ) -> Result<IdempotentAppendOutcome, StoreError> {
        match self.append_idempotent_with_artifacts_up_to(records, u64::MAX)? {
            BlobLimitedAppendOutcome::Appended(outcome) => Ok(outcome),
            BlobLimitedAppendOutcome::LimitExceeded => {
                Err(StoreError::Other("unexpected unlimited blob limit".into()))
            }
        }
    }

    /// Persist a replay-safe batch while atomically serializing blob quota
    /// calculation and writes. Duplicate event ids are filtered before blobs.
    pub fn append_idempotent_with_artifacts_up_to(
        &self,
        records: Vec<EventWithArtifacts>,
        max_blob_bytes: u64,
    ) -> Result<BlobLimitedAppendOutcome, StoreError> {
        let _blob_guard = self
            .blob_intake
            .lock()
            .map_err(|_| StoreError::Other("blob intake lock poisoned".into()))?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;
        let mut pending = Vec::with_capacity(records.len());
        let mut seen = HashSet::new();
        let mut duplicates = 0;
        for record in records {
            let event_id = record.event.id.to_string();
            let exists = tx
                .query_row(
                    "SELECT 1 FROM events WHERE id = ?1",
                    params![event_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(StoreError::db)?
                .is_some();
            if exists || !seen.insert(record.event.id) {
                duplicates += 1;
            } else {
                pending.push(record);
            }
        }
        let current_blob_bytes = self.blobs.total_bytes()?;
        let additional_blob_bytes = self.blobs.additional_bytes(
            pending
                .iter()
                .flat_map(|record| record.artifacts.iter())
                .map(|artifact| artifact.bytes.as_slice()),
        )?;
        if additional_blob_bytes > 0
            && current_blob_bytes.saturating_add(additional_blob_bytes) > max_blob_bytes
        {
            return Ok(BlobLimitedAppendOutcome::LimitExceeded);
        }

        let mut prepared = Vec::with_capacity(pending.len());
        for record in pending {
            let mut event = record.event;
            for artifact in record.artifacts {
                event
                    .artifacts
                    .push(self.blobs.put_bytes(artifact.media_type, &artifact.bytes)?);
            }
            prepared.push(event);
        }
        let mut accepted = 0;
        for event in &prepared {
            if insert_event_idempotent(&tx, event)? {
                accepted += 1;
            } else {
                duplicates += 1;
            }
        }
        tx.commit().map_err(StoreError::db)?;
        Ok(BlobLimitedAppendOutcome::Appended(IdempotentAppendOutcome {
            accepted,
            duplicates,
        }))
    }

    /// Read one source in insertion order without exposing SQLite rowids as
    /// part of the event schema. The returned cursor is local to this store.
    pub fn list_source_after_cursor(
        &self,
        source: &SourceKind,
        after: i64,
        limit: usize,
    ) -> Result<Vec<CursorEvent>, StoreError> {
        let source = serde_json::to_string(source).map_err(StoreError::json)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT rowid, id, source, kind, ts, session_id, payload
                   FROM events
                   WHERE source = ?1 AND rowid > ?2
                   ORDER BY rowid ASC
                   LIMIT ?3"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(
                params![source, after.max(0), limit.clamp(1, 10_000) as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        EventRow {
                            id: row.get(1)?,
                            source: row.get(2)?,
                            kind: row.get(3)?,
                            ts: row.get(4)?,
                            session_id: row.get(5)?,
                            payload: row.get(6)?,
                        },
                    ))
                },
            )
            .map_err(StoreError::db)?;
        let mut raw = Vec::new();
        for row in rows {
            raw.push(row.map_err(StoreError::db)?);
        }
        drop(stmt);

        let mut output = Vec::with_capacity(raw.len());
        for (cursor, row) in raw {
            let mut event = row_to_event(row)?;
            event.artifacts = load_artifacts(&conn, event.id)?;
            output.push(CursorEvent { cursor, event });
        }
        Ok(output)
    }

    pub fn get_browser_visit(
        &self,
        visit_id: Uuid,
    ) -> Result<Option<BrowserVisitProjection>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        conn.query_row(
            r#"SELECT visit_id, document_id, content_id, url, opened_at, document_ready_at,
                      first_visible_at, last_visible_at, closed_at, active_ms, visible_ms,
                      background_ms, max_scroll_ratio, revisit_index, opener_tab_id,
                      referrer, transition, close_reason, extraction_status, snapshot_hashes
               FROM browser_visits WHERE visit_id = ?1"#,
            params![visit_id.to_string()],
            |row| {
                Ok(BrowserVisitProjection {
                    visit_id: Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    document_id: row.get(1)?,
                    content_id: row.get(2)?,
                    url: row.get(3)?,
                    opened_at: optional_sql_ts(row.get(4)?),
                    document_ready_at: optional_sql_ts(row.get(5)?),
                    first_visible_at: optional_sql_ts(row.get(6)?),
                    last_visible_at: optional_sql_ts(row.get(7)?),
                    closed_at: optional_sql_ts(row.get(8)?),
                    active_ms: row.get(9)?,
                    visible_ms: row.get(10)?,
                    background_ms: row.get(11)?,
                    max_scroll_ratio: row.get(12)?,
                    revisit_index: row.get(13)?,
                    opener_tab_id: row.get(14)?,
                    referrer: row.get(15)?,
                    transition: row.get(16)?,
                    close_reason: row.get(17)?,
                    extraction_status: row.get(18)?,
                    snapshot_hashes: serde_json::from_str(&row.get::<_, String>(19)?)
                        .unwrap_or_default(),
                })
            },
        )
        .optional()
        .map_err(StoreError::db)
    }

    pub fn upsert_session(&self, session: &ActivitySession) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        conn.execute(
            r#"INSERT INTO activity_sessions
               (id, started_at, ended_at, primary_app, primary_bundle, trigger, snapshot_count, status)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
               ON CONFLICT(id) DO UPDATE SET
                 ended_at=excluded.ended_at,
                 primary_app=excluded.primary_app,
                 primary_bundle=excluded.primary_bundle,
                 trigger=excluded.trigger,
                 snapshot_count=excluded.snapshot_count,
                 status=excluded.status"#,
            params![
                session.id.to_string(),
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
                session.primary_app,
                session.primary_bundle,
                session.trigger,
                session.snapshot_count as i64,
                session.status.as_str(),
            ],
        )
        .map_err(StoreError::db)?;
        Ok(())
    }

    /// Enqueue a job unless one is already pending/running for the same event+kind.
    /// Returns `Ok(None)` when skipped as duplicate open job.
    pub fn enqueue_job(
        &self,
        event_id: Uuid,
        kind: impl Into<String>,
    ) -> Result<Option<JobRecord>, StoreError> {
        let kind = kind.into();
        let now = Utc::now();
        let job = JobRecord {
            id: Uuid::new_v4(),
            event_id,
            kind: kind.clone(),
            status: JobStatus::Pending,
            attempts: 0,
            last_error: None,
            updated_at: now,
            available_at: Some(now),
        };
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        // Skip if derived already exists for ocr-like idempotency at enqueue time
        // (caller may also check; store enforces open-job uniqueness).
        let res = conn.execute(
            r#"INSERT INTO jobs (id, event_id, kind, status, attempts, last_error, updated_at, available_at, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            params![
                job.id.to_string(),
                job.event_id.to_string(),
                job.kind,
                job.status.as_str(),
                job.attempts,
                job.last_error,
                job.updated_at.to_rfc3339(),
                job.available_at.map(|t| t.to_rfc3339()),
                now.to_rfc3339(),
            ],
        );
        match res {
            Ok(_) => Ok(Some(job)),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(None)
            }
            Err(e) => Err(StoreError::db(e)),
        }
    }

    /// Reclaim jobs stuck in `running` longer than `stale_for`.
    pub fn reclaim_stale_running(
        &self,
        kind: &str,
        stale_for: chrono::Duration,
    ) -> Result<usize, StoreError> {
        let cutoff = (Utc::now() - stale_for).to_rfc3339();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let n = conn
            .execute(
                r#"UPDATE jobs
                   SET status = 'pending', available_at = ?1, updated_at = ?1,
                       last_error = COALESCE(last_error, 'reclaimed stale running')
                   WHERE kind = ?2 AND status = 'running' AND updated_at < ?3"#,
                params![now, kind, cutoff],
            )
            .map_err(StoreError::db)?;
        Ok(n)
    }

    /// Claim pending jobs that are due (`available_at` null or <= now).
    pub fn claim_pending_jobs(&self, kind: &str, limit: usize) -> Result<Vec<JobRecord>, StoreError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;
        let now = Utc::now();
        let now_s = now.to_rfc3339();
        let mut stmt = tx
            .prepare(
                r#"SELECT id, event_id, kind, status, attempts, last_error, updated_at, available_at
                   FROM jobs
                   WHERE status = 'pending' AND kind = ?1
                     AND (available_at IS NULL OR available_at <= ?2)
                   ORDER BY available_at ASC, updated_at ASC
                   LIMIT ?3"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![kind, now_s, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(StoreError::db)?;
        let mut claimed = Vec::new();
        for r in rows {
            let (id, event_id, kind, attempts, last_error, _, _) = r.map_err(StoreError::db)?;
            let changed = tx
                .execute(
                    r#"UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = ?1
                       WHERE id = ?2 AND status = 'pending'"#,
                    params![now_s, id],
                )
                .map_err(StoreError::db)?;
            if changed == 0 {
                continue;
            }
            claimed.push(JobRecord {
                id: parse_uuid(id)?,
                event_id: parse_uuid(event_id)?,
                kind,
                status: JobStatus::Running,
                attempts: attempts + 1,
                last_error,
                updated_at: now,
                available_at: Some(now),
            });
        }
        drop(stmt);
        tx.commit().map_err(StoreError::db)?;
        Ok(claimed)
    }

    pub fn complete_job(
        &self,
        job_id: Uuid,
        status: JobStatus,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        self.complete_job_at(job_id, status, error, None)
    }

    /// Complete or re-queue with optional `available_at` (for pending retry backoff).
    pub fn complete_job_at(
        &self,
        job_id: Uuid,
        status: JobStatus,
        error: Option<&str>,
        available_at: Option<DateTime<Utc>>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let now = Utc::now();
        conn.execute(
            r#"UPDATE jobs SET status = ?1, last_error = ?2, updated_at = ?3, available_at = ?4
               WHERE id = ?5"#,
            params![
                status.as_str(),
                error,
                now.to_rfc3339(),
                available_at.or(Some(now)).map(|t| t.to_rfc3339()),
                job_id.to_string()
            ],
        )
        .map_err(StoreError::db)?;
        Ok(())
    }

    /// Insert or replace derived body for (event_id, kind).
    /// When `kind == "ocr.v1"`, also upserts searchable `ocr_docs` + FTS.
    pub fn insert_derived(
        &self,
        event_id: Uuid,
        kind: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<Uuid, StoreError> {
        let kind = kind.into();
        let body = body.into();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;
        let existing: Option<String> = tx
            .query_row(
                r#"SELECT id FROM derived WHERE event_id = ?1 AND kind = ?2"#,
                params![event_id.to_string(), kind],
                |r| r.get(0),
            )
            .optional()
            .map_err(StoreError::db)?;
        let id = if let Some(e) = existing {
            let id = parse_uuid(e)?;
            tx.execute(
                r#"UPDATE derived SET body = ?1, created_at = ?2 WHERE id = ?3"#,
                params![body, Utc::now().to_rfc3339(), id.to_string()],
            )
            .map_err(StoreError::db)?;
            id
        } else {
            let id = Uuid::new_v4();
            tx.execute(
                r#"INSERT INTO derived (id, event_id, kind, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    id.to_string(),
                    event_id.to_string(),
                    kind,
                    body,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(StoreError::db)?;
            id
        };

        // Index OCR + ASR + rule summaries into the same FTS surface.
        if kind == "ocr.v1" || kind == "transcript.v1" || kind == "summary.v1" {
            upsert_ocr_doc_tx(&tx, event_id, &body)?;
        }
        tx.commit().map_err(StoreError::db)?;
        Ok(id)
    }

    /// Full-text search over OCR documents.
    ///
    /// Uses FTS5 when available; falls back to LIKE for short tokens (trigram
    /// needs ≥3 chars) or when FTS returns no hits.
    pub fn search_ocr(&self, query: &str, limit: usize) -> Result<Vec<OcrSearchHit>, StoreError> {
        let fts_q = sanitize_fts_query(query);
        let like_q = like_pattern(query);
        if fts_q.is_empty() && like_q.is_none() {
            return Ok(vec![]);
        }
        let limit = limit.clamp(1, 200);
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;

        let fts_ok = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='ocr_fts'",
                [],
                |_| Ok(1i32),
            )
            .optional()
            .map_err(StoreError::db)?
            .is_some();

        if fts_ok && !fts_q.is_empty() {
            let sql = r#"
                SELECT d.event_id, d.session_id, d.event_ts, d.confidence, d.text,
                       snippet(ocr_fts, 0, '「', '」', '…', 16) AS snip
                FROM ocr_fts
                JOIN ocr_docs d ON d.id = ocr_fts.rowid
                WHERE ocr_fts MATCH ?1
                ORDER BY bm25(ocr_fts)
                LIMIT ?2
            "#;
            match conn.prepare(sql) {
                Ok(mut stmt) => {
                    let rows = stmt.query_map(params![fts_q, limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    });
                    if let Ok(rows) = rows {
                        let mut out = Vec::new();
                        let mut ok = true;
                        for r in rows {
                            match r {
                                Ok((eid, sid, ets, conf, text, snip)) => {
                                    match parse_uuid(eid) {
                                        Ok(event_id) => out.push(OcrSearchHit {
                                            event_id,
                                            session_id: sid
                                                .and_then(|s| Uuid::parse_str(&s).ok()),
                                            event_ts: ets.and_then(|s| {
                                                DateTime::parse_from_rfc3339(&s)
                                                    .ok()
                                                    .map(|d| d.with_timezone(&Utc))
                                            }),
                                            confidence: conf,
                                            snippet: snip,
                                            text_preview: preview_text(&text, 240),
                                        }),
                                        Err(_) => ok = false,
                                    }
                                }
                                Err(_) => ok = false,
                            }
                        }
                        if ok && !out.is_empty() {
                            return Ok(out);
                        }
                    }
                }
                Err(_) => { /* fall through to LIKE */ }
            }
        }

        // LIKE fallback (short CJK, FTS miss, or FTS unavailable).
        let Some(like) = like_q else {
            return Ok(vec![]);
        };
        let mut stmt = conn
            .prepare(
                r#"SELECT event_id, session_id, event_ts, confidence, text
                   FROM ocr_docs WHERE text LIKE ?1 ESCAPE '\'
                   ORDER BY updated_at DESC LIMIT ?2"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![like, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            let (eid, sid, ets, conf, text) = r.map_err(StoreError::db)?;
            out.push(OcrSearchHit {
                event_id: parse_uuid(eid)?,
                session_id: sid.and_then(|s| Uuid::parse_str(&s).ok()),
                event_ts: ets.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                confidence: conf,
                snippet: preview_text(&text, 120),
                text_preview: preview_text(&text, 240),
            });
        }
        Ok(out)
    }

    /// Rebuild ocr_docs/FTS from derived `ocr.v1` and `transcript.v1` rows.
    pub fn reindex_ocr_docs(&self) -> Result<usize, StoreError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        // Collect first so we never nest statements on the same connection.
        let derived: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare(
                    r#"SELECT event_id, body FROM derived
                       WHERE kind IN ('ocr.v1', 'transcript.v1', 'summary.v1')"#,
                )
                .map_err(StoreError::db)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(StoreError::db)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(StoreError::db)?);
            }
            out
        };

        let tx = conn.transaction().map_err(StoreError::db)?;
        tx.execute_batch("DELETE FROM ocr_docs;").map_err(StoreError::db)?;
        // Contentless/external FTS rebuild (ignore if FTS unavailable).
        let _ = tx.execute_batch("INSERT INTO ocr_fts(ocr_fts) VALUES('delete-all');");
        let mut n = 0usize;
        for (eid, body) in derived {
            let event_id = parse_uuid(eid)?;
            upsert_ocr_doc_tx(&tx, event_id, &body)?;
            n += 1;
        }
        tx.commit().map_err(StoreError::db)?;
        Ok(n)
    }

    pub fn ocr_doc_count(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let n: i64 = conn
            .query_row("SELECT COUNT(1) FROM ocr_docs", [], |r| r.get(0))
            .map_err(StoreError::db)?;
        Ok(n as usize)
    }

    pub fn has_derived(&self, event_id: Uuid, kind: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let n: i64 = conn
            .query_row(
                r#"SELECT COUNT(1) FROM derived WHERE event_id = ?1 AND kind = ?2"#,
                params![event_id.to_string(), kind],
                |r| r.get(0),
            )
            .map_err(StoreError::db)?;
        Ok(n > 0)
    }

    pub fn job_counts_by_status(&self, kind: &str) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT status, COUNT(1) FROM jobs WHERE kind = ?1 GROUP BY status"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(StoreError::db)?);
        }
        Ok(out)
    }

    pub fn list_derived_for_event(&self, event_id: Uuid) -> Result<Vec<(Uuid, String, String)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT id, kind, body FROM derived WHERE event_id = ?1 ORDER BY created_at ASC"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![event_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, kind, body) = r.map_err(StoreError::db)?;
            out.push((parse_uuid(id)?, kind, body));
        }
        Ok(out)
    }

    /// Events of one session joined with a derived doc kind, oldest first.
    ///
    /// Used by transcript export: `audio_chunk.v1` events + their
    /// `transcript.v1` bodies. Chunks without that derived doc (silence,
    /// pending/failed ASR) are not returned — the export timeline keeps a
    /// hole there instead.
    pub fn list_session_derived(
        &self,
        session_id: Uuid,
        event_kind: &str,
        derived_kind: &str,
    ) -> Result<Vec<SessionDerivedRow>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT e.id, e.ts, e.payload, d.body
                   FROM events e
                   JOIN derived d ON d.event_id = e.id AND d.kind = ?3
                   WHERE e.session_id = ?1 AND e.kind = ?2
                   ORDER BY e.ts ASC, e.rowid ASC"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(
                params![session_id.to_string(), event_kind, derived_kind],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, ts, payload, body) = r.map_err(StoreError::db)?;
            out.push(SessionDerivedRow {
                event_id: parse_uuid(id)?,
                ts: parse_ts(ts)?,
                payload: serde_json::from_str(&payload).unwrap_or(serde_json::json!({})),
                body,
            });
        }
        Ok(out)
    }

    /// Enriched timeline for product UI (newest first).
    pub fn list_timeline(&self, q: TimelineQuery) -> Result<Vec<TimelineItem>, StoreError> {
        let limit = q.limit.clamp(1, 500);
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut sql = String::from(
            r#"SELECT e.id, e.source, e.kind, e.ts, e.session_id, e.payload,
                      a.media_type, a.path, a.bytes,
                      d.kind AS dkind, d.body AS dbody
               FROM events e
               LEFT JOIN artifacts a ON a.event_id = e.id AND a.ordinal = 0
               LEFT JOIN derived d ON d.event_id = e.id
                 AND d.kind IN ('ocr.v1', 'transcript.v1', 'summary.v1')
               WHERE 1=1"#,
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(since) = q.since {
            sql.push_str(" AND e.ts >= ?");
            binds.push(Box::new(since.to_rfc3339()));
        }
        if let Some(until) = q.until {
            sql.push_str(" AND e.ts <= ?");
            binds.push(Box::new(until.to_rfc3339()));
        }
        if !q.kind_contains.trim().is_empty() {
            sql.push_str(" AND e.kind LIKE ?");
            binds.push(Box::new(format!("%{}%", q.kind_contains.trim())));
        }
        sql.push_str(" ORDER BY e.ts DESC, e.rowid DESC LIMIT ?");
        binds.push(Box::new(limit as i64));

        // Prefer ocr/transcript over picking arbitrary derived when multiple — query may return
        // multiple rows per event; collapse in Rust.
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            binds.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })
            .map_err(StoreError::db)?;

        use std::collections::HashMap;
        let mut by_id: HashMap<String, TimelineItem> = HashMap::new();
        let mut order: Vec<String> = Vec::new();

        for r in rows {
            let (
                id_s,
                source_json,
                kind,
                ts_s,
                session_s,
                payload_s,
                media,
                path,
                bytes,
                dkind,
                dbody,
            ) = r.map_err(StoreError::db)?;
            if !by_id.contains_key(&id_s) {
                order.push(id_s.clone());
                let source: String = serde_json::from_str(&source_json)
                    .map(|v: serde_json::Value| match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string().trim_matches('"').to_string(),
                    })
                    .unwrap_or(source_json);
                let payload: serde_json::Value =
                    serde_json::from_str(&payload_s).unwrap_or(serde_json::json!({}));
                let app_name = payload
                    .get("app_name")
                    .or_else(|| payload.get("app"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let window_title = payload
                    .get("window_title")
                    .or_else(|| payload.get("title"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                by_id.insert(
                    id_s.clone(),
                    TimelineItem {
                        id: parse_uuid(&id_s)?,
                        source,
                        kind,
                        ts: parse_ts(&ts_s)?,
                        session_id: session_s.and_then(|s| Uuid::parse_str(&s).ok()),
                        app_name,
                        window_title,
                        text_preview: None,
                        text_kind: None,
                        media_type: media,
                        artifact_path: path,
                        artifact_bytes: bytes.map(|b| b as u64),
                    },
                );
            }
            if let (Some(dk), Some(body)) = (dkind, dbody) {
                if let Some(item) = by_id.get_mut(&id_s) {
                    // Prefer ocr/transcript; summary only if nothing else.
                    let prefer = matches!(dk.as_str(), "ocr.v1" | "transcript.v1")
                        || item.text_preview.is_none();
                    if prefer {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                            if let Some(t) = v.get("text").and_then(|x| x.as_str()) {
                                let preview = preview_text(t, 280);
                                if !preview.is_empty() {
                                    item.text_preview = Some(preview);
                                    item.text_kind = Some(dk);
                                }
                            }
                        }
                    }
                }
            }
        }

        let app_filter = q.app_contains.trim().to_lowercase();
        let mut out: Vec<TimelineItem> = order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .filter(|item| {
                if app_filter.is_empty() {
                    return true;
                }
                let app = item.app_name.as_deref().unwrap_or("").to_lowercase();
                let title = item.window_title.as_deref().unwrap_or("").to_lowercase();
                let text = item.text_preview.as_deref().unwrap_or("").to_lowercase();
                app.contains(&app_filter) || title.contains(&app_filter) || text.contains(&app_filter)
            })
            .collect();
        // Already newest-first from SQL; re-sort after filter keep order
        Ok(out.drain(..).take(limit).collect())
    }

    /// Absolute path for a relative artifact path under data_dir.
    pub fn resolve_artifact_path(&self, relative: &str) -> std::path::PathBuf {
        self.data_dir.join(relative)
    }

    /// Build a simple day summary from stored events (rule-based, no LLM).
    pub fn build_day_summary_body(&self, day: &str) -> Result<String, StoreError> {
        // day = YYYY-MM-DD
        let since = format!("{day}T00:00:00+00:00");
        let until = format!("{day}T23:59:59.999999999+00:00");
        let since_dt = DateTime::parse_from_rfc3339(&since)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| StoreError::Other(format!("day: {e}")))?;
        let until_dt = DateTime::parse_from_rfc3339(&until)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| StoreError::Other(format!("day: {e}")))?;
        let items = self.list_timeline(TimelineQuery {
            limit: 500,
            since: Some(since_dt),
            until: Some(until_dt),
            ..Default::default()
        })?;
        let mut shots = 0usize;
        let mut audio = 0usize;
        let mut samples: Vec<String> = Vec::new();
        for it in &items {
            if it.kind.contains("screenshot") {
                shots += 1;
            }
            if it.kind.contains("audio") {
                audio += 1;
            }
            if let Some(t) = &it.text_preview {
                if samples.len() < 8 && t.chars().count() > 8 {
                    samples.push(t.clone());
                }
            }
        }

        // Top apps — prefer duration from the activity projection (accurate);
        // fall back to event-count when no segments exist yet (e.g. observe
        // was off all day). The projection is the time-tracking source of
        // truth, the count is a coarse fallback.
        let top_apps: Vec<String> = {
            let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
            let mut stmt = conn
                .prepare(
                    r#"SELECT app_name, SUM(duration_ms) AS ms, COUNT(1) AS segs
                       FROM activity_segments
                       WHERE day = ?1 AND is_idle = 0 AND app_name IS NOT NULL
                       GROUP BY app_name
                       ORDER BY ms DESC
                       LIMIT 8"#,
                )
                .map_err(StoreError::db)?;
            let rows = stmt
                .query_map(params![day], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        row.get::<_, i64>(1)?,
                    ))
                })
                .map_err(StoreError::db)?;
            let mut by_duration: Vec<(String, i64)> = Vec::new();
            for r in rows {
                by_duration.push(r.map_err(StoreError::db)?);
            }
            drop(stmt);
            drop(conn);

            if by_duration.is_empty() {
                // Fallback: event-count based (pre-activity-projection behavior).
                let mut apps: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for it in &items {
                    if let Some(app) = &it.app_name {
                        *apps.entry(app.clone()).or_default() += 1;
                    }
                }
                let mut v: Vec<_> = apps.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                v.into_iter()
                    .take(8)
                    .map(|(k, n)| format!("{k} ({n} events)"))
                    .collect()
            } else {
                by_duration
                    .into_iter()
                    .map(|(k, ms)| format!("{k} ({})", fmt_ms_compact(ms)))
                    .collect()
            }
        };

        let text = format!(
            "Day {day}\nEvents: {}\nScreenshots: {shots}\nAudio chunks: {audio}\nTop apps (by duration): {}\n\nText samples:\n{}",
            items.len(),
            if top_apps.is_empty() {
                "—".into()
            } else {
                top_apps.join(", ")
            },
            if samples.is_empty() {
                "—".into()
            } else {
                samples
                    .into_iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        );
        Ok(serde_json::json!({
            "payload_version": 1,
            "kind": "day",
            "day": day,
            "text": text,
            "event_count": items.len(),
            "screenshots": shots,
            "audio_chunks": audio,
        })
        .to_string())
    }

    /// List all activity segments for one day (local-day bucket), ordered by
    /// start time. Returns the dashboard's timeline data.
    pub fn list_activity_segments(&self, day: &str) -> Result<Vec<ActivitySegmentDto>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT seg_id, day, app_name, bundle_id, window_title,
                          started_at, ended_at, duration_ms, is_idle, is_locked,
                          category, productivity_level, event_count, source
                   FROM activity_segments
                   WHERE day = ?1
                   ORDER BY started_at ASC"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![day], |row| {
                let started: String = row.get(5)?;
                let started_at = chrono::DateTime::parse_from_rfc3339(&started)
                    .map(|d| d.with_timezone(&Utc))
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    ))?;
                let ended: Option<String> = row.get(6)?;
                let ended_at = ended
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc));
                Ok(ActivitySegmentDto {
                    seg_id: row.get(0)?,
                    day: row.get(1)?,
                    app_name: row.get(2)?,
                    bundle_id: row.get(3)?,
                    window_title: row.get(4)?,
                    started_at,
                    ended_at,
                    duration_ms: row.get(7)?,
                    is_idle: row.get::<_, i64>(8)? != 0,
                    is_locked: row.get::<_, i64>(9)? != 0,
                    category: row.get(10)?,
                    productivity_level: row.get(11)?,
                    event_count: row.get(12)?,
                    source: row.get::<_, Option<String>>(13)?.unwrap_or_else(|| "auto".into()),
                })
            })
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(StoreError::db)?);
        }
        Ok(out)
    }

    /// Aggregated stats for one day — feeds the dashboard's stat cards, hour
    /// distribution chart, category breakdown, and top-apps ranking.
    pub fn activity_day_stats(&self, day: &str) -> Result<DayStatsDto, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;

        // Active/idle totals + context switches (count of active segments).
        let (total_active_ms, total_idle_ms, context_switches): (i64, i64, i64) = conn
            .query_row(
                r#"SELECT
                       COALESCE(SUM(CASE WHEN is_idle = 0 THEN duration_ms ELSE 0 END), 0),
                       COALESCE(SUM(CASE WHEN is_idle = 1 THEN duration_ms ELSE 0 END), 0),
                       COUNT(CASE WHEN is_idle = 0 THEN 1 END)
                   FROM activity_segments WHERE day = ?1"#,
                params![day],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(StoreError::db)?;

        // Pulse score: weighted average of classified active segments only.
        let (weighted_sum, classified_ms): (f64, f64) = conn
            .query_row(
                r#"SELECT
                       COALESCE(SUM(CASE productivity_level
                           WHEN 'productive' THEN duration_ms
                           WHEN 'neutral' THEN duration_ms * 0.5
                           WHEN 'distracting' THEN 0
                           ELSE 0 END), 0.0),
                       SUM(CASE WHEN productivity_level IS NOT NULL THEN duration_ms ELSE 0 END)
                   FROM activity_segments
                   WHERE day = ?1 AND is_idle = 0"#,
                params![day],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
            )
            .map_err(StoreError::db)?;
        let pulse_score = if classified_ms > 0.0 {
            Some(100.0 * weighted_sum / classified_ms)
        } else {
            None
        };

        // By category.
        let mut cat_stmt = conn
            .prepare(
                r#"SELECT COALESCE(category, 'Uncategorized'), productivity_level,
                          SUM(duration_ms)
                   FROM activity_segments
                   WHERE day = ?1 AND is_idle = 0
                   GROUP BY COALESCE(category, 'Uncategorized'), productivity_level
                   ORDER BY SUM(duration_ms) DESC"#,
            )
            .map_err(StoreError::db)?;
        let cat_rows = cat_stmt
            .query_map(params![day], |row| {
                Ok(CategoryTotal {
                    category: row.get(0)?,
                    productivity_level: row.get(1)?,
                    ms: row.get(2)?,
                })
            })
            .map_err(StoreError::db)?;
        let mut by_category = Vec::new();
        for r in cat_rows {
            by_category.push(r.map_err(StoreError::db)?);
        }

        // Top apps — group by bundle identity so "Lumen Navi" and
        // "lumen-navi-desktop" collapse; prefer a human display name.
        let mut app_stmt = conn
            .prepare(
                r#"SELECT GROUP_CONCAT(DISTINCT app_name, char(31)),
                          bundle_id, SUM(duration_ms),
                          MAX(category), MAX(productivity_level), COUNT(1)
                   FROM activity_segments
                   WHERE day = ?1 AND is_idle = 0 AND app_name IS NOT NULL
                   GROUP BY COALESCE(bundle_id, app_name)
                   ORDER BY SUM(duration_ms) DESC
                   LIMIT 20"#,
            )
            .map_err(StoreError::db)?;
        let app_rows = app_stmt
            .query_map(params![day], |row| {
                Ok(AppTotal {
                    app_name: preferred_name_from_concat(
                        &row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    ),
                    bundle_id: row.get(1)?,
                    ms: row.get(2)?,
                    category: row.get(3)?,
                    productivity_level: row.get(4)?,
                    segment_count: row.get(5)?,
                })
            })
            .map_err(StoreError::db)?;
        let mut top_apps = Vec::new();
        for r in app_rows {
            top_apps.push(r.map_err(StoreError::db)?);
        }

        // Per-hour distribution (local-hour buckets from started_at).
        let mut hour_stmt = conn
            .prepare(
                r#"SELECT started_at, duration_ms, is_idle FROM activity_segments
                   WHERE day = ?1"#,
            )
            .map_err(StoreError::db)?;
        let hour_rows = hour_stmt
            .query_map(params![day], |row| {
                let ts: String = row.get(0)?;
                let ms: i64 = row.get(1)?;
                let is_idle: bool = row.get::<_, i64>(2)? != 0;
                Ok((ts, ms, is_idle))
            })
            .map_err(StoreError::db)?;
        let mut by_hour = [0i64; 24];
        for r in hour_rows {
            let (ts, ms, is_idle) = r.map_err(StoreError::db)?;
            if is_idle {
                continue;
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts) {
                let hour = dt.with_timezone(&chrono::Local).format("%H").to_string();
                if let Ok(h) = hour.parse::<usize>() {
                    if h < 24 {
                        by_hour[h] = by_hour[h].saturating_add(ms);
                    }
                }
            }
        }

        Ok(DayStatsDto {
            day: day.to_string(),
            total_active_ms,
            total_idle_ms,
            pulse_score,
            context_switches,
            by_category,
            top_apps,
            by_hour,
        })
    }

    /// Aggregate activity across a date range `[from_day, to_day]` inclusive
    /// (YYYY-MM-DD). Returns a per-day rollup plus range-wide totals, top apps,
    /// and category breakdown — the weekly-view payload.
    pub fn activity_range_stats(
        &self,
        from_day: &str,
        to_day: &str,
    ) -> Result<RangeStatsDto, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;

        // Range totals + pulse.
        let (total_active_ms, total_idle_ms): (i64, i64) = conn
            .query_row(
                r#"SELECT
                       COALESCE(SUM(CASE WHEN is_idle = 0 THEN duration_ms ELSE 0 END), 0),
                       COALESCE(SUM(CASE WHEN is_idle = 1 THEN duration_ms ELSE 0 END), 0)
                   FROM activity_segments WHERE day BETWEEN ?1 AND ?2"#,
                params![from_day, to_day],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(StoreError::db)?;

        let (weighted_sum, classified_ms): (f64, f64) = conn
            .query_row(
                r#"SELECT
                       COALESCE(SUM(CASE productivity_level
                           WHEN 'productive' THEN duration_ms
                           WHEN 'neutral' THEN duration_ms * 0.5
                           WHEN 'distracting' THEN 0
                           ELSE 0 END), 0.0),
                       SUM(CASE WHEN productivity_level IS NOT NULL THEN duration_ms ELSE 0 END)
                   FROM activity_segments
                   WHERE day BETWEEN ?1 AND ?2 AND is_idle = 0"#,
                params![from_day, to_day],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
            )
            .map_err(StoreError::db)?;
        let pulse_score = if classified_ms > 0.0 {
            Some(100.0 * weighted_sum / classified_ms)
        } else {
            None
        };

        // Per-day rollups.
        let mut day_stmt = conn
            .prepare(
                r#"SELECT day,
                          COALESCE(SUM(CASE WHEN is_idle = 0 THEN duration_ms ELSE 0 END), 0),
                          COALESCE(SUM(CASE WHEN is_idle = 1 THEN duration_ms ELSE 0 END), 0),
                          COUNT(CASE WHEN is_idle = 0 THEN 1 END)
                   FROM activity_segments
                   WHERE day BETWEEN ?1 AND ?2
                   GROUP BY day
                   ORDER BY day ASC"#,
            )
            .map_err(StoreError::db)?;
        let day_rows = day_stmt
            .query_map(params![from_day, to_day], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(StoreError::db)?;

        let mut days: Vec<DayRollupDto> = Vec::new();
        // Collect each day's category breakdown in a follow-up query for efficiency.
        let mut cat_stmt = conn
            .prepare(
                r#"SELECT day, COALESCE(category, 'Uncategorized'), productivity_level,
                          SUM(duration_ms)
                   FROM activity_segments
                   WHERE day BETWEEN ?1 AND ?2 AND is_idle = 0
                   GROUP BY day, COALESCE(category, 'Uncategorized'), productivity_level"#,
            )
            .map_err(StoreError::db)?;
        let cat_rows: std::result::Result<Vec<_>, _> = cat_stmt
            .query_map(params![from_day, to_day], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(StoreError::db)?
            .collect();
        let cat_rows = cat_rows.map_err(StoreError::db)?;
        drop(cat_stmt);

        // Bucket categories by day.
        use std::collections::BTreeMap;
        let mut cats_by_day: BTreeMap<String, Vec<CategoryTotal>> = BTreeMap::new();
        for (day, category, level, ms) in cat_rows {
            cats_by_day
                .entry(day)
                .or_default()
                .push(CategoryTotal { category, productivity_level: level, ms });
        }
        for v in cats_by_day.values_mut() {
            v.sort_by(|a, b| b.ms.cmp(&a.ms));
        }

        for r in day_rows {
            let (day, active, idle, switches) = r.map_err(StoreError::db)?;
            // Per-day pulse.
            let day_cats = cats_by_day.get(&day).cloned().unwrap_or_default();
            let day_pulse = {
                let (w, c): (f64, f64) = day_cats
                    .iter()
                    .filter(|ct| ct.productivity_level.is_some())
                    .fold((0.0, 0.0), |(w, c), ct| {
                        let weight = match ct.productivity_level.as_deref() {
                            Some("productive") => 100.0,
                            Some("neutral") => 50.0,
                            Some("distracting") => 0.0,
                            _ => return (w, c),
                        };
                        (w + weight * ct.ms as f64, c + ct.ms as f64)
                    });
                if c > 0.0 { Some(100.0 * w / c) } else { None }
            };
            days.push(DayRollupDto {
                day,
                total_active_ms: active,
                total_idle_ms: idle,
                pulse_score: day_pulse,
                context_switches: switches,
                by_category: day_cats,
            });
        }
        drop(day_stmt);

        // Range-wide top apps (bundle identity + preferred display name).
        let mut app_stmt = conn
            .prepare(
                r#"SELECT GROUP_CONCAT(DISTINCT app_name, char(31)),
                          bundle_id, SUM(duration_ms),
                          MAX(category), MAX(productivity_level), COUNT(1)
                   FROM activity_segments
                   WHERE day BETWEEN ?1 AND ?2 AND is_idle = 0 AND app_name IS NOT NULL
                   GROUP BY COALESCE(bundle_id, app_name)
                   ORDER BY SUM(duration_ms) DESC
                   LIMIT 15"#,
            )
            .map_err(StoreError::db)?;
        let app_rows = app_stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(AppTotal {
                    app_name: preferred_name_from_concat(
                        &row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    ),
                    bundle_id: row.get(1)?,
                    ms: row.get(2)?,
                    category: row.get(3)?,
                    productivity_level: row.get(4)?,
                    segment_count: row.get(5)?,
                })
            })
            .map_err(StoreError::db)?;
        let mut top_apps = Vec::new();
        for r in app_rows {
            top_apps.push(r.map_err(StoreError::db)?);
        }

        // Range-wide category breakdown.
        let mut rcat_stmt = conn
            .prepare(
                r#"SELECT COALESCE(category, 'Uncategorized'), productivity_level,
                          SUM(duration_ms)
                   FROM activity_segments
                   WHERE day BETWEEN ?1 AND ?2 AND is_idle = 0
                   GROUP BY COALESCE(category, 'Uncategorized'), productivity_level
                   ORDER BY SUM(duration_ms) DESC"#,
            )
            .map_err(StoreError::db)?;
        let rcat_rows = rcat_stmt
            .query_map(params![from_day, to_day], |row| {
                Ok(CategoryTotal {
                    category: row.get(0)?,
                    productivity_level: row.get(1)?,
                    ms: row.get(2)?,
                })
            })
            .map_err(StoreError::db)?;
        let mut by_category = Vec::new();
        for r in rcat_rows {
            by_category.push(r.map_err(StoreError::db)?);
        }

        Ok(RangeStatsDto {
            days,
            total_active_ms,
            total_idle_ms,
            pulse_score,
            top_apps,
            by_category,
        })
    }

    /// Add a manually-entered activity segment (the retro-entry feature —
    /// "I was actually in a meeting 10:00–10:30"). Inserts directly into
    /// `activity_segments` with `source='manual'`, is_idle=false, and applies
    /// the current classification rules. Returns the new seg_id.
    pub fn add_manual_segment(
        &self,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        app_name: &str,
        window_title: Option<&str>,
        category: Option<&str>,
        productivity_level: Option<&str>,
    ) -> Result<String, StoreError> {
        let ts_str = started_at.to_rfc3339();
        let day = started_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        let duration_ms = (ended_at - started_at).num_milliseconds().max(0) as i64;
        let seg_id = blake3::hash(
            format!("manual|{day}|{ts_str}|{app_name}").as_bytes(),
        )
        .to_hex()
        .to_string();
        let now = Utc::now().to_rfc3339();

        // Classify unless the caller pinned a category explicitly.
        let (eff_cat, eff_level): (Option<String>, Option<String>) = match (category, productivity_level) {
            (Some(c), l) => (Some(c.to_string()), l.map(str::to_string)),
            (None, _) => {
                let user_rules = {
                    let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
                    let raw: Option<String> = conn
                        .query_row(
                            "SELECT value FROM kv WHERE key = 'activity.category_rules'",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(StoreError::db)?;
                    drop(conn);
                    match raw {
                        Some(json) => serde_json::from_str::<Vec<CategoryRule>>(&json)
                            .unwrap_or_default(),
                        None => Vec::new(),
                    }
                };
                let c = crate::categorization::classify(
                    &ActivityFields {
                        bundle_id: None,
                        app_name: Some(app_name),
                        window_title,
                        url: None,
                        ls_category_type: None,
                    },
                    &user_rules,
                );
                (
                    c.category,
                    c.level.map(productivity_level_str).map(str::to_string),
                )
            }
        };

        let mut conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;
        tx.execute(
            r#"INSERT OR IGNORE INTO activity_segments
               (seg_id, day, app_name, bundle_id, window_title, url,
                started_at, ended_at, duration_ms, is_idle, is_locked,
                category, project, productivity_level, event_count, updated_at, source)
               VALUES (?1, ?2, ?3, NULL, ?4, NULL, ?5, ?6, ?7, 0, 0, ?8, NULL, ?9, 1, ?10, 'manual')"#,
            params![
                seg_id,
                day,
                app_name,
                window_title,
                ts_str,
                ended_at.to_rfc3339(),
                duration_ms,
                eff_cat.as_deref(),
                eff_level.as_deref(),
                now,
            ],
        )
        .map_err(StoreError::db)?;
        tx.commit().map_err(StoreError::db)?;
        Ok(seg_id)
    }

    /// Delete a manual segment by seg_id (only manual entries are deletable;
    /// auto-tracked segments are regenerated from events and not user-removable).
    pub fn delete_manual_segment(&self, seg_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let n = conn
            .execute(
                "DELETE FROM activity_segments WHERE seg_id = ?1 AND source = 'manual'",
                params![seg_id],
            )
            .map_err(StoreError::db)?;
        if n == 0 {
            return Err(StoreError::Other(
                "segment not found or not manually entered".into(),
            ));
        }
        Ok(())
    }

    /// Get the current user-defined category rules (from the `kv` table).
    pub fn list_category_rules(&self) -> Result<Vec<CategoryRule>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM kv WHERE key = 'activity.category_rules'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::db)?;
        match raw {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| StoreError::Other(format!("parse category rules: {e}"))),
            None => Ok(Vec::new()),
        }
    }

    /// Save the full user-defined rule list (replaces existing) and re-apply
    /// categorization to all segments so historical stats reflect the new rules.
    pub fn save_category_rules_and_reapply(
        &self,
        rules: Vec<CategoryRule>,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string(&rules)
            .map_err(|e| StoreError::Other(format!("serialize category rules: {e}")))?;

        let mut conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;

        // Upsert the rule list.
        tx.execute(
            r#"INSERT INTO kv (key, value) VALUES ('activity.category_rules', ?1)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value"#,
            params![json],
        )
        .map_err(StoreError::db)?;

        // Re-classify every segment: load its fields, re-run classify, update.
        let mut stmt = tx
            .prepare(
                r#"SELECT seg_id, app_name, bundle_id, window_title FROM activity_segments"#,
            )
            .map_err(StoreError::db)?;
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(StoreError::db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::db)?;
        drop(stmt);

        for (seg_id, app_name, bundle_id, window_title) in rows {
            let fields = ActivityFields {
                app_name: app_name.as_deref(),
                bundle_id: bundle_id.as_deref(),
                window_title: window_title.as_deref(),
                url: None,
                // Historical segments don't store LSApplicationCategoryType;
                // re-apply still benefits from expanded default rules + lumen family.
                ls_category_type: None,
            };
            let c = crate::categorization::classify(&fields, &rules);
            let level = c.level.map(productivity_level_str);
            tx.execute(
                r#"UPDATE activity_segments
                   SET category = ?1, productivity_level = ?2
                   WHERE seg_id = ?3"#,
                params![c.category.as_deref(), level.as_deref(), seg_id],
            )
            .map_err(StoreError::db)?;
        }

        tx.commit().map_err(StoreError::db)?;
        Ok(())
    }

    /// Load first artifact bytes for an event (relative path under data_dir).
    pub fn load_first_artifact_bytes(&self, event_id: Uuid) -> Result<Option<(String, Vec<u8>)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let row = conn
            .query_row(
                r#"SELECT media_type, path FROM artifacts WHERE event_id = ?1 ORDER BY ordinal ASC LIMIT 1"#,
                params![event_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StoreError::db)?;
        let Some((media, rel)) = row else {
            return Ok(None);
        };
        drop(conn);
        let bytes = self.blobs.read_relative(&rel)?;
        Ok(Some((media, bytes)))
    }

    pub fn list_jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT id, event_id, kind, status, attempts, last_error, updated_at, available_at
                   FROM jobs ORDER BY updated_at DESC LIMIT ?1"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let available_at = row
                    .get::<_, Option<String>>(7)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                Ok(JobRecord {
                    id: parse_uuid(row.get::<_, String>(0)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    event_id: parse_uuid(row.get::<_, String>(1)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    kind: row.get(2)?,
                    status: JobStatus::parse(&row.get::<_, String>(3)?),
                    attempts: row.get(4)?,
                    last_error: row.get(5)?,
                    updated_at: parse_ts(row.get::<_, String>(6)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    available_at,
                })
            })
            .map_err(StoreError::db)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(StoreError::db)?);
        }
        Ok(out)
    }

    fn append_sync(&self, events: &[SourceEvent]) -> Result<(), StoreError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let tx = conn.transaction().map_err(StoreError::db)?;
        for event in events {
            insert_event(&tx, event)?;
        }
        tx.commit().map_err(StoreError::db)?;
        Ok(())
    }

    fn list_recent_sync(&self, limit: usize) -> Result<Vec<SourceEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT id, source, kind, ts, session_id, payload
                   FROM events ORDER BY ts DESC, rowid DESC LIMIT ?1"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(EventRow {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    kind: row.get(2)?,
                    ts: row.get(3)?,
                    session_id: row.get(4)?,
                    payload: row.get(5)?,
                })
            })
            .map_err(StoreError::db)?;

        let mut events = Vec::new();
        for row in rows {
            let row = row.map_err(StoreError::db)?;
            let mut event = row_to_event(row)?;
            event.artifacts = load_artifacts(&conn, event.id)?;
            events.push(event);
        }
        // list_recent historically returned chronological order (oldest→newest among the window)
        events.reverse();
        Ok(events)
    }

    fn get_sync(&self, id: Uuid) -> Result<Option<SourceEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let row = conn
            .query_row(
                r#"SELECT id, source, kind, ts, session_id, payload FROM events WHERE id = ?1"#,
                params![id.to_string()],
                |row| {
                    Ok(EventRow {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        kind: row.get(2)?,
                        ts: row.get(3)?,
                        session_id: row.get(4)?,
                        payload: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::db)?;

        match row {
            None => Ok(None),
            Some(row) => {
                let mut event = row_to_event(row)?;
                event.artifacts = load_artifacts(&conn, event.id)?;
                Ok(Some(event))
            }
        }
    }

    fn wipe_sync(&self) -> Result<(), StoreError> {
        let _blob_guard = self
            .blob_intake
            .lock()
            .map_err(|_| StoreError::Other("blob intake lock poisoned".into()))?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".into()))?;
        conn.execute_batch(
            r#"
                DELETE FROM ocr_docs;
                DELETE FROM browser_visits;
                DELETE FROM derived;
                DELETE FROM jobs;
                DELETE FROM artifacts;
                DELETE FROM events;
                DELETE FROM kv;
                "#,
        )
        .map_err(StoreError::db)?;
        self.blobs.wipe_all()?;
        Ok(())
    }

    fn len_sync(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .map_err(StoreError::db)?;
        Ok(n as usize)
    }
}

#[async_trait]
impl EventStore for SqliteStore {
    async fn append(&self, events: Vec<SourceEvent>) -> Result<(), StoreError> {
        self.append_sync(&events)
    }

    async fn list_recent(&self, limit: usize) -> Result<Vec<SourceEvent>, StoreError> {
        self.list_recent_sync(limit)
    }

    async fn get(&self, id: Uuid) -> Result<Option<SourceEvent>, StoreError> {
        self.get_sync(id)
    }

    async fn wipe_all(&self) -> Result<(), StoreError> {
        self.wipe_sync()
    }

    async fn len(&self) -> Result<usize, StoreError> {
        self.len_sync()
    }
}

struct EventRow {
    id: String,
    source: String,
    kind: String,
    ts: String,
    session_id: Option<String>,
    payload: String,
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(MIGRATE_V1).map_err(StoreError::db)?;
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::db)?;

    let mut v: i64 = current.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    if current.is_none() {
        // Fresh DB after V1 tables: stamp as 1 then upgrade.
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('version', '1')",
            [],
        )
        .map_err(StoreError::db)?;
        v = 1;
    }

    if v > SCHEMA_VERSION {
        return Err(StoreError::Other(format!(
            "database schema version {v} is newer than supported {SCHEMA_VERSION}"
        )));
    }

    if v < 2 {
        conn.execute_batch(MIGRATE_V2).map_err(StoreError::db)?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["2"],
        )
        .map_err(StoreError::db)?;
        v = 2;
    }

    if v < 3 {
        let _ = conn.execute("ALTER TABLE jobs ADD COLUMN available_at TEXT", []);
        let _ = conn.execute("ALTER TABLE jobs ADD COLUMN created_at TEXT", []);
        let _ = conn.execute_batch(
            "UPDATE jobs SET available_at = updated_at WHERE available_at IS NULL;
             UPDATE jobs SET created_at = updated_at WHERE created_at IS NULL;",
        );
        // Keep newest open job per (event_id, kind); mark older open as dead.
        let _ = conn.execute_batch(
            r#"
            UPDATE jobs SET status = 'dead', last_error = 'deduped at schema v3'
            WHERE status IN ('pending', 'running')
              AND id NOT IN (
                SELECT id FROM (
                  SELECT id,
                         ROW_NUMBER() OVER (
                           PARTITION BY event_id, kind
                           ORDER BY updated_at DESC, rowid DESC
                         ) AS rn
                  FROM jobs
                  WHERE status IN ('pending', 'running')
                ) WHERE rn = 1
              );
            "#,
        );
        // Fallback if window functions unavailable: delete extras via group (sqlite 3.25+)
        // If above failed, ignore — try unique index.
        let _ = conn.execute_batch(
            r#"
            DELETE FROM derived WHERE id NOT IN (
              SELECT id FROM (
                SELECT id, ROW_NUMBER() OVER (
                  PARTITION BY event_id, kind ORDER BY created_at DESC, rowid DESC
                ) rn FROM derived
              ) WHERE rn = 1
            );
            "#,
        );
        conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_jobs_claim
              ON jobs(kind, status, available_at, updated_at);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_open_ocr
              ON jobs(event_id, kind)
              WHERE status IN ('pending', 'running');
            CREATE UNIQUE INDEX IF NOT EXISTS idx_derived_event_kind
              ON derived(event_id, kind);
            "#,
        )
        .map_err(StoreError::db)?;
        let _ = MIGRATE_V3;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["3"],
        )
        .map_err(StoreError::db)?;
        v = 3;
    }

    if v < 4 {
        conn.execute_batch(MIGRATE_V4).map_err(StoreError::db)?;
        // FTS5: try trigram, fall back to unicode61.
        let fts = conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS ocr_fts USING fts5(
              text,
              content='ocr_docs',
              content_rowid='id',
              tokenize='trigram'
            );
            "#,
        );
        if fts.is_err() {
            conn.execute_batch(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS ocr_fts USING fts5(
                  text,
                  content='ocr_docs',
                  content_rowid='id',
                  tokenize='unicode61'
                );
                "#,
            )
            .map_err(StoreError::db)?;
        }
        conn.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS ocr_docs_ai AFTER INSERT ON ocr_docs BEGIN
              INSERT INTO ocr_fts(rowid, text) VALUES (new.id, new.text);
            END;
            CREATE TRIGGER IF NOT EXISTS ocr_docs_ad AFTER DELETE ON ocr_docs BEGIN
              INSERT INTO ocr_fts(ocr_fts, rowid, text) VALUES('delete', old.id, old.text);
            END;
            CREATE TRIGGER IF NOT EXISTS ocr_docs_au AFTER UPDATE ON ocr_docs BEGIN
              INSERT INTO ocr_fts(ocr_fts, rowid, text) VALUES('delete', old.id, old.text);
              INSERT INTO ocr_fts(rowid, text) VALUES (new.id, new.text);
            END;
            "#,
        )
        .map_err(StoreError::db)?;
        // Backfill from existing OCR + transcripts (collect first — no nested statements).
        let derived: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare(
                    r#"SELECT event_id, body FROM derived
                       WHERE kind IN ('ocr.v1', 'transcript.v1')"#,
                )
                .map_err(StoreError::db)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(StoreError::db)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(StoreError::db)?);
            }
            out
        };
        for (eid, body) in derived {
            if let Ok(event_id) = Uuid::parse_str(&eid) {
                let _ = upsert_ocr_doc_conn(conn, event_id, &body);
            }
        }
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["4"],
        )
        .map_err(StoreError::db)?;
        v = 4;
    }

    if v < 5 {
        conn.execute_batch(MIGRATE_V5).map_err(StoreError::db)?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["5"],
        )
        .map_err(StoreError::db)?;
        v = 5;
    }

    if v < 6 {
        conn.execute_batch(MIGRATE_V6).map_err(StoreError::db)?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["6"],
        )
        .map_err(StoreError::db)?;
        v = 6;
    }

    if v < 7 {
        conn.execute_batch(MIGRATE_V7).map_err(StoreError::db)?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["7"],
        )
        .map_err(StoreError::db)?;
        v = 7;
    }

    if v < 8 {
        conn.execute_batch(MIGRATE_V8).map_err(StoreError::db)?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'version'",
            params!["8"],
        )
        .map_err(StoreError::db)?;
        v = 8;
    }

    let _ = v;
    Ok(())
}

fn upsert_ocr_doc_tx(
    tx: &rusqlite::Transaction<'_>,
    event_id: Uuid,
    body_json: &str,
) -> Result<(), StoreError> {
    let (text, confidence, session_id, event_ts) = parse_ocr_body(body_json, event_id)?;
    // Enrich session/ts from events table when missing.
    let (session_id, event_ts) = {
        let row = tx
            .query_row(
                r#"SELECT session_id, ts FROM events WHERE id = ?1"#,
                params![event_id.to_string()],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(StoreError::db)?;
        match row {
            Some((s, t)) => (session_id.or(s), event_ts.or(t)),
            None => (session_id, event_ts),
        }
    };
    let now = Utc::now().to_rfc3339();
    tx.execute(
        r#"INSERT INTO ocr_docs (event_id, text, confidence, session_id, event_ts, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
           ON CONFLICT(event_id) DO UPDATE SET
             text=excluded.text,
             confidence=excluded.confidence,
             session_id=excluded.session_id,
             event_ts=excluded.event_ts,
             updated_at=excluded.updated_at"#,
        params![
            event_id.to_string(),
            text,
            confidence,
            session_id,
            event_ts,
            now
        ],
    )
    .map_err(StoreError::db)?;
    Ok(())
}

fn upsert_ocr_doc_conn(
    conn: &Connection,
    event_id: Uuid,
    body_json: &str,
) -> Result<(), StoreError> {
    let (text, confidence, session_id, event_ts) = parse_ocr_body(body_json, event_id)?;
    let (session_id, event_ts) = {
        let row = conn
            .query_row(
                r#"SELECT session_id, ts FROM events WHERE id = ?1"#,
                params![event_id.to_string()],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(StoreError::db)?;
        match row {
            Some((s, t)) => (session_id.or(s), event_ts.or(t)),
            None => (session_id, event_ts),
        }
    };
    let now = Utc::now().to_rfc3339();
    conn.execute(
        r#"INSERT INTO ocr_docs (event_id, text, confidence, session_id, event_ts, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
           ON CONFLICT(event_id) DO UPDATE SET
             text=excluded.text,
             confidence=excluded.confidence,
             session_id=excluded.session_id,
             event_ts=excluded.event_ts,
             updated_at=excluded.updated_at"#,
        params![
            event_id.to_string(),
            text,
            confidence,
            session_id,
            event_ts,
            now
        ],
    )
    .map_err(StoreError::db)?;
    Ok(())
}

fn parse_ocr_body(
    body_json: &str,
    event_id: Uuid,
) -> Result<(String, f64, Option<String>, Option<String>), StoreError> {
    let v: serde_json::Value =
        serde_json::from_str(body_json).map_err(|e| StoreError::json(e.to_string()))?;
    let text = v
        .get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let confidence = v.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.0);
    // session/event_ts may not be in body — filled from events table.
    let _ = event_id;
    Ok((text, confidence, None, None))
}

/// FTS5 query sanitizer: keep letters/numbers/CJK; join with spaces (AND).
/// Drops tokens shorter than 3 chars (trigram tokenizer minimum).
fn sanitize_fts_query(raw: &str) -> String {
    let mut parts = Vec::new();
    let mut cur = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() || is_cjk(ch) {
            cur.push(ch);
        } else if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
        .into_iter()
        .filter(|p| p.chars().count() >= 3)
        .map(|p| format!("\"{}\"", p.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4e00}'..='\u{9fff}' // CJK Unified
            | '\u{3400}'..='\u{4dbf}' // Extension A
            | '\u{f900}'..='\u{faff}' // Compatibility
            | '\u{3000}'..='\u{303f}' // CJK punctuation (rarely searched)
    )
}

/// Escape LIKE wildcards; return None if nothing searchable remains.
fn like_pattern(raw: &str) -> Option<String> {
    let trimmed: String = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if trimmed.is_empty() {
        return None;
    }
    let mut esc = String::with_capacity(trimmed.len() + 2);
    for ch in trimmed.chars() {
        match ch {
            '%' | '_' | '\\' => {
                esc.push('\\');
                esc.push(ch);
            }
            _ => esc.push(ch),
        }
    }
    if esc.is_empty() {
        None
    } else {
        Some(format!("%{esc}%"))
    }
}

fn preview_text(s: &str, max: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        t
    } else {
        t.chars().take(max).collect::<String>() + "…"
    }
}

fn insert_event(tx: &rusqlite::Transaction<'_>, event: &SourceEvent) -> Result<(), StoreError> {
    insert_event_with_mode(tx, event, false).map(|_| ())
}

fn insert_event_idempotent(
    tx: &rusqlite::Transaction<'_>,
    event: &SourceEvent,
) -> Result<bool, StoreError> {
    insert_event_with_mode(tx, event, true)
}

fn insert_event_with_mode(
    tx: &rusqlite::Transaction<'_>,
    event: &SourceEvent,
    idempotent: bool,
) -> Result<bool, StoreError> {
    let source = serde_json::to_string(&event.source).map_err(StoreError::json)?;
    let payload = serde_json::to_string(&event.payload).map_err(StoreError::json)?;
    let session = event.session_id.map(|s| s.to_string());
    let created = Utc::now().to_rfc3339();

    let statement = if idempotent {
        r#"INSERT OR IGNORE INTO events (id, source, kind, ts, session_id, payload, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#
    } else {
        r#"INSERT INTO events (id, source, kind, ts, session_id, payload, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#
    };
    let inserted = tx.execute(
        statement,
        params![
            event.id.to_string(),
            source,
            event.kind,
            event.ts.to_rfc3339(),
            session,
            payload,
            created,
        ],
    )
    .map_err(StoreError::db)?;
    if inserted == 0 {
        return Ok(false);
    }

    for (ordinal, art) in event.artifacts.iter().enumerate() {
        tx.execute(
            r#"INSERT INTO artifacts (id, event_id, media_type, path, bytes, content_hash, ordinal)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![
                art.id.to_string(),
                event.id.to_string(),
                art.media_type,
                art.path,
                art.bytes.map(|b| b as i64),
                art.content_hash,
                ordinal as i64,
            ],
        )
        .map_err(StoreError::db)?;
    }
    project_browser_event(tx, event)?;
    project_activity_event(tx, event)?;
    Ok(true)
}

fn productivity_level_str(level: crate::categorization::ProductivityLevel) -> &'static str {
    use crate::categorization::ProductivityLevel::*;
    match level {
        Productive => "productive",
        Neutral => "neutral",
        Distracting => "distracting",
    }
}

/// Compact human duration: "6h 42m" / "12m" / "45s" — for the day summary.
fn fmt_ms_compact(ms: i64) -> String {
    let s = (ms / 1000).max(0) as u64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    let rem_s = s % 60;
    if m < 60 {
        if rem_s > 0 {
            return format!("{m}m {rem_s}s");
        }
        return format!("{m}m");
    }
    let h = m / 60;
    let rem_m = m % 60;
    if rem_m > 0 {
        format!("{h}h {rem_m}m")
    } else {
        format!("{h}h")
    }
}

/// Prefer a human display name from GROUP_CONCAT(DISTINCT app_name, unit-sep).
fn preferred_name_from_concat(concat: &str) -> String {
    let names: Vec<&str> = concat
        .split('\u{001f}')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    crate::categorization::preferred_display_name(&names)
}

/// Fold `activity.focus.v1` events into continuous `activity_segments` rows.
///
/// The accumulator emits an event on every state change plus a heartbeat every
/// ~5s during steady activity. This projection merges consecutive events with
/// identical (app, bundle, title, is_idle, is_locked) into a single segment,
/// extending `ended_at`/`duration_ms`/`event_count` as new heartbeats arrive.
/// A new segment starts whenever the identity changes or a gap larger than the
/// heartbeat window appears (so two genuinely separate sessions of the same app
/// don't get glued together).
fn project_activity_event(
    tx: &rusqlite::Transaction<'_>,
    event: &SourceEvent,
) -> Result<(), StoreError> {
    if event.source != SourceKind::Activity || event.kind != event_kind::ACTIVITY_FOCUS_V1 {
        return Ok(());
    }

    let app_name = event
        .payload
        .get("app_name")
        .and_then(serde_json::Value::as_str);
    let bundle_id = event
        .payload
        .get("bundle_id")
        .and_then(serde_json::Value::as_str);
    let window_title = event
        .payload
        .get("window_title")
        .and_then(serde_json::Value::as_str);
    let is_idle = event
        .payload
        .get("is_idle")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let is_locked = event
        .payload
        .get("is_locked")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let ts = event.ts;
    let ts_str = ts.to_rfc3339();
    // Local-day bucket (the day the user experienced, not UTC). chrono Local
    // gives us the system timezone; convert the UTC event ts to it.
    let day = ts.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string();

    // Classification (deterministic — user rules tried first, then defaults).
    let user_rules = load_user_category_rules(tx).unwrap_or_default();
    let ls_category_type = event
        .payload
        .get("ls_category_type")
        .and_then(serde_json::Value::as_str);
    let classification = crate::categorization::classify(
        &ActivityFields {
            bundle_id,
            app_name,
            window_title,
            url: None,
            ls_category_type,
        },
        &user_rules,
    );

    // Look for a segment with the same identity that ended within the merge
    // window (heartbeat interval + slack). If found, extend it; else insert.
    // 30s window covers the 5s heartbeat with margin for scheduler jitter.
    let identity_match = r#"
        app_name IS ?1
        AND bundle_id IS ?2
        AND window_title IS ?3
        AND is_idle = ?4
        AND is_locked = ?5
        AND ended_at IS NOT NULL
        AND (julianday(?6) - julianday(ended_at)) * 86400.0 < 30.0
        AND (julianday(?6) - julianday(ended_at)) * 86400.0 >= 0.0
        ORDER BY ended_at DESC LIMIT 1"#;
    let existing: Option<(String, String)> = tx
        .query_row(
            &format!(
                "SELECT seg_id, started_at FROM activity_segments WHERE {identity_match}"
            ),
            params![
                app_name,
                bundle_id,
                window_title,
                is_idle as i64,
                is_locked as i64,
                ts_str,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(StoreError::db)?;

    let now = Utc::now().to_rfc3339();
    if let Some((seg_id, started_at)) = existing {
        // Extend: recompute duration from the (immutable) start to this event.
        let started_dt = chrono::DateTime::parse_from_rfc3339(&started_at)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| StoreError::Other(format!("parse started_at: {e}")))?;
        let duration_ms = (ts - started_dt).num_milliseconds().max(0) as i64;
        tx.execute(
            r#"UPDATE activity_segments
               SET ended_at = ?1,
                   duration_ms = ?2,
                   event_count = event_count + 1,
                   updated_at = ?3
               WHERE seg_id = ?4"#,
            params![ts_str, duration_ms, now, seg_id],
        )
        .map_err(StoreError::db)?;
    } else {
        // New segment. Deterministic id so replays are idempotent.
        let identity = format!(
            "{day}|{app_name:?}|{bundle_id:?}|{window_title:?}|{is_idle}|{is_locked}|{ts_str}"
        );
        let seg_id = blake3::hash(identity.as_bytes())
            .to_hex()
            .to_string();
        let level_str = classification.level.map(productivity_level_str);
        tx.execute(
            r#"INSERT OR IGNORE INTO activity_segments
               (seg_id, day, app_name, bundle_id, window_title, url,
                started_at, ended_at, duration_ms, is_idle, is_locked,
                category, project, productivity_level, event_count, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, 0, ?8, ?9,
                       ?10, NULL, ?11, 1, ?12)"#,
            params![
                seg_id,
                day,
                app_name,
                bundle_id,
                window_title,
                ts_str,
                ts_str,
                is_idle as i64,
                is_locked as i64,
                classification.category.as_deref(),
                level_str.as_deref(),
                now,
            ],
        )
        .map_err(StoreError::db)?;
    }

    Ok(())
}

/// Load user-defined category rules from the `kv` table (JSON array), if any.
fn load_user_category_rules(
    tx: &rusqlite::Transaction<'_>,
) -> Result<Vec<CategoryRule>, StoreError> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM kv WHERE key = 'activity.category_rules'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::db)?;
    match raw {
        Some(json) => serde_json::from_str(&json)
            .map_err(|e| StoreError::Other(format!("parse category rules: {e}"))),
        None => Ok(Vec::new()),
    }
}

fn project_browser_event(
    tx: &rusqlite::Transaction<'_>,
    event: &SourceEvent,
) -> Result<(), StoreError> {
    if event.source != SourceKind::Browser {
        return Ok(());
    }
    if !matches!(
        event.kind.as_str(),
        "browser.navigation_committed.v1"
            | "browser.document_ready.v1"
            | "browser.visibility_focus_change.v1"
            | "browser.visit_closed.v1"
    ) {
        return Ok(());
    }
    let visit_id = event
        .payload
        .get("visit_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .or(event.session_id);
    let Some(visit_id) = visit_id else {
        return Ok(());
    };
    let data = event
        .payload
        .get("data")
        .unwrap_or(&serde_json::Value::Null);
    let is_navigation = event.kind == "browser.navigation_committed.v1";
    let is_ready = event.kind == "browser.document_ready.v1";
    let is_visibility = event.kind == "browser.visibility_focus_change.v1";
    let is_close = event.kind == "browser.visit_closed.v1";
    let max_scroll = data
        .get("max_scroll_ratio")
        .and_then(serde_json::Value::as_f64);
    let active_ms = json_i64(data.get("active_ms"));
    let visible_ms = json_i64(data.get("visible_ms"));
    let background_ms = json_i64(data.get("background_ms"));
    let visible_now = (is_visibility
        && data
            .get("visible")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
        || (is_close
            && data
                .get("visible_at_close")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false));
    let identity_source = event
        .artifacts
        .iter()
        .find_map(|artifact| artifact.content_hash.as_deref())
        .or_else(|| data.get("canonical").and_then(serde_json::Value::as_str))
        .or_else(|| event.payload.get("url").and_then(serde_json::Value::as_str));
    let content_id = identity_source.map(|value| blake3::hash(value.as_bytes()).to_hex().to_string());
    let existing: Option<(Option<String>, String)> = tx
        .query_row(
            "SELECT content_id, snapshot_hashes FROM browser_visits WHERE visit_id = ?1",
            params![visit_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(StoreError::db)?;
    let effective_content_id = content_id
        .clone()
        .or_else(|| existing.as_ref().and_then(|item| item.0.clone()));
    let revisit_index = if let Some(content_id) = effective_content_id.as_deref() {
        Some(
            tx.query_row(
                "SELECT COUNT(1) FROM browser_visits WHERE content_id = ?1 AND visit_id <> ?2",
                params![content_id, visit_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StoreError::db)?,
        )
    } else {
        None
    };
    let mut snapshot_hashes: Vec<String> = existing
        .as_ref()
        .and_then(|item| serde_json::from_str(&item.1).ok())
        .unwrap_or_default();
    for hash in event
        .artifacts
        .iter()
        .filter_map(|artifact| artifact.content_hash.clone())
    {
        if !snapshot_hashes.contains(&hash) {
            snapshot_hashes.push(hash);
        }
    }
    let snapshot_hashes = serde_json::to_string(&snapshot_hashes).map_err(StoreError::json)?;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        r#"INSERT INTO browser_visits
           (visit_id, document_id, content_id, url, opened_at, document_ready_at,
            first_visible_at, last_visible_at, closed_at, active_ms, visible_ms,
            background_ms, max_scroll_ratio, revisit_index, opener_tab_id, referrer,
            transition, close_reason, extraction_status, snapshot_hashes, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                   ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
           ON CONFLICT(visit_id) DO UPDATE SET
             document_id=COALESCE(excluded.document_id, browser_visits.document_id),
             content_id=COALESCE(excluded.content_id, browser_visits.content_id),
             url=COALESCE(excluded.url, browser_visits.url),
             opened_at=COALESCE(browser_visits.opened_at, excluded.opened_at),
             document_ready_at=COALESCE(browser_visits.document_ready_at, excluded.document_ready_at),
             first_visible_at=COALESCE(browser_visits.first_visible_at, excluded.first_visible_at),
             last_visible_at=COALESCE(excluded.last_visible_at, browser_visits.last_visible_at),
             closed_at=COALESCE(excluded.closed_at, browser_visits.closed_at),
             active_ms=COALESCE(excluded.active_ms, browser_visits.active_ms),
             visible_ms=COALESCE(excluded.visible_ms, browser_visits.visible_ms),
             background_ms=COALESCE(excluded.background_ms, browser_visits.background_ms),
             max_scroll_ratio=CASE
               WHEN excluded.max_scroll_ratio IS NULL THEN browser_visits.max_scroll_ratio
               WHEN browser_visits.max_scroll_ratio IS NULL THEN excluded.max_scroll_ratio
               ELSE MAX(browser_visits.max_scroll_ratio, excluded.max_scroll_ratio)
             END,
             revisit_index=COALESCE(excluded.revisit_index, browser_visits.revisit_index),
             opener_tab_id=COALESCE(excluded.opener_tab_id, browser_visits.opener_tab_id),
             referrer=COALESCE(excluded.referrer, browser_visits.referrer),
             transition=COALESCE(excluded.transition, browser_visits.transition),
             close_reason=COALESCE(excluded.close_reason, browser_visits.close_reason),
             extraction_status=COALESCE(excluded.extraction_status, browser_visits.extraction_status),
             snapshot_hashes=excluded.snapshot_hashes,
             updated_at=excluded.updated_at"#,
        params![
            visit_id.to_string(),
            event.payload.get("document_id").and_then(serde_json::Value::as_str),
            content_id,
            event.payload.get("url").and_then(serde_json::Value::as_str),
            is_navigation.then(|| event.ts.to_rfc3339()),
            is_ready.then(|| event.ts.to_rfc3339()),
            visible_now.then(|| event.ts.to_rfc3339()),
            visible_now.then(|| event.ts.to_rfc3339()),
            is_close.then(|| event.ts.to_rfc3339()),
            active_ms,
            visible_ms,
            background_ms,
            max_scroll,
            revisit_index,
            json_i64(data.get("opener_tab_id")),
            data.get("referrer").and_then(serde_json::Value::as_str),
            data.get("transition").and_then(serde_json::Value::as_str),
            data.get("close_reason").and_then(serde_json::Value::as_str),
            data.get("extraction_status").and_then(serde_json::Value::as_str),
            snapshot_hashes,
            now,
        ],
    )
    .map_err(StoreError::db)?;
    Ok(())
}

fn json_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|number| number.round() as i64))
    })
}

fn optional_sql_ts(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn load_artifacts(conn: &Connection, event_id: Uuid) -> Result<Vec<ArtifactRef>, StoreError> {
    let mut stmt = conn
        .prepare(
            r#"SELECT id, media_type, path, bytes, content_hash
               FROM artifacts WHERE event_id = ?1 ORDER BY ordinal ASC"#,
        )
        .map_err(StoreError::db)?;
    let rows = stmt
        .query_map(params![event_id.to_string()], |row| {
            Ok(ArtifactRef {
                id: parse_uuid(row.get::<_, String>(0)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                media_type: row.get(1)?,
                path: row.get(2)?,
                bytes: row
                    .get::<_, Option<i64>>(3)?
                    .map(|b| b as u64),
                content_hash: row.get(4)?,
            })
        })
        .map_err(StoreError::db)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(StoreError::db)?);
    }
    Ok(out)
}

fn row_to_event(row: EventRow) -> Result<SourceEvent, StoreError> {
    let source: SourceKind = serde_json::from_str(&row.source).map_err(StoreError::json)?;
    let payload = serde_json::from_str(&row.payload).map_err(StoreError::json)?;
    Ok(SourceEvent {
        id: parse_uuid(row.id)?,
        source,
        kind: row.kind,
        ts: parse_ts(row.ts)?,
        session_id: match row.session_id {
            Some(s) => Some(parse_uuid(s)?),
            None => None,
        },
        payload,
        artifacts: Vec::new(),
    })
}

fn parse_uuid(s: impl AsRef<str>) -> Result<Uuid, StoreError> {
    Uuid::parse_str(s.as_ref()).map_err(|e| StoreError::Other(format!("uuid: {e}")))
}

fn parse_ts(s: impl AsRef<str>) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(s.as_ref())
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StoreError::Other(format!("timestamp: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventStore;
    use lumen_types::event_kind;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn append_survives_reopen() {
        let dir = tempdir().unwrap();
        let id = {
            let store = SqliteStore::open(dir.path()).unwrap();
            let mut event = SourceEvent::new(
                SourceKind::Screen,
                event_kind::SCREENSHOT_V1,
                json!({"reason": "test"}),
            );
            let art = store.blobs().put_bytes("image/png", b"png-bytes").unwrap();
            event.artifacts.push(art);
            let id = event.id;
            store.append(vec![event]).await.unwrap();
            assert_eq!(store.len().await.unwrap(), 1);
            id
        };

        let store = SqliteStore::open(dir.path()).unwrap();
        assert_eq!(store.len().await.unwrap(), 1);
        let got = store.get(id).await.unwrap().expect("event");
        assert_eq!(got.kind, event_kind::SCREENSHOT_V1);
        assert_eq!(got.artifacts.len(), 1);
        assert_eq!(
            store.blobs().read_relative(&got.artifacts[0].path).unwrap(),
            b"png-bytes"
        );
    }

    #[tokio::test]
    async fn idempotent_artifact_batch_can_be_replayed_without_duplication() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let event = SourceEvent::new(
            SourceKind::Browser,
            "browser.document_ready.v1",
            json!({"url": "https://example.test/article"}),
        );
        let input = EventWithArtifacts {
            event: event.clone(),
            artifacts: vec![ArtifactInput {
                media_type: "text/markdown".into(),
                bytes: b"Synthetic article body".to_vec(),
            }],
        };

        let first = store
            .append_idempotent_with_artifacts(vec![input.clone()])
            .unwrap();
        let mut conflicting_replay = input;
        conflicting_replay.artifacts[0].bytes = b"Conflicting replay body".to_vec();
        let replay = store
            .append_idempotent_with_artifacts(vec![conflicting_replay])
            .unwrap();

        assert_eq!((first.accepted, first.duplicates), (1, 0));
        assert_eq!((replay.accepted, replay.duplicates), (0, 1));
        let stored = store.get(event.id).await.unwrap().unwrap();
        assert_eq!(stored.artifacts.len(), 1);
        assert_eq!(
            store
                .blobs()
                .read_relative(&stored.artifacts[0].path)
                .unwrap(),
            b"Synthetic article body"
        );
        assert_eq!(store.blobs().total_bytes().unwrap(), b"Synthetic article body".len() as u64);
    }

    #[test]
    fn concurrent_blob_intake_cannot_race_past_the_limit() {
        let dir = tempdir().unwrap();
        let store = std::sync::Arc::new(SqliteStore::open(dir.path()).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for body in [b"body-one".to_vec(), b"body-two".to_vec()] {
            let store = std::sync::Arc::clone(&store);
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let record = EventWithArtifacts {
                    event: SourceEvent::new(
                        SourceKind::Browser,
                        "browser.document_ready.v1",
                        json!({}),
                    ),
                    artifacts: vec![ArtifactInput {
                        media_type: "text/markdown".into(),
                        bytes: body,
                    }],
                };
                barrier.wait();
                store
                    .append_idempotent_with_artifacts_up_to(vec![record], 8)
                    .unwrap()
            }));
        }
        let outcomes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, BlobLimitedAppendOutcome::Appended(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, BlobLimitedAppendOutcome::LimitExceeded))
                .count(),
            1
        );
        assert_eq!(store.blobs().total_bytes().unwrap(), 8);
    }

    #[tokio::test]
    async fn source_export_uses_a_monotonic_cursor() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        store
            .append(vec![
                SourceEvent::new(SourceKind::Screen, event_kind::SCREENSHOT_V1, json!({})),
                SourceEvent::new(
                    SourceKind::Browser,
                    "browser.navigation_committed.v1",
                    json!({"visit_id": "00000000-0000-4000-8000-000000000010"}),
                ),
                SourceEvent::new(
                    SourceKind::Browser,
                    "browser.visit_closed.v1",
                    json!({"visit_id": "00000000-0000-4000-8000-000000000010"}),
                ),
            ])
            .await
            .unwrap();

        let first = store
            .list_source_after_cursor(&SourceKind::Browser, 0, 1)
            .unwrap();
        let second = store
            .list_source_after_cursor(&SourceKind::Browser, first[0].cursor, 10)
            .unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert!(second[0].cursor > first[0].cursor);
        assert_eq!(second[0].event.kind, "browser.visit_closed.v1");
    }

    #[test]
    fn browser_visit_projection_is_updated_from_accepted_events() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let visit_id = Uuid::parse_str("00000000-0000-4000-8000-000000000301").unwrap();
        let mut opened = SourceEvent::new(
            SourceKind::Browser,
            "browser.navigation_committed.v1",
            json!({
                "visit_id": visit_id,
                "document_id": "fixture-document",
                "url": "https://example.test/article",
                "data": {"transition": "typed", "opener_tab_id": 7}
            }),
        );
        opened.session_id = Some(visit_id);
        let mut ready = SourceEvent::new(
            SourceKind::Browser,
            "browser.document_ready.v1",
            json!({
                "visit_id": visit_id,
                "document_id": "fixture-document",
                "url": "https://example.test/article",
                "data": {
                    "canonical": "https://example.test/article",
                    "referrer": "https://example.test/index",
                    "extraction_status": "success"
                }
            }),
        );
        ready.session_id = Some(visit_id);
        let mut visible = SourceEvent::new(
            SourceKind::Browser,
            "browser.visibility_focus_change.v1",
            json!({
                "visit_id": visit_id,
                "document_id": "fixture-document",
                "url": "https://example.test/article",
                "data": {"visible": true, "focused": true, "max_scroll_ratio": 0.5}
            }),
        );
        visible.session_id = Some(visit_id);
        let mut closed = SourceEvent::new(
            SourceKind::Browser,
            "browser.visit_closed.v1",
            json!({
                "visit_id": visit_id,
                "document_id": "fixture-document",
                "url": "https://example.test/article",
                "data": {
                    "active_ms": 12000.0,
                    "visible_ms": 15000.0,
                    "background_ms": 3000.0,
                    "visible_at_close": true,
                    "max_scroll_ratio": 0.75,
                    "close_reason": "pagehide"
                }
            }),
        );
        closed.session_id = Some(visit_id);

        store
            .append_idempotent_with_artifacts(vec![
                EventWithArtifacts {
                    event: opened,
                    artifacts: vec![],
                },
                EventWithArtifacts {
                    event: ready,
                    artifacts: vec![ArtifactInput {
                        media_type: "text/markdown".into(),
                        bytes: b"fixture body".to_vec(),
                    }],
                },
                EventWithArtifacts {
                    event: visible,
                    artifacts: vec![],
                },
                EventWithArtifacts {
                    event: closed,
                    artifacts: vec![],
                },
            ])
            .unwrap();

        let visit = store.get_browser_visit(visit_id).unwrap().unwrap();
        assert_eq!(visit.document_id.as_deref(), Some("fixture-document"));
        assert_eq!(visit.active_ms, Some(12_000));
        assert_eq!(visit.visible_ms, Some(15_000));
        assert_eq!(visit.background_ms, Some(3_000));
        assert_eq!(visit.max_scroll_ratio, Some(0.75));
        assert!(visit.content_id.is_some());
        assert!(visit.first_visible_at.is_some());
        assert!(visit.last_visible_at.is_some());
        assert_eq!(visit.revisit_index, Some(0));
        assert_eq!(visit.opener_tab_id, Some(7));
        assert_eq!(visit.referrer.as_deref(), Some("https://example.test/index"));
        assert_eq!(visit.transition.as_deref(), Some("typed"));
        assert_eq!(visit.snapshot_hashes.len(), 1);
        assert_eq!(visit.close_reason.as_deref(), Some("pagehide"));
    }

    #[tokio::test]
    async fn wipe_clears_events_and_blobs() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        store
            .put_and_append(
                SourceEvent::new(SourceKind::Audio, event_kind::AUDIO_CHUNK_V1, json!({})),
                "audio/wav",
                b"RIFF",
            )
            .unwrap();
        assert_eq!(store.len().await.unwrap(), 1);
        store.wipe_all().await.unwrap();
        assert_eq!(store.len().await.unwrap(), 0);
        assert!(store.list_recent(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn enqueue_job_persists() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let event = SourceEvent::new(SourceKind::Screen, event_kind::SCREENSHOT_V1, json!({}));
        let eid = event.id;
        store.append(vec![event]).await.unwrap();
        assert!(store.enqueue_job(eid, "ocr_screen").unwrap().is_some());
        assert!(store.enqueue_job(eid, "ocr_screen").unwrap().is_none()); // dedup open
        let jobs = store.list_jobs(10).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "ocr_screen");
        assert_eq!(jobs[0].status, JobStatus::Pending);

        let claimed = store.claim_pending_jobs("ocr_screen", 10).unwrap();
        assert_eq!(claimed.len(), 1);
        store
            .complete_job(claimed[0].id, JobStatus::Done, None)
            .unwrap();
        // can enqueue again after done? unique only on pending/running — yes
        assert!(store.enqueue_job(eid, "ocr_screen").unwrap().is_some());
    }

    #[tokio::test]
    async fn derived_upsert_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let event = SourceEvent::new(
            SourceKind::Screen,
            lumen_types::event_kind::SCREENSHOT_V1,
            serde_json::json!({}),
        );
        let eid = event.id;
        store.append(vec![event]).await.unwrap();
        let a = store.insert_derived(eid, "ocr.v1", r#"{"text":"a"}"#).unwrap();
        let b = store.insert_derived(eid, "ocr.v1", r#"{"text":"b"}"#).unwrap();
        assert_eq!(a, b);
        let list = store.list_derived_for_event(eid).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].2.contains("\"b\""));
    }

    #[tokio::test]
    async fn ocr_search_indexes_on_insert_derived() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let event = SourceEvent::new(
            SourceKind::Screen,
            event_kind::SCREENSHOT_V1,
            json!({}),
        );
        let eid = event.id;
        store.append(vec![event]).await.unwrap();
        store
            .insert_derived(
                eid,
                "ocr.v1",
                r#"{"payload_version":1,"text":"unique-lumen-navi alpha 中文检索","confidence":0.9}"#,
            )
            .unwrap();
        assert_eq!(store.ocr_doc_count().unwrap(), 1);

        let hits = store.search_ocr("unique-lumen-navi", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].event_id, eid);
        assert!(hits[0].text_preview.contains("unique-lumen-navi"));

        let zh = store.search_ocr("中文", 10).unwrap();
        assert_eq!(zh.len(), 1);

        // Reindex rebuilds from derived without loss.
        let n = store.reindex_ocr_docs().unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.ocr_doc_count().unwrap(), 1);
        assert_eq!(store.search_ocr("alpha", 5).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ocr_search_empty_query_is_empty() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        assert!(store.search_ocr("   ", 10).unwrap().is_empty());
        assert!(store.search_ocr("!!!", 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_timeline_includes_text_and_artifact() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let mut event = SourceEvent::new(
            SourceKind::Screen,
            event_kind::SCREENSHOT_V1,
            json!({"app_name": "Safari", "window_title": "Example"}),
        );
        let art = store.blobs().put_bytes("image/jpeg", b"fake-jpeg").unwrap();
        event.artifacts.push(art);
        let eid = event.id;
        store.append(vec![event]).await.unwrap();
        store
            .insert_derived(
                eid,
                "ocr.v1",
                r#"{"text":"hello timeline preview text","confidence":0.5}"#,
            )
            .unwrap();
        let items = store
            .list_timeline(TimelineQuery {
                limit: 10,
                app_contains: "Safari".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, eid);
        assert_eq!(items[0].app_name.as_deref(), Some("Safari"));
        assert!(items[0]
            .text_preview
            .as_deref()
            .unwrap_or("")
            .contains("timeline"));
        assert_eq!(items[0].media_type.as_deref(), Some("image/jpeg"));
        assert!(items[0].artifact_path.is_some());

        let body = store.build_day_summary_body(&Utc::now().format("%Y-%m-%d").to_string());
        assert!(body.is_ok());
        assert!(body.unwrap().contains("Screenshots"));
    }

    #[test]
    fn sanitize_fts_keeps_cjk_and_alnum() {
        let q = sanitize_fts_query("hello, 世界检索!!");
        assert!(q.contains("hello"));
        assert!(q.contains("世界检索"));
        // tokens shorter than 3 chars are dropped for trigram FTS
        assert!(sanitize_fts_query("中文").is_empty());
        assert!(like_pattern("中文").is_some());
    }

    #[test]
    fn activity_projection_merges_heartbeats_and_splits_on_change() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let base = chrono::DateTime::parse_from_rfc3339("2026-08-07T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mk = |ts_offset: i64, app: &str, is_idle: bool| {
            let mut e = SourceEvent::new(
                SourceKind::Activity,
                event_kind::ACTIVITY_FOCUS_V1,
                json!({
                    "app_name": app,
                    "bundle_id": format!("com.{app}"),
                    "window_title": "doc",
                    "is_idle": is_idle,
                    "is_locked": false,
                    "idle_seconds": if is_idle { 200.0 } else { 1.0 },
                }),
            );
            e.ts = base + chrono::Duration::seconds(ts_offset);
            e.id = Uuid::new_v4();
            e
        };

        // Three heartbeats for Safari (0s, 5s, 10s) — same identity → one segment.
        store.append_event(mk(0, "Safari", false)).unwrap();
        store.append_event(mk(5, "Safari", false)).unwrap();
        store.append_event(mk(10, "Safari", false)).unwrap();
        // Switch to Mail — new segment.
        store.append_event(mk(12, "Mail", false)).unwrap();
        // Idle boundary on Safari — new segment (is_idle=true differs).
        store.append_event(mk(200, "Safari", true)).unwrap();

        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(1) FROM activity_segments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3, "three distinct segments: safari, mail, safari-idle");

        // The Safari segment should span 0–10s (10s = 10000ms).
        let safari_ms: i64 = conn
            .query_row(
                "SELECT duration_ms FROM activity_segments WHERE app_name = 'Safari' AND is_idle = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(safari_ms, 10_000, "merged segment duration covers first→last heartbeat");

        // Safari should be classified (browser dev domain? no — falls back;
        // here it's unclassified since "Safari" isn't in default rules).
        drop(conn);
    }

    #[test]
    fn activity_projection_classifies_known_apps() {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();

        let mut e = SourceEvent::new(
            SourceKind::Activity,
            event_kind::ACTIVITY_FOCUS_V1,
            json!({
                "app_name": "Code",
                "bundle_id": "com.microsoft.VSCode",
                "window_title": "main.rs",
                "is_idle": false,
                "is_locked": false,
            }),
        );
        e.id = Uuid::new_v4();
        store.append_event(e).unwrap();

        let conn = store.conn.lock().unwrap();
        let (cat, level): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT category, productivity_level FROM activity_segments WHERE bundle_id = 'com.microsoft.VSCode'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        drop(conn);
        assert_eq!(cat.as_deref(), Some("Development"));
        assert_eq!(level.as_deref(), Some("productive"));
    }
}
