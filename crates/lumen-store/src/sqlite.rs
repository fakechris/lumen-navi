//! Durable SQLite + blob store.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lumen_types::{ActivitySession, ArtifactRef, SourceEvent, SourceKind};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::blob::BlobStore;
use crate::schema::{MIGRATE_V1, MIGRATE_V2, SCHEMA_VERSION};
use crate::{EventStore, JobRecord, JobStatus, StoreError};

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

    pub fn enqueue_job(&self, event_id: Uuid, kind: impl Into<String>) -> Result<JobRecord, StoreError> {
        let job = JobRecord {
            id: Uuid::new_v4(),
            event_id,
            kind: kind.into(),
            status: JobStatus::Pending,
            attempts: 0,
            last_error: None,
            updated_at: Utc::now(),
        };
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        conn.execute(
            r#"INSERT INTO jobs (id, event_id, kind, status, attempts, last_error, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![
                job.id.to_string(),
                job.event_id.to_string(),
                job.kind,
                job.status.as_str(),
                job.attempts,
                job.last_error,
                job.updated_at.to_rfc3339(),
            ],
        )
        .map_err(StoreError::db)?;
        Ok(job)
    }

    pub fn list_jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Other("lock poisoned".into()))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT id, event_id, kind, status, attempts, last_error, updated_at
                   FROM jobs ORDER BY updated_at DESC LIMIT ?1"#,
            )
            .map_err(StoreError::db)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
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

    let _ = v;
    Ok(())
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
        store.enqueue_job(eid, "ocr_screen").unwrap();
        let jobs = store.list_jobs(10).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, "ocr_screen");
        assert_eq!(jobs[0].status, JobStatus::Pending);
    }
}
