//! Tauri commands for the Navi desktop shell.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Local, Utc};
use lumen_api::{
    AiMessageDto, AiThreadDto, EventSummary, HealthResponse, OcrSearchHitDto, RoastIndexDto,
    RoastRecordDto, SourceStatus, API_VERSION,
};
use lumen_config::Config;
use lumen_platform::{pcm_rms_peak, pcm_s16le_to_wav, MicOpenConfig, PcmChunk};
use lumen_platform_host as host;
use lumen_store::{EventStore, TimelineQuery, SCHEMA_VERSION};
use lumen_types::{event_kind, SourceEvent, SourceKind};
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
pub struct AudioReadinessDto {
    pub permission: String,
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AudioDeviceDto {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Serialize)]
pub struct AudioDevicesDto {
    pub devices: Vec<AudioDeviceDto>,
    pub default_device: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AudioRecordingTestDto {
    pub permission: String,
    pub success: bool,
    pub device: Option<String>,
    pub chunks: u64,
    pub frames: u64,
    pub captured_duration_ms: u64,
    pub rms: f32,
    pub peak: f32,
    pub signal_detected: bool,
    pub event_written: bool,
    pub error: Option<String>,
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
    pub audio_device: String,
    pub asr_locale: String,
    pub asr_engine: String,
    pub asr_model_dir: String,
    pub asr_http_base_url: String,
    pub asr_http_model: String,
    pub asr_fallback_speech: bool,
    pub system_audio: bool,
    pub input_enabled: bool,
    pub input_interactions: bool,
}

#[derive(Debug, Serialize)]
pub struct OverviewRangeDto {
    pub stored_events: usize,
    pub ocr_docs: usize,
    pub audio_events: usize,
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
    pub audio_device: Option<String>,
    pub input_enabled: Option<bool>,
    pub input_interactions: Option<bool>,
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
    pub window_title_missing_reason: Option<String>,
    pub text_preview: Option<String>,
    pub text_kind: Option<String>,
    pub media_type: Option<String>,
    pub has_image: bool,
    pub artifact_bytes: Option<u64>,
}

/// What this OS build can actually do, so the UI never offers a remedy the
/// platform does not have.
#[derive(Debug, Serialize)]
pub struct PlatformInfoDto {
    pub os: String,
    pub screen_capture: bool,
    pub microphone: bool,
    pub ocr: bool,
    pub system_speech_asr: bool,
    pub text_selection: bool,
    pub screen_permission_gate: bool,
    pub accessibility_gate: bool,
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
    let daemon_socket = state.data_dir.join("daemon.sock");
    let daemon_health = if observe {
        fetch_daemon_health(&daemon_socket).await
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
        closed_eyes: daemon_health
            .as_ref()
            .map(|h| h.closed_eyes)
            .unwrap_or(cfg.privacy.closed_eyes),
        stored_events: n,
        stored_audio_events: daemon_health
            .as_ref()
            .map(|h| h.stored_audio_events)
            .unwrap_or_else(|| {
                state
                    .store
                    .event_count_by_kind(lumen_types::event_kind::AUDIO_CHUNK_V1)
                    .unwrap_or(0)
            }),
        ocr_docs,
        schema_version: SCHEMA_VERSION,
        browser,
        observe: daemon_health.as_ref().and_then(|h| h.observe.clone()),
        liveness: daemon_health.as_ref().and_then(|h| h.liveness.clone()),
    })
}

/// Validate the microphone without changing macOS permissions.
///
/// This checks permission first, then opens the configured input stream long
/// enough to verify that the device/configuration can actually start. The
/// stream is dropped immediately; the daemon owns the real capture lifecycle.
#[tauri::command]
pub async fn check_audio_readiness(
    state: State<'_, AppState>,
) -> Result<AudioReadinessDto, String> {
    let permission = host::permissions().status().await.map_err(err)?;
    let permission_label = format!("{:?}", permission.microphone);
    if !permission.can_record_mic() {
        return Ok(AudioReadinessDto {
            permission: permission_label,
            ready: false,
            error: Some(
                "麦克风权限尚未允许。请在系统设置中允许 Lumen Navi 访问麦克风，然后回来重试。"
                    .into(),
            ),
        });
    }

    let config = state.load_config().map_err(err)?;
    let open_config = MicOpenConfig {
        preferred_sample_rate: config.audio.sample_rate,
        preferred_channels: config.audio.channels,
        chunk_ms: config.audio.chunk_ms,
        device: config.audio.device,
    };
    let result = tokio::task::spawn_blocking(move || {
        host::mic()
            .open(open_config)
            .map(|stream| drop(stream))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("麦克风检测任务失败: {error}"))?;

    match result {
        Ok(()) => Ok(AudioReadinessDto {
            permission: permission_label,
            ready: true,
            error: None,
        }),
        Err(error) => Ok(AudioReadinessDto {
            permission: permission_label,
            ready: false,
            error: Some(error),
        }),
    }
}

#[tauri::command]
pub async fn list_audio_devices() -> Result<AudioDevicesDto, String> {
    tokio::task::spawn_blocking(|| {
        let (default_device, names) = host::mic_devices().map_err(err)?;
        let devices = names
            .into_iter()
            .map(|name| AudioDeviceDto {
                is_default: default_device.as_deref() == Some(name.as_str()),
                name,
            })
            .collect();
        Ok(AudioDevicesDto {
            devices,
            default_device,
        })
    })
    .await
    .map_err(|error| format!("设备列表任务失败: {error}"))?
}

/// Capture a short, bounded sample for Settings validation. The default is
/// deliberately non-persistent so a diagnostic cannot pollute the timeline.
#[tauri::command]
pub async fn record_audio_test(
    state: State<'_, AppState>,
    duration_ms: Option<u64>,
    write_event: Option<bool>,
) -> Result<AudioRecordingTestDto, String> {
    let permission = host::permissions().status().await.map_err(err)?;
    let permission_label = format!("{:?}", permission.microphone);
    let base = AudioRecordingTestDto {
        permission: permission_label.clone(),
        success: false,
        device: None,
        chunks: 0,
        frames: 0,
        captured_duration_ms: 0,
        rms: 0.0,
        peak: 0.0,
        signal_detected: false,
        event_written: false,
        error: None,
    };
    if !permission.can_record_mic() {
        return Ok(AudioRecordingTestDto {
            error: Some("麦克风权限尚未允许。请在系统设置中允许 Lumen Navi 访问麦克风后重试。".into()),
            ..base
        });
    }

    let config = state.load_config().map_err(err)?;
    let duration_ms = duration_ms.unwrap_or(3_000).clamp(1_000, 5_000);
    let threshold = config.audio.vad_rms_threshold;
    let open_config = MicOpenConfig {
        preferred_sample_rate: config.audio.sample_rate,
        preferred_channels: config.audio.channels,
        chunk_ms: duration_ms,
        device: config.audio.device,
    };
    let capture = tokio::task::spawn_blocking(move || capture_audio_test(open_config, duration_ms))
        .await
        .map_err(|error| format!("录音自测任务失败: {error}"))?;
    let (chunk, chunk_count) = match capture {
        Ok(capture) => capture,
        Err(error) => {
            return Ok(AudioRecordingTestDto {
                error: Some(error),
                ..base
            });
        }
    };

    let event_written = if write_event.unwrap_or(false) {
        let wav = pcm_s16le_to_wav(&chunk.samples, chunk.sample_rate, 1);
        let event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({
                "payload_version": 1,
                "device": chunk.device_name.clone(),
                "sample_rate": chunk.sample_rate,
                "channels": 1,
                "duration_ms": chunk.duration_ms,
                "samples": chunk.samples.len(),
                "mode": "settings_test",
                "rms": chunk.rms,
                "peak": chunk.peak,
                "format": "wav_s16le",
                "voice": chunk.rms >= threshold,
                "test": true,
            }),
        );
        state
            .store
            .put_and_append(event, "audio/wav", &wav)
            .map_err(err)?;
        true
    } else {
        false
    };

    Ok(AudioRecordingTestDto {
        permission: permission_label,
        success: true,
        device: Some(chunk.device_name),
        chunks: chunk_count,
        frames: chunk.samples.len() as u64,
        captured_duration_ms: chunk.duration_ms,
        rms: chunk.rms,
        peak: chunk.peak,
        signal_detected: chunk.rms >= threshold,
        event_written,
        error: None,
    })
}

