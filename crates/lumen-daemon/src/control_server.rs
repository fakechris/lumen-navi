//! Loopback HTTP control plane for health + OCR search.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use lumen_api::{
    BrowserHealthResponse, BrowserIngestResponse, BrowserPolicyResponse, ControlRequest,
    ControlResponse, EventSummary, HealthResponse, OcrSearchHitDto, SourceStatus, API_VERSION,
};
use lumen_sources_browser::{
    validate_batch, BrowserBatch, BrowserIngestPolicy, BROWSER_SCHEMA_VERSION,
};
use lumen_store::{
    ArtifactInput, BlobLimitedAppendOutcome, EventStore, EventWithArtifacts, SqliteStore,
    SCHEMA_VERSION,
};
use lumen_types::SourceKind;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct ControlState {
    pub store: Arc<SqliteStore>,
    pub paused: Arc<AtomicBool>,
    closed_eyes: bool,
    max_blob_bytes: u64,
    screen_locked: Arc<dyn Fn() -> bool + Send + Sync>,
    pub sources: Vec<SourceStatus>,
    pub browser: BrowserRuntimeState,
}

#[derive(Debug, Clone)]
pub struct BrowserRuntimeConfig {
    pub enabled: bool,
    pub token: String,
    pub policy: BrowserIngestPolicy,
}

#[derive(Debug, Default)]
struct BrowserMetrics {
    accepted_events: u64,
    duplicate_events: u64,
    rejected_batches: u64,
    last_ingest_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct BrowserRuntimeState {
    enabled: bool,
    token: String,
    policy: BrowserIngestPolicy,
    paused: Arc<AtomicBool>,
    metrics: Arc<Mutex<BrowserMetrics>>,
}

impl ControlState {
    pub fn new(
        store: Arc<SqliteStore>,
        paused: bool,
        closed_eyes: bool,
        max_blob_bytes: u64,
        sources: Vec<SourceStatus>,
        browser: BrowserRuntimeConfig,
    ) -> Self {
        Self {
            store,
            paused: Arc::new(AtomicBool::new(paused)),
            closed_eyes,
            max_blob_bytes,
            screen_locked: Arc::new(lumen_platform_host::is_screen_locked),
            sources,
            browser: BrowserRuntimeState {
                enabled: browser.enabled,
                token: browser.token,
                policy: browser.policy,
                paused: Arc::new(AtomicBool::new(false)),
                metrics: Arc::new(Mutex::new(BrowserMetrics::default())),
            },
        }
    }
}

/// Primary control channel: a Unix domain socket. The desktop shell connects
/// here, so there is no TCP port to allocate or conflict over. Mirrors the
/// `lumen-cua` socket lifecycle: create parent dir, probe-and-unlink stale
/// socket, bind, chmod 0o600, serve. The socket file is removed on clean exit.
pub async fn serve(socket_path: &Path, state: ControlState) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("create socket dir {}: {e}", parent.display()))?;
        }
        // Stale-socket detection: if connect() succeeds, a live daemon owns it.
        if socket_path.exists() {
            if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                anyhow::bail!(
                    "control socket already in use by a live daemon: {}",
                    socket_path.display()
                );
            }
            let _ = std::fs::remove_file(socket_path);
        }
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| anyhow::anyhow!("bind socket {}: {e}", socket_path.display()))?;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| anyhow::anyhow!("chmod socket {}: {e}", socket_path.display()))?;

        let app = router(state);
        info!(socket = %socket_path.display(), "control API listening (unix socket)");
        // Remove the socket file when serve returns (shutdown) so the next
        // start sees a clean path.
        let path_for_cleanup = socket_path.to_path_buf();
        let result = axum::serve(listener, app).await;
        let _ = std::fs::remove_file(&path_for_cleanup);
        result.map_err(|e| anyhow::anyhow!("control socket serve: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = (socket_path, state);
        anyhow::bail!("unix sockets unsupported on this platform")
    }
}

