//! Selection popup (划词弹窗) — floating panel window + mouse-selection glue.
//!
//! Flow: global left-mouse-up → short delay → AX selected text of the
//! frontmost app → show/reposition the borderless popup window near the
//! selection. Clicking elsewhere (mouse-up with no selection in another app)
//! hides it. Our own pid is ignored so popup interactions never retrigger it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lumen_platform_macos::{
    accessibility_trusted, focused_element_pid, focused_selection, mouse_location,
    normalize_selection, start_mouse_up_monitor,
};
use serde_json::json;
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

pub const POPUP_LABEL: &str = "selection-popup";
const POPUP_W: f64 = 440.0;
const POPUP_H: f64 = 360.0;
/// Same selection re-shown within this window is ignored (double-click etc.).
const RESHOW_DEBOUNCE: Duration = Duration::from_secs(3);
/// Let the selection settle after mouse-up before querying AX.
const QUERY_DELAY: Duration = Duration::from_millis(200);
/// Chromium/Electron lazily build their AX tree on first contact — retry the
/// query a few times before giving up (and before treating it as a dismiss).
const AX_MAX_ATTEMPTS: u32 = 3;
const AX_RETRY_DELAY: Duration = Duration::from_millis(150);

static MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
static POPUP_ENABLED: AtomicBool = AtomicBool::new(false);
/// Monotonic id of the latest mouse-up query; stale tasks bail out.
static QUERY_EPOCH: AtomicU64 = AtomicU64::new(0);
static LAST_SHOWN: Mutex<Option<(String, Instant)>> = Mutex::new(None);
/// Text of the latest show, for the popup webview to pull on first load.
static PENDING_TEXT: Mutex<Option<String>> = Mutex::new(None);

pub fn popup_enabled() -> bool {
    POPUP_ENABLED.load(Ordering::SeqCst)
}

/// Enable/disable the popup feature. When enabling, ensures the mouse
/// monitor is running (requires Accessibility trust; otherwise it stays off
/// and enabling can be retried after the user grants it).
pub fn set_popup_enabled(app: &AppHandle, enabled: bool) {
    POPUP_ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        ensure_monitor(app);
    } else {
        hide_popup(app);
    }
}

/// Latest text handed to the popup (webview pulls this on load to avoid
/// racing the `selection-changed` event), then clears it.
pub fn take_pending_text() -> Option<String> {
    PENDING_TEXT.lock().ok()?.take()
}

/// Start the monitor at app launch when the feature is enabled and trusted.
pub fn init_from_config(app: &AppHandle, enabled: bool) {
    POPUP_ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        ensure_monitor(app);
    }
}

/// Idempotently start the mouse monitor (used by config changes and by the
/// settings poll for self-heal after the user grants Accessibility).
pub fn ensure_monitor(app: &AppHandle) {
    if MONITOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !accessibility_trusted(false) {
        // Reset so a later enable (after the user grants permission) retries.
        MONITOR_STARTED.store(false, Ordering::SeqCst);
        tracing::info!("selection popup enabled but Accessibility not granted yet");
        return;
    }
    let handle = app.clone();
    if let Err(e) = start_mouse_up_monitor(move |up| on_mouse_up(&handle, up)) {
        MONITOR_STARTED.store(false, Ordering::SeqCst);
        tracing::warn!(error = %e, "start selection monitor failed");
    }
}

