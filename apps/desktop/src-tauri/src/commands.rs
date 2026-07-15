//! Tauri commands for the Navi desktop shell.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Utc};
use lumen_api::{EventSummary, HealthResponse, OcrSearchHitDto, SourceStatus, API_VERSION};
use lumen_platform::PermissionProbe;
use lumen_platform_macos::MacPermissions;
use lumen_store::{EventStore, SCHEMA_VERSION, TimelineQuery};
use lumen_types::event_kind;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct PermissionsDto {
    pub screen_recording: String,
    pub microphone: String,
    pub accessibility: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary {
    pub data_dir: String,
    pub config_path: String,
    pub screen: bool,
    pub audio: bool,
    pub ocr: bool,
    pub asr: bool,
    pub paused: bool,
    pub api_bind: String,
    pub audio_chunk_ms: u64,
    pub asr_locale: String,
    pub system_audio: bool,
}

#[derive(Debug, Deserialize)]
pub struct SourcesUpdate {
    pub screen: Option<bool>,
    pub audio: Option<bool>,
    pub ocr: Option<bool>,
    pub asr: Option<bool>,
    pub paused: Option<bool>,
    pub system_audio: Option<bool>,
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
    Ok(HealthResponse {
        api_version: API_VERSION,
        product: "lumen-navi".into(),
        sources: vec![
            SourceStatus {
                id: "screen".into(),
                enabled: cfg.sources.screen,
                running: observe && cfg.sources.screen,
                last_error: None,
            },
            SourceStatus {
                id: "audio".into(),
                enabled: cfg.sources.audio,
                running: observe && cfg.sources.audio,
                last_error: None,
            },
        ],
        paused,
        stored_events: n,
        ocr_docs,
        schema_version: SCHEMA_VERSION,
    })
}

#[tauri::command]
pub async fn get_permissions() -> Result<PermissionsDto, String> {
    let p = MacPermissions;
    let st = p.status().await.map_err(|e| e.to_string())?;
    Ok(PermissionsDto {
        screen_recording: format!("{:?}", st.screen_recording),
        microphone: format!("{:?}", st.microphone),
        accessibility: format!("{:?}", st.accessibility),
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
        ocr: cfg.ocr.enabled,
        asr: cfg.asr.enabled,
        paused,
        api_bind: cfg.api.bind.clone(),
        audio_chunk_ms: cfg.audio.chunk_ms,
        asr_locale: cfg.asr.locale.clone(),
        system_audio: cfg.audio.system_audio,
    })
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
    let id = Uuid::parse_str(&event_id).map_err(|e| e.to_string())?;
    let Some((media, bytes)) = state.store.load_first_artifact_bytes(id).map_err(err)? else {
        return Ok(None);
    };
    if !media.starts_with("image/") {
        return Ok(None);
    }
    // Cap thumbnail payload (~1.5MB base64 ~ 2MB string).
    if bytes.len() > 1_500_000 {
        return Ok(None);
    }
    Ok(Some(format!("data:{media};base64,{}", B64.encode(&bytes))))
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
    state.save_config(&cfg).map_err(err)?;
    // Observe child must be restarted to pick up source flags.
    get_config_summary(state)
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
    tauri::async_runtime::block_on(async {
        state.store.append(vec![event]).await.map_err(err)
    })?;
    state
        .store
        .insert_derived(eid, "summary.v1", body.clone())
        .map_err(err)?;
    Ok(body)
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

    let child = Command::new(&daemon)
        .current_dir(&state.data_dir)
        .env("LUMEN_NAVI_CONFIG", state.config_path.display().to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
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
pub fn set_onboarding_step(state: State<'_, AppState>, step: u32) -> Result<OnboardingState, String> {
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
pub fn request_screen_permission() -> Result<bool, String> {
    Ok(lumen_platform_macos::request_screen_recording())
}

#[tauri::command]
pub fn open_privacy_settings(kind: String) -> Result<(), String> {
    let url = match kind.as_str() {
        "screen" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture"
        }
        "microphone" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone"
        }
        "speech" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_SpeechRecognition"
        }
        "accessibility" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility"
        }
        _ => {
            return Err(format!("unknown privacy pane: {kind}"));
        }
    };
    open_url(url)
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
        Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("open url only supported on macOS".into())
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
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/lumen-daemon"),
    );

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