/// Secondary control channel for consumers that can only speak HTTP over TCP
/// (the browser extension — browsers cannot open Unix sockets). Best-effort:
/// tries the configured `bind`, then increments the port up to `max_attempts`
/// times; on success writes the actual port (with PID) to `port_file` so a
/// future extension discovery path can read it. Bind failure is logged at WARN
/// and returns Ok(()) — the extension is optional; the shell uses the socket.
pub async fn serve_tcp(
    bind: &str,
    max_attempts: u32,
    port_file: &Path,
    state: ControlState,
) -> anyhow::Result<()> {
    let app = router(state.clone());
    let addr: SocketAddr = match bind.parse() {
        Ok(a) => a,
        Err(e) => {
            warn!(bind, error = %e, "invalid api.bind for TCP listener; browser extension disabled");
            return Ok(());
        }
    };
    let mut bound_addr = addr;
    let listener = {
        let mut last_err = None;
        let mut ok = None;
        for i in 0..max_attempts {
            let candidate = SocketAddr::new(addr.ip(), addr.port().saturating_add(i as u16));
            match tokio::net::TcpListener::bind(candidate).await {
                Ok(l) => {
                    bound_addr = candidate;
                    ok = Some(l);
                    break;
                }
                Err(e) => last_err = Some((candidate, e)),
            }
        }
        match ok {
            Some(l) => l,
            None => {
                if let Some((c, e)) = last_err {
                    warn!(attempted = %c, error = %e, "TCP control API disabled (port range exhausted); browser extension will not connect");
                }
                return Ok(());
            }
        }
    };
    // Publish the actual port (+ pid) so the shell/extension can discover it.
    // Stale-file detection on read: caller should verify the pid is alive.
    if let Some(parent) = port_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let pid = std::process::id();
    let port_line = format!("{}\n{}\n", bound_addr.port(), pid);
    let _ = std::fs::write(port_file, port_line);

    info!(addr = %bound_addr, "control API listening (tcp, for browser extension)");
    axum::serve(listener, app).await.map_err(|e| anyhow::anyhow!("control tcp serve: {e}"))
}

pub fn router(state: ControlState) -> Router {
    Router::new()
        .route("/health", get(get_health))
        .route("/v1/health", get(get_health))
        .route("/v1/ocr/search", get(get_ocr_search))
        .route("/v1/browser/batches", post(post_browser_batch))
        .route("/v1/browser/policy", get(get_browser_policy))
        .route("/v1/browser/export", get(get_browser_export))
        .route("/v1/control", post(post_control))
        .route("/v1/activity/segments", get(get_activity_segments))
        .route("/v1/activity/scenes", get(get_activity_scenes))
        .route("/v1/activity/stats", get(get_activity_stats))
        .route("/v1/activity/range", get(get_activity_range))
        .route("/v1/activity/rules", get(get_activity_rules).post(post_activity_rules))
        .route("/v1/activity/segment", post(post_activity_segment).delete(delete_activity_segment))
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
        .with_state(state)
}

async fn get_browser_policy(
    State(st): State<ControlState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(status) = authorize_browser(&st, &headers) {
        return status.into_response();
    }
    Json(BrowserPolicyResponse {
        schema_version: BROWSER_SCHEMA_VERSION,
        capture_allowed: !st.paused.load(Ordering::Relaxed)
            && !st.browser.paused.load(Ordering::Relaxed)
            && !st.closed_eyes
            && !(st.screen_locked)(),
        content_allow_hosts: st.browser.policy.content_allow_hosts.clone(),
        excluded_hosts: st.browser.policy.excluded_hosts.clone(),
        max_batch_size: st.browser.policy.max_batch_size,
        max_artifact_bytes: st.browser.policy.max_artifact_bytes,
    })
    .into_response()
}

