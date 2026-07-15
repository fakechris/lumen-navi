//! Lumen Navi desktop shell — store browser + control + observe sidecar + tray.

mod commands;
mod shell;
mod state;
mod tray;

use state::AppState;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "lumen_navi_desktop=info,warn".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let state = AppState::open().expect("open app state");
    let launch_observe = state
        .shell
        .lock()
        .map(|s| s.launch_observe && !s.needs_onboarding())
        .unwrap_or(false);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(move |app| {
            if let Err(e) = tray::setup_tray(app.handle()) {
                tracing::warn!(error = %e, "tray setup failed");
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_health,
            commands::get_permissions,
            commands::search_text,
            commands::list_events,
            commands::list_timeline,
            commands::get_event_image_data_url,
            commands::reindex_search,
            commands::get_config_summary,
            commands::update_sources_config,
            commands::generate_day_summary,
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
            commands::open_privacy_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lumen Navi");
}
