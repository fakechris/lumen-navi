//! Lumen Navi daemon — Observe (screen + mic) + OCR + ASR + local control API.
//!
//! Screen and audio never wait on each other. OCR/ASR never block capture.

mod control_server;
#[cfg(unix)]
mod mcp;
mod summarizer;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use lumen_api::{HealthResponse, SourceStatus};
use lumen_asr_engine::{
    build_engine, probe_status, samples_to_wav_mono_i16, AsrEngine, AsrEngineId, AsrError,
    AsrRequest, AsrResult, EngineBuildConfig, EngineKind,
};
use lumen_config::{AsrConfig, AudioConfig, Config, PolicyGate, PrivacyConfig};
use lumen_cua::{CuaCaptureAdapter, CuaClient};
use lumen_platform::{DisplayEnumerator, MicOpenConfig, PlatformError, ScreenCapturer};
use lumen_platform_host as host;
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
use lumen_platform_macos;
use lumen_process::{
    AxWorker, AxWorkerConfig, OcrWorker, OcrWorkerConfig, TranscribeWorker, TranscribeWorkerConfig,
    JOB_KIND_TRANSCRIBE_AUDIO,
};
use lumen_sources_browser::BrowserIngestPolicy;
use lumen_sources_media::{
    drain_transition, session_matches_frontmost, ActivityPoll, AudioOrchestrator,
    CaptureOrchestrator, CapturedBatch, InteractionCoalescer, InteractionContext,
    SessionTransition, SharedSessionBinder,
};
use lumen_store::{
    EventStore, PixelHashWindow, ReclaimKind, RecoveryPolicy, SqliteStore, SCHEMA_VERSION,
};
use lumen_types::{event_kind, SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

const CUA_SOCKET_ENV: &str = "LUMEN_CUA_SOCKET";
const CUA_TOKEN_FILE_ENV: &str = "LUMEN_CUA_TOKEN_FILE";
const PARENT_PID_ENV: &str = "LUMEN_NAVI_PARENT_PID";

/// Unix time (secs) of the last successful event write, read by the write
/// watchdog below. Static so every write path (screen loop, standalone
/// activity loop, persist task, audio loop) can report without plumbing.
static LAST_WRITE_UNIX: AtomicU64 = AtomicU64::new(0);

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn note_write() {
    LAST_WRITE_UNIX.store(unix_now(), Ordering::Relaxed);
}

/// Platform default data dir, mirroring the desktop shell's
/// `state.rs::default_data_dir` (`~/Library/Application Support/LumenNavi`
/// on macOS). Used only when the daemon runs without any explicit config.
fn default_data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home).join("Library/Application Support/LumenNavi")
    }
    #[cfg(target_os = "windows")]
    {
        // Local (not Roaming) AppData: the store is a SQLite database plus
        // screenshot blobs — machine-local, frequently written, and far too
        // large to sync with a roaming profile.
        match std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            Some(local) => std::path::PathBuf::from(local).join("LumenNavi"),
            None => std::env::temp_dir().join("LumenNavi"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home).join(".lumen-navi")
    }
}

fn debug_skip_hid_without_session() {}

fn persist_interaction_events(
    store: &SqliteStore,
    events: Vec<SourceEvent>,
    counters: &control_server::ObserveCounters,
) {
    for ev in events {
        if ev.session_id.is_none() {
            debug_skip_hid_without_session();
            continue;
        }
        if let Err(e) = store.append_event(ev) {
            warn!(error = %e, "append interaction event failed");
            counters.note_persist_failed();
        } else {
            note_write();
            counters.note_persisted();
        }
    }
}

fn attach_selection(ctx: &InteractionContext, text: String) -> SourceEvent {
    let payload = json!({
        "payload_version": 1,
        "app_name": ctx.app_name,
        "bundle_id": ctx.bundle_id,
        "window_title": ctx.window_title,
        "url": ctx.url,
        "selection": { "text": text },
    });
    let mut ev = SourceEvent::new(
        SourceKind::Screen,
        event_kind::SELECTION_CHANGED_V1,
        payload,
    );
    ev.session_id = ctx.session_id;
    ev
}

fn persist_activity_poll(
    store: &SqliteStore,
    poll: ActivityPoll,
    counters: &control_server::ObserveCounters,
) {
    if poll.upsert_sessions.is_empty() && poll.events.is_empty() {
        return;
    }
    if let Err(e) = store.apply_activity_transition(&poll.upsert_sessions, &poll.events) {
        warn!(error = %e, "apply activity transition failed");
        counters.note_persist_failed();
        return;
    }
    note_write();
    counters.note_persisted();
}

fn persist_session_transition(
    store: &SqliteStore,
    trans: SessionTransition,
    counters: &control_server::ObserveCounters,
) {
    persist_activity_poll(
        store,
        ActivityPoll {
            events: trans.events,
            upsert_sessions: trans.upserts,
        },
        counters,
    );
}

fn observe_policy_gate(
    paused: bool,
    closed_eyes: bool,
    locked: bool,
    privacy: &PrivacyConfig,
    bundle_id: Option<&str>,
    frontmost_known: bool,
) -> PolicyGate {
    PolicyGate::evaluate(
        paused,
        closed_eyes,
        locked,
        privacy,
        bundle_id,
        frontmost_known,
    )
}

/// Open a session only when none exists. App switches stay with the activity
/// owner so HID cannot race a poll and flip the session backwards.
/// Bind and persist stay under the binder lock so a later Mail transition
/// cannot be overwritten by a stale Safari-open write.
fn ensure_session_if_absent(
    store: &SqliteStore,
    sessions: &SharedSessionBinder,
    app: Option<&str>,
    bundle: Option<&str>,
) -> Option<Uuid> {
    sessions.mutate(|manager| {
        if let Some(current) = manager.current() {
            if session_matches_frontmost(current, app, bundle) {
                return Some(current.id);
            }
            return None;
        }
        let (_, closed) = manager.touch(app, bundle, "interaction");
        let trans = drain_transition(manager, closed);
        if trans.is_empty() {
            return manager.current().map(|s| s.id);
        }
        match store.apply_activity_transition(&trans.upserts, &trans.events) {
            Ok(()) => {
                note_write();
            }
            Err(e) => warn!(error = %e, "apply activity transition failed"),
        }
        manager.current().map(|s| s.id)
    })
}

