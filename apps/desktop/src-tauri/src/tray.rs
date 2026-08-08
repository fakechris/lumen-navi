//! System tray for the local background service + time-tracking display.

use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime,
};

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Lumen Navi", true, None::<&str>)?;
    let pause = MenuItem::with_id(
        app,
        "toggle_pause",
        "Toggle Privacy Pause",
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show, &sep, &pause, &sep, &quit])?;

    let icon = app.default_window_icon().cloned().or_else(|| {
        // Fallback: load png from resources if default missing.
        Image::from_bytes(include_bytes!("../icons/32x32.png")).ok()
    });

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Lumen Navi")
        .on_menu_event(|app, event| {
            match event.id.as_ref() {
                "show" => show_main(app),
                "toggle_pause" => {
                    let _ = app.emit("tray://toggle-pause", ());
                }
                "quit" => {
                    // Best-effort stop child before exit.
                    if let Some(state) = app.try_state::<crate::state::AppState>() {
                        if let Ok(mut guard) = state.observe_child.lock() {
                            if let Some(mut child) = guard.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                        }
                    }
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }

    let _tray = builder.build(app)?;

    // Refresh loop: update menu-bar title + tooltip with today's time + current
    // activity every 30s. Runs on the Tauri async runtime.
    spawn_tray_refresh(app.clone());

    Ok(())
}

/// Background refresh: query today's stats + latest segment, update the tray
/// title (macOS menubar text) and tooltip.
fn spawn_tray_refresh<R: Runtime>(app: AppHandle<R>) {
    let app = Arc::new(app);
    let app_clone = Arc::clone(&app);
    tauri::async_runtime::spawn(async move {
        loop {
            refresh_tray_display(&app_clone);
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

fn refresh_tray_display<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<crate::state::AppState>() else {
        return;
    };
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();

    let stats = state.store.activity_day_stats(&today).ok();
    let latest = state
        .store
        .list_activity_segments(&today)
        .ok()
        .and_then(|mut segs| segs.pop());

    if let Some(tray) = app.tray_by_id("main") {
        // macOS menu-bar text: compact duration (e.g. "6h42m").
        #[cfg(target_os = "macos")]
        {
            let title = match &stats {
                Some(s) if s.total_active_ms > 0 => fmt_tray_title(s.total_active_ms),
                _ => String::new(),
            };
            let _ = tray.set_title(Some(&title));
        }
        let _ = tray.set_tooltip(Some(&build_tooltip(stats.as_ref(), latest.as_ref())));
    }
}

fn fmt_tray_title(ms: i64) -> String {
    let total_min = (ms / 60_000).max(0);
    let h = total_min / 60;
    let m = total_min % 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{m}m")
    }
}

fn build_tooltip(
    stats: Option<&lumen_api::DayStatsDto>,
    latest: Option<&lumen_api::ActivitySegmentDto>,
) -> String {
    let mut parts = vec!["Lumen Navi".to_string()];
    if let Some(s) = stats {
        let h = s.total_active_ms / 3_600_000;
        let m = (s.total_active_ms % 3_600_000) / 60_000;
        parts.push(format!("今日活跃 {h}h{m}m"));
        if let Some(p) = s.pulse_score {
            parts.push(format!("生产力 {:.0}", p));
        }
    }
    if let Some(seg) = latest {
        if !seg.is_idle {
            let name = seg.app_name.as_deref().unwrap_or("未知");
            parts.push(format!("当前: {name}"));
        }
    }
    parts.join(" · ")
}

fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}
