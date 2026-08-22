//! Quick composer — the ⌥Space mini panel (Act-plane D1).
//!
//! A user-invoked sibling of the selection popup: no selection required, the
//! user types a free prompt with the same context (<attached-*>) / agent /
//! inject machinery. Shown via a global shortcut; the frontmost app at
//! invocation time is captured as the inject target.

use serde_json::json;
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::selection_popup::{self, PendingTarget};

pub const COMPOSER_LABEL: &str = "quick-composer";
const COMPOSER_W: f64 = 520.0;
const COMPOSER_H: f64 = 440.0;

/// Toggle the composer window. On show, captures the currently focused app
/// (before our window takes focus) as the injection target.
pub fn toggle(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(COMPOSER_LABEL) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            return;
        }
    }
    show(app);
}

fn show(app: &AppHandle) {
    // Capture the frontmost app BEFORE creating/focusing our window.
    let target = lumen_platform_host::selection::focused_element_pid().and_then(|pid| {
        if pid == std::process::id() as i32 {
            return None;
        }
        lumen_platform_host::selection::app_identity_for_pid(pid)
            .map(|(app_name, bundle_id)| PendingTarget { pid, app_name, bundle_id })
    });

    let win = match app.get_webview_window(COMPOSER_LABEL) {
        Some(w) => w,
        None => match WebviewWindowBuilder::new(
            app,
            COMPOSER_LABEL,
            WebviewUrl::App("composer.html".into()),
        )
        .title("Lumen Composer")
        .inner_size(COMPOSER_W, COMPOSER_H)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()
        {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "create composer window failed");
                return;
            }
        },
    };

    // Center on the primary monitor (a global launcher reads as "spotlight").
    let (x, y) = match app.primary_monitor() {
        Ok(Some(m)) => {
            let scale = m.scale_factor();
            let size = m.size();
            let pos = m.position();
            (
                pos.x as f64 / scale + (size.width as f64 / scale - COMPOSER_W) / 2.0,
                pos.y as f64 / scale + (size.height as f64 / scale - COMPOSER_H) / 2.5,
            )
        }
        _ => (120.0, 120.0),
    };
    let _ = win.set_position(LogicalPosition::new(x, y));

    // Share the pending-target slot with the popup so assistant_inject works
    // unchanged; publish the target to the composer webview.
    selection_popup::set_pending_target(target.clone());
    if let Err(e) = win.show() {
        tracing::warn!(error = %e, "composer show failed");
        return;
    }
    let _ = win.set_focus();
    let target_name = target.as_ref().map(|t| t.app_name.clone());
    let _ = app.emit_to(COMPOSER_LABEL, "composer-shown", json!({ "target": target_name }));
}

pub fn hide(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(COMPOSER_LABEL) {
        let _ = win.hide();
    }
    selection_popup::clear_pending_target();
}