#[cfg(target_os = "macos")]
fn start_macos_input_loop(
    config: &Config,
    store: &Arc<SqliteStore>,
    observe_paused: &Arc<AtomicBool>,
    observe_closed_eyes: &Arc<AtomicBool>,
    session_binder: &Arc<SharedSessionBinder>,
    session_persist: &Arc<tokio::sync::Mutex<()>>,
    counters: &Arc<control_server::ObserveCounters>,
) -> Option<tokio::task::JoinHandle<()>> {
    static INPUT_STATE: std::sync::OnceLock<lumen_platform_macos::InputCounterState> =
        std::sync::OnceLock::new();
    if !config.input.enabled {
        info!("input counter disabled in config");
        return None;
    }
    let state = INPUT_STATE.get_or_init(lumen_platform_macos::InputCounterState::default);
    match lumen_platform_macos::start_input_counter(state) {
        Ok(()) => {
            info!(
                flush_s = config.input.flush_interval_s,
                observe = config.input.observe_interactions,
                record_text = config.input.record_text,
                "input tap started"
            );
            let store_in = Arc::clone(store);
            let flush_every = Duration::from_secs(config.input.flush_interval_s.max(30));
            let stats_on = config.input.enabled;
            let observe_on = config.input.observe_interactions;
            let record_text = config.input.record_text;
            let privacy = config.privacy.clone();
            let observe_paused = Arc::clone(observe_paused);
            let observe_closed_eyes = Arc::clone(observe_closed_eyes);
            let session_binder = Arc::clone(session_binder);
            let session_persist = Arc::clone(session_persist);
            let counters = Arc::clone(counters);
            Some(tokio::spawn(async move {
                let mut stats_tick = tokio::time::interval(flush_every);
                stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut hid_tick = tokio::time::interval(Duration::from_millis(50));
                hid_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                stats_tick.tick().await;
                hid_tick.tick().await;
                let mut coal = InteractionCoalescer::default();
                let front = host::frontmost();
                loop {
                    tokio::select! {
                        _ = stats_tick.tick() => {
                            if !stats_on {
                                continue;
                            }
                            let paused = observe_paused.load(Ordering::Relaxed);
                            let closed_eyes = observe_closed_eyes.load(Ordering::Relaxed);
                            let locked = host::is_screen_locked();
                            let frontmost = front.frontmost().await.ok().flatten();
                            if !observe_policy_gate(
                                paused,
                                closed_eyes,
                                locked,
                                &privacy,
                                frontmost.as_ref().and_then(|f| f.bundle_id.as_deref()),
                                frontmost.is_some(),
                            )
                            .allows()
                            {
                                lumen_platform_macos::input_reset(state);
                                continue;
                            }
                            let counts = lumen_platform_macos::input_snapshot(state);
                            lumen_platform_macos::input_reset(state);
                            let event = SourceEvent::new(
                                SourceKind::Screen,
                                event_kind::INPUT_STATS_V1,
                                serde_json::to_value(&counts).unwrap_or_default(),
                            );
                            if let Err(e) = store_in.append_event(event) {
                                warn!(error = %e, "append input.stats.v1 failed");
                                counters.note_persist_failed();
                            } else {
                                note_write();
                                counters.note_persisted();
                            }
                        }
                        _ = hid_tick.tick() => {
                            let paused = observe_paused.load(Ordering::Relaxed);
                            let closed_eyes = observe_closed_eyes.load(Ordering::Relaxed);
                            let locked = host::is_screen_locked();
                            let frontmost = front.frontmost().await.ok().flatten();
                            if !observe_on
                                || !observe_policy_gate(
                                    paused,
                                    closed_eyes,
                                    locked,
                                    &privacy,
                                    frontmost.as_ref().and_then(|f| f.bundle_id.as_deref()),
                                    frontmost.is_some(),
                                )
                                .allows()
                            {
                                let _ = lumen_platform_macos::input_drain_hid(state);
                                lumen_platform_macos::input_reset(state);
                                coal.discard_text();
                                continue;
                            }
                            let raw = lumen_platform_macos::input_drain_hid(state);
                            if raw.is_empty() {
                                if record_text {
                                    persist_interaction_events(
                                        &store_in,
                                        coal.flush_due(std::time::Instant::now()),
                                        &counters,
                                    );
                                } else {
                                    coal.discard_text();
                                }
                                continue;
                            }
                            let _persist = session_persist.lock().await;
                            let session_id = ensure_session_if_absent(
                                &store_in,
                                &session_binder,
                                frontmost.as_ref().map(|f| f.app_name.as_str()),
                                frontmost.as_ref().and_then(|f| f.bundle_id.as_deref()),
                            );
                            if session_id.is_none() {
                                coal.discard_text();
                                continue;
                            }
                            let ctx = InteractionContext {
                                app_name: frontmost.as_ref().map(|f| f.app_name.clone()),
                                bundle_id: frontmost.as_ref().and_then(|f| f.bundle_id.clone()),
                                window_title: frontmost.as_ref().and_then(|f| f.window_title.clone()),
                                url: frontmost.as_ref().and_then(|f| f.tab_url.clone()),
                                session_id,
                            };
                            let now = std::time::Instant::now();
                            let mut last_selection: Option<String> = None;
                            for hid in raw {
                                let mut evs = coal.push(hid, ctx.clone(), now);
                                if !record_text {
                                    evs.retain(|e| e.kind != event_kind::KEYBOARD_TEXT_INPUT_V1);
                                }
                                for ev in evs {
                                    let is_sel_trigger = ev.kind == event_kind::MOUSE_CLICK_V1
                                        || ev.kind == event_kind::MOUSE_CONTEXT_MENU_V1
                                        || ev.kind == event_kind::MOUSE_DRAG_V1;
                                    if record_text && is_sel_trigger {
                                        if let Some(sel) = lumen_platform_macos::focused_selection() {
                                            if !sel.text.trim().is_empty()
                                                && last_selection.as_deref() != Some(sel.text.as_str())
                                            {
                                                last_selection = Some(sel.text.clone());
                                                persist_interaction_events(
                                                    &store_in,
                                                    vec![attach_selection(&ctx, sel.text)],
                                                    &counters,
                                                );
                                            }
                                        }
                                    }
                                    persist_interaction_events(&store_in, vec![ev], &counters);
                                }
                            }
                            if record_text {
                                persist_interaction_events(
                                    &store_in,
                                    coal.flush_due(now),
                                    &counters,
                                );
                            } else {
                                coal.discard_text();
                            }
                        }
                    }
                }
            }))
        }
        Err(e) => {
            warn!(error = %e, "input counter failed to start (needs Input Monitoring permission)");
            None
        }
    }
}

fn recovery_policy_from_config(config: &Config) -> RecoveryPolicy {
    let mut reclaim_kinds = Vec::new();
    let mut skip_kinds = Vec::new();
    if config.ocr.enabled {
        reclaim_kinds.push(ReclaimKind {
            kind: "ocr_screen".into(),
            stale_running: chrono::Duration::milliseconds(config.ocr.stale_running_ms as i64),
        });
    } else {
        skip_kinds.push(("ocr_screen".into(), "ocr_disabled_on_boot".into()));
    }
    if config.asr.enabled {
        reclaim_kinds.push(ReclaimKind {
            kind: "transcribe_audio".into(),
            stale_running: chrono::Duration::milliseconds(config.asr.stale_running_ms as i64),
        });
    } else {
        skip_kinds.push(("transcribe_audio".into(), "asr_disabled_on_boot".into()));
    }
    if config.ax.enabled {
        reclaim_kinds.push(ReclaimKind {
            kind: "ax_screen".into(),
            stale_running: chrono::Duration::milliseconds(config.ax.stale_running_ms as i64),
        });
    } else {
        skip_kinds.push(("ax_screen".into(), "ax_disabled_on_boot".into()));
    }
    RecoveryPolicy {
        now: chrono::Utc::now(),
        reclaim_kinds,
        skip_kinds,
    }
}

fn cua_client_from_env() -> Option<CuaClient> {
    let socket = std::env::var_os(CUA_SOCKET_ENV)?;
    let token_file = std::env::var_os(CUA_TOKEN_FILE_ENV)?;
    Some(CuaClient::new(socket, token_file))
}