async fn post_browser_batch(
    State(st): State<ControlState>,
    headers: HeaderMap,
    Json(batch): Json<BrowserBatch>,
) -> impl IntoResponse {
    if let Err(status) = authorize_browser(&st, &headers) {
        return status.into_response();
    }
    if st.paused.load(Ordering::Relaxed)
        || st.browser.paused.load(Ordering::Relaxed)
        || st.closed_eyes
        || (st.screen_locked)()
    {
        return StatusCode::LOCKED.into_response();
    }

    let validated = match validate_batch(batch, &st.browser.policy) {
        Ok(value) => value,
        Err(error) => {
            if let Ok(mut metrics) = st.browser.metrics.lock() {
                metrics.rejected_batches += 1;
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            )
                .into_response();
        }
    };

    let validation_rejected_artifacts = validated.rejected_artifacts;
    let mut artifacts_by_event: HashMap<_, Vec<ArtifactInput>> = HashMap::new();
    for artifact in validated.artifacts {
        artifacts_by_event
            .entry(artifact.event_id)
            .or_default()
            .push(ArtifactInput {
                media_type: artifact.media_type,
                bytes: artifact.bytes,
            });
    }
    let records: Vec<EventWithArtifacts> = validated
        .events
        .into_iter()
        .map(|event| EventWithArtifacts {
            artifacts: artifacts_by_event.remove(&event.id).unwrap_or_default(),
            event,
        })
        .collect();
    let rejected_artifacts = records.iter().map(|record| record.artifacts.len()).sum::<usize>();
    let fallback_records = records.clone();
    let (outcome, rejected_artifacts) = match st
        .store
        .append_idempotent_with_artifacts_up_to(records, st.max_blob_bytes)
    {
        Ok(BlobLimitedAppendOutcome::Appended(value)) => {
            (value, validation_rejected_artifacts)
        }
        Ok(BlobLimitedAppendOutcome::LimitExceeded) => {
            let metadata_only = fallback_records
                .into_iter()
                .map(|mut record| {
                    if !record.artifacts.is_empty() {
                        record.artifacts.clear();
                        if let Some(data) = record
                            .event
                            .payload
                            .get_mut("data")
                            .and_then(serde_json::Value::as_object_mut)
                        {
                            data.insert(
                                "extraction_status".into(),
                                json!("retention_blocked"),
                            );
                            data.insert("privacy_gate".into(), json!("metadata_only"));
                        }
                    }
                    record
                })
                .collect();
            match st
                .store
                .append_idempotent_with_artifacts_up_to(metadata_only, st.max_blob_bytes)
            {
                Ok(BlobLimitedAppendOutcome::Appended(value)) => {
                    (value, validation_rejected_artifacts + rejected_artifacts)
                }
                Ok(BlobLimitedAppendOutcome::LimitExceeded) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "metadata-only fallback exceeded blob limit"})),
                    )
                        .into_response();
                }
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": error.to_string()})),
                    )
                        .into_response();
                }
            }
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
                .into_response();
        }
    };
    if let Ok(mut metrics) = st.browser.metrics.lock() {
        metrics.accepted_events += outcome.accepted as u64;
        metrics.duplicate_events += outcome.duplicates as u64;
        metrics.last_ingest_at = Some(Utc::now());
    }
    (
        StatusCode::OK,
        Json(BrowserIngestResponse {
            schema_version: BROWSER_SCHEMA_VERSION,
            accepted: outcome.accepted,
            duplicates: outcome.duplicates,
            rejected_artifacts,
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct BrowserExportQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "default_export_limit")]
    limit: usize,
}

fn default_export_limit() -> usize {
    1_000
}

async fn get_browser_export(
    State(st): State<ControlState>,
    headers: HeaderMap,
    Query(query): Query<BrowserExportQuery>,
) -> impl IntoResponse {
    if let Err(status) = authorize_browser(&st, &headers) {
        return status.into_response();
    }
    let events =
        match st
            .store
            .list_source_after_cursor(&SourceKind::Browser, query.after, query.limit)
        {
            Ok(value) => value,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": error.to_string()})),
                )
                    .into_response();
            }
        };
    let next_cursor = events.last().map(|item| item.cursor).unwrap_or(query.after);
    let mut records = Vec::with_capacity(events.len() * 2);
    for item in events {
        let source_event_id = item.event.id;
        let projection = if item.event.kind == "browser.visit_closed.v1" {
            item.event
                .session_id
                .and_then(|visit_id| st.store.get_browser_visit(visit_id).ok().flatten())
        } else {
            None
        };
        records.push(
            serde_json::to_string(&json!({
                "record_type": "event",
                "cursor": item.cursor,
                "event": item.event,
            }))
            .expect("event serialization"),
        );
        if let Some(visit) = projection {
            records.push(
                serde_json::to_string(&json!({
                    "record_type": "visit_projection",
                    "source_event_id": source_event_id,
                    "visit": visit,
                }))
                .expect("visit projection serialization"),
            );
        }
    }
    let records_body = records.join("\n");
    let checksum = blake3::hash(records_body.as_bytes()).to_hex().to_string();
    let mut lines = Vec::with_capacity(records.len() + 2);
    lines.push(
        serde_json::to_string(&json!({
            "record_type": "export_header",
            "export_schema_version": 1,
            "browser_schema_version": BROWSER_SCHEMA_VERSION,
            "navi_version": env!("CARGO_PKG_VERSION"),
            "generated_at": Utc::now(),
            "after": query.after,
            "record_count": records.len(),
            "records_checksum": checksum,
            "checksum_algorithm": "blake3",
        }))
        .expect("export header serialization"),
    );
    lines.extend(records);
    lines.push(
        serde_json::to_string(&json!({
            "record_type": "export_cursor",
            "next_cursor": next_cursor,
        }))
        .expect("export cursor serialization"),
    );
    (
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        lines.join("\n") + "\n",
    )
        .into_response()
}

