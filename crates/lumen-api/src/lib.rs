//! Versioned local control plane schema.
//!
//! Transport (UDS / loopback HTTP) is chosen by the daemon. Chrome and desktop
//! UI must speak this schema so the core does not grow ad-hoc endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// API schema version advertised by the daemon.
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub api_version: u32,
    pub product: String,
    pub sources: Vec<SourceStatus>,
    pub paused: bool,
    pub stored_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub id: String,
    pub enabled: bool,
    pub running: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlRequest {
    Health,
    Pause { source: Option<String> },
    Resume { source: Option<String> },
    ListEvents { limit: usize },
    Wipe,
    Permissions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "ok", rename_all = "snake_case")]
pub enum ControlResponse {
    Health(HealthResponse),
    Ack,
    Events { events: Vec<EventSummary> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: Uuid,
    pub source: String,
    pub kind: String,
    pub ts: DateTime<Utc>,
}

impl HealthResponse {
    pub fn scaffold(sources: Vec<SourceStatus>, stored_events: usize, paused: bool) -> Self {
        Self {
            api_version: API_VERSION,
            product: "lumen-navi".into(),
            sources,
            paused,
            stored_events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_roundtrip() {
        let h = HealthResponse::scaffold(
            vec![SourceStatus {
                id: "screen".into(),
                enabled: true,
                running: false,
                last_error: None,
            }],
            0,
            false,
        );
        let s = serde_json::to_string(&h).unwrap();
        let back: HealthResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.api_version, API_VERSION);
        assert_eq!(back.product, "lumen-navi");
    }
}