fn on_mouse_up(app: &AppHandle, up: lumen_platform_macos::MouseUp) {
    if !popup_enabled() {
        return;
    }
    // Each mouse-up supersedes the previous pending query, so a stale task
    // (e.g. still retrying for Chromium AX activation) can't hide or move the
    // panel behind a newer click.
    let epoch = QUERY_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if !up.maybe_selection {
            // Plain click — cannot create a selection, so dismiss instantly.
            // One cheap pid probe keeps clicks on our own windows from hiding
            // the panel (button clicks are plain clicks too).
            let pid = tauri::async_runtime::spawn_blocking(focused_element_pid)
                .await
                .ok()
                .flatten();
            if pid == Some(std::process::id() as i32) {
                return;
            }
            if QUERY_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            hide_popup(&handle);
            return;
        }
        tokio::time::sleep(QUERY_DELAY).await;
        // Retry a few times: Chromium/Electron apps build their accessibility
        // tree lazily on first AX contact, so the very first query after app
        // launch often comes back empty before the tree is ready.
        let mut info = None;
        for attempt in 0..AX_MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(AX_RETRY_DELAY).await;
            }
            if QUERY_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            match tauri::async_runtime::spawn_blocking(focused_selection).await {
                Ok(Some(sel)) => {
                    info = Some(sel);
                    break;
                }
                _ => {}
            }
        }
        if QUERY_EPOCH.load(Ordering::SeqCst) != epoch {
            return;
        }
        match info {
            // Interacting with our own popup/main window — keep state as-is.
            Some(sel) if sel.pid == Some(std::process::id() as i32) => {
                tracing::debug!("mouse-up: own window, ignored");
            }
            Some(sel) => {
                let max_chars = current_max_chars(&handle);
                match normalize_selection(&sel.text, max_chars) {
                    Some(text) => {
                        if should_reshow(&text) {
                            tracing::info!(chars = text.len(), pid = ?sel.pid, "selection → show popup");
                            let h2 = handle.clone();
                            let _ = handle.run_on_main_thread(move || {
                                show_popup(&h2, text, sel.bounds);
                            });
                        }
                    }
                    None => {
                        clipboard_fallback_or_hide(&handle, epoch).await;
                    }
                }
            }
            // AX exposed no selection — last resort for AX-less apps
            // (canvas editors, GPU terminals): ⌘C with pasteboard restore.
            None => {
                clipboard_fallback_or_hide(&handle, epoch).await;
            }
        }
    });
}

/// ⌘C fallback for AX-less apps, then dismiss when even that finds nothing.
async fn clipboard_fallback_or_hide(handle: &AppHandle, epoch: u64) {
    if clipboard_fallback_enabled(handle) {
        // Never ⌘C into our own windows.
        let pid = tauri::async_runtime::spawn_blocking(focused_element_pid)
            .await
            .ok()
            .flatten();
        if pid != Some(std::process::id() as i32) {
            let grabbed = tauri::async_runtime::spawn_blocking(
                lumen_platform_macos::clipboard_grab_selection,
            )
            .await
            .ok()
            .flatten();
            if QUERY_EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            if let Some(grabbed) = grabbed {
                let max_chars = current_max_chars(handle);
                if let Some(text) = normalize_selection(&grabbed, max_chars) {
                    if should_reshow(&text) {
                        tracing::info!(chars = text.len(), "selection (⌘C) → show popup");
                        let h2 = handle.clone();
                        let _ = handle.run_on_main_thread(move || {
                            show_popup(&h2, text, None);
                        });
                    }
                    return;
                }
            }
        }
    }
    tracing::debug!("mouse-up: no selection → hide");
    hide_popup(handle);
}

fn should_reshow(text: &str) -> bool {
    if let Ok(mut guard) = LAST_SHOWN.lock() {
        if let Some((last, at)) = guard.as_ref() {
            if last == text && at.elapsed() < RESHOW_DEBOUNCE {
                return false;
            }
        }
        *guard = Some((text.to_string(), Instant::now()));
    }
    true
}

/// Selection char limit, from the current config (fallback to default).
fn current_max_chars(app: &AppHandle) -> usize {
    app.try_state::<crate::state::AppState>()
        .and_then(|s| s.load_config().ok())
        .map(|c| c.assistant.max_selection_chars)
        .unwrap_or(4_000)
}

/// ⌘C pasteboard fallback toggle, from the current config (default on).
fn clipboard_fallback_enabled(app: &AppHandle) -> bool {
    app.try_state::<crate::state::AppState>()
        .and_then(|s| s.load_config().ok())
        .map(|c| c.assistant.clipboard_fallback)
        .unwrap_or(true)
}

/// Get or lazily create the borderless always-on-top popup window.
fn popup_window(app: &AppHandle) -> Result<tauri::WebviewWindow, tauri::Error> {
    if let Some(win) = app.get_webview_window(POPUP_LABEL) {
        return Ok(win);
    }
    WebviewWindowBuilder::new(app, POPUP_LABEL, WebviewUrl::App("popup.html".into()))
        .title("Lumen Selection")
        .inner_size(POPUP_W, POPUP_H)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
}