fn authorize_browser(st: &ControlState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !st.browser.enabled || st.browser.token.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let expected = format!("Bearer {}", st.browser.token);
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
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

#[derive(Deserialize)]
struct ActivityDayQuery {
    day: String,
    /// "app" (default) or "site" — switches top-apps grouping between bundle
    /// identity and registrable domain.
    #[serde(default)]
    group_by: Option<String>,
}

#[derive(Deserialize)]
struct ActivityRangeQuery {
    from: String,
    to: String,
    #[serde(default)]
    group_by: Option<String>,
}

/// Parse the `group_by` query param into the store enum. Anything other than
/// "site" (case-insensitive) falls back to the default App grouping.
fn parse_group_by(s: Option<&str>) -> lumen_store::GroupBy {
    match s.map(|x| x.to_ascii_lowercase()).as_deref() {
        Some("site") | Some("domain") | Some("website") => lumen_store::GroupBy::Site,
        _ => lumen_store::GroupBy::App,
    }
}

#[derive(Deserialize)]
struct ManualSegmentRequest {
    started_at: chrono::DateTime<chrono::Utc>,
    ended_at: chrono::DateTime<chrono::Utc>,
    app_name: String,
    #[serde(default)]
    window_title: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    productivity_level: Option<String>,
}

#[derive(Deserialize)]
struct DeleteSegmentQuery {
    seg_id: String,
}

async fn get_activity_segments(
    State(st): State<ControlState>,
    Query(q): Query<ActivityDayQuery>,
) -> impl IntoResponse {
    match st.store.list_activity_segments(&q.day) {
        Ok(segments) => (StatusCode::OK, Json(segments)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_activity_scenes(
    State(st): State<ControlState>,
    Query(q): Query<ActivityDayQuery>,
) -> impl IntoResponse {
    match st.store.list_scene_day(&q.day) {
        Ok(day) => (StatusCode::OK, Json(day)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_activity_stats(
    State(st): State<ControlState>,
    Query(q): Query<ActivityDayQuery>,
) -> impl IntoResponse {
    match st.store.activity_day_stats(&q.day, parse_group_by(q.group_by.as_deref())) {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_activity_range(
    State(st): State<ControlState>,
    Query(q): Query<ActivityRangeQuery>,
) -> impl IntoResponse {
    match st.store.activity_range_stats(&q.from, &q.to, parse_group_by(q.group_by.as_deref())) {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn post_activity_segment(
    State(st): State<ControlState>,
    Json(req): Json<ManualSegmentRequest>,
) -> impl IntoResponse {
    match st.store.add_manual_segment(
        req.started_at,
        req.ended_at,
        &req.app_name,
        req.window_title.as_deref(),
        req.category.as_deref(),
        req.productivity_level.as_deref(),
    ) {
        Ok(seg_id) => (StatusCode::OK, Json(serde_json::json!({ "seg_id": seg_id }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error { message: e.to_string() }),
        ).into_response(),
    }
}

async fn delete_activity_segment(
    State(st): State<ControlState>,
    Query(q): Query<DeleteSegmentQuery>,
) -> impl IntoResponse {
    match st.store.delete_manual_segment(&q.seg_id) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error { message: e.to_string() }),
        ).into_response(),
    }
}

async fn get_activity_rules(
    State(st): State<ControlState>,
) -> impl IntoResponse {
    match st.store.list_category_rules() {
        Ok(rules) => (StatusCode::OK, Json(rules)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse::Error {
                message: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn post_activity_rules(
    State(st): State<ControlState>,
    Json(rules): Json<Vec<lumen_store::CategoryRule>>,
) -> impl IntoResponse {
    match st.store.save_category_rules_and_reapply(rules) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
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
    headers: HeaderMap,
    Json(req): Json<ControlRequest>,
) -> impl IntoResponse {
    let controls_browser = matches!(
        &req,
        ControlRequest::Pause { source } | ControlRequest::Resume { source }
            if source.as_deref() == Some("browser")
    );
    if controls_browser {
        if let Err(status) = authorize_browser(&st, &headers) {
            return status.into_response();
        }
    }
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
        ControlRequest::Pause { source } => {
            if source.as_deref() == Some("browser") {
                st.browser.paused.store(true, Ordering::Relaxed);
            } else {
                st.paused.store(true, Ordering::Relaxed);
            }
            Ok(ControlResponse::Ack)
        }
        ControlRequest::Resume { source } => {
            if source.as_deref() == Some("browser") {
                st.browser.paused.store(false, Ordering::Relaxed);
            } else {
                st.paused.store(false, Ordering::Relaxed);
            }
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
    let browser_metrics = st
        .browser
        .metrics
        .lock()
        .map_err(|_| anyhow::anyhow!("browser metrics lock poisoned"))?;
    Ok(HealthResponse {
        api_version: API_VERSION,
        product: "lumen-navi".into(),
        sources: st.sources.clone(),
        paused: st.paused.load(Ordering::Relaxed),
        stored_events: stored,
        ocr_docs,
        schema_version: SCHEMA_VERSION,
        browser: Some(BrowserHealthResponse {
            enabled: st.browser.enabled,
            configured: !st.browser.token.is_empty(),
            paused: st.browser.paused.load(Ordering::Relaxed),
            accepted_events: browser_metrics.accepted_events,
            duplicate_events: browser_metrics.duplicate_events,
            rejected_batches: browser_metrics.rejected_batches,
            last_ingest_at: browser_metrics.last_ingest_at,
        }),
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

/// Spawn the primary Unix-socket control server. Bind failure is FATAL: the
/// shell depends on this socket, so if it can't come up we'd rather exit and
/// let the supervisor (lib.rs) alert + restart than run a daemon the shell
/// can't talk to. Stale-socket detection happens inside `serve`.
pub fn spawn(socket_path: &Path, state: ControlState) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let path = socket_path.to_path_buf();
    Ok(tokio::spawn(async move {
        if let Err(e) = serve(&path, state).await {
            error!(error = %e, socket = %path.display(), "control socket stopped (fatal)");
            // Propagate to process exit so the supervisor sees a crash.
            std::process::exit(1);
        }
    }))
}

/// Spawn the secondary TCP listener for the browser extension. Best-effort:
/// any failure (bad bind string, port exhaustion) is logged and swallowed —
/// the shell uses the socket, the extension is optional.
pub fn spawn_tcp(
    bind: &str,
    max_attempts: u32,
    port_file: &Path,
    state: ControlState,
) -> Option<tokio::task::JoinHandle<()>> {
    let bind = bind.to_string();
    let port_file = port_file.to_path_buf();
    Some(tokio::spawn(async move {
        if let Err(e) = serve_tcp(&bind, max_attempts, &port_file, state).await {
            warn!(error = %e, "control TCP listener stopped (non-fatal; browser extension affected)");
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use http_body_util::BodyExt;
    use lumen_sources_browser::{
        BrowserArtifact, BrowserBatch, BrowserIngestPolicy, BrowserObservation,
        BROWSER_SCHEMA_VERSION,
    };
    use serde_json::json;
    use tempfile::tempdir;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{router, BrowserRuntimeConfig, ControlState};

    fn state() -> (tempfile::TempDir, ControlState) {
        let dir = tempdir().unwrap();
        let store = Arc::new(lumen_store::SqliteStore::open(dir.path()).unwrap());
        let mut state = ControlState::new(
            store,
            false,
            false,
            1024 * 1024,
            vec![],
            BrowserRuntimeConfig {
                enabled: true,
                token: "fixture-browser-token".into(),
                policy: BrowserIngestPolicy::default(),
            },
        );
        // Unit tests must not depend on whether the host desktop happens to be locked.
        state.screen_locked = Arc::new(|| false);
        (dir, state)
    }

    fn request_body() -> Vec<u8> {
        serde_json::to_vec(&BrowserBatch {
            installation_id: "00000000-0000-4000-8000-000000000001".into(),
            schema_version: BROWSER_SCHEMA_VERSION,
            capture_profile_version: "browser-mvp-v1".into(),
            config_hash: "fixture-config-hash".into(),
            observations: vec![BrowserObservation {
                id: Uuid::parse_str("00000000-0000-4000-8000-000000000101").unwrap(),
                kind: "browser.navigation_committed.v1".into(),
                ts: Utc::now(),
                visit_id: Uuid::parse_str("00000000-0000-4000-8000-000000000201").unwrap(),
                document_id: Some("fixture-document".into()),
                url: Some("https://example.test/article?token=secret".into()),
                payload: json!({"transition": "typed"}),
            }],
            artifacts: vec![],
        })
        .unwrap()
    }

    fn ingest_request(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/v1/browser/batches")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(Body::from(request_body())).unwrap()
    }

    #[tokio::test]
    async fn browser_ingest_requires_token_and_is_replay_safe() {
        let (_dir, state) = state();
        let app = router(state);

        let unauthorized = app.clone().oneshot(ingest_request(None)).await.unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let unauthorized_pause = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/control")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"op":"pause","source":"browser"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized_pause.status(), StatusCode::UNAUTHORIZED);

        let policy = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/browser/policy")
                    .header("authorization", "Bearer fixture-browser-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(policy.status(), StatusCode::OK);

        let accepted = app
            .clone()
            .oneshot(ingest_request(Some("fixture-browser-token")))
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        let accepted_body = accepted.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&accepted_body).unwrap()["accepted"],
            1
        );

        let replay = app
            .clone()
            .oneshot(ingest_request(Some("fixture-browser-token")))
            .await
            .unwrap();
        let replay_body = replay.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&replay_body).unwrap()["duplicates"],
            1
        );

        let export = app
            .oneshot(
                Request::builder()
                    .uri("/v1/browser/export?after=0&limit=10")
                    .header("authorization", "Bearer fixture-browser-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(export.status(), StatusCode::OK);
        let export_body = export.into_body().collect().await.unwrap().to_bytes();
        let export_text = String::from_utf8(export_body.to_vec()).unwrap();
        let header: serde_json::Value =
            serde_json::from_str(export_text.lines().next().unwrap()).unwrap();
        assert_eq!(header["navi_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(header["checksum_algorithm"], "blake3");
        assert!(header["records_checksum"].as_str().unwrap().len() >= 64);
        assert!(export_text.contains("browser.navigation_committed.v1"));
        assert!(!export_text.contains("secret"));
        assert!(export_text.contains("export_cursor"));
    }

    #[tokio::test]
    async fn closed_eyes_is_a_hard_browser_write_gate() {
        let (_dir, mut state) = state();
        state.closed_eyes = true;
        let response = router(state)
            .oneshot(ingest_request(Some("fixture-browser-token")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::LOCKED);
    }

    #[tokio::test]
    async fn a_locked_screen_is_a_hard_browser_write_gate() {
        let (_dir, mut state) = state();
        state.screen_locked = Arc::new(|| true);
        let response = router(state)
            .oneshot(ingest_request(Some("fixture-browser-token")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::LOCKED);
    }

    #[tokio::test]
    async fn artifact_intake_respects_the_blob_retention_limit() {
        let (_dir, mut state) = state();
        state.max_blob_bytes = 1;
        state.browser.policy.content_allow_hosts = vec!["example.test".into()];
        let event_id = Uuid::parse_str("00000000-0000-4000-8000-000000000102").unwrap();
        let body = serde_json::to_vec(&BrowserBatch {
            installation_id: "00000000-0000-4000-8000-000000000001".into(),
            schema_version: BROWSER_SCHEMA_VERSION,
            capture_profile_version: "browser-mvp-v1".into(),
            config_hash: "fixture-config-hash".into(),
            observations: vec![BrowserObservation {
                id: event_id,
                kind: "browser.document_ready.v1".into(),
                ts: Utc::now(),
                visit_id: Uuid::parse_str("00000000-0000-4000-8000-000000000202").unwrap(),
                document_id: Some("fixture-document".into()),
                url: Some("https://example.test/article".into()),
                payload: json!({
                    "privacy_gate": "allowed",
                    "extraction_status": "success",
                    "has_password_input": false,
                    "has_email_input": false,
                    "has_contenteditable": false,
                    "noindex": false
                }),
            }],
            artifacts: vec![BrowserArtifact {
                event_id,
                media_type: "text/markdown".into(),
                body: "too large for retention".into(),
                content_hash: None,
            }],
        })
        .unwrap();
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/browser/batches")
                    .header("authorization", "Bearer fixture-browser-token")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["rejected_artifacts"],
            1
        );
    }
}
