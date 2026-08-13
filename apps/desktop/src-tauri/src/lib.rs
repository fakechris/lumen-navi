//! Lumen Navi desktop shell — store browser + control + observe sidecar + tray.

mod asr_models;
mod assistant;
mod commands;
mod cua;
mod selection_popup;
mod shell;
mod state;
mod tray;

use state::AppState;
use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen_navi_desktop=info,warn".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let state = AppState::open().expect("open app state");
    let launch_observe = state
        .shell
        .lock()
        .map(|s| s.launch_observe && !s.needs_onboarding())
        .unwrap_or(false);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(move |app| {
            if let Err(e) = tray::setup_tray(app.handle()) {
                tracing::warn!(error = %e, "tray setup failed");
            }
            {
                let popup_enabled = app
                    .try_state::<AppState>()
                    .and_then(|s| s.load_config().ok())
                    .map(|c| c.assistant.popup_enabled)
                    .unwrap_or(false);
                selection_popup::init_from_config(&app.handle(), popup_enabled);
            }
            if launch_observe {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Small delay so window is ready.
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    if let Some(state) = handle.try_state::<AppState>() {
                        match commands::observe_start_inner(&state) {
                            Ok(st) => tracing::info!(?st.pid, "auto-started Observe"),
                            Err(e) => tracing::warn!(error = %e, "auto-start Observe failed"),
                        }
                    }
                });
            }
            // Daemon supervisor: watch the child process and act on crashes.
            // Without this, a SIGSEGV/Panic in lumen-daemon was invisible — the
            // UI silently showed "本地服务未运行" with no alert and no restart.
            // This task emits `daemon://exited` on an unexpected exit (so the UI
            // can show a banner) and auto-restarts with capped backoff.
            //
            // Liveness check: connect to the daemon's Unix socket. This is more
            // reliable than checking the child slot — it recognizes daemons the
            // supervisor didn't spawn (e.g. an orphan from a previous app run
            // that's still serving on the socket) and avoids restart loops where
            // a freshly-spawned daemon exits immediately (socket-in-use) but the
            // child slot briefly looks alive.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // First check is delayed so the auto-start above has time to spawn
                    // and bind the socket.
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(2));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut consecutive_crashes = 0u32;
                    let mut last_restart_at: Option<std::time::Instant> = None;
                    loop {
                        interval.tick().await;
                        let Some(state) = handle.try_state::<AppState>() else {
                            continue;
                        };
                        // Intentional stop: nothing to watch. Reset backoff.
                        if state
                            .observe_stopping
                            .load(std::sync::atomic::Ordering::SeqCst)
                        {
                            consecutive_crashes = 0;
                            continue;
                        }
                        // PRIMARY liveness: socket reachable → daemon is serving,
                        // regardless of the child slot. This handles orphans and
                        // avoids false crash reports.
                        let socket = state.data_dir.join("daemon.sock");
                        if daemon_socket_alive(&socket) {
                            consecutive_crashes = 0;
                            // Reap a dead child slot if any (harmless when alive).
                            let _ = state.observe_running();
                            continue;
                        }
                        // Grace period after a restart: give a freshly spawned
                        // daemon ~8s to bind the socket before counting a crash.
                        // Without this, the first 2s tick after restart sees the
                        // socket still down and re-counts, producing the "always
                        // crash #1" loop.
                        if let Some(t) = last_restart_at {
                            if t.elapsed() < std::time::Duration::from_secs(8) {
                                continue;
                            }
                        }
                        // Daemon truly down. Notify + restart with backoff.
                        consecutive_crashes = consecutive_crashes.saturating_add(1);
                        tracing::error!(
                            crashes = consecutive_crashes,
                            "observe daemon unreachable; notifying UI + attempting restart"
                        );
                        let _ = handle.emit(
                            "daemon://exited",
                            consecutive_crashes,
                        );
                        // Cap retries to avoid a tight crash loop (e.g. a bad
                        // config or a crash-on-start). After 5 crashes within
                        // the supervisor's lifetime, stop trying and let the
                        // user investigate — they can still manually restart.
                        if consecutive_crashes > 5 {
                            continue;
                        }
                        // Backoff: 2s, 4s, 8s, 16s, 32s between attempts.
                        let backoff_secs = 2u64 << consecutive_crashes.min(5);
                        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                        match commands::observe_start_inner(&state) {
                            Ok(st) => {
                                last_restart_at = Some(std::time::Instant::now());
                                tracing::info!(
                                    ?st.pid,
                                    attempt = consecutive_crashes,
                                    "observe daemon restarted after crash"
                                );
                            }
                            Err(e) => tracing::warn!(error = %e, "observe daemon restart failed"),
                        }
                    }
                });
            }
            // Cua supervisor: the capture helper (Lumen Cua) has no parent
            // watching it — when it crashed (production: SIGSEGV inside an AX
            // walk), nothing relaunched it and screen capture stayed dead for
            // hours until the app restarted. While Observe is active with
            // screen capture enabled, probe Cua every 30s; CuaController's
            // status path self-heals (relaunch via `open -n -g`) under the
            // same lifecycle lock the UI's status polling uses, so this never
            // races the UI. Consecutive failures are capped, mirroring the
            // daemon supervisor's crash cap.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(30));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut consecutive_failures = 0u32;
                    loop {
                        interval.tick().await;
                        let Some(state) = handle.try_state::<AppState>() else {
                            continue;
                        };
                        // Only heal under the same condition that makes
                        // observe_start_inner ensure Cua: Observe active and
                        // screen capture enabled (macOS-only feature).
                        #[cfg(target_os = "macos")]
                        let should_heal = state.observe_running()
                            && state
                                .load_config()
                                .map(|c| c.sources.screen)
                                .unwrap_or(false);
                        #[cfg(not(target_os = "macos"))]
                        let should_heal = false;
                        if !should_heal {
                            consecutive_failures = 0;
                            continue;
                        }
                        if consecutive_failures > 5 {
                            continue;
                        }
                        // status() probes Cua and, on failure, relaunches it —
                        // the same code path as UI status polling. Blocking
                        // (lifecycle lock + launch wait), so spawn_blocking.
                        let cua = state.cua.clone();
                        let ok = tauri::async_runtime::spawn_blocking(move || cua.status().is_ok())
                            .await
                            .unwrap_or(false);
                        if ok {
                            consecutive_failures = 0;
                        } else {
                            consecutive_failures += 1;
                            tracing::warn!(
                                failures = consecutive_failures,
                                "cua supervisor: Lumen Cua unreachable after relaunch attempt"
                            );
                        }
                    }
                });
            }
            // Category enrichment when the shell holds the store (covers
            // cases where observe is off but the user is browsing history).
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    let mut every =
                        tokio::time::interval(std::time::Duration::from_secs(30 * 60));
                    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        if let Some(state) = handle.try_state::<AppState>() {
                            let data_dir = state.data_dir.clone();
                            match tauri::async_runtime::spawn_blocking(move || {
                                let store = lumen_store::SqliteStore::open(&data_dir)?;
                                store.process_category_enrichment(25, true)
                            })
                            .await
                            {
                                Ok(Ok(r)) if r.attempted > 0 || r.brew_index_refreshed => {
                                    tracing::info!(
                                        attempted = r.attempted,
                                        resolved = r.resolved,
                                        "category enrichment pass"
                                    );
                                }
                                Ok(Err(e)) => {
                                    tracing::debug!(error = %e, "category enrichment skipped")
                                }
                                Err(e) => {
                                    tracing::debug!(error = %e, "category enrichment join failed")
                                }
                                _ => {}
                            }
                        }
                        every.tick().await;
                    }
                });
            }
            // --- Capture health monitor ---
            // Every 30s, checks whether the capture pipeline is producing
            // events. If stagnant for >60s with screen enabled, attempts
            // self-healing (restart daemon/cua). If healing fails, emits
            // "health://alert" to the frontend (banner + dock badge).
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Delay first check so startup has time to produce events.
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(30));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                    let mut last_event_count: i64 = -1;
                    let mut stagnant_ticks: u32 = 0;
                    let mut alert_active = false;

                    loop {
                        interval.tick().await;
                        let Some(state) = handle.try_state::<AppState>() else {
                            continue;
                        };

                        // Check if daemon socket is alive.
                        let socket = state.data_dir.join("daemon.sock");
                        let daemon_ok = daemon_socket_alive(&socket);

                        // If daemon is down, the existing supervisor handles
                        // restart. We just track the state.
                        if !daemon_ok {
                            stagnant_ticks = stagnant_ticks.saturating_add(1);
                        } else {
                            // Daemon alive — query event count to detect
                            // stagnation (events flowing but not increasing).
                            let count = tauri::async_runtime::spawn_blocking({
                                let dir = state.data_dir.clone();
                                move || -> i64 {
                                    let store = match lumen_store::SqliteStore::open(&dir) {
                                        Ok(s) => s,
                                        Err(_) => return -1,
                                    };
                                    store.total_event_count().unwrap_or(-1)
                                }
                            })
                            .await
                            .unwrap_or(-1);

                            if count > last_event_count && last_event_count >= 0 {
                                // Events are flowing — reset.
                                stagnant_ticks = 0;
                            } else if count == last_event_count {
                                stagnant_ticks = stagnant_ticks.saturating_add(1);
                            }
                            last_event_count = count;
                        }

                        // After 2 stagnant ticks (~60s), try self-healing.
                        let needs_heal = stagnant_ticks >= 2;

                        if needs_heal && !alert_active {
                            // Attempt self-heal.
                            let mut healed = false;
                            if !daemon_ok {
                                match commands::observe_start_inner(&state) {
                                    Ok(_) => {
                                        tracing::info!("health monitor: restarted daemon");
                                        healed = true;
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "health monitor: daemon restart failed");
                                    }
                                }
                            }
                            // Check cua if screen is enabled.
                            let cfg = state.load_config().ok();
                            if cfg.as_ref().is_some_and(|c| c.sources.screen) {
                                match state.cua.ensure_running() {
                                    Ok(_) => {
                                        tracing::info!("health monitor: cua ensured running");
                                        healed = true;
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "health monitor: cua restart failed");
                                    }
                                }
                            }

                            if !healed {
                                // Self-heal failed — alert the user.
                                let reason = if !daemon_ok {
                                    "本地服务无响应"
                                } else {
                                    "采集停滞（长时间无新数据）"
                                };
                                tracing::warn!(reason, "health monitor: alerting user");
                                let _ = handle.emit(
                                    "health://alert",
                                    serde_json::json!({ "reason": reason }),
                                );
                                alert_active = true;
                            } else {
                                // Healed — reset and give it time.
                                stagnant_ticks = 0;
                            }
                        }

                        // If we were in alert but now events are flowing,
                        // emit recovery.
                        if alert_active && stagnant_ticks == 0 {
                            tracing::info!("health monitor: recovered");
                            let _ = handle.emit("health://recovered", ());
                            alert_active = false;
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_health,
            commands::get_permissions,
            commands::get_platform_info,
            commands::search_text,
            commands::list_events,
            commands::list_timeline,
            commands::activity_segments,
            commands::activity_stats,
            commands::activity_range,
            commands::activity_add_manual_segment,
            commands::activity_delete_segment,
            commands::activity_list_category_rules,
            commands::activity_save_category_rules,
            commands::get_event_image_data_url,
            commands::get_event_media_data_url,
            commands::reindex_search,
            commands::get_config_summary,
            commands::get_browser_pairing,
            commands::enable_browser_pairing,
            commands::update_sources_config,
            commands::generate_day_summary,
            commands::export_session_transcript,
            commands::set_privacy_paused,
            commands::observe_status,
            commands::observe_start,
            commands::observe_stop,
            commands::open_data_dir,
            commands::get_onboarding,
            commands::set_onboarding_step,
            commands::complete_onboarding,
            commands::skip_onboarding,
            commands::reopen_onboarding,
            commands::set_launch_observe,
            commands::request_screen_permission,
            commands::refresh_screen_permission,
            commands::request_microphone_permission,
            commands::open_privacy_settings,
            asr_models::check_asr_model_status,
            asr_models::list_local_asr_models,
            asr_models::use_existing_asr_model,
            asr_models::set_asr_engine_preference,
            asr_models::set_asr_models_root,
            asr_models::start_asr_model_download,
            asr_models::cancel_asr_model_download,
            commands::assistant_get_config,
            commands::assistant_update_config,
            commands::assistant_run,
            commands::assistant_cancel,
            commands::request_accessibility_permission,
            commands::selection_popup_hide,
            commands::selection_popup_current,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Lumen Navi");

    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            if let Some(state) = handle.try_state::<AppState>() {
                state.stop_owned_observe();
            }
        }
    });
}

/// Quick liveness probe for the daemon's Unix socket: true if a client can
/// connect (the daemon is bound and accepting). Used by the supervisor as the
/// primary "is the daemon serving" signal — more reliable than checking the
/// child slot, since it recognizes daemons the supervisor didn't spawn (e.g.
/// an orphan from a prior app run still holding the socket).
pub(crate) fn daemon_socket_alive(socket_path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        use std::time::Duration;
        UnixStream::connect(socket_path)
            .and_then(|s| {
                s.set_read_timeout(Some(Duration::from_millis(200)))?;
                s.set_write_timeout(Some(Duration::from_millis(200)))?;
                Ok(())
            })
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        false
    }
}
