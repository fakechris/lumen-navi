//! Loopback HTTP control plane for health + OCR search.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use lumen_api::{
    ControlRequest, ControlResponse, EventSummary, HealthResponse, OcrSearchHitDto, SourceStatus,
    API_VERSION,
};
use lumen_store::{EventStore, SCHEMA_VERSION, SqliteStore};
use serde::Deserialize;
use tracing::{info, warn};

#[derive(Clone)]
pub struct ControlState {
    pub store: Arc<SqliteStore>,
    pub paused: bool,
    pub sources: Vec<SourceStatus>,
}

pub async fn serve(bind: SocketAddr, state: ControlState) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/health", get(get_health))
        .route("/v1/health", get(get_health))
        .route("/v1/ocr/search", get(get_ocr_search))
        .route("/v1/control", post(post_control))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "control API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_health(State(st): State<ControlState>) -> impl IntoResponse {
    match build_health(&st).await {
        Ok(h) => (StatusCode::OK, Json(h)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

async fn get_ocr_search(
    State(st): State<ControlState>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    match search_ocr(&st, &q.q, q.limit) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn post_control(
    State(st): State<ControlState>,
    Json(req): Json<ControlRequest>,
) -> impl IntoResponse {
    match handle_control(&st, req).await {
        Ok(resp) => {
            let code = match &resp {
                ControlResponse::Error { .. } => StatusCode::BAD_REQUEST,
                _ => StatusCode::OK,
            };
            (code, Json(resp)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn handle_control(
    st: &ControlState,
    req: ControlRequest,
) -> Result<ControlResponse, anyhow::Error> {
    match req {
        ControlRequest::Health => Ok(ControlResponse::Health(build_health(st).await?)),
        ControlRequest::SearchOcr { query, limit } => {
            Ok(search_ocr(st, &query, limit.unwrap_or(20))?)
        }
        ControlRequest::ReindexOcr => {
            let indexed = st.store.reindex_ocr_docs()?;
            info!(indexed, "ocr reindex complete");
            Ok(ControlResponse::Reindex { indexed })
        }
        ControlRequest::ListEvents { limit } => {
            let events = st.store.list_recent(limit.clamp(1, 500)).await?;
            let summaries = events
                .into_iter()
                .map(|e| EventSummary {
                    id: e.id,
                    source: format!("{:?}", e.source),
                    kind: e.kind,
                    ts: e.ts,
                })
                .collect();
            Ok(ControlResponse::Events { events: summaries })
        }
        ControlRequest::Wipe => {
            st.store.wipe_all().await?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::Pause { .. } | ControlRequest::Resume { .. } => {
            // Privacy pause is config-driven in this phase; acknowledge only.
            Ok(ControlResponse::Ack)
        }
        ControlRequest::Permissions => Ok(ControlResponse::Error {
            message: "permissions probe not exposed on HTTP yet".into(),
        }),
    }
}

async fn build_health(st: &ControlState) -> Result<HealthResponse, anyhow::Error> {
    let stored = st.store.len().await?;
    let ocr_docs = st.store.ocr_doc_count().unwrap_or(0);
    Ok(HealthResponse {
        api_version: API_VERSION,
        product: "lumen-navi".into(),
        sources: st.sources.clone(),
        paused: st.paused,
        stored_events: stored,
        ocr_docs,
        schema_version: SCHEMA_VERSION,
    })
}

fn search_ocr(
    st: &ControlState,
    query: &str,
    limit: usize,
) -> Result<ControlResponse, anyhow::Error> {
    let hits = st.store.search_ocr(query, limit)?;
    let hits: Vec<OcrSearchHitDto> = hits
        .into_iter()
        .map(|h| OcrSearchHitDto {
            event_id: h.event_id,
            session_id: h.session_id,
            event_ts: h.event_ts,
            confidence: h.confidence,
            snippet: h.snippet,
            text_preview: h.text_preview,
        })
        .collect();
    Ok(ControlResponse::OcrSearch {
        query: query.to_string(),
        hits,
    })
}

/// Spawn the control server; logs and exits the task on bind/serve failure.
pub fn spawn(bind: &str, state: ControlState) -> Option<tokio::task::JoinHandle<()>> {
    let addr: SocketAddr = match bind.parse() {
        Ok(a) => a,
        Err(e) => {
            warn!(bind, error = %e, "invalid api.bind; control API disabled");
            return None;
        }
    };
    Some(tokio::spawn(async move {
        if let Err(e) = serve(addr, state).await {
            warn!(error = %e, "control API stopped");
        }
    }))
}