#[tokio::main]
async fn main() -> Result<()> {
    // INFO by default; RUST_LOG upgrades per-crate (e.g. RUST_LOG=lumen_daemon=debug).
    let max_level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|v| v.to_ascii_uppercase().parse::<tracing::Level>().ok())
        .unwrap_or(Level::INFO);
    let subscriber = FmtSubscriber::builder().with_max_level(max_level).finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("mcp") {
        #[cfg(unix)]
        {
            return mcp::run().await;
        }
        #[cfg(not(unix))]
        {
            anyhow::bail!("lumen-daemon mcp requires a Unix domain socket");
        }
    }

    info!(
        product = "lumen-navi",
        repo = "https://github.com/fakechris/lumen-navi",
        phase = "S3-audio-asr",
        "daemon starting"
    );

    // Parent watchdog: when spawned by the desktop app (which passes its pid
    // via env), exit once the parent is gone. The app cannot always reap us —
    // SIGTERM/pkill skips Tauri's RunEvent::Exit — which used to leave daemons
    // orphaned to launchd (PPID=1) forever. Manual runs without this env var
    // (tests, dogfood shells) skip the watchdog entirely.
    #[cfg(unix)]
    if let Some(ppid) = std::env::var(PARENT_PID_ENV)
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        info!(ppid, "parent watchdog armed");
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(5));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                // kill(pid, 0): 0 = alive; ESRCH = gone; EPERM still means alive.
                let alive = unsafe { libc::kill(ppid, 0) } == 0
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                if !alive {
                    error!(ppid, "parent process gone; exiting (orphaned daemon)");
                    std::process::exit(1);
                }
            }
        });
    }

    // The desktop app passes the config path via env; fall back to cwd-relative
    // `navi.toml` for standalone runs (tests, dogfood from a shell).
    let config_path = std::env::var("LUMEN_NAVI_CONFIG").unwrap_or_else(|_| "navi.toml".into());
    let mut config = Config::load_or_default(&config_path).unwrap_or_default();
    // A bare default config has data_dir="data" (cwd-relative). When no
    // explicit config was provided (env unset and no cwd navi.toml), anchor
    // the data dir to the platform default so a daemon started from an
    // arbitrary cwd doesn't scatter `<cwd>/data/meta/navi.db` orphans across
    // the filesystem. An existing cwd navi.toml is still honored as-is.
    if std::env::var_os("LUMEN_NAVI_CONFIG").is_none()
        && !std::path::Path::new(&config_path).exists()
        && config.data_dir == std::path::Path::new("data")
    {
        config.data_dir = default_data_dir();
    }
    info!(
        data_dir = %config.data_dir.display(),
        screen = config.sources.screen,
        audio = config.sources.audio,
        audio_mode = %config.audio.mode,
        audio_chunk_ms = config.audio.chunk_ms,
        audio_silence_ms = config.audio.session_silence_ms,
        ocr = config.ocr.enabled,
        asr = config.asr.enabled,
        asr_engine = %config.asr.engine,
        asr_locale = %config.asr.locale,
        ticks_screen = config.capture.screen_ticks,
        ticks_audio = config.audio.ticks,
        api = config.api.enabled,
        api_bind = %config.api.bind,
        browser = config.sources.browser,
        browser_configured = !config.browser.effective_ingest_token().is_empty(),
        browser_content_hosts = config.browser.content_allow_hosts.len(),
        "config"
    );

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data_dir {}", config.data_dir.display()))?;

    // Single-instance guard: an exclusive, non-blocking flock on
    // <data_dir>/daemon.lock. The desktop app can't reliably reap the daemon
    // (SIGTERM skips RunEvent::Exit), so orphaned daemons used to pile up —
    // 7 were observed double-writing the same store. The handle must stay
    // alive for the process lifetime; the lock is released on fd close.
    // The control-socket placeholder below stays as a second line of defense.
    let _daemon_lock = {
        use fs2::FileExt;
        let lock_path = config.data_dir.join("daemon.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open lock file {}", lock_path.display()))?;
        if let Err(e) = file.try_lock_exclusive() {
            error!(
                path = %lock_path.display(),
                error = %e,
                "another lumen-daemon already holds the data-dir lock; exiting"
            );
            std::process::exit(1);
        }
        file
    };

    let store = Arc::new(
        SqliteStore::open(&config.data_dir)
            .with_context(|| format!("open store {}", config.data_dir.display()))?,
    );
    let ocr_docs = store.ocr_doc_count().unwrap_or(0);
    info!(
        existing = store.len().await?,
        ocr_docs,
        schema = SCHEMA_VERSION,
        "durable store open"
    );

    let observe_paused = Arc::new(std::sync::atomic::AtomicBool::new(config.privacy.paused));
    let observe_counters = Arc::new(control_server::ObserveCounters::default());
    let observe_closed_eyes = Arc::new(std::sync::atomic::AtomicBool::new(
        config.privacy.closed_eyes,
    ));
    let session_binder = SharedSessionBinder::new(config.capture.idle_session_ms);
    let session_persist = Arc::new(tokio::sync::Mutex::new(()));

    let recovery = store
        .recover_after_unclean_shutdown(&recovery_policy_from_config(&config))
        .with_context(|| "startup store recovery")?;
    info!(
        sessions_closed = recovery.sessions_closed,
        jobs_reclaimed = recovery.jobs_reclaimed,
        jobs_skipped = recovery.jobs_skipped,
        "startup recovery"
    );

    store
        .append(vec![SourceEvent::new(
            SourceKind::Other("daemon".into()),
            "daemon.boot.v1",
            json!({
                "phase": "fact-layer-f1a",
                "observe": true,
                "screen": config.sources.screen,
                "audio": config.sources.audio,
                "ocr": config.ocr.enabled,
                "asr": config.asr.enabled,
                "ax": config.ax.enabled,
                "api": config.api.enabled,
                "build": env!("CARGO_PKG_VERSION"),
                "schema": SCHEMA_VERSION,
                "boot_reason": "startup",
                "recovery": {
                    "sessions_closed": recovery.sessions_closed,
                    "jobs_reclaimed": recovery.jobs_reclaimed,
                    "jobs_skipped": recovery.jobs_skipped,
                },
            }),
        )])
        .await?;

    let perms = host::permissions();
    let mut status = perms.status().await?;
    info!(screen = ?status.screen_recording, mic = ?status.microphone, "permissions");
    if config.sources.screen && !status.can_capture_screen() {
        let _ = host::request_screen_recording();
        status = perms.status().await?;
        info!(screen = ?status.screen_recording, "after screen request");
    }

    let cua_client = cua_client_from_env();
    let (screen_ready, screen_error) = if config.sources.screen {
        if host::capabilities().os == "macos" {
            match cua_client.as_ref() {
                Some(client) => {
                    let client = client.clone();
                    match tokio::task::spawn_blocking(move || client.status()).await {
                        Ok(Ok(status))
                            if status.screen_recording
                                == lumen_platform::PermissionState::Granted =>
                        {
                            (true, None)
                        }
                        Ok(Ok(_)) => (
                            false,
                            Some("Screen Recording permission is required for Lumen Cua".into()),
                        ),
                        Ok(Err(error)) => (false, Some(error.to_string())),
                        Err(error) => (
                            false,
                            Some(format!("Lumen Cua status task failed: {error}")),
                        ),
                    }
                }
                None => (false, Some("Lumen Cua connection was not provided".into())),
            }
        } else if status.can_capture_screen() {
            (true, None)
        } else {
            (
                false,
                Some("Screen capture permission is unavailable".into()),
            )
        }
    } else {
        (false, None)
    };

    // Cua readiness retry: screen capture may be enabled while Cua is not
    // ready at boot (Cua crashed, or was launched after the daemon).
    // Previously screen_ready froze to false for the daemon's whole lifetime
    // and capture stayed dead until restart (observed: a 4h outage). The
    // screen loop below gates its capture ticks on this shared flag; this
    // task flips it once Cua answers with Screen Recording granted.
    // CuaClient opens a fresh connection per call, so a relaunched Cua is
    // picked up without any client-side reset.
    let cua_ready = Arc::new(AtomicBool::new(screen_ready));
    let cua_retry = !screen_ready
        && config.sources.screen
        && config.capture.screen_ticks == 0
        && host::capabilities().os == "macos"
        && cua_client.is_some();
    if cua_retry {
        let client = cua_client.clone().expect("cua_retry implies a Cua client");
        let flag = Arc::clone(&cua_ready);
        info!("Lumen Cua not ready at boot; retrying every 60s, capture starts when ready");
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                let c = client.clone();
                if let Ok(Ok(status)) = tokio::task::spawn_blocking(move || c.status()).await {
                    if status.screen_recording == lumen_platform::PermissionState::Granted {
                        info!("Lumen Cua became ready; enabling screen capture");
                        flag.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });
    }

    // --- OCR worker ---
    let (ocr_cancel_tx, ocr_cancel_rx) = watch::channel(false);
    let ocr_handle = if config.ocr.enabled {
        let engine = host::ocr(config.ocr.max_image_bytes as usize);
        if engine.is_supported() {
            let worker = Arc::new(OcrWorker::new(
                Arc::clone(&store),
                engine,
                OcrWorkerConfig {
                    languages: config.ocr.languages.clone(),
                    poll_interval: Duration::from_millis(config.ocr.poll_interval_ms),
                    batch_size: config.ocr.batch_size.max(1),
                    include_boxes: config.ocr.include_boxes,
                    boxes_when_empty_only: config.ocr.boxes_when_empty_only,
                    max_attempts: config.ocr.max_attempts as i64,
                    retry_base: Duration::from_millis(config.ocr.retry_base_ms),
                    retry_max: Duration::from_millis(config.ocr.retry_max_ms),
                    engine_timeout: Duration::from_millis(config.ocr.timeout_ms),
                    stale_running: Duration::from_millis(config.ocr.stale_running_ms),
                    max_image_bytes: config.ocr.max_image_bytes as usize,
                    max_text_chars: config.ocr.max_text_chars as usize,
                    shutdown_drain: Duration::from_millis(config.ocr.shutdown_drain_ms),
                },
            ));
            let _ = worker.reclaim_stale();
            let w = Arc::clone(&worker);
            let rx = ocr_cancel_rx.clone();
            Some((
                worker,
                tokio::spawn(async move {
                    w.run_until_cancelled(rx).await;
                }),
            ))
        } else {
            warn!(
                os = host::capabilities().os,
                "on-device OCR not available on this OS; worker not started"
            );
            None
        }
    } else {
        info!("OCR disabled in config");
        None
    };

    // --- AX tree worker (deep accessibility text for recall/search) ---
    // The daemon has NO Accessibility TCC — AX tree walks go through Lumen Cua
    // (which holds the permission), over the Unix socket. If cua is not
    // available (dev mode without screen_ready), the AX worker stays idle.
    let (ax_cancel_tx, ax_cancel_rx) = watch::channel(false);
    let ax_handle = if config.ax.enabled && cua_client.is_some() {
        let walker: Arc<dyn lumen_platform::AxTreeWalker> = Arc::new(
            lumen_cua::CuaAxTreeAdapter::new(cua_client.clone().unwrap()),
        );
        if walker.is_supported() {
            let worker = Arc::new(AxWorker::new(
                Arc::clone(&store),
                walker,
                AxWorkerConfig {
                    poll_interval: Duration::from_millis(config.ax.poll_interval_ms),
                    batch_size: config.ax.batch_size.max(1),
                    max_attempts: config.ax.max_attempts as i64,
                    retry_base: Duration::from_millis(config.ax.retry_base_ms),
                    retry_max: Duration::from_millis(config.ax.retry_max_ms),
                    stale_running: Duration::from_millis(config.ax.stale_running_ms),
                    max_text_chars: config.ax.max_text_chars as usize,
                    shutdown_drain: Duration::from_millis(config.ax.shutdown_drain_ms),
                    walk: lumen_platform::AxTreeWalkConfig {
                        max_depth: config.ax.max_depth,
                        max_nodes: config.ax.max_nodes,
                        walk_timeout_ms: config.ax.walk_timeout_ms,
                        element_timeout_ms: config.ax.element_timeout_ms,
                        max_text_length: config.ax.max_text_chars as usize,
                    },
                },
            ));
            let _ = worker.reclaim_stale();
            let w = Arc::clone(&worker);
            info!("AX tree worker started");
            Some((
                worker,
                tokio::spawn(async move {
                    w.run_until_cancelled(ax_cancel_rx).await;
                }),
            ))
        } else {
            warn!("AX tree walker not supported on this OS");
            None
        }
    } else {
        info!("AX tree capture disabled in config");
        None
    };

    // --- Input counter (roast feature: behavioral keys + clicks, opt-in) ---
    // A single static state: the CGEventTap callback holds a &'static ref.
    #[cfg(target_os = "macos")]
    let _input_handle = start_macos_input_loop(
        &config,
        &store,
        &observe_paused,
        &observe_closed_eyes,
        &session_binder,
        &session_persist,
        &observe_counters,
    );
    #[cfg(not(target_os = "macos"))]
    let _input_handle = {
        if config.input.enabled {
            info!("input counter requires macOS; disabled on this OS");
        }
        None::<tokio::task::JoinHandle<()>>
    };
    // Continuous mic → audio_chunk → transcribe_audio jobs → engine (SenseVoice default).
    let (asr_cancel_tx, asr_cancel_rx) = watch::channel(false);
    let asr_handle = if config.asr.enabled {
        match build_asr_engine(&config.asr) {
            Ok(engine) => {
                if engine.is_supported() {
                    let worker = Arc::new(TranscribeWorker::new(
                        Arc::clone(&store),
                        engine,
                        TranscribeWorkerConfig {
                            locale: config.asr.locale.clone(),
                            poll_interval: Duration::from_millis(config.asr.poll_interval_ms),
                            batch_size: config.asr.batch_size.max(1),
                            max_attempts: config.asr.max_attempts as i64,
                            retry_base: Duration::from_millis(config.asr.retry_base_ms),
                            retry_max: Duration::from_millis(config.asr.retry_max_ms),
                            engine_timeout: Duration::from_millis(config.asr.timeout_ms),
                            stale_running: Duration::from_millis(config.asr.stale_running_ms),
                            max_audio_bytes: config.asr.max_audio_bytes as usize,
                            max_text_chars: config.asr.max_text_chars as usize,
                            shutdown_drain: Duration::from_millis(config.asr.shutdown_drain_ms),
                        },
                    ));
                    let _ = worker.reclaim_stale();
                    let w = Arc::clone(&worker);
                    let rx = asr_cancel_rx.clone();
                    Some((
                        worker,
                        tokio::spawn(async move {
                            w.run_until_cancelled(rx).await;
                        }),
                    ))
                } else {
                    warn!("ASR engine reports not supported; worker not started");
                    None
                }
            }
            Err(e) => {
                warn!(error = %e, "ASR engine unavailable; worker not started");
                None
            }
        }
    } else {
        info!("ASR disabled in config");
        None
    };

    let mut screen_status = SourceStatus {
        id: "screen".into(),
        enabled: config.sources.screen,
        running: false,
        last_error: screen_error,
    };
    let audio_status = Arc::new(std::sync::Mutex::new(SourceStatus {
        id: "audio".into(),
        enabled: config.sources.audio,
        running: false,
        last_error: None,
    }));

    // --- Local control API ---
    // Two listeners: a Unix socket (primary — the shell connects here, no TCP
    // port to conflict over) and TCP loopback (best-effort — for the browser
    // extension, which can't open sockets). Socket bind failure is fatal
    // (exit 1 → supervisor alert + restart); TCP failure is non-fatal.
    let _api_handle: Option<tokio::task::JoinHandle<()>> = if config.api.enabled {
        let control_state = control_server::ControlState::new(
            Arc::clone(&store),
            Arc::clone(&observe_paused),
            Arc::clone(&observe_closed_eyes),
            config.retention.max_blob_mb.saturating_mul(1024 * 1024),
            vec![
                screen_status.clone(),
                audio_status
                    .lock()
                    .map(|status| status.clone())
                    .unwrap_or(SourceStatus {
                        id: "audio".into(),
                        enabled: config.sources.audio,
                        running: false,
                        last_error: Some("audio status unavailable".into()),
                    }),
                SourceStatus {
                    id: "browser".into(),
                    enabled: config.sources.browser,
                    running: config.sources.browser
                        && !config.browser.effective_ingest_token().is_empty(),
                    last_error: None,
                },
            ],
            Arc::clone(&audio_status),
            control_server::BrowserRuntimeConfig {
                enabled: config.sources.browser,
                token: config.browser.effective_ingest_token(),
                policy: BrowserIngestPolicy {
                    content_allow_hosts: config.browser.content_allow_hosts.clone(),
                    excluded_hosts: config.browser.excluded_hosts.clone(),
                    max_batch_size: config.browser.max_batch_size,
                    max_artifact_bytes: config.browser.max_artifact_bytes,
                },
            },
            Arc::clone(&observe_counters),
            config.privacy.app_blocklist.clone(),
        );

        let socket_path = config.data_dir.join("daemon.sock");
        // Fatal on bind failure: the shell depends on this socket.
        match control_server::spawn(&socket_path, control_state.clone()) {
            Ok(handle) => {
                // Best-effort TCP listener for the browser extension. Tries
                // 7420, then increments; writes actual port to daemon.tcp_port.
                let port_file = config.data_dir.join("daemon.tcp_port");
                let _ = control_server::spawn_tcp(&config.api.bind, 20, &port_file, control_state);
                Some(handle)
            }
            Err(e) => {
                error!(error = %e, "fatal: control socket bind failed; exiting so supervisor can restart");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    {
        let store_slots = Arc::clone(&store);
        let assistant = config.assistant.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                match store_slots.persist_closed_history_slots() {
                    Ok(n) if n > 0 => {
                        info!(slots = n, "persisted closed history slots");
                    }
                    Err(e) => warn!(error = %e, "persist history slots failed"),
                    _ => {}
                }
                if let Err(e) = summarizer::fill_pending_slot_narratives(&store_slots, &assistant) {
                    warn!(error = %e, "slot narrative job failed");
                }
            }
        });
    }

    // Background category enrichment (Homebrew index + iTunes). Off the
    // sampling path; fills app_category_cache and re-applies segments.
    {
        let store_en = Arc::clone(&store);
        tokio::spawn(async move {
            // First pass after a short delay so capture can start first.
            tokio::time::sleep(Duration::from_secs(45)).await;
            let mut every = tokio::time::interval(Duration::from_secs(15 * 60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let store = Arc::clone(&store_en);
                match tokio::task::spawn_blocking(move || {
                    store.process_category_enrichment(40, true)
                })
                .await
                {
                    Ok(Ok(report)) => {
                        if report.attempted > 0 || report.brew_index_refreshed {
                            info!(
                                attempted = report.attempted,
                                resolved = report.resolved,
                                failed = report.failed,
                                brew_rows = report.brew_index_rows,
                                brew_refreshed = report.brew_index_refreshed,
                                "category enrichment pass"
                            );
                        }
                    }
                    Ok(Err(e)) => warn!(error = %e, "category enrichment failed"),
                    Err(e) => warn!(error = %e, "category enrichment task join failed"),
                }
                every.tick().await;
            }
        });
    }

    // Shared cancel for long-running observe tasks.
    let (observe_cancel_tx, observe_cancel_rx) = watch::channel(false);

    // --- Audio (concurrent with screen) ---
    let audio_task = if config.sources.audio {
        let store_a = Arc::clone(&store);
        let audio_cfg = config.audio.clone();
        let privacy = config.privacy.clone();
        let cancel = observe_cancel_rx.clone();
        let counters = Arc::clone(&observe_counters);
        let audio_status_for_task = Arc::clone(&audio_status);
        Some(tokio::spawn(async move {
            let result = run_audio_loop(
                store_a,
                audio_cfg,
                privacy,
                cancel,
                counters,
                audio_status_for_task.clone(),
            )
            .await;
            if let Err(error) = &result {
                if let Ok(mut status) = audio_status_for_task.lock() {
                    status.running = false;
                    status.last_error = Some(error.to_string());
                }
            }
            result
        }))
    } else {
        None
    };

    let mut ran_long_loop = false;
    let expect_long = ((screen_ready || cua_retry) && config.capture.screen_ticks == 0)
        || (config.sources.audio && config.audio.ticks == 0);

    // Write stall log: the desktop health monitor owns restarts (shared
    // 10-minute budget). This process must not suicide on a quiet desk —
    // identity-only activity no longer inserts a row every few seconds.
    if expect_long {
        note_write();
        let watchdog_paused = Arc::clone(&observe_paused);
        let watchdog_closed_eyes = Arc::clone(&observe_closed_eyes);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                if watchdog_paused.load(Ordering::Relaxed)
                    || watchdog_closed_eyes.load(Ordering::Relaxed)
                {
                    note_write();
                    continue;
                }
                let last = LAST_WRITE_UNIX.load(Ordering::Relaxed);
                let silent_for = unix_now().saturating_sub(last);
                if last > 0 && silent_for > 300 {
                    warn!(
                        silent_for_secs = silent_for,
                        "no events written for over 5 minutes while observing"
                    );
                }
            }
        });
    }

    // Activity tracking runs independently of screen capture. When screen is
    // ready, the capture loop's focus_tick already drives poll_activity(). But
    // when screen capture is unavailable (no Cua / no permission) — and won't
    // become available this run — we still want time tracking, so spin up a
    // standalone activity loop here. (When cua_retry is set, the screen loop
    // below runs in activity-only mode until Cua is ready and already covers
    // activity tracking.)
    if !screen_ready && !cua_retry {
        let store_act = Arc::clone(&store);
        let mut orch = CaptureOrchestrator::with_sessions(
            Arc::new(lumen_platform::NullDisplays),
            Arc::new(lumen_platform::NullCapturer),
            host::frontmost(),
            host::screen_lock(),
            host::idle(),
            host::display_sleep(),
            config.capture.clone(),
            config.privacy.clone(),
            Arc::clone(&session_binder),
        );
        let mut tick =
            tokio::time::interval(Duration::from_millis(config.capture.focus_poll_ms.max(500)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut act_cancel = observe_cancel_rx.clone();
        let observe_paused = Arc::clone(&observe_paused);
        let observe_closed_eyes = Arc::clone(&observe_closed_eyes);
        let session_persist = Arc::clone(&session_persist);
        let counters = Arc::clone(&observe_counters);
        tokio::spawn(async move {
            tick.tick().await; // consume the immediate tick
            info!("activity tracker running (screen capture unavailable, tracking time only)");
            orch.set_paused(observe_paused.load(Ordering::Relaxed));
            orch.set_closed_eyes(observe_closed_eyes.load(Ordering::Relaxed));
            {
                let _persist = session_persist.lock().await;
                persist_activity_poll(&store_act, orch.poll_activity().await, &counters);
            }
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        orch.set_paused(observe_paused.load(Ordering::Relaxed));
                        orch.set_closed_eyes(observe_closed_eyes.load(Ordering::Relaxed));
                        let _persist = session_persist.lock().await;
                        persist_activity_poll(&store_act, orch.poll_activity().await, &counters);
                        persist_session_transition(&store_act, orch.close_idle_session(), &counters);
                    }
                    _ = act_cancel.changed() => {
                        if *act_cancel.borrow() {
                            let _persist = session_persist.lock().await;
                            persist_session_transition(&store_act, orch.force_close_session(), &counters);
                            break;
                        }
                    }
                }
            }
        });
        ran_long_loop = true;
    }

    if screen_ready || cua_retry {
        let (displays, capturer): (Arc<dyn DisplayEnumerator>, Arc<dyn ScreenCapturer>) =
            if host::capabilities().os == "macos" {
                let capture = Arc::new(CuaCaptureAdapter::new(
                    cua_client.expect("macOS screen loop requires an initialized Lumen Cua client"),
                ));
                (capture.clone(), capture)
            } else {
                (host::displays(), host::screen_capturer())
            };
        let mut orch = CaptureOrchestrator::with_sessions(
            displays,
            capturer,
            host::frontmost(),
            host::screen_lock(),
            host::idle(),
            host::display_sleep(),
            config.capture.clone(),
            config.privacy.clone(),
            Arc::clone(&session_binder),
        );

        let (tx, mut rx) = mpsc::channel::<CapturedBatch>(config.capture.queue_capacity);
        let store_w = Arc::clone(&store);
        let ocr_on = config.ocr.enabled;
        let ax_on = config.ax.enabled;
        let persist_counters = Arc::clone(&observe_counters);
        let persist = tokio::spawn(async move {
            let mut ocr_hashes = PixelHashWindow::default();
            while let Some(batch) = rx.recv().await {
                let mut upserts = Vec::new();
                if let Some(closed) = batch.closed_session {
                    upserts.push(closed);
                }
                if let Some(open) = batch.open_session {
                    upserts.push(open);
                }
                if !upserts.is_empty() || !batch.session_events.is_empty() {
                    if let Err(e) =
                        store_w.apply_activity_transition(&upserts, &batch.session_events)
                    {
                        warn!(error = %e, "apply capture session transition failed");
                        persist_counters.note_persist_failed();
                    } else {
                        persist_counters.note_persisted();
                    }
                }
                for (event, frame) in batch.frames {
                    let pixel_hash = event
                        .payload
                        .get("pixel_hash")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    match store_w.put_and_append(
                        event,
                        frame.media_type.clone(),
                        &frame.png_or_jpeg_bytes,
                    ) {
                        Ok(stored) => {
                            note_write();
                            persist_counters.note_persisted();
                            if ocr_on {
                                let skip_dup_hash = pixel_hash
                                    .as_deref()
                                    .is_some_and(|h| ocr_hashes.contains(h));
                                if skip_dup_hash {
                                    debug_skip_dup_ocr();
                                } else {
                                    match store_w.enqueue_job(stored.id, "ocr_screen") {
                                        Ok(Some(_)) => {
                                            if let Some(h) = pixel_hash.as_deref() {
                                                ocr_hashes.insert(h);
                                            }
                                        }
                                        Ok(None) => debug_skip_dup_ocr(),
                                        Err(e) => warn!(error = %e, "enqueue ocr_screen failed"),
                                    }
                                }
                            }
                            if ax_on {
                                if let Err(e) = store_w.enqueue_job(stored.id, "ax_screen") {
                                    warn!(error = %e, "enqueue ax_screen failed");
                                }
                            }
                            info!(
                                id = %stored.id,
                                kind = %stored.kind,
                                media = %frame.media_type,
                                bytes = frame.png_or_jpeg_bytes.len(),
                                "persisted screenshot"
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, "persist failed");
                            persist_counters.note_persist_failed();
                        }
                    }
                }
            }
        });

        screen_status.running = true;
        let interval = Duration::from_millis(config.capture.screen_interval_ms);
        let focus_every = Duration::from_millis(config.capture.focus_poll_ms);
        let max_ticks = config.capture.screen_ticks;
        let mut full_ticks = 0u64;
        let mut interval_ticks = 0u64;
        let mut focus_tick = tokio::time::interval(focus_every);
        focus_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut capture_tick = tokio::time::interval(interval);
        capture_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        focus_tick.tick().await;
        capture_tick.tick().await;
        orch.set_paused(observe_paused.load(Ordering::Relaxed));
        orch.set_closed_eyes(observe_closed_eyes.load(Ordering::Relaxed));
        {
            let _persist = session_persist.lock().await;
            persist_activity_poll(&store, orch.poll_activity().await, &observe_counters);
        }

        info!("observe screen loop running (Ctrl+C to stop if ticks=0)");
        if !cua_ready.load(Ordering::Relaxed) {
            info!(
                "Lumen Cua not ready; loop is activity-only until the retry task enables capture"
            );
        }
        if max_ticks == 0 {
            ran_long_loop = true;
        }

        loop {
            if max_ticks > 0 && (full_ticks >= max_ticks || interval_ticks >= max_ticks) {
                break;
            }

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl+C");
                    break;
                }
                _ = focus_tick.tick() => {
                    orch.set_paused(observe_paused.load(Ordering::Relaxed));
                    orch.set_closed_eyes(observe_closed_eyes.load(Ordering::Relaxed));
                    {
                        let _persist = session_persist.lock().await;
                        persist_activity_poll(&store, orch.poll_activity().await, &observe_counters);
                    }

                    // Capture is gated on Cua readiness: while Cua is down the
                    // loop keeps tracking activity only, and starts capturing
                    // the first tick after the retry task flips the flag.
                    if cua_ready.load(Ordering::Relaxed) {
                        if let Some(reason) = orch.poll_focus_trigger().await {
                            match orch.capture_tick(reason).await {
                                Ok(Some(batch)) => {
                                    full_ticks += 1;
                                    if tx.try_send(batch).is_err() {
                                        orch.note_backpressure_drop();
                                        warn!("backpressure: drop capture batch");
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    warn!(error = %e, "focus capture failed");
                                    screen_status.last_error = Some(e);
                                }
                            }
                        }
                    }
                    {
                        let _persist = session_persist.lock().await;
                        persist_session_transition(
                            &store,
                            orch.close_idle_session(),
                            &observe_counters,
                        );
                    }
                    {
                        let st = orch.stats();
                        observe_counters
                            .sync_capture_stats(st.skipped_gate, st.dropped_backpressure);
                    }
                }
                _ = capture_tick.tick() => {
                    orch.set_paused(observe_paused.load(Ordering::Relaxed));
                    orch.set_closed_eyes(observe_closed_eyes.load(Ordering::Relaxed));
                    if cua_ready.load(Ordering::Relaxed) {
                        interval_ticks += 1;
                        match orch.capture_tick(TriggerReason::Interval).await {
                            Ok(Some(batch)) => {
                                full_ticks += 1;
                                if tx.try_send(batch).is_err() {
                                    orch.note_backpressure_drop();
                                    warn!("backpressure: drop capture batch");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(error = %e, "interval capture failed");
                                screen_status.last_error = Some(e);
                            }
                        }
                        let st = orch.stats();
                        observe_counters
                            .sync_capture_stats(st.skipped_gate, st.dropped_backpressure);
                    }
                }
            }
        }

        {
            let _persist = session_persist.lock().await;
            persist_session_transition(&store, orch.force_close_session(), &observe_counters);
        }
        drop(tx);
        let _ = persist.await;
        screen_status.running = false;

        let st = orch.stats();
        info!(
            full = st.full_captures,
            probes = st.probes,
            skip_visual = st.skipped_visual,
            skip_debounce = st.skipped_debounce,
            skip_gate = st.skipped_gate,
            drop_bp = st.dropped_backpressure,
            "capture stats"
        );
    } else if config.sources.audio && config.audio.ticks == 0 {
        // Audio-only continuous: wait until Ctrl+C (audio task runs in background).
        ran_long_loop = true;
        info!("audio-only observe running; Ctrl+C to stop");
        let _ = tokio::signal::ctrl_c().await;
        info!("Ctrl+C");
    } else if config.sources.audio && config.audio.ticks > 0 {
        // Finite audio smoke without screen: wait for audio task / cancel after grace.
        let wait_ms = config
            .audio
            .chunk_ms
            .saturating_mul(config.audio.ticks.saturating_add(2));
        tokio::time::sleep(Duration::from_millis(wait_ms.max(2_000))).await;
    }

    // Stop audio + OCR.
    let _ = observe_cancel_tx.send(true);
    if let Some(handle) = audio_task {
        match handle.await {
            Ok(Ok(st)) => {
                info!(
                    emitted = st.chunks_emitted,
                    silent = st.chunks_dropped_silent,
                    pause = st.chunks_dropped_pause,
                    sessions_open = st.sessions_opened,
                    sessions_close = st.sessions_closed,
                    "audio stats"
                );
            }
            Ok(Err(e)) => {
                warn!(error = %e, "audio task failed");
            }
            Err(e) => {
                if let Ok(mut status) = audio_status.lock() {
                    status.running = false;
                    status.last_error = Some(format!("audio task join failed: {e}"));
                }
                warn!(error = %e, "audio task join failed");
            }
        }
    }

    if let Some((worker, handle)) = ocr_handle {
        let _ = ocr_cancel_tx.send(true);
        let _ = handle.await;
        if config.capture.screen_ticks > 0 {
            let st = worker.drain(40).await;
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                skipped = st.skipped_existing,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "ocr stats"
            );
        } else {
            let st = worker.stats();
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "ocr stats"
            );
        }
        if let Ok(counts) = store.job_counts_by_status("ocr_screen") {
            info!(?counts, "ocr job counts");
        }
    }

    if let Some((_worker, handle)) = ax_handle {
        let _ = ax_cancel_tx.send(true);
        let _ = handle.await;
        if let Ok(counts) = store.job_counts_by_status("ax_screen") {
            info!(?counts, "ax job counts");
        }
    }

    if let Some((worker, handle)) = asr_handle {
        let _ = asr_cancel_tx.send(true);
        let _ = handle.await;
        if config.audio.ticks > 0 || config.capture.screen_ticks > 0 {
            let st = worker.drain(40).await;
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                skipped = st.skipped_existing,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "asr stats"
            );
        } else {
            let st = worker.stats();
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "asr stats"
            );
        }
        if let Ok(counts) = store.job_counts_by_status(JOB_KIND_TRANSCRIBE_AUDIO) {
            info!(?counts, "asr job counts");
        }
    }

    // API-only keep-alive when no long observe ran.
    if config.api.enabled && expect_long && !ran_long_loop {
        info!(
            bind = %config.api.bind,
            "control API idle; Ctrl+C to stop"
        );
        tokio::signal::ctrl_c().await?;
        info!("Ctrl+C");
    } else if config.api.enabled && !expect_long && !screen_ready && !config.sources.audio {
        info!(
            bind = %config.api.bind,
            "control API only; Ctrl+C to stop"
        );
        tokio::signal::ctrl_c().await?;
        info!("Ctrl+C");
    }

    let final_audio_status =
        audio_status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(SourceStatus {
                id: "audio".into(),
                enabled: config.sources.audio,
                running: false,
                last_error: Some("audio status unavailable".into()),
            });
    let health = HealthResponse::scaffold(
        vec![screen_status, final_audio_status],
        store.len().await?,
        config.privacy.paused,
        store.ocr_doc_count().unwrap_or(0),
        SCHEMA_VERSION,
    );
    info!(
        stored = health.stored_events,
        ocr_docs = health.ocr_docs,
        "health"
    );
    Ok(())
}

