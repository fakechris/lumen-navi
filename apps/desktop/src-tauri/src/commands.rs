//! Tauri commands for the Navi desktop shell.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Utc};
use lumen_api::{EventSummary, HealthResponse, OcrSearchHitDto, SourceStatus, API_VERSION};
use lumen_config::Config;
use lumen_platform_macos::{accessibility_permission_state, microphone_permission_state};
use lumen_store::{EventStore, TimelineQuery, SCHEMA_VERSION};
use lumen_types::event_kind;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::assistant::{self, AssistantJob};
use crate::selection_popup::{self, POPUP_LABEL};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct PermissionsDto {
    pub screen_recording: String,
    pub screen_capture_ready: Option<bool>,
    pub direct_capture_status: String,
    pub direct_capture_error: Option<String>,
    pub microphone: String,
    pub accessibility: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary {
    pub data_dir: String,
    pub config_path: String,
    pub screen: bool,
    pub audio: bool,
    pub browser: bool,
    pub ocr: bool,
    pub asr: bool,
    pub paused: bool,
    pub api_bind: String,
    pub audio_chunk_ms: u64,
    pub asr_locale: String,
    pub asr_engine: String,
    pub asr_model_dir: String,
    pub asr_http_base_url: String,
    pub asr_http_model: String,
    pub asr_fallback_speech: bool,
    pub system_audio: bool,
}

#[derive(Debug, Serialize)]
pub struct BrowserPairingDto {
    pub enabled: bool,
    pub configured: bool,
    pub endpoint: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct SourcesUpdate {
    pub screen: Option<bool>,
    pub audio: Option<bool>,
    pub browser: Option<bool>,
    pub ocr: Option<bool>,
    pub asr: Option<bool>,
    pub paused: Option<bool>,
    pub system_audio: Option<bool>,
    pub asr_engine: Option<String>,
    pub asr_model_dir: Option<String>,
    pub asr_http_base_url: Option<String>,
    pub asr_http_model: Option<String>,
    pub asr_locale: Option<String>,
    pub asr_fallback_speech: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct TimelineItemDto {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub ts: String,
    pub session_id: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub text_preview: Option<String>,
    pub text_kind: Option<String>,
    pub media_type: Option<String>,
    pub has_image: bool,
    pub artifact_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ObserveStatus {
    pub running: bool,
    pub pid: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct OnboardingState {
    pub needs_onboarding: bool,
    pub completed: bool,
    pub skipped: bool,
    pub step: u32,
    pub launch_observe: bool,
}

#[tauri::command]
pub async fn get_health(state: State<'_, AppState>) -> Result<HealthResponse, String> {
    let store = &state.store;
    let n = store.len().await.map_err(err)?;
    let ocr_docs = store.ocr_doc_count().unwrap_or(0);
    let paused = *state.paused.lock().map_err(err)?;
    let cfg = state.load_config().map_err(err)?;
    let observe = state.observe_running();
    let daemon_health = if observe {
        fetch_daemon_health(&cfg.api.bind).await
    } else {
        None
    };
    let browser = daemon_health
        .as_ref()
        .and_then(|health| health.browser.clone());
    Ok(HealthResponse {
        api_version: API_VERSION,
        product: "lumen-navi".into(),
        sources: health_sources(
            observe,
            cfg.sources.screen,
            cfg.sources.audio,
            daemon_health.as_ref(),
        ),
        paused,
        stored_events: n,
        ocr_docs,
        schema_version: SCHEMA_VERSION,
        browser,
    })
}

fn health_sources(
    observe: bool,
    screen_enabled: bool,
    audio_enabled: bool,
    daemon_health: Option<&HealthResponse>,
) -> Vec<SourceStatus> {
    if let Some(health) = daemon_health {
        return health.sources.clone();
    }
    vec![
        SourceStatus {
            id: "screen".into(),
            enabled: screen_enabled,
            running: false,
            last_error: observe.then(|| "Local service health is unavailable".into()),
        },
        SourceStatus {
            id: "audio".into(),
            enabled: audio_enabled,
            running: false,
            last_error: observe.then(|| "Local service health is unavailable".into()),
        },
    ]
}

async fn fetch_daemon_health(api_bind: &str) -> Option<HealthResponse> {
    let url = daemon_health_url(api_bind);
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(750))
        .build()
        .ok()?
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response.json::<HealthResponse>().await.ok()
}

fn daemon_health_url(api_bind: &str) -> String {
    let bind = api_bind.trim().trim_end_matches('/');
    if bind.starts_with("http://") || bind.starts_with("https://") {
        format!("{bind}/health")
    } else {
        format!("http://{bind}/health")
    }
}

#[tauri::command]
pub async fn get_permissions(state: State<'_, AppState>) -> Result<PermissionsDto, String> {
    let microphone = microphone_permission_state();
    let accessibility = accessibility_permission_state();
    let cua = state.cua.clone();
    let cua_status = tauri::async_runtime::spawn_blocking(move || cua.status())
        .await
        .map_err(err)?
        .map_err(|error| {
            tracing::warn!(%error, "Lumen Cua permission status unavailable");
        });
    let (screen_recording, screen_capture_ready, direct_capture_status, direct_capture_error) =
        match cua_status {
            Ok(status) => {
                let direct_capture_status = serde_json::to_value(status.direct_capture_status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".into());
                let direct_capture_error = status
                    .direct_capture_error
                    .map(|error| format!("{}: {}", error.code, error.message));
                (
                    format!("{:?}", status.screen_recording),
                    status.screen_recording_capturable,
                    direct_capture_status,
                    direct_capture_error,
                )
            }
            Err(()) => ("Unavailable".into(), None, "unavailable".into(), None),
        };
    Ok(PermissionsDto {
        screen_recording,
        screen_capture_ready,
        direct_capture_status,
        direct_capture_error,
        microphone: format!("{microphone:?}"),
        accessibility: format!("{accessibility:?}"),
    })
}

#[tauri::command]
pub fn search_text(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<OcrSearchHitDto>, String> {
    let hits = state
        .store
        .search_ocr(&query, limit.unwrap_or(30))
        .map_err(err)?;
    Ok(hits
        .into_iter()
        .map(|h| OcrSearchHitDto {
            event_id: h.event_id,
            session_id: h.session_id,
            event_ts: h.event_ts,
            confidence: h.confidence,
            snippet: h.snippet,
            text_preview: h.text_preview,
        })
        .collect())
}

#[tauri::command]
pub async fn list_events(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<EventSummary>, String> {
    let events = state
        .store
        .list_recent(limit.unwrap_or(50).clamp(1, 500))
        .await
        .map_err(err)?;
    Ok(events
        .into_iter()
        .rev()
        .map(|e| EventSummary {
            id: e.id,
            source: format!("{:?}", e.source),
            kind: e.kind,
            ts: e.ts,
        })
        .collect())
}

#[tauri::command]
pub fn reindex_search(state: State<'_, AppState>) -> Result<usize, String> {
    state.store.reindex_ocr_docs().map_err(err)
}

#[tauri::command]
pub fn get_config_summary(state: State<'_, AppState>) -> Result<ConfigSummary, String> {
    let cfg = state.load_config().map_err(err)?;
    let paused = *state.paused.lock().map_err(err)?;
    Ok(ConfigSummary {
        data_dir: cfg.data_dir.display().to_string(),
        config_path: state.config_path.display().to_string(),
        screen: cfg.sources.screen,
        audio: cfg.sources.audio,
        browser: cfg.sources.browser,
        ocr: cfg.ocr.enabled,
        asr: cfg.asr.enabled,
        paused,
        api_bind: cfg.api.bind.clone(),
        audio_chunk_ms: cfg.audio.chunk_ms,
        asr_locale: cfg.asr.locale.clone(),
        asr_engine: cfg.asr.engine.clone(),
        asr_model_dir: cfg.asr.model_dir.clone(),
        asr_http_base_url: cfg.asr.http_base_url.clone(),
        asr_http_model: cfg.asr.http_model.clone(),
        asr_fallback_speech: cfg.asr.fallback_speech,
        system_audio: cfg.audio.system_audio,
    })
}

#[tauri::command]
pub fn get_browser_pairing(state: State<'_, AppState>) -> Result<BrowserPairingDto, String> {
    let cfg = state.load_config().map_err(err)?;
    Ok(browser_pairing_dto(&cfg))
}

#[tauri::command]
pub fn enable_browser_pairing(
    state: State<'_, AppState>,
    rotate: bool,
) -> Result<BrowserPairingDto, String> {
    let mut cfg = state.load_config().map_err(err)?;
    ensure_browser_pairing(&mut cfg, rotate);
    state.save_config(&cfg).map_err(err)?;
    reload_local_service(&state)?;
    Ok(browser_pairing_dto(&cfg))
}

fn ensure_browser_pairing(cfg: &mut Config, rotate: bool) {
    cfg.api.enabled = true;
    cfg.sources.browser = true;
    if rotate || cfg.browser.ingest_token.is_empty() {
        cfg.browser.ingest_token =
            format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    }
}

fn browser_pairing_dto(cfg: &Config) -> BrowserPairingDto {
    let token = cfg.browser.effective_ingest_token();
    BrowserPairingDto {
        enabled: cfg.sources.browser,
        configured: cfg.sources.browser && !token.is_empty(),
        endpoint: daemon_health_url(&cfg.api.bind)
            .trim_end_matches("/health")
            .to_string(),
        token,
    }
}

#[tauri::command]
pub fn list_timeline(
    state: State<'_, AppState>,
    limit: Option<usize>,
    kind_contains: Option<String>,
    app_contains: Option<String>,
    since: Option<String>,
    until: Option<String>,
) -> Result<Vec<TimelineItemDto>, String> {
    let since = parse_opt_ts(since)?;
    let until = parse_opt_ts(until)?;
    let items = state
        .store
        .list_timeline(TimelineQuery {
            limit: limit.unwrap_or(80),
            kind_contains: kind_contains.unwrap_or_default(),
            app_contains: app_contains.unwrap_or_default(),
            since,
            until,
        })
        .map_err(err)?;
    Ok(items
        .into_iter()
        .map(|it| {
            let has_image = it
                .media_type
                .as_deref()
                .map(|m| m.starts_with("image/"))
                .unwrap_or(false);
            TimelineItemDto {
                id: it.id.to_string(),
                source: it.source,
                kind: it.kind,
                ts: it.ts.to_rfc3339(),
                session_id: it.session_id.map(|s| s.to_string()),
                app_name: it.app_name,
                window_title: it.window_title,
                text_preview: it.text_preview,
                text_kind: it.text_kind,
                media_type: it.media_type,
                has_image,
                artifact_bytes: it.artifact_bytes,
            }
        })
        .collect())
}

#[tauri::command]
pub fn get_event_image_data_url(
    state: State<'_, AppState>,
    event_id: String,
) -> Result<Option<String>, String> {
    get_event_media_data_url_inner(&state, &event_id, MediaKind::Image)
}

#[tauri::command]
pub fn get_event_media_data_url(
    state: State<'_, AppState>,
    event_id: String,
) -> Result<Option<String>, String> {
    get_event_media_data_url_inner(&state, &event_id, MediaKind::ImageOrAudio)
}

#[derive(Clone, Copy)]
enum MediaKind {
    Image,
    ImageOrAudio,
}

fn get_event_media_data_url_inner(
    state: &AppState,
    event_id: &str,
    kind: MediaKind,
) -> Result<Option<String>, String> {
    let id = Uuid::parse_str(event_id).map_err(|e| e.to_string())?;
    let Some((media, bytes)) = state.store.load_first_artifact_bytes(id).map_err(err)? else {
        return Ok(None);
    };
    if !media_allowed(&media, kind) {
        return Ok(None);
    }
    // Timeline media is loaded into the WebView as a data URL. Keep the
    // payload bounded even if a malformed artifact bypassed ingest limits.
    if bytes.len() > 10 * 1024 * 1024 {
        return Ok(None);
    }
    Ok(Some(format!("data:{media};base64,{}", B64.encode(&bytes))))
}

fn media_allowed(media: &str, kind: MediaKind) -> bool {
    media.starts_with("image/")
        || matches!(kind, MediaKind::ImageOrAudio) && media.starts_with("audio/")
}

#[tauri::command]
pub fn update_sources_config(
    state: State<'_, AppState>,
    update: SourcesUpdate,
) -> Result<ConfigSummary, String> {
    let mut cfg = state.load_config().map_err(err)?;
    if let Some(v) = update.screen {
        cfg.sources.screen = v;
    }
    if let Some(v) = update.audio {
        cfg.sources.audio = v;
    }
    if let Some(v) = update.browser {
        cfg.sources.browser = v;
    }
    if let Some(v) = update.ocr {
        cfg.ocr.enabled = v;
    }
    if let Some(v) = update.asr {
        cfg.asr.enabled = v;
    }
    if let Some(v) = update.paused {
        cfg.privacy.paused = v;
        *state.paused.lock().map_err(err)? = v;
    }
    if let Some(v) = update.system_audio {
        cfg.audio.system_audio = v;
    }
    if let Some(v) = update.asr_engine {
        let t = v.trim().to_ascii_lowercase();
        if !t.is_empty() {
            cfg.asr.engine = t;
        }
    }
    if let Some(v) = update.asr_model_dir {
        cfg.asr.model_dir = v;
    }
    if let Some(v) = update.asr_http_base_url {
        cfg.asr.http_base_url = v;
    }
    if let Some(v) = update.asr_http_model {
        cfg.asr.http_model = v;
    }
    if let Some(v) = update.asr_locale {
        let t = v.trim();
        if !t.is_empty() {
            cfg.asr.locale = t.to_string();
        }
    }
    if let Some(v) = update.asr_fallback_speech {
        cfg.asr.fallback_speech = v;
    }
    state.save_config(&cfg).map_err(err)?;
    reload_local_service(&state)?;
    get_config_summary(state)
}

fn reload_local_service(state: &AppState) -> Result<(), String> {
    observe_stop_inner(state)?;
    observe_start_inner(state)?;
    Ok(())
}

#[tauri::command]
pub fn generate_day_summary(
    state: State<'_, AppState>,
    day: Option<String>,
) -> Result<String, String> {
    let day = day.unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    let body = state.store.build_day_summary_body(&day).map_err(err)?;
    // Synthetic event so it appears on timeline / search.
    let event = lumen_types::SourceEvent::new(
        lumen_types::SourceKind::Other("summary".into()),
        event_kind::SUMMARY_V1,
        serde_json::json!({ "day": day, "kind": "day" }),
    );
    let eid = event.id;
    tauri::async_runtime::block_on(async { state.store.append(vec![event]).await.map_err(err) })?;
    state
        .store
        .insert_derived(eid, "summary.v1", body.clone())
        .map_err(err)?;
    Ok(body)
}

#[derive(Debug, Serialize)]
pub struct TranscriptExportDto {
    pub path: String,
    pub segments: usize,
    pub duration_seconds: Option<f64>,
}

/// Export one audio session's transcripts as a `lumen-transcript.v1` JSON
/// file (importable by lumen-cut). `dest_path` comes from a frontend save
/// dialog; empty → `<data_dir>/exports/<session>.lumen-transcript.json`.
#[tauri::command]
pub fn export_session_transcript(
    state: State<'_, AppState>,
    session_id: String,
    dest_path: Option<String>,
) -> Result<TranscriptExportDto, String> {
    let session = Uuid::parse_str(session_id.trim()).map_err(|e| format!("session_id: {e}"))?;
    let doc = lumen_process::export_session_transcript(&state.store, session).map_err(err)?;
    let json = doc.to_json_string_pretty().map_err(err)?;

    let path = match dest_path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            let dir = state.data_dir.join("exports");
            std::fs::create_dir_all(&dir).map_err(err)?;
            dir.join(format!("{session}.lumen-transcript.json"))
        }
    };
    std::fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(TranscriptExportDto {
        path: path.display().to_string(),
        segments: doc.segments.len(),
        duration_seconds: doc.media.as_ref().and_then(|m| m.duration_seconds),
    })
}

fn parse_opt_ts(s: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    match s {
        None => Ok(None),
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => DateTime::parse_from_rfc3339(raw.trim())
            .map(|d| Some(d.with_timezone(&Utc)))
            .or_else(|_| {
                // Accept date-only YYYY-MM-DD as start-of-day UTC.
                let padded = format!("{raw}T00:00:00Z");
                DateTime::parse_from_rfc3339(&padded)
                    .map(|d| Some(d.with_timezone(&Utc)))
                    .map_err(|e| e.to_string())
            }),
    }
}

#[tauri::command]
pub fn set_privacy_paused(state: State<'_, AppState>, paused: bool) -> Result<(), String> {
    let mut cfg = state.load_config().map_err(err)?;
    cfg.privacy.paused = paused;
    state.save_config(&cfg).map_err(err)?;
    *state.paused.lock().map_err(err)? = paused;
    reload_local_service(&state)?;
    Ok(())
}

#[tauri::command]
pub fn observe_status(state: State<'_, AppState>) -> Result<ObserveStatus, String> {
    let mut guard = state.observe_child.lock().map_err(err)?;
    let (running, pid) = match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                *guard = None;
                (false, None)
            }
            Ok(None) => (true, Some(child.id())),
            Err(_) => {
                *guard = None;
                (false, None)
            }
        },
        None => (false, None),
    };
    Ok(ObserveStatus { running, pid })
}

/// Shared start logic for command + auto-launch + tray.
pub fn observe_start_inner(state: &AppState) -> Result<ObserveStatus, String> {
    if state.observe_running() {
        let running = true;
        let pid = state
            .observe_child
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|c| c.id()));
        return Ok(ObserveStatus { running, pid });
    }
    let cfg = state.load_config().map_err(err)?;
    state.save_config(&cfg).map_err(err)?;

    let daemon = resolve_daemon_binary().ok_or_else(|| {
        String::from(
            "lumen-daemon binary not found. Build with: cargo build -p lumen-daemon --release",
        )
    })?;

    let log_path = state.data_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_path);
    let stdout = std::fs::File::create(log_path.join("daemon.stdout.log")).map_err(err)?;
    let stderr = std::fs::File::create(log_path.join("daemon.stderr.log")).map_err(err)?;

    let cua_ready = if cfg.sources.screen {
        match state.cua.ensure_running() {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(%error, "screen channel unavailable; starting other Observe channels");
                false
            }
        }
    } else {
        false
    };

    let mut daemon_command = Command::new(&daemon);
    daemon_command
        .current_dir(&state.data_dir)
        .env("LUMEN_NAVI_CONFIG", state.config_path.display().to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    if cua_ready {
        daemon_command
            .env("LUMEN_CUA_SOCKET", state.cua.socket_path())
            .env("LUMEN_CUA_TOKEN_FILE", state.cua.token_file());
    }
    let child = daemon_command
        .spawn()
        .map_err(|e| format!("spawn lumen-daemon: {e}"))?;

    let pid = child.id();
    *state.observe_child.lock().map_err(err)? = Some(child);
    tracing::info!(pid, path = %daemon.display(), "observe daemon started");
    Ok(ObserveStatus {
        running: true,
        pid: Some(pid),
    })
}

