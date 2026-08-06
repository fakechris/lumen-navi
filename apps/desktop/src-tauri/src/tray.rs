//! System tray for the local background service.

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

    let mut builder = TrayIconBuilder::new()
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
    Ok(())
}

fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}