/// `voice` flag from the audio_chunk payload; missing (old events) counts as voiced.
fn stored_voice_flag(event: &lumen_types::SourceEvent) -> bool {
    event
        .payload
        .get("voice")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

async fn run_audio_loop(
    store: Arc<SqliteStore>,
    config: AudioConfig,
    privacy: PrivacyConfig,
    mut cancel: watch::Receiver<bool>,
    counters: Arc<control_server::ObserveCounters>,
    audio_status: Arc<std::sync::Mutex<SourceStatus>>,
) -> Result<lumen_sources_media::AudioStats> {
    let open_cfg = MicOpenConfig {
        preferred_sample_rate: config.sample_rate,
        preferred_channels: config.channels,
        chunk_ms: config.effective_chunk_ms(),
        device: config.device.clone(),
    };
    let capturer = host::mic();
    let stream = tokio::task::spawn_blocking(move || capturer.open(open_cfg))
        .await
        .context("join mic open")?
        .context("open microphone")?;

    if let Ok(mut status) = audio_status.lock() {
        status.running = true;
        status.last_error = None;
    }
    info!(
        mode = %config.mode,
        chunk_ms = config.effective_chunk_ms(),
        silence_ms = config.session_silence_ms,
        max_session_ms = config.max_session_ms,
        ticks = config.ticks,
        enqueue_transcribe = config.enqueue_transcribe,
        "audio observe started"
    );

    let enqueue_asr = config.enqueue_transcribe;
    let mut orch = AudioOrchestrator::new(config.clone(), privacy);
    let max_ticks = config.ticks;
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_audio_report = Instant::now();

    loop {
        if *cancel.borrow() {
            break;
        }
        if max_ticks > 0 && orch.stats().chunks_emitted >= max_ticks {
            break;
        }

        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            _ = poll.tick() => {
                let batch = orch
                    .drain_ready(&stream)
                    .context("microphone stream disconnected")?;
                if last_audio_report.elapsed() >= Duration::from_secs(10) {
                    let stats = orch.stats();
                    info!(
                        received = stats.chunks_received,
                        last_rms = stats.last_rms,
                        max_rms = stats.max_rms,
                        emitted = stats.chunks_emitted,
                        silent = stats.chunks_dropped_silent,
                        paused = stats.chunks_dropped_pause,
                        "audio input stats"
                    );
                    if let Ok(mut status) = audio_status.lock() {
                        if stats.chunks_received > 0 && stats.chunks_emitted == 0 {
                            status.last_error = Some(format!(
                                "本进程已收到 {} 个音频块，但当前运行期间都被静音检测过滤（最大 RMS {:.4}），暂未新增 audio_chunk.v1。请靠近麦克风说话，或检查系统输入设备。",
                                stats.chunks_received, stats.max_rms
                            ));
                        } else if stats.chunks_emitted > 0 {
                            status.last_error = None;
                        }
                    }
                    last_audio_report = Instant::now();
                }
                for cap in batch {
                    let bytes = cap.wav.len();
                    // Only voiced chunks are worth ASR; silent ones may still be
                    // stored (drop_silent_chunks=false) but must not burn
                    // transcription work.
                    let voiced = stored_voice_flag(&cap.event);
                    match store.put_and_append(cap.event, cap.media_type, &cap.wav) {
                        Ok(stored) => {
                            note_write();
                            counters.note_persisted();
                            if enqueue_asr && voiced {
                                match store.enqueue_job(stored.id, JOB_KIND_TRANSCRIBE_AUDIO) {
                                    Ok(Some(_)) => {}
                                    Ok(None) => {}
                                    Err(e) => warn!(error = %e, "enqueue transcribe_audio failed"),
                                }
                            }
                            info!(
                                id = %stored.id,
                                kind = %stored.kind,
                                bytes,
                                session = ?stored.session_id,
                                "persisted audio chunk"
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, "audio persist failed");
                            counters.note_persist_failed();
                        }
                    }
                    if max_ticks > 0 && orch.stats().chunks_emitted >= max_ticks {
                        break;
                    }
                }
            }
        }
    }

    orch.force_close_session();
    stream.stop();
    if let Ok(mut status) = audio_status.lock() {
        status.running = false;
        status.last_error = None;
    }
    Ok(orch.stats())
}

