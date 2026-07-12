//! Multi-source intake runtime.
//!
//! Adapters implement [`Source`]. The runtime pulls/receives events and hands
//! them to a sink (typically the store layer).

use async_trait::async_trait;
use lumen_types::SourceEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IntakeError {
    #[error("source `{0}` is not running")]
    NotRunning(String),
    #[error("source error: {0}")]
    Source(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Something that can produce [`SourceEvent`]s continuously.
#[async_trait]
pub trait Source: Send + Sync {
    fn id(&self) -> &str;

    async fn start(&mut self) -> Result<(), IntakeError>;
    async fn stop(&mut self) -> Result<(), IntakeError>;

    /// Poll or drain pending events. Empty vec is normal (idle).
    async fn poll(&mut self) -> Result<Vec<SourceEvent>, IntakeError>;
}

/// Destination for accepted events (store, queue, or test double).
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn accept(&self, events: Vec<SourceEvent>) -> Result<(), IntakeError>;
}

/// Minimal in-memory sink for tests and early daemon wiring.
#[derive(Default)]
pub struct MemorySink {
    pub events: tokio::sync::Mutex<Vec<SourceEvent>>,
}

#[async_trait]
impl EventSink for MemorySink {
    async fn accept(&self, events: Vec<SourceEvent>) -> Result<(), IntakeError> {
        self.events.lock().await.extend(events);
        Ok(())
    }
}

/// One-shot poll loop helper used by the daemon and tests.
pub async fn drain_once(
    source: &mut dyn Source,
    sink: &dyn EventSink,
) -> Result<usize, IntakeError> {
    let batch = source.poll().await?;
    let n = batch.len();
    if n > 0 {
        sink.accept(batch).await?;
    }
    Ok(n)
}

/// Placeholder source that emits nothing — keeps the adapter surface honest.
pub struct NullSource {
    id: String,
    running: bool,
}

impl NullSource {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            running: false,
        }
    }
}

#[async_trait]
impl Source for NullSource {
    fn id(&self) -> &str {
        &self.id
    }

    async fn start(&mut self) -> Result<(), IntakeError> {
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), IntakeError> {
        self.running = false;
        Ok(())
    }

    async fn poll(&mut self) -> Result<Vec<SourceEvent>, IntakeError> {
        if !self.running {
            return Err(IntakeError::NotRunning(self.id.clone()));
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_types::{SourceEvent, SourceKind};
    use serde_json::json;

    struct OnceSource {
        fired: bool,
    }

    #[async_trait]
    impl Source for OnceSource {
        fn id(&self) -> &str {
            "once"
        }

        async fn start(&mut self) -> Result<(), IntakeError> {
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), IntakeError> {
            Ok(())
        }

        async fn poll(&mut self) -> Result<Vec<SourceEvent>, IntakeError> {
            if self.fired {
                return Ok(vec![]);
            }
            self.fired = true;
            Ok(vec![SourceEvent::new(
                SourceKind::Browser,
                "page_visit",
                json!({"url": "https://example.com"}),
            )])
        }
    }

    #[tokio::test]
    async fn drain_once_forwards_events() {
        let mut source = OnceSource { fired: false };
        let sink = MemorySink::default();
        let n = drain_once(&mut source, &sink).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(sink.events.lock().await.len(), 1);
    }
}