#[tauri::command]
pub fn observe_start(state: State<'_, AppState>) -> Result<ObserveStatus, String> {
    observe_start_inner(&state)
}

#[tauri::command]
pub fn observe_stop(state: State<'_, AppState>) -> Result<ObserveStatus, String> {
    observe_stop_inner(&state)
}

fn observe_stop_inner(state: &AppState) -> Result<ObserveStatus, String> {
    let mut guard = state.observe_child.lock().map_err(err)?;
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
        tracing::info!("observe daemon stopped");
    }
    Ok(ObserveStatus {
        running: false,
        pid: None,
    })
}

#[tauri::command]
pub fn open_data_dir(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.data_dir.display().to_string();
    open_path(&dir)
}

#[tauri::command]
pub fn get_onboarding(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    let shell = state.shell.lock().map_err(err)?;
    Ok(OnboardingState {
        needs_onboarding: shell.needs_onboarding(),
        completed: shell.onboarding_completed,
        skipped: shell.onboarding_skipped,
        step: shell.onboarding_step,
        launch_observe: shell.launch_observe,
    })
}

#[tauri::command]
pub fn set_onboarding_step(
    state: State<'_, AppState>,
    step: u32,
) -> Result<OnboardingState, String> {
    {
        let mut shell = state.shell.lock().map_err(err)?;
        shell.onboarding_step = step.min(4);
    }
    state.save_shell().map_err(err)?;
    get_onboarding(state)
}

