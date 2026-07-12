//! Processing / enrichment over stored intake events.
//!
//! Capture must not wait on this layer. Jobs read raw events and write derived
//! records (summaries, transcripts, redactions) without mutating originals.

use async_trait::async_trait;
use lumen_types::SourceEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("processor `{0}` failed: {1}")]
    Failed(String, String),
}

/// A unit of derived work over one or more events.
#[async_trait]
pub trait Processor: Send + Sync {
    fn name(&self) -> &str;
    async fn process(&self, event: &SourceEvent) -> Result<ProcessOutcome, ProcessError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// Nothing to do for this event.
    Skipped,
    /// Processor produced a human-readable note (placeholder for derived records).
    Note(String),
}

/// No-op processor used while the pipeline shape settles.
pub struct IdentityProcessor;

#[async_trait]
impl Processor for IdentityProcessor {
    fn name(&self) -> &str {
        "identity"
    }

    async fn process(&self, event: &SourceEvent) -> Result<ProcessOutcome, ProcessError> {
        Ok(ProcessOutcome::Note(format!(
            "seen {}/{}",
            format!("{:?}", event.source).to_ascii_lowercase(),
            event.kind
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_types::{SourceEvent, SourceKind};
    use serde_json::json;

    #[tokio::test]
    async fn identity_notes_event() {
        let p = IdentityProcessor;
        let event = SourceEvent::new(SourceKind::Audio, "chunk", json!({}));
        let out = p.process(&event).await.unwrap();
        assert!(matches!(out, ProcessOutcome::Note(_)));
    }
}