/// Map a navi `asr.engine` config value to the shared [`EngineKind`].
///
/// Config compatibility: navi historically routed bare `qwen` / `qwen3-asr`
/// to the OpenAI-compatible HTTP path (it has no local MLX worker wiring).
/// The shared crate now parses those names as the *local* Qwen engine, so we
/// keep the old meaning for existing `navi.toml` files here. Explicitly
/// local names (`local_qwen`) still reach the shared parser unchanged.
fn engine_kind_from_config(name: &str) -> Option<EngineKind> {
    match name.trim().to_ascii_lowercase().as_str() {
        "qwen" | "qwen3-asr" | "qwen3_asr" => Some(EngineKind::OpenAiAudio),
        other => EngineKind::parse(other),
    }
}

/// Consumer-side model-dir resolution (shared `build_engine`/`probe_status`
/// no longer fall back to default directories): a configured dir wins when it
/// is actually ready, otherwise resolve the shared Lumen default via
/// `lumen_models`.
fn resolve_model_dir(
    kind: EngineKind,
    configured: &str,
    models_root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let configured = configured.trim();
    let configured_path = (!configured.is_empty()).then(|| std::path::PathBuf::from(configured));
    match kind {
        EngineKind::SenseVoice => Some(match configured_path {
            Some(p) if lumen_models::sensevoice_ready(&p) => p,
            _ => lumen_models::default_sensevoice_dir_with_root(models_root),
        }),
        EngineKind::Whisper => Some(match configured_path {
            Some(p) if lumen_models::whisper_ready(&p) => p,
            _ => lumen_models::default_whisper_dir_with_root(models_root),
        }),
        EngineKind::Qwen => Some(match configured_path {
            Some(p) if lumen_models::qwen_ready(&p) => p,
            _ => lumen_models::default_qwen_dir(),
        }),
        EngineKind::Speech | EngineKind::OpenAiAudio => None,
    }
}

