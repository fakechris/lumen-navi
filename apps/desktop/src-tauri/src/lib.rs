//! Lumen Navi desktop shell — store browser + control + observe sidecar.

mod commands;
mod state;

use state::AppState;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "lumen_navi_desktop=info,warn".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let state = AppState::open().expect("open app state");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_health,
            commands::get_permissions,
            commands::search_text,
            commands::list_events,
            commands::reindex_search,
            commands::get_config_summary,
            commands::set_privacy_paused,
            commands::observe_status,
            commands::observe_start,
            commands::observe_stop,
            commands::open_data_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lumen Navi");
}
