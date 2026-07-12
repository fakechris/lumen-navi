//! Event and artifact persistence.
//!
//! - [`MemoryEventStore`] — tests and ephemeral smoke
//! - [`SqliteStore`] — durable Phase S1 store (SQLite meta + CA blobs)

mod blob;
mod schema;
mod sqlite;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lumen_types::SourceEvent;
use thiserror::Error;
use uuid::Uuid;

pub use blob::BlobStore;
pub use schema::SCHEMA_VERSION;
pub use sqlite::SqliteStore;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(String),
    #[error("json: {0}")]
    Json(String),
    #[error("store error: {0}")]
    Other(String),
}

impl StoreError {
    pub(crate) fn io(err: std::io::Error) -> Self {
        Self::Io(err)
    }

    pub(crate) fn db(err: impl ToString) -> Self {
        Self::Db(err.to_string())
    }

    pub(crate) fn json(err: impl ToString) -> Self {
        Self::Json(err.to_string())
    }
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, events: Vec<SourceEvent>) -> Result<(), StoreError>;
    async fn list_recent(&self, limit: usize) -> Result<Vec<SourceEvent>, StoreError>;
    async fn get(&self, id: Uuid) -> Result<Option<SourceEvent>, StoreError>;
    async fn wipe_all(&self) -> Result<(), StoreError>;
    async fn len(&self) -> Result<usize, StoreError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
    Dead,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Dead => "dead",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "dead" => Self::Dead,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: Uuid,
    pub event_id: Uuid,
    pub kind: String,
    pub status: JobStatus,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Process-local store for scaffolding and tests.
#[derive(Default)]
pub struct MemoryEventStore {
    events: tokio::sync::Mutex<Vec<SourceEvent>>,
}

#[async_trait]
impl EventStore for MemoryEventStore {
    async fn append(&self, events: Vec<SourceEvent>) -> Result<(), StoreError> {
        self.events.lock().await.extend(events);
        Ok(())
    }

    async fn list_recent(&self, limit: usize) -> Result<Vec<SourceEvent>, StoreError> {
        let guard = self.events.lock().await;
        let start = guard.len().saturating_sub(limit);
        Ok(guard[start..].to_vec())
    }

    async fn get(&self, id: Uuid) -> Result<Option<SourceEvent>, StoreError> {
        Ok(self
            .events
            .lock()
            .await
            .iter()
            .find(|e| e.id == id)
            .cloned())
    }

    async fn wipe_all(&self) -> Result<(), StoreError> {
        self.events.lock().await.clear();
        Ok(())
    }

    async fn len(&self) -> Result<usize, StoreError> {
        Ok(self.events.lock().await.len())
    }
}

/// Bridge so intake can write through any [`EventStore`].
pub struct StoreSink<S: EventStore> {
    pub store: Arc<S>,
}

impl<S: EventStore> StoreSink<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: EventStore> lumen_intake::EventSink for StoreSink<S> {
    async fn accept(&self, events: Vec<SourceEvent>) -> Result<(), lumen_intake::IntakeError> {
        self.store
            .append(events)
            .await
            .map_err(|e| lumen_intake::IntakeError::Source(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_types::{SourceEvent, SourceKind};
    use serde_json::json;

    #[tokio::test]
    async fn memory_append_and_list() {
        let store = MemoryEventStore::default();
        store
            .append(vec![SourceEvent::new(
                SourceKind::Screen,
                "screenshot.v1",
                json!({}),
            )])
            .await
            .unwrap();
        assert_eq!(store.len().await.unwrap(), 1);
        assert_eq!(store.list_recent(10).await.unwrap().len(), 1);
    }
}
