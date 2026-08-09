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
use tauri::Manager;
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