/// Resolve continuous Observe ASR engine from config.
/// Default: SenseVoice (sherpa). Optional: Whisper, OpenAI-compatible HTTP (Qwen ASR), Speech.
fn build_asr_engine(asr: &AsrConfig) -> Result<Arc<dyn AsrEngine>, String> {
    let kind = engine_kind_from_config(asr.engine_name()).unwrap_or_else(|| {
        warn!(
            engine = %asr.engine,
            "unknown asr.engine; defaulting to sensevoice"
        );
        EngineKind::SenseVoice
    });
    let models_root = asr.models_root_path();
    let model_dir = resolve_model_dir(kind, &asr.model_dir, models_root.as_deref());
    let st = probe_status(kind, model_dir.as_deref());
    info!(
        engine = %kind.as_str(),
        ready = st.ready,
        model_dir = %st.model_dir,
        models_root = %models_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(shared Lumen default)".into()),
        detail = %st.detail,
        "ASR engine status"
    );

    let build_cfg = EngineBuildConfig {
        kind,
        model_dir: model_dir.unwrap_or_default(),
        locale: asr.locale.clone(),
        max_audio_bytes: asr.max_audio_bytes as usize,
        http_base_url: asr.http_base_url.clone(),
        http_api_key: asr.effective_http_api_key(),
        http_model: asr.http_model.clone(),
        http_timeout_ms: asr.timeout_ms,
        http_engine_label: asr.http_engine_label.clone(),
        qwen_python: std::path::PathBuf::new(),
        qwen_timeout_ms: asr.timeout_ms,
    };

    match kind {
        EngineKind::Speech => Ok(speech_engine(asr)),
        other => match build_engine(&build_cfg) {
            Ok(Some(eng)) => Ok(eng),
            Ok(None) => Err(format!("engine {other:?} unexpectedly returned none")),
            // The `speech` fallback only exists where the OS ships a
            // recognizer. On Windows there is none, so surface the real
            // engine error instead of swapping in an unsupported engine.
            Err(e)
                if asr.fallback_speech
                    && other != EngineKind::OpenAiAudio
                    && host::capabilities().system_speech_asr =>
            {
                warn!(
                    error = %e,
                    "preferred ASR engine failed; falling back to system speech"
                );
                Ok(speech_engine(asr))
            }
            Err(e) => Err(e),
        },
    }
}