#[tauri::command]
pub fn complete_onboarding(
    state: State<'_, AppState>,
    launch_observe: bool,
) -> Result<OnboardingState, String> {
    {
        let mut shell = state.shell.lock().map_err(err)?;
        shell.onboarding_completed = true;
        shell.onboarding_skipped = false;
        shell.launch_observe = launch_observe;
        shell.onboarding_step = 4;
    }
    state.save_shell().map_err(err)?;
    get_onboarding(state)
}

#[tauri::command]
pub fn skip_onboarding(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    {
        let mut shell = state.shell.lock().map_err(err)?;
        shell.onboarding_skipped = true;
        shell.onboarding_completed = false;
    }
    state.save_shell().map_err(err)?;
    get_onboarding(state)
}

#[tauri::command]
pub fn reopen_onboarding(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    {
        let mut shell = state.shell.lock().map_err(err)?;
        shell.onboarding_completed = false;
        shell.onboarding_skipped = false;
        shell.onboarding_step = 0;
    }
    state.save_shell().map_err(err)?;
    get_onboarding(state)
}

#[tauri::command]
pub fn set_launch_observe(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    {
        let mut shell = state.shell.lock().map_err(err)?;
        shell.launch_observe = enabled;
    }
    state.save_shell().map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn request_screen_permission(state: State<'_, AppState>) -> Result<bool, String> {
    let cua = state.cua.clone();
    tauri::async_runtime::spawn_blocking(move || cua.request_screen_permission())
        .await
        .map_err(|error| format!("Lumen Cua permission task failed: {error}"))?
}

#[tauri::command]
pub async fn refresh_screen_permission(state: State<'_, AppState>) -> Result<bool, String> {
    let cua = state.cua.clone();
    tauri::async_runtime::spawn_blocking(move || cua.refresh_screen_permission())
        .await
        .map_err(|error| format!("Lumen Cua permission refresh task failed: {error}"))?
}

#[tauri::command]
pub async fn request_microphone_permission() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(lumen_platform_macos::request_microphone_access)
        .await
        .map_err(|e| format!("microphone permission task failed: {e}"))?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_privacy_settings(kind: String) -> Result<(), String> {
    open_url(privacy_settings_url(&kind)?)
}

fn privacy_settings_url(kind: &str) -> Result<&'static str, String> {
    match kind {
        "screen" => {
            Ok("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture")
        }
        "microphone" => {
            Ok("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone")
        }
        "speech" => {
            Ok("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_SpeechRecognition")
        }
        "accessibility" => {
            Ok("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility")
        }
        _ => Err(format!("unknown privacy pane: {kind}")),
    }
}

