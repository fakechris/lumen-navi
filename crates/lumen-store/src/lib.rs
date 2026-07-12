//! Event and artifact persistence.
//!
//! Phase 0 ships an in-memory store so the daemon and intake layers can be
//! wired without choosing a final SQLite schema yet.

use std::sync::Arc;

use async_trait::async_trait;
use lumen_types::SourceEvent;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("store error: {0}")]
    Other(String),
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, events: Vec<SourceEvent>) -> Result<(), StoreError>;
    async fn list_recent(&self, limit: usize) -> Result<Vec<SourceEvent>, StoreError>;
    async fn get(&self, id: Uuid) -> Result<Option<SourceEvent>, StoreError>;
    async fn wipe_all(&self) -> Result<(), StoreError>;
    async fn len(&self) -> Result<usize, StoreError>;
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
    async fn append_and_list() {
        let store = MemoryEventStore::default();
        store
            .append(vec![SourceEvent::new(
                SourceKind::Screen,
                "screenshot",
                json!({}),
            )])
            .await
            .unwrap();
        assert_eq!(store.len().await.unwrap(), 1);
        assert_eq!(store.list_recent(10).await.unwrap().len(), 1);
    }
}
