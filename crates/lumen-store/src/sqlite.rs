//! Durable SQLite + blob store.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lumen_types::{ActivitySession, ArtifactRef, SourceEvent, SourceKind};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::blob::BlobStore;
use crate::schema::{MIGRATE_V1, MIGRATE_V2, MIGRATE_V3, MIGRATE_V4, SCHEMA_VERSION};
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

/// On-disk store: `$data_dir/meta/navi.db` + `$data_dir/blobs/...`.
pub struct SqliteStore {
    data_dir: PathBuf,
    conn: Mutex<Connection>,
    blobs: BlobStore,
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
        let artifact = self.blobs.put_bytes(media_type, bytes)?;
        event.artifacts.push(artifact);
        self.append_sync(std::slice::from_ref(&event))?;
        Ok(event)
    }

    pub fn upsert_session(&self, session: &ActivitySession) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
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
        let mut apps: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let mut samples: Vec<String> = Vec::new();
        for it in &items {
            if it.kind.contains("screenshot") {
                shots += 1;
            }
            if it.kind.contains("audio") {
                audio += 1;
            }
            if let Some(app) = &it.app_name {
                *apps.entry(app.clone()).or_default() += 1;
            }
            if let Some(t) = &it.text_preview {
                if samples.len() < 8 && t.chars().count() > 8 {
                    samples.push(t.clone());
                }
            }
        }
        let top_apps: Vec<String> = {
            let mut v: Vec<_> = apps.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.into_iter()
                .take(8)
                .map(|(k, n)| format!("{k} ({n})"))
                .collect()
        };
        let text = format!(
            "Day {day}\nEvents: {}\nScreenshots: {shots}\nAudio chunks: {audio}\nTop apps: {}\n\nText samples:\n{}",
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
        {
            let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
            conn.execute_batch(
                r#"
                DELETE FROM ocr_docs;
                DELETE FROM derived;
                DELETE FROM jobs;
                DELETE FROM artifacts;
                DELETE FROM events;
                DELETE FROM kv;
                "#,
            )
            .map_err(StoreError::db)?;
        }
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
    let source = serde_json::to_string(&event.source).map_err(StoreError::json)?;
    let payload = serde_json::to_string(&event.payload).map_err(StoreError::json)?;
    let session = event.session_id.map(|s| s.to_string());
    let created = Utc::now().to_rfc3339();

    tx.execute(
        r#"INSERT INTO events (id, source, kind, ts, session_id, payload, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
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
    Ok(())
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
}