fn open_path(path: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("open path only supported on macOS".into())
    }
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("open failed for {url}: {status}"))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("open url only supported on macOS".into())
    }
}

#[cfg(test)]
mod command_tests {
    use super::{
        daemon_health_url, ensure_browser_pairing, health_sources, media_allowed,
        privacy_settings_url, MediaKind,
    };
    use lumen_api::{HealthResponse, SourceStatus, API_VERSION};
    use lumen_config::Config;

    #[test]
    fn privacy_pane_routes_are_stable_and_unknown_values_fail() {
        assert!(privacy_settings_url("accessibility")
            .unwrap()
            .ends_with("Privacy_Accessibility"));
        assert!(privacy_settings_url("microphone")
            .unwrap()
            .ends_with("Privacy_Microphone"));
        assert!(privacy_settings_url("unknown").is_err());
    }

    #[test]
    fn daemon_health_url_accepts_bind_or_url() {
        assert_eq!(
            daemon_health_url("127.0.0.1:7420"),
            "http://127.0.0.1:7420/health"
        );
        assert_eq!(
            daemon_health_url("http://127.0.0.1:7420/"),
            "http://127.0.0.1:7420/health"
        );
    }

    #[test]
    fn daemon_source_health_overrides_process_level_inference() {
        let daemon = HealthResponse {
            api_version: API_VERSION,
            product: "lumen-navi".into(),
            sources: vec![SourceStatus {
                id: "screen".into(),
                enabled: true,
                running: false,
                last_error: Some("Screen Recording permission is required".into()),
            }],
            paused: false,
            stored_events: 0,
            ocr_docs: 0,
            schema_version: 0,
            browser: None,
        };

        let sources = health_sources(true, true, false, Some(&daemon));
        let screen = sources.iter().find(|source| source.id == "screen").unwrap();
        assert!(!screen.running);
        assert_eq!(
            screen.last_error.as_deref(),
            Some("Screen Recording permission is required")
        );
    }