fn capture_audio_test(
    open_config: MicOpenConfig,
    duration_ms: u64,
) -> Result<(PcmChunk, u64), String> {
    let stream = host::mic()
        .open(open_config)
        .map_err(|error| error.to_string())?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(duration_ms + 1_000);
    let mut samples = Vec::new();
    let mut sample_rate = 0;
    let mut device_name = String::new();
    let mut chunks = 0u64;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match stream
            .recv_timeout(remaining.min(std::time::Duration::from_millis(250)))
            .map_err(|error| error.to_string())?
        {
            Some(chunk) => {
                if sample_rate == 0 {
                    sample_rate = chunk.sample_rate;
                    device_name = chunk.device_name.clone();
                }
                samples.extend(chunk.samples);
                chunks = chunks.saturating_add(1);
                let captured_requested_duration =
                    !samples.is_empty() && sample_rate > 0
                        && samples.len() as u64 * 1_000
                            >= u64::from(sample_rate) * duration_ms;
                if chunks >= 2 || captured_requested_duration {
                    break;
                }
            }
            None => {}
        }
    }
    stream.stop();
    if samples.is_empty() || sample_rate == 0 {
        return Err("录音流已启动，但在测试时长内没有收到 PCM 音频帧。请检查输入设备和系统权限。".into());
    }
    let (rms, peak) = pcm_rms_peak(&samples);
    Ok((
        PcmChunk {
            duration_ms: samples.len() as u64 * 1_000 / u64::from(sample_rate),
            samples,
            sample_rate,
            channels: 1,
            rms,
            peak,
            device_name,
        },
        chunks,
    ))
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

async fn fetch_daemon_health(socket_path: &Path) -> Option<HealthResponse> {
    // The shell connects to the daemon over a Unix domain socket (no TCP port
    // to conflict over). reqwest 0.12 doesn't speak Unix sockets, so this is
    // a minimal HTTP/1.1 GET: connect, write request, read until body, parse.
    // Single endpoint, small JSON, 750ms timeout — keeps it dependency-free.
    #[cfg(unix)]
    {
        let path = socket_path.to_path_buf();
        tokio::task::spawn_blocking(move || fetch_health_via_unix_socket(&path))
            .await
            .ok()
            .flatten()
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        None
    }
}

