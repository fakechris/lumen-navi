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
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // First check is delayed so the auto-start above has time to spawn.
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(2));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut consecutive_crashes = 0u32;
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
                        if state.observe_running() {
                            consecutive_crashes = 0;
                            continue;
                        }
                        // Daemon died unexpectedly. Tell the UI, then try to restart.
                        consecutive_crashes = consecutive_crashes.saturating_add(1);
                        tracing::error!(
                            crashes = consecutive_crashes,
                            "observe daemon exited unexpectedly; notifying UI + attempting restart"
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
                            Ok(st) => tracing::info!(
                                ?st.pid,
                                attempt = consecutive_crashes,
                                "observe daemon restarted after crash"
                            ),
                            Err(e) => tracing::warn!(error = %e, "observe daemon restart failed"),
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_health,
            commands::get_permissions,
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