/// Position near the selection (fallback: mouse), clamp to its monitor,
/// show, and hand the text to the webview.
fn show_popup(app: &AppHandle, text: String, anchor: Option<(f64, f64, f64, f64)>) {
    let win = match popup_window(app) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "create selection popup failed");
            return;
        }
    };
    let (x, y) = position_for_anchor(app, anchor);
    if let Ok(mut guard) = PENDING_TEXT.lock() {
        *guard = Some(text.clone());
    }
    if let Err(e) = win.set_position(LogicalPosition::new(x, y)) {
        tracing::warn!(error = %e, "popup set_position failed");
    }
    if let Err(e) = win.show() {
        tracing::warn!(error = %e, "popup show failed");
    }
    if let Err(e) = app.emit_to(POPUP_LABEL, "selection-changed", json!({ "text": text })) {
        tracing::warn!(error = %e, "popup emit failed");
    }
}

/// Compute on-screen popup position: below the selection when it fits,
/// flipped above it near the screen bottom, always clamped inside the
/// nearest monitor. Anchors are global logical points (AX / CGEvent space).
fn position_for_anchor(app: &AppHandle, anchor: Option<(f64, f64, f64, f64)>) -> (f64, f64) {
    let (ax, ay) = match anchor {
        Some((bx, by, _bw, bh)) => (bx, by + bh + 8.0),
        None => mouse_location()
            .map(|(mx, my)| (mx + 12.0, my + 16.0))
            .unwrap_or((100.0, 100.0)),
    };
    let Some((mx, my, mw, mh)) = nearest_monitor_rect(app, ax, ay) else {
        return (ax, ay);
    };
    let mut y = ay;
    // Not enough room below → flip above the selection.
    if y + POPUP_H + 4.0 > my + mh {
        if let Some((_, by, _, _)) = anchor {
            y = by - POPUP_H - 8.0;
        }
    }
    (
        ax.clamp(mx + 4.0, (mx + mw - POPUP_W - 4.0).max(mx + 4.0)),
        y.clamp(my + 4.0, (my + mh - POPUP_H - 4.0).max(my + 4.0)),
    )
}

/// Logical rect (x, y, w, h) of the monitor nearest to the point —
/// "containing" when inside, closest by rect distance when outside
/// (e.g. an anchor a few px past the screen edge).
fn nearest_monitor_rect(app: &AppHandle, x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let monitors = app.available_monitors().unwrap_or_default();
    let mut best: Option<(f64, f64, f64, f64, f64)> = None;
    for m in &monitors {
        let scale = m.scale_factor();
        let pos = m.position();
        let size = m.size();
        let (mx, my) = (pos.x as f64 / scale, pos.y as f64 / scale);
        let (mw, mh) = (size.width as f64 / scale, size.height as f64 / scale);
        let dx = (mx - x).max(0.0).max(x - (mx + mw));
        let dy = (my - y).max(0.0).max(y - (my + mh));
        let dist = dx * dx + dy * dy;
        if best.map_or(true, |(d, ..)| dist < d) {
            best = Some((dist, mx, my, mw, mh));
        }
    }
    best.map(|(_, mx, my, mw, mh)| (mx, my, mw, mh))
}

pub(crate) fn hide_popup(app: &AppHandle) {
    // Reset debounce so re-selecting the same text right after a dismiss
    // brings the panel back.
    if let Ok(mut guard) = LAST_SHOWN.lock() {
        *guard = None;
    }
    let app = app.clone();
    let inner = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = inner.get_webview_window(POPUP_LABEL) {
            if win.is_visible().unwrap_or(false) {
                let _ = win.hide();
                let _ = inner.emit_to(POPUP_LABEL, "popup-hidden", json!({}));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reshow_debounces_same_text() {
        assert!(should_reshow("unique-text-a"));
        assert!(!should_reshow("unique-text-a"));
        assert!(should_reshow("unique-text-b"));
    }
}