#[cfg(unix)]
fn fetch_health_via_unix_socket(socket_path: &Path) -> Option<HealthResponse> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(socket_path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(750))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(750))).ok()?;
    let request = b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request).ok()?;
    // Read the whole response (Connection: close → server closes after body).
    let mut buf = Vec::with_capacity(8192);
    stream.read_to_end(&mut buf).ok()?;
    // Split headers / body at the first blank line.
    let body_start = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)?;
    let body = &buf[body_start..];
    // If the response used chunked encoding this won't decode; axum serves a
    // plain Content-Length body for small JSON, so a direct parse is fine.
    serde_json::from_slice::<HealthResponse>(body).ok()
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
    let status = host::permissions().status().await.map_err(err)?;
    let microphone = status.microphone;
    let accessibility = status.accessibility;
    let (screen_recording, screen_capture_ready, direct_capture_status, direct_capture_error) =
        if host::capabilities().os == "macos" {
            let cua = state.cua.clone();
            let cua_status = tauri::async_runtime::spawn_blocking(move || cua.status())
                .await
                .map_err(err)?
                .map_err(|error| {
                    tracing::warn!(%error, "Lumen Cua permission status unavailable");
                });
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
            }
        } else {
            (
                format!("{:?}", status.screen_recording),
                Some(status.can_capture_screen()),
                "native".into(),
                None,
            )
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
pub fn get_platform_info() -> Result<PlatformInfoDto, String> {
    let c = host::capabilities();
    Ok(PlatformInfoDto {
        os: c.os.into(),
        screen_capture: c.screen_capture,
        microphone: c.microphone,
        ocr: c.ocr,
        system_speech_asr: c.system_speech_asr,
        text_selection: c.text_selection,
        screen_permission_gate: c.screen_permission_gate,
        accessibility_gate: c.accessibility_gate,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildInfoDto {
    pub version: String,
    pub sha: String,
}

#[tauri::command]
pub fn get_build_info() -> Result<BuildInfoDto, String> {
    Ok(BuildInfoDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        sha: env!("LUMEN_BUILD_SHA").to_string(),
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
        audio_device: cfg.audio.device.clone(),
        asr_locale: cfg.asr.locale.clone(),
        asr_engine: cfg.asr.engine.clone(),
        asr_model_dir: cfg.asr.model_dir.clone(),
        asr_http_base_url: cfg.asr.http_base_url.clone(),
        asr_http_model: cfg.asr.http_model.clone(),
        asr_fallback_speech: cfg.asr.fallback_speech,
        system_audio: cfg.audio.system_audio,
        input_enabled: cfg.input.enabled,
        input_interactions: cfg.input.observe_interactions,
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
                window_title_missing_reason: it.window_title_missing_reason,
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
pub fn activity_segments(state: State<'_, AppState>, day: String) -> Result<Vec<lumen_api::ActivitySegmentDto>, String> {
    state.store.list_activity_segments(&day).map_err(err)
}

#[tauri::command]
pub fn activity_scenes(state: State<'_, AppState>, day: String) -> Result<lumen_api::SceneDayDto, String> {
    state.store.list_scene_day(&day).map_err(err)
}

#[tauri::command]
pub fn activity_history_slots(
    state: State<'_, AppState>,
    day: String,
) -> Result<Vec<lumen_api::HistorySlotDto>, String> {
    state.store.list_history_slots(&day).map_err(err)
}

/// Explicit Act: replay the first suggested skill on a History card once.
/// Observe never calls this.
#[tauri::command]
pub fn replay_history_skill(
    state: State<'_, AppState>,
    slot_start: String,
    type_texts: Option<Vec<Option<String>>>,
) -> Result<String, String> {
    let start = chrono::DateTime::parse_from_rfc3339(&slot_start)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&slot_start.replace('Z', "+00:00")))
        .map_err(|e| format!("bad slot_start: {e}"))?
        .with_timezone(&chrono::Utc);
    let day = start
        .with_timezone(&Local)
        .format("%Y-%m-%d")
        .to_string();
    let slots = state.store.list_history_slots(&day).map_err(err)?;
    let slot = slots
        .into_iter()
        .find(|s| s.slot_start == start)
        .ok_or_else(|| "找不到这张 15 分钟卡".to_string())?;
    let skill = slot
        .suggested_skills
        .first()
        .cloned()
        .ok_or_else(|| "这张卡没有可回放的 skill".to_string())?;
    if skill.steps.len() < 2 {
        return Err("步骤太少，无法回放".into());
    }
    let mut ops = Vec::new();
    for (i, step) in skill.steps.iter().enumerate() {
        let bundle = step_bundle(&slot, step);
        // User-provided text for type steps, aligned by step index. Typed
        // text is never recorded — only what the user enters at confirm time.
        let text = if step.action == "type" {
            type_texts.as_ref().and_then(|v| v.get(i).cloned().flatten())
        } else {
            None
        };
        ops.push(lumen_cua::InputStep {
            action: step.action.clone(),
            bundle_id: bundle,
            window: step.window.clone(),
            target: step.target.clone(),
            keys: step.keys.clone().or_else(|| {
                if step.action == "submit" {
                    Some("return".into())
                } else {
                    None
                }
            }),
            nx: step.rel_x,
            ny: step.rel_y,
            wait_ms: Some(200),
            text,
        });
    }
    let n = ops.len();
    state
        .cua
        .ensure_running()
        .map_err(err)?
        .input_replay(ops)
        .map_err(|e| format!("CUA 回放失败: {e}"))?;
    Ok(format!("已回放 {n} 步「{}」", skill.name))
}

fn step_bundle(
    slot: &lumen_api::HistorySlotDto,
    step: &lumen_api::SkillStepDto,
) -> Option<String> {
    slot.apps
        .iter()
        .find(|a| a.app_name == step.app)
        .and_then(|a| a.bundle_id.clone())
        .filter(|s| !s.is_empty())
}

/// Bundle-id → 64px PNG data URL for History app marks. Missing apps omitted.
#[tauri::command]
pub fn app_icons(
    state: State<'_, AppState>,
    bundle_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let cache = state.data_dir.join("icon-cache");
    let mut out = std::collections::HashMap::new();
    for id in bundle_ids {
        if let Some(url) = crate::app_icon::data_url_for_bundle(&cache, &id) {
            out.insert(id, url);
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn day_roast_summary(
    state: State<'_, AppState>,
    day: String,
) -> Result<lumen_api::DayRoastSummaryDto, String> {
    state.store.day_roast_summary(&day).map_err(err)
}

// ── Unified LLM client (provider-catalog aware, Anthropic-native) ───────

/// A small vendored subset of the provider catalog for endpoint/auth/style
/// resolution server-side. Mirrors the frontend catalog (provider-catalog.v1.json).
static LLM_CATALOG: &str = include_str!("../../src/llm/provider-catalog.v1.json");

#[derive(Debug, Clone)]
enum LlmStyle {
    OpenAiCompat,
    Anthropic,
}

#[derive(Debug, Clone)]
struct LlmEndpoint {
    /// Full chat URL (base + chat path).
    url: String,
    /// Base URL without chat path (for /models listing).
    base: String,
    /// Extra headers to set (auth + provider-required headers).
    headers: Vec<(String, String)>,
    style: LlmStyle,
}

#[derive(serde::Deserialize)]
struct CatalogProvider {
    id: String,
    #[serde(default)]
    api_style: String,
    #[serde(default)]
    endpoints: std::collections::HashMap<String, CatalogEndpoint>,
    #[serde(default)]
    chat_path: Option<String>,
    #[serde(default)]
    needs_key: bool,
    #[serde(default)]
    auth: Option<CatalogAuth>,
    #[serde(default)]
    extra_headers: Option<std::collections::HashMap<String, String>>,
}

#[derive(serde::Deserialize)]
struct CatalogEndpoint {
    base_url: String,
}

#[derive(serde::Deserialize, Clone)]
struct CatalogAuth {
    header: String,
    value_template: String,
}

fn catalog_provider(id: &str) -> Option<CatalogProvider> {
    let file: serde_json::Value = serde_json::from_str(LLM_CATALOG).ok()?;
    file.get("providers")?
        .as_array()?
        .iter()
        .filter_map(|p| serde_json::from_value::<CatalogProvider>(p.clone()).ok())
        .find(|p| p.id == id)
}

/// Resolve the effective endpoint for the configured provider. Provider
/// presets win over base_url; "custom" (or unknown id) uses base_url as-is.
fn resolve_llm_endpoint(cfg: &lumen_config::AssistantConfig) -> Result<LlmEndpoint, String> {
    let key = {
        let k = cfg.effective_api_key().trim().to_string();
        if k.is_empty() { None } else { Some(k) }
    };

    let preset = if cfg.provider_id == "custom" || cfg.provider_id.is_empty() {
        None
    } else {
        catalog_provider(&cfg.provider_id)
    };

    let (base, chat_path, style, auth_header, auth_template, extra) = match &preset {
        None => {
            // Custom: base_url must be set; treat as OpenAI-compat.
            let b = cfg.base_url.trim().trim_end_matches('/').to_string();
            if b.is_empty() {
                return Err("LLM 未配置 — 请在 设置 → LLM 配置 选择 provider 或填写 base_url".into());
            }
            (b, "/chat/completions".to_string(), LlmStyle::OpenAiCompat, None, None, None)
        }
        Some(p) => {
            let ep = if cfg.region == "global" {
                p.endpoints.get("global").or_else(|| p.endpoints.get("cn"))
            } else {
                p.endpoints.get("cn").or_else(|| p.endpoints.get("global"))
            };
            let base = match ep {
                Some(e) => e.base_url.trim().trim_end_matches('/').to_string(),
                None => {
                    // Fallback to user's base_url if the catalog entry lacks endpoints.
                    let b = cfg.base_url.trim().trim_end_matches('/').to_string();
                    if b.is_empty() {
                        return Err(format!("provider {} 没有 endpoint 配置", cfg.provider_id));
                    }
                    b
                }
            };
            let chat = p.chat_path.clone()
                .unwrap_or_else(|| "/chat/completions".into());
            let style = if p.api_style == "anthropic" { LlmStyle::Anthropic } else { LlmStyle::OpenAiCompat };
            (base, chat, style, p.auth.clone(), p.auth.clone().map(|a| a.value_template), p.extra_headers.clone())
        }
    };

    // Build headers: auth first, then extra (extra wins on conflict).
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(k) = &key {
        match (&auth_header, &auth_template) {
            (Some(auth), Some(t)) => {
                headers.push((auth.header.clone(), t.replace("{key}", k)));
            }
            _ => {
                headers.push(("Authorization".into(), format!("Bearer {k}")));
            }
        }
    } else if preset.as_ref().map(|p| p.needs_key).unwrap_or(false) {
        return Err(format!("401: {} 需要 API key，请在 设置 → LLM 配置 中配置", cfg.provider_id));
    }
    if let Some(extra) = extra {
        for (k, v) in extra {
            headers.push((k, v));
        }
    }

    let url = format!("{base}{chat_path}");
    Ok(LlmEndpoint { url, base, headers, style })
}

/// A completion result: the visible answer plus the model's chain-of-thought
/// when the provider returns it in a separate field (reasoning models).
#[derive(Debug, Serialize)]
pub struct LlmReply {
    pub content: String,
    pub reasoning: Option<String>,
}

/// Some OpenAI-compatible providers (MiniMax, Qwen, GLM…) leave the model's
/// chain-of-thought inline in content as `<think>…</think>` instead of a
/// separate field. Split those blocks out so the UI can render them
/// collapsed instead of spilling thinking into the answer.
fn split_inline_think(content: &str) -> (String, Option<String>) {
    const TAGS: [(&str, &str); 2] = [("<think>", "</think>"), ("<thinking>", "</thinking>")];
    let lower = content.to_ascii_lowercase();
    let mut thoughts: Vec<String> = Vec::new();
    let mut clean = String::with_capacity(content.len());
    let mut cursor = 0usize;
    while cursor < content.len() {
        // Earliest opening tag from here.
        let next = TAGS
            .iter()
            .filter_map(|(open, close)| {
                lower[cursor..]
                    .find(open)
                    .map(|i| (cursor + i, open.len(), close))
            })
            .min_by_key(|(pos, _, _)| *pos);
        match next {
            None => {
                clean.push_str(&content[cursor..]);
                break;
            }
            Some((pos, open_len, close)) => {
                clean.push_str(&content[cursor..pos]);
                match lower[pos..].find(close) {
                    Some(c) => {
                        thoughts.push(content[pos + open_len..pos + c].trim().to_string());
                        cursor = pos + c + close.len();
                    }
                    None => {
                        // Unterminated think block: treat the rest as thought.
                        thoughts.push(content[pos + open_len..].trim().to_string());
                        break;
                    }
                }
            }
        }
    }
    let reasoning = if thoughts.is_empty() {
        None
    } else {
        let joined = thoughts.join("\n\n");
        if joined.is_empty() { None } else { Some(joined) }
    };
    (clean.trim().to_string(), reasoning)
}

/// Merge field-based reasoning (reasoning_content) with inline <think>
/// blocks extracted from content; dedupes when both carry the same text.
fn merge_reasoning(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) if a.trim() == b.trim() => Some(a),
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (x, None) | (None, x) => x,
    }
}

/// Send a chat completion (non-streaming) and extract the assistant text.
/// Handles both OpenAI-compat and Anthropic-native response shapes.
async fn llm_chat_complete(
    cfg: &lumen_config::AssistantConfig,
    endpoint: &LlmEndpoint,
    messages: Vec<serde_json::Value>,
    temperature: f64,
) -> Result<LlmReply, String> {
    let body = match endpoint.style {
        LlmStyle::OpenAiCompat => serde_json::json!({
            "model": cfg.model,
            "messages": messages,
            "temperature": temperature,
            "stream": false,
        }),
        LlmStyle::Anthropic => {
            // Anthropic Messages API: system prompt is a top-level field,
            // messages must alternate user/assistant, max_tokens required.
            let system = messages.iter()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let chat: Vec<&serde_json::Value> = messages.iter()
                .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
                .collect();
            serde_json::json!({
                "model": cfg.model,
                "max_tokens": 4096,
                "system": system,
                "messages": chat,
            })
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(cfg.timeout_ms.max(30_000)))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut req = client.post(&endpoint.url).json(&body);
    for (k, v) in &endpoint.headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| format!("LLM 请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("LLM 返回 {status}: {text}"));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| format!("解析响应: {e}"))?;
    let (content, reasoning) = match endpoint.style {
        LlmStyle::OpenAiCompat => {
            // Reasoning models (DeepSeek-R1 / GLM / Qwen) expose CoT separately.
            let reasoning = ["reasoning_content", "reasoning"]
                .iter()
                .filter_map(|k| {
                    json.pointer(&format!("/choices/0/message/{k}"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .find(|s| !s.trim().is_empty());
            (
                json.pointer("/choices/0/message/content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                reasoning,
            )
        }
        LlmStyle::Anthropic => {
            // Content is an array of blocks; thinking blocks carry the CoT.
            let mut text = String::new();
            let mut thinking = String::new();
            if let Some(blocks) = json.get("content").and_then(|c| c.as_array()) {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("thinking") => {
                            if let Some(t) = b.get("thinking").and_then(|v| v.as_str()) {
                                thinking.push_str(t);
                            }
                        }
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                text.push_str(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            (
                if text.is_empty() { None } else { Some(text) },
                if thinking.trim().is_empty() { None } else { Some(thinking) },
            )
        }
    };
    let content = content.ok_or_else(|| "LLM 响应缺少 content".to_string())?;
    // Field-based reasoning first; then also strip inline <think> blocks
    // some providers leave inside content.
    let (clean, inline) = split_inline_think(&content);
    if clean.is_empty() && inline.is_none() {
        return Err("LLM 响应缺少 content".to_string());
    }
    Ok(LlmReply {
        content: clean,
        reasoning: merge_reasoning(reasoning, inline),
    })
}

/// Human duration for prompts. Never give the model raw millisecond integers.
fn fmt_dur(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    if secs < 60 {
        format!("{secs}秒")
    } else if secs < 3600 {
        let m = secs / 60;
        let r = secs % 60;
        if r == 0 {
            format!("{m}分钟")
        } else {
            format!("{m}分{r}秒")
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}小时")
        } else {
            format!("{h}小时{m}分钟")
        }
    }
}

#[tauri::command]
pub async fn roast_day(
    state: State<'_, AppState>,
    day: String,
    tone: Option<String>,
) -> Result<LlmReply, String> {
    let summary = state.store.day_roast_summary(&day).map_err(err)?;
    let cfg = state.load_config().map_err(err)?.assistant;
    if cfg.model.trim().is_empty() {
        return Err("LLM 未配置 — 请在 设置 → LLM 配置 选择 model".into());
    }
    let endpoint = resolve_llm_endpoint(&cfg)?;
    let prompt = lumen_store::roast::build_prompt(&summary, tone.as_deref().unwrap_or("roast"));
    let reply = llm_chat_complete(
        &cfg,
        &endpoint,
        vec![serde_json::json!({"role": "user", "content": prompt})],
        0.8,
    )
    .await?;
    // Archive every roast so the calendar can replay past days.
    state
        .store
        .roast_save(&day, &cfg.model, &reply.content, reply.reasoning.as_deref())
        .map_err(err)?;
    Ok(reply)
}

/// Archived roasts for one day, newest first.
#[tauri::command]
pub fn roast_list(state: State<'_, AppState>, day: String) -> Result<Vec<RoastRecordDto>, String> {
    state.store.roast_list_for_day(&day).map_err(err)
}

/// Which days have roasts (calendar markers).
#[tauri::command]
pub fn roast_index(state: State<'_, AppState>) -> Result<Vec<RoastIndexDto>, String> {
    state.store.roast_index().map_err(err)
}

/// Result of ai_send: which thread the exchange landed in (created on first
/// send) plus the reply.
#[derive(Debug, Serialize)]
pub struct AiSendResult {
    pub thread_id: String,
    pub content: String,
    pub reasoning: Option<String>,
}

/// AI chat send: persists the exchange into an ai_threads/ai_messages
/// conversation. Thread context is rebuilt from stored history so switching
/// tabs or restarting the app resumes where you left off.
#[tauri::command]
pub async fn ai_send(
    state: State<'_, AppState>,
    thread_id: Option<String>,
    content: String,
) -> Result<AiSendResult, String> {
    let user = content.trim().to_string();
    if user.is_empty() {
        return Err("空消息".into());
    }
    let cfg = state.load_config().map_err(err)?.assistant;
    if cfg.model.trim().is_empty() {
        return Err("LLM 未配置 — 请在 设置 → LLM 配置 选择 model".into());
    }

    // Rebuild conversation context from persisted history.
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    if let Some(tid) = thread_id.as_deref() {
        for m in state.store.ai_list_messages(tid).map_err(err)? {
            if m.role == "user" || m.role == "assistant" {
                msgs.push(serde_json::json!({"role": m.role, "content": m.content}));
            }
        }
    }
    msgs.push(serde_json::json!({"role": "user", "content": user}));

    let endpoint = resolve_llm_endpoint(&cfg)?;
    let reply = llm_chat_complete(&cfg, &endpoint, msgs, 0.7).await?;

    // Persist only on success — failed sends stay ephemeral.
    let tid = match thread_id {
        Some(t) => t,
        None => state.store.ai_create_thread("").map_err(err)?.id,
    };
    state
        .store
        .ai_append_exchange(&tid, &user, &reply.content, reply.reasoning.as_deref())
        .map_err(err)?;
    Ok(AiSendResult {
        thread_id: tid,
        content: reply.content,
        reasoning: reply.reasoning,
    })
}

#[tauri::command]
pub fn ai_thread_list(state: State<'_, AppState>) -> Result<Vec<AiThreadDto>, String> {
    state.store.ai_list_threads(100).map_err(err)
}

#[tauri::command]
pub fn ai_thread_messages(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<Vec<AiMessageDto>, String> {
    state.store.ai_list_messages(&thread_id).map_err(err)
}

#[tauri::command]
pub fn ai_thread_delete(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<(), String> {
    state.store.ai_delete_thread(&thread_id).map_err(err)
}

/// Test the LLM connection with a minimal ping.
#[tauri::command]
pub async fn llm_test(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = state.load_config().map_err(err)?.assistant;
    let endpoint = resolve_llm_endpoint(&cfg)?;
    if cfg.model.trim().is_empty() {
        return Err("未选择 model".into());
    }
    let ep = endpoint.clone();
    // Minimal ping: one token max.
    let body = match ep.style {
        LlmStyle::OpenAiCompat => serde_json::json!({
            "model": cfg.model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": false,
        }),
        LlmStyle::Anthropic => serde_json::json!({
            "model": cfg.model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        }),
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(15_000))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut req = client.post(&ep.url).json(&body);
    for (k, v) in &ep.headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| format!("连接失败: {e}"))?;
    if resp.status().is_success() {
        Ok("连接成功".into())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        // Truncate long error bodies for display.
        let short: String = text.chars().take(200).collect();
        Err(format!("返回 {status}: {short}"))
    }
}

/// List available models from the provider (GET /models).
#[tauri::command]
pub async fn llm_list_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let cfg = state.load_config().map_err(err)?.assistant;
    let endpoint = resolve_llm_endpoint(&cfg)?;
    let url = format!("{}/models", endpoint.base);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(10_000))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut req = client.get(&url);
    for (k, v) in &endpoint.headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("返回 {status}（该 provider 可能不支持 model 列表）"));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| format!("解析: {e}"))?;
    // OpenAI-compat: /data[].id ; Anthropic: /data[].id too (v1/models).
    let mut out = Vec::new();
    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                out.push(id.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

#[tauri::command]
pub fn activity_stats(
    state: State<'_, AppState>,
    day: String,
    group_by: Option<String>,
) -> Result<lumen_api::DayStatsDto, String> {
    state
        .store
        .activity_day_stats(&day, parse_group_by(group_by.as_deref()))
        .map_err(err)
}

#[tauri::command]
pub fn activity_range(
    state: State<'_, AppState>,
    from: String,
    to: String,
    group_by: Option<String>,
) -> Result<lumen_api::RangeStatsDto, String> {
    state
        .store
        .activity_range_stats(&from, &to, parse_group_by(group_by.as_deref()))
        .map_err(err)
}

#[tauri::command]
pub fn overview_range(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> Result<OverviewRangeDto, String> {
    let (stored_events, ocr_docs, audio_events) = state
        .store
        .overview_range_counts(&from, &to)
        .map_err(err)?;
    Ok(OverviewRangeDto {
        stored_events,
        ocr_docs,
        audio_events,
    })
}

/// Parse the `group_by` invoke param: "site"/"domain"/"website" → Site,
/// anything else (including None) → App (default).
fn parse_group_by(s: Option<&str>) -> lumen_store::GroupBy {
    match s.map(|x| x.to_ascii_lowercase()).as_deref() {
        Some("site") | Some("domain") | Some("website") => lumen_store::GroupBy::Site,
        _ => lumen_store::GroupBy::App,
    }
}

#[tauri::command]
pub fn activity_add_manual_segment(
    state: State<'_, AppState>,
    started_at: String,
    ended_at: String,
    app_name: String,
    window_title: Option<String>,
    category: Option<String>,
    productivity_level: Option<String>,
) -> Result<String, String> {
    let start = chrono::DateTime::parse_from_rfc3339(&started_at)
        .map_err(|e| format!("started_at: {e}"))?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(&ended_at)
        .map_err(|e| format!("ended_at: {e}"))?
        .with_timezone(&chrono::Utc);
    state.store.add_manual_segment(
        start,
        end,
        &app_name,
        window_title.as_deref(),
        category.as_deref(),
        productivity_level.as_deref(),
    ).map_err(err)
}

#[tauri::command]
pub fn activity_delete_segment(state: State<'_, AppState>, seg_id: String) -> Result<(), String> {
    state.store.delete_manual_segment(&seg_id).map_err(err)
}

#[tauri::command]
pub fn activity_list_category_rules(state: State<'_, AppState>) -> Result<Vec<lumen_store::CategoryRule>, String> {
    state.store.list_category_rules().map_err(err)
}

#[tauri::command]
pub fn activity_save_category_rules(
    state: State<'_, AppState>,
    rules: Vec<lumen_store::CategoryRule>,
) -> Result<(), String> {
    state.store.save_category_rules_and_reapply(rules).map_err(err)
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
    if let Some(v) = update.audio_device {
        cfg.audio.device = v.trim().to_string();
    }
    if let Some(v) = update.input_enabled {
        cfg.input.enabled = v;
    }
    if let Some(v) = update.input_interactions {
        cfg.input.observe_interactions = v;
    }
    state.save_config(&cfg).map_err(err)?;
    reload_local_service(&state)?;
    get_config_summary(state)
}

fn reload_local_service(state: &AppState) -> Result<(), String> {
    // Keep the intentional-stop flag set across the swap so the supervisor
    // does not treat a config reload as a crash and consume the budget.
    observe_stop_inner(state)?;
    let status = observe_start_inner_opts(state, false)?;
    let socket = state.data_dir.join("daemon.sock");
    for _ in 0..25 {
        if crate::daemon_socket_alive(&socket) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    state
        .observe_stopping
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = status;
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
    observe_start_inner_opts(state, true)
}

fn observe_start_inner_opts(
    state: &AppState,
    clear_stopping: bool,
) -> Result<ObserveStatus, String> {
    if state.observe_running() {
        let running = true;
        let pid = state
            .observe_child
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|c| c.id()));
        return Ok(ObserveStatus { running, pid });
    }
    // No child in our slot — but an orphan daemon from a previous app run may
    // still be serving on the Unix socket. Probe it before spawning a new one;
    // spawning when the socket is occupied causes the new daemon to fatal-exit
    // and the supervisor loops trying to restart it. If the socket answers,
    // treat the orphan as "already running" and return success without spawning.
    let daemon_socket = state.data_dir.join("daemon.sock");
    if crate::daemon_socket_alive(&daemon_socket) {
        tracing::info!(
            socket = %daemon_socket.display(),
            "observe daemon already serving (orphan from prior app run); adopting, not spawning"
        );
        return Ok(ObserveStatus { running: true, pid: None });
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

    let cua_ready = if cfg.sources.screen && host::capabilities().os == "macos" {
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
        // Lets the daemon self-terminate if this app dies without reaping it
        // (SIGTERM/pkill skips RunEvent::Exit, which used to orphan daemons).
        .env("LUMEN_NAVI_PARENT_PID", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    if cua_ready {
        daemon_command
            .env("LUMEN_CUA_SOCKET", state.cua.socket_path())
            .env("LUMEN_CUA_TOKEN_FILE", state.cua.token_file());
    }
    // CREATE_NO_WINDOW — without it Windows gives the console-subsystem daemon
    // its own black console window on every start.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        daemon_command.creation_flags(0x0800_0000);
    }
    let child = daemon_command
        .spawn()
        .map_err(|e| format!("spawn lumen-daemon: {e}"))?;

    let pid = child.id();
    *state.observe_child.lock().map_err(err)? = Some(child);
    // User/manual start clears the intentional-stop flag. Reload keeps it
    // set until the new socket answers, so the supervisor stays quiet.
    if clear_stopping {
        state
            .observe_stopping
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
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
    // Mark intentional stop so the supervisor doesn't treat the upcoming
    // child exit as a crash and auto-restart it.
    state.observe_stopping.store(true, std::sync::atomic::Ordering::SeqCst);
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
    host::shell::open_path(&state.data_dir).map_err(err)
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
    if host::capabilities().os != "macos" {
        return Ok(host::request_screen_recording());
    }
    let cua = state.cua.clone();
    tauri::async_runtime::spawn_blocking(move || cua.request_screen_permission())
        .await
        .map_err(|error| format!("Lumen Cua permission task failed: {error}"))?
}

#[tauri::command]
pub async fn refresh_screen_permission(state: State<'_, AppState>) -> Result<bool, String> {
    if host::capabilities().os != "macos" {
        return host::permissions()
            .status()
            .await
            .map(|status| status.can_capture_screen())
            .map_err(err);
    }
    let cua = state.cua.clone();
    tauri::async_runtime::spawn_blocking(move || cua.refresh_screen_permission())
        .await
        .map_err(|error| format!("Lumen Cua permission refresh task failed: {error}"))?
}

#[tauri::command]
pub async fn request_microphone_permission() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(host::request_microphone_access)
        .await
        .map_err(|e| format!("microphone permission task failed: {e}"))?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_privacy_settings(kind: String) -> Result<(), String> {
    match host::shell::privacy_settings_uri(&kind) {
        Some(uri) => host::shell::open_uri(uri).map_err(err),
        None => Err(format!(
            "{} has no settings page for '{kind}' on this system",
            host::capabilities().os
        )),
    }
}

fn privacy_settings_url(kind: &str) -> Result<&'static str, String> {
    match kind {
        // Prefer the classic Security privacy URL (works across more macOS
        // versions; same as CuaDriver). The modern PrivacySecurity.extension
        // form is a fallback used by open_screen_recording_settings.
        "screen" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        "microphone" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        }
        "speech" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition")
        }
        "accessibility" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        _ => Err(format!("unknown privacy pane: {kind}")),
    }
}


/// Executable name of the bundled Observe daemon for this platform.
const DAEMON_BIN: &str = if cfg!(windows) {
    "lumen-daemon.exe"
} else {
    "lumen-daemon"
};

#[cfg(test)]
mod command_tests {
    use super::{
        daemon_health_url, ensure_browser_pairing, fmt_dur, health_sources, media_allowed,
        privacy_settings_url, roast_prompt_data, MediaKind,
    };
    use lumen_api::{HealthResponse, SourceStatus, API_VERSION};
    use lumen_config::Config;

    #[test]
    fn roast_prompt_uses_human_durations_not_raw_ms() {
        assert_eq!(fmt_dur(57_000), "57秒");
        assert_eq!(fmt_dur(957_939), "15分57秒");
        assert_eq!(fmt_dur(10_056_219), "2小时47分钟");
        let summary = lumen_api::DayRoastSummaryDto {
            day: "2026-08-16".into(),
            total_active_ms: 38 * 60_000,
            total_idle_ms: 10_056_219,
            pulse_score: None,
            context_switches: 3,
            screenshot_count: 10,
            ax_sample_count: 10,
            input_counts: None,
            top_apps: vec![lumen_api::RoastAppTotal {
                app: "ZCode".into(),
                ms: 957_939,
                pct: 41.5,
                category: None,
                user_active_ms: 57_000,
            }],
            top_domains: vec![],
            top_scenes: vec![],
            notable_titles: vec![],
            busiest_hour: None,
            hour_histogram: vec![],
            user_active_ms: Some(9 * 60_000 + 32_000),
            attribution: Some("interactions".into()),
            switches_user: Some(13),
            switches_passive: Some(26),
        };
        let data = roast_prompt_data(&summary);
        let s = data.to_string();
        assert!(!s.contains("_ms"), "prompt JSON must not leak raw ms fields: {s}");
        assert!(s.contains("15分57秒"));
        assert!(s.contains("2小时47分钟"));
        assert!(s.contains("监控开启后的窗口"));
        assert_eq!(data["应用TOP"][0]["短段高活跃"], false);
    }

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
            closed_eyes: false,
            stored_events: 0,
            stored_audio_events: 0,
            ocr_docs: 0,
            schema_version: 0,
            browser: None,
            observe: None,
            liveness: None,
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

    // 1) Bundled next to the desktop binary (Tauri externalBin layout — the
    //    .app on macOS, the install directory on Windows).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(DAEMON_BIN));
            // Some macOS layouts keep helpers under ../Resources.
            if let Some(contents) = dir.parent() {
                candidates.push(contents.join("Resources").join(DAEMON_BIN));
                candidates.push(contents.join("MacOS").join(DAEMON_BIN));
            }
        }
    }

    // 2) Workspace builds during development.
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target");
    candidates.push(workspace.join("release").join(DAEMON_BIN));
    candidates.push(workspace.join("debug").join(DAEMON_BIN));

    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }

    // 3) PATH
    for dir in std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = dir.join(DAEMON_BIN);
        if candidate.is_file() {
            return Some(candidate);
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
    pub provider_id: String,
    pub region: String,
    pub base_url: String,
    pub model: String,
    pub target_lang: String,
    pub max_selection_chars: usize,
    /// Never echoes the key back — only whether one is configured.
    pub api_key_set: bool,
    pub accessibility_trusted: bool,
    /// False where the OS backend cannot read another app's selection, so the
    /// settings page can explain instead of showing a permission remedy.
    pub selection_supported: bool,
    pub clipboard_fallback: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssistantUpdate {
    pub enabled: Option<bool>,
    pub popup_enabled: Option<bool>,
    pub provider_id: Option<String>,
    pub region: Option<String>,
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
        provider_id: cfg.assistant.provider_id.clone(),
        region: cfg.assistant.region.clone(),
        base_url: cfg.assistant.base_url.clone(),
        model: cfg.assistant.model.clone(),
        target_lang: cfg.assistant.target_lang.clone(),
        max_selection_chars: cfg.assistant.max_selection_chars,
        api_key_set: !cfg.assistant.effective_api_key().is_empty(),
        accessibility_trusted: host::selection::accessibility_trusted(false),
        selection_supported: host::selection::supported(),
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
    if let Some(v) = update.provider_id {
        cfg.assistant.provider_id = v.trim().to_string();
    }
    if let Some(v) = update.region {
        let r = v.trim().to_string();
        if r == "cn" || r == "global" {
            cfg.assistant.region = r;
        }
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
    Ok(host::selection::accessibility_trusted(true))
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

/// Write the assistant result back into the app the text was selected from
/// (划词 popup「写入原文」). Explicit user action — the frontend only shows the
/// button after `assistant-done`. `mode` is "replace" or "append".
#[tauri::command]
pub async fn assistant_inject(app: AppHandle, mode: String, text: String) -> Result<String, String> {
    let m = lumen_platform_host::selection::InjectMode::parse(&mode)
        .ok_or_else(|| format!("未知注入模式 {mode}（replace|append）"))?;
    let target = selection_popup::pending_target()
        .ok_or_else(|| "来源应用已丢失，无法写回（重新划词后再试）".to_string())?;
    if text.trim().is_empty() {
        return Err("没有可写入的内容".into());
    }
    let pid = target.pid;
    tauri::async_runtime::spawn_blocking(move || {
        lumen_platform_host::selection::inject_text(pid, &text, m)
    })
    .await
    .map_err(|e| format!("inject task: {e}"))??;
    selection_popup::hide_popup(&app);
    Ok("已写入".into())
}

#[cfg(test)]
mod llm_think_tests {
    use super::{merge_reasoning, split_inline_think};

    #[test]
    fn splits_think_block_from_answer() {
        let (content, reasoning) = split_inline_think(
            "<think>plan the greeting</think>Hello there! 👋",
        );
        assert_eq!(content, "Hello there! 👋");
        assert_eq!(reasoning.as_deref(), Some("plan the greeting"));
    }

    #[test]
    fn splits_thinking_variant_and_multiple_blocks() {
        let (content, reasoning) = split_inline_think(
            "<THINKING>first</THINKING>mid<thinking>second</thinking>end",
        );
        assert_eq!(content, "midend");
        assert_eq!(reasoning.as_deref(), Some("first\n\nsecond"));
    }

    #[test]
    fn unterminated_block_takes_the_rest() {
        let (content, reasoning) = split_inline_think("intro<think>trailing thought");
        assert_eq!(content, "intro");
        assert_eq!(reasoning.as_deref(), Some("trailing thought"));
    }

    #[test]
    fn plain_content_untouched() {
        let (content, reasoning) = split_inline_think("just an answer, no tags");
        assert_eq!(content, "just an answer, no tags");
        assert!(reasoning.is_none());
    }

    #[test]
    fn merge_dedupes_identical_reasoning() {
        let m = merge_reasoning(Some("same".into()), Some("same".into()));
        assert_eq!(m.as_deref(), Some("same"));
        let m = merge_reasoning(Some("a".into()), Some("b".into()));
        assert_eq!(m.as_deref(), Some("a\n\nb"));
        assert!(merge_reasoning(None, None).is_none());
    }
}