    #[test]
    fn timeline_media_allows_images_and_audio_but_not_active_content() {
        assert!(media_allowed("image/jpeg", MediaKind::Image));
        assert!(!media_allowed("audio/wav", MediaKind::Image));
        assert!(media_allowed("audio/wav", MediaKind::ImageOrAudio));
        assert!(!media_allowed("text/html", MediaKind::ImageOrAudio));
    }

    #[test]
    fn enabling_browser_pairing_creates_a_stable_token_until_rotation() {
        let mut cfg = Config::default();
        ensure_browser_pairing(&mut cfg, false);
        let original = cfg.browser.ingest_token.clone();
        assert!(cfg.sources.browser);
        assert!(cfg.api.enabled);
        assert_eq!(original.len(), 64);

        ensure_browser_pairing(&mut cfg, false);
        assert_eq!(cfg.browser.ingest_token, original);
        ensure_browser_pairing(&mut cfg, true);
        assert_ne!(cfg.browser.ingest_token, original);
    }
}

fn resolve_daemon_binary() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1) Bundled next to the desktop binary (Tauri externalBin / DMG layout).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("lumen-daemon"));
            // Some layouts keep helpers under ../Resources
            if let Some(contents) = dir.parent() {
                candidates.push(contents.join("Resources/lumen-daemon"));
                candidates.push(contents.join("MacOS/lumen-daemon"));
            }
        }
    }

    // 2) Workspace builds during development.
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/release/lumen-daemon"),
    );
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/lumen-daemon"));

    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }

    // 3) PATH
    if let Ok(out) = Command::new("which").arg("lumen-daemon").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                let path = PathBuf::from(p);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn err(e: impl ToString) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Selection popup assistant (划词弹窗 + LLM)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AssistantConfigDto {
    pub enabled: bool,
    pub popup_enabled: bool,
    pub base_url: String,
    pub model: String,
    pub target_lang: String,
    pub max_selection_chars: usize,
    /// Never echoes the key back — only whether one is configured.
    pub api_key_set: bool,
    pub accessibility_trusted: bool,
    pub clipboard_fallback: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssistantUpdate {
    pub enabled: Option<bool>,
    pub popup_enabled: Option<bool>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub target_lang: Option<String>,
    /// `None` = keep, `Some("")` = clear, `Some(v)` = set.
    pub api_key: Option<String>,
    pub clipboard_fallback: Option<bool>,
}

fn assistant_dto(cfg: &lumen_config::Config) -> AssistantConfigDto {
    AssistantConfigDto {
        enabled: cfg.assistant.enabled,
        popup_enabled: cfg.assistant.popup_enabled,
        base_url: cfg.assistant.base_url.clone(),
        model: cfg.assistant.model.clone(),
        target_lang: cfg.assistant.target_lang.clone(),
        max_selection_chars: cfg.assistant.max_selection_chars,
        api_key_set: !cfg.assistant.effective_api_key().is_empty(),
        accessibility_trusted: lumen_platform_macos::accessibility_trusted(false),
        clipboard_fallback: cfg.assistant.clipboard_fallback,
    }
}

#[tauri::command]
pub fn assistant_get_config(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AssistantConfigDto, String> {
    let cfg = state.load_config().map_err(err)?;
    let dto = assistant_dto(&cfg);
    // Self-heal: settings UI polls this every few seconds; once Accessibility
    // is granted, (re)start the monitor without requiring a manual re-toggle.
    if cfg.assistant.popup_enabled && dto.accessibility_trusted {
        selection_popup::ensure_monitor(&app);
    }
    Ok(dto)
}

#[tauri::command]
pub fn assistant_update_config(
    app: AppHandle,
    state: State<'_, AppState>,
    update: AssistantUpdate,
) -> Result<AssistantConfigDto, String> {
    let mut cfg = state.load_config().map_err(err)?;
    if let Some(v) = update.enabled {
        cfg.assistant.enabled = v;
    }
    if let Some(v) = update.popup_enabled {
        cfg.assistant.popup_enabled = v;
    }
    if let Some(v) = update.base_url {
        cfg.assistant.base_url = v.trim().to_string();
    }
    if let Some(v) = update.model {
        let t = v.trim();
        if !t.is_empty() {
            cfg.assistant.model = t.to_string();
        }
    }
    if let Some(v) = update.target_lang {
        let t = v.trim();
        if !t.is_empty() {
            cfg.assistant.target_lang = t.to_string();
        }
    }
    if let Some(v) = update.api_key {
        cfg.assistant.api_key = v.trim().to_string();
    }
    if let Some(v) = update.clipboard_fallback {
        cfg.assistant.clipboard_fallback = v;
    }
    state.save_config(&cfg).map_err(err)?;
    selection_popup::set_popup_enabled(&app, cfg.assistant.popup_enabled);
    Ok(assistant_dto(&cfg))
}

#[tauri::command]
pub fn request_accessibility_permission() -> Result<bool, String> {
    Ok(lumen_platform_macos::accessibility_trusted(true))
}

/// Start a streaming assistant request; returns its id. Progress arrives as
/// `assistant-stream` / `assistant-done` / `assistant-error` popup events.
#[tauri::command]
pub fn assistant_run(
    app: AppHandle,
    state: State<'_, AppState>,
    action: String,
    text: String,
    question: Option<String>,
) -> Result<String, String> {
    let cfg = state.load_config().map_err(err)?.assistant;
    if !cfg.enabled {
        return Err("assistant is disabled (enable it in Settings)".into());
    }
    let action = assistant::AssistantAction::parse(&action)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("empty selection text".into());
    }
    if action == assistant::AssistantAction::Ask
        && question.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        return Err("ask action requires a question".into());
    }

    let id = Uuid::new_v4().to_string();
    let handle = app.clone();
    let job = AssistantJob {
        id: id.clone(),
        action,
        text,
        question,
    };
    let task_id = id.clone();
    let join = tauri::async_runtime::spawn(async move {
        let result = assistant::run_stream(handle.clone(), cfg, job).await;
        if let Some(st) = handle.try_state::<AppState>() {
            if let Ok(mut tasks) = st.assistant_tasks.lock() {
                tasks.remove(&task_id);
            }
        }
        match result {
            Ok(()) => {
                let _ = handle.emit_to(POPUP_LABEL, "assistant-done", json!({ "id": task_id }));
            }
            Err(e) => {
                let _ = handle.emit_to(
                    POPUP_LABEL,
                    "assistant-error",
                    json!({ "id": task_id, "message": e }),
                );
            }
        }
    });
    state
        .assistant_tasks
        .lock()
        .map_err(err)?
        .insert(id.clone(), join);
    Ok(id)
}

#[tauri::command]
pub fn assistant_cancel(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let handle = state.assistant_tasks.lock().map_err(err)?.remove(&id);
    if let Some(h) = handle {
        h.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn selection_popup_hide(app: AppHandle) -> Result<(), String> {
    selection_popup::hide_popup(&app);
    Ok(())
}

/// Popup webview pulls this on load (avoids racing `selection-changed`).
#[tauri::command]
pub fn selection_popup_current() -> Result<Option<String>, String> {
    Ok(selection_popup::take_pending_text())
}