fn speech_engine(asr: &AsrConfig) -> Arc<dyn AsrEngine> {
    Arc::new(SpeechEngineAdapter {
        inner: host::system_speech_asr(asr.max_audio_bytes as usize),
        locale: asr.locale.clone(),
        max_audio_bytes: asr.max_audio_bytes as usize,
    })
}

/// The OS speech recognizer stays in navi's platform layer (it implements
/// `lumen_platform::AsrEngine`); this adapter exposes it through the shared
/// `lumen_asr_engine::AsrEngine` trait, matching the shared crate's
/// `EngineKind::Speech → Ok(None)` contract. Where the OS ships no recognizer
/// the inner engine reports `is_supported() == false`.
struct SpeechEngineAdapter {
    inner: Arc<dyn lumen_platform::AsrEngine>,
    locale: String,
    max_audio_bytes: usize,
}

#[async_trait]
impl AsrEngine for SpeechEngineAdapter {
    fn id(&self) -> AsrEngineId {
        AsrEngineId::Speech
    }

    fn is_supported(&self) -> bool {
        self.inner.is_supported()
    }

    fn max_audio_bytes(&self) -> Option<usize> {
        Some(self.max_audio_bytes)
    }

    async fn transcribe(&self, req: AsrRequest) -> Result<AsrResult, AsrError> {
        // PCM path: Speech.framework consumes files/blobs, so re-encode.
        let wav = samples_to_wav_mono_i16(&req.samples, req.sample_rate);
        let locale = req
            .language_hint
            .clone()
            .unwrap_or_else(|| self.locale.clone());
        self.transcribe_wav(&wav, &locale).await
    }

    async fn transcribe_wav(&self, audio: &[u8], locale: &str) -> Result<AsrResult, AsrError> {
        let r = self
            .inner
            .transcribe(audio, locale)
            .await
            .map_err(platform_err_to_asr)?;
        let mut out = AsrResult::new(r.text, AsrEngineId::Speech);
        out.engine_label = r.engine; // "speech" — transcript.v1 label unchanged
        out.language = r.language;
        out.confidence = r.confidence as f32;
        Ok(out)
    }
}

fn platform_err_to_asr(e: PlatformError) -> AsrError {
    match e {
        PlatformError::Unsupported(m) => AsrError::Unsupported(m),
        // Preserve "permission denied" in the message: the transcribe worker
        // classifies it as a permanent failure.
        PlatformError::PermissionDenied(m) => {
            AsrError::Inference(format!("permission denied: {m}"))
        }
        PlatformError::Message(m) => AsrError::Inference(m),
        PlatformError::WindowGone(id) => AsrError::Inference(format!("ax window {id} is gone")),
    }
}

#[inline]
fn debug_skip_dup_ocr() {
    // open job already exists — normal under burst captures
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old navi config values keep their meaning: bare `qwen` names stay on
    /// the HTTP path even though the shared crate parses them as local.
    #[test]
    fn config_qwen_still_means_http() {
        assert_eq!(
            engine_kind_from_config("qwen"),
            Some(EngineKind::OpenAiAudio)
        );
        assert_eq!(
            engine_kind_from_config("qwen3-asr"),
            Some(EngineKind::OpenAiAudio)
        );
        assert_eq!(
            engine_kind_from_config("qwen_asr_0.8b"),
            Some(EngineKind::OpenAiAudio)
        );
        assert_eq!(
            engine_kind_from_config("sensevoice"),
            Some(EngineKind::SenseVoice)
        );
        assert_eq!(engine_kind_from_config("speech"), Some(EngineKind::Speech));
        assert_eq!(
            engine_kind_from_config("local_qwen"),
            Some(EngineKind::Qwen)
        );
        assert_eq!(engine_kind_from_config("nope"), None);
    }

    /// Ported from the removed internal crate: a configured-but-gone model dir
    /// falls back to the ready shared model under models_root.
    #[test]
    fn invalid_selected_model_falls_back_to_ready_shared_model() {
        let root = std::env::temp_dir().join(format!(
            "lumen-navi-invalid-selected-model-{}",
            std::process::id()
        ));
        let shared = root.join("sensevoice");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("model.int8.onnx"), b"model").unwrap();
        std::fs::write(shared.join("tokens.txt"), b"tokens").unwrap();

        let selected = root.join("deleted-custom-model");
        assert_eq!(
            resolve_model_dir(
                EngineKind::SenseVoice,
                &selected.display().to_string(),
                Some(&root)
            ),
            Some(shared)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The cluster models contract doc navi ships must stay byte-identical to
    /// cluster v1 (same pin as lumen-suite / lumen-asr). Was previously
    /// asserted in the removed internal lumen-asr-engine crate.
    #[test]
    fn shared_model_contract_matches_cluster_v1() {
        let bytes = include_bytes!("../../../docs/SHARED_MODELS_CONTRACT.md");
        assert_eq!(fnv1a64(bytes), 0xc877_89f4_de20_5e71);
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    #[test]
    fn hid_writes_use_the_same_policy_gate_as_screenshots() {
        let privacy = PrivacyConfig::default();
        assert!(observe_policy_gate(false, false, false, &privacy, Some("a.b"), true).allows());
        assert_eq!(
            observe_policy_gate(true, false, false, &privacy, Some("a.b"), true),
            PolicyGate::Paused
        );
        assert_eq!(
            observe_policy_gate(false, true, false, &privacy, Some("a.b"), true),
            PolicyGate::ClosedEyes
        );
        assert_eq!(
            observe_policy_gate(false, false, true, &privacy, Some("a.b"), true),
            PolicyGate::Locked
        );
        let mut blocked = PrivacyConfig::default();
        blocked.app_blocklist = vec!["com.secret".into()];
        assert_eq!(
            observe_policy_gate(false, false, false, &blocked, None, false),
            PolicyGate::FrontmostUnknown
        );
        assert_eq!(
            observe_policy_gate(false, false, false, &blocked, Some("com.secret"), true),
            PolicyGate::AppBlocklist
        );
    }

    #[test]
    fn hid_opens_missing_session_but_does_not_steal_on_focus_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let binder = SharedSessionBinder::new(60_000);

        let opened =
            ensure_session_if_absent(&store, &binder, Some("Safari"), Some("com.apple.Safari"));
        assert!(opened.is_some());
        let safari = opened.unwrap();
        assert_eq!(binder.current_id(), Some(safari));

        let same =
            ensure_session_if_absent(&store, &binder, Some("Safari"), Some("com.apple.Safari"));
        assert_eq!(same, Some(safari));

        let mismatch =
            ensure_session_if_absent(&store, &binder, Some("Mail"), Some("com.apple.mail"));
        assert!(mismatch.is_none(), "focus change is owned by activity poll");
        assert_eq!(binder.current_id(), Some(safari));

        persist_session_transition(
            &store,
            binder.bind(Some("Mail"), Some("com.apple.mail"), "focus_change"),
            &control_server::ObserveCounters::default(),
        );
        let mail = binder.current_id().unwrap();
        assert_ne!(mail, safari);
        assert_eq!(
            ensure_session_if_absent(&store, &binder, Some("Mail"), Some("com.apple.mail")),
            Some(mail)
        );
    }
}
