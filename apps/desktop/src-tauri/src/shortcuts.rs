//! Configurable global shortcuts for the desktop shell (quick composer).
//!
//! The accelerator strings follow the tauri global-shortcut / global_hotkey
//! syntax (`"Alt+Space"`, `"CommandOrControl+Shift+P"`, …). Registration is
//! the only reliable validation — a taken combo only fails at register time —
//! so `apply` registers the new shortcut *before* unregistering the old one
//! and reports the OS error verbatim to the Settings UI.

use std::sync::mpsc;

use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::state::AppState;

/// Modifier tokens accepted in an accelerator (lowercase). Used to reject
/// modifier-less shortcuts, which would swallow a plain typing key globally.
const MODIFIER_TOKENS: [&str; 12] = [
    "alt",
    "option",
    "ctrl",
    "control",
    "shift",
    "super",
    "cmd",
    "command",
    "meta",
    "win",
    "cmdorctrl",
    "commandorcontrol",
];

pub fn has_modifier(accelerator: &str) -> bool {
    accelerator
        .split('+')
        .any(|part| MODIFIER_TOKENS.contains(&part.trim().to_ascii_lowercase().as_str()))
}

/// Register `accelerator` + toggle handler on the main thread, blocking until
/// the plugin answers. `run_on_main_thread` returns before the closure runs,
/// so an mpsc channel turns it back into a synchronous call.
fn register_blocking<R: Runtime>(app: &AppHandle<R>, accelerator: &str) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let acc = accelerator.to_string();
    let h_for_api = app.clone();
    let h_for_handler = app.clone();
    let _ = app.run_on_main_thread(move || {
        let res = h_for_api
            .global_shortcut()
            .on_shortcut(acc.as_str(), move |_app, _sc, event| {
                if event.state() == ShortcutState::Pressed {
                    let h2 = h_for_handler.clone();
                    let _ = h_for_handler
                        .run_on_main_thread(move || crate::composer::toggle(&h2));
                }
            })
            .map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    rx.recv()
        .map_err(|_| "shortcut main-thread dispatch failed".to_string())?
}

fn unregister_blocking<R: Runtime>(app: &AppHandle<R>, accelerator: &str) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let acc = accelerator.to_string();
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        let res = h.global_shortcut()
            .unregister(acc.as_str())
            .map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    rx.recv()
        .map_err(|_| "shortcut main-thread dispatch failed".to_string())?
}

fn remember(state: &AppState, active: &str, error: Option<String>) {
    if let Ok(mut s) = state.composer_shortcut.lock() {
        *s = active.to_string();
    }
    if let Ok(mut e) = state.composer_shortcut_error.lock() {
        *e = error;
    }
}

/// Register the configured composer shortcut at app startup. A conflict is
/// not fatal: it is recorded in `AppState` and surfaced in Settings while the
/// app keeps running (tray menu still opens the composer).
pub fn register_at_startup<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    let desired = state
        .load_config()
        .map(|c| c.shortcuts.composer.trim().to_string())
        .unwrap_or_else(|_| "Alt+Space".into());
    if desired.is_empty() {
        // Explicitly disabled.
        return;
    }
    match register_blocking(app, &desired) {
        Ok(()) => remember(state, &desired, None),
        Err(e) => {
            tracing::warn!(error = %e, shortcut = %desired, "register composer shortcut failed");
            remember(state, "", Some(format!("「{desired}」注册失败：{e}")));
        }
    }
}

/// Swap the composer shortcut at runtime. `new_shortcut == ""` disables it.
/// The previous shortcut stays live until the new one registers, so a
/// conflict never leaves the user without any shortcut.
pub fn apply<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    new_shortcut: &str,
) -> Result<(), String> {
    let new_shortcut = new_shortcut.trim();
    if !new_shortcut.is_empty() && !has_modifier(new_shortcut) {
        return Err("快捷键必须包含至少一个修饰键（⌘/⌥/⇧/Ctrl），否则会拦截正常打字。".into());
    }
    let old = state
        .composer_shortcut
        .lock()
        .map_err(|_| "shortcut state lock".to_string())?
        .clone();
    if new_shortcut == old {
        return Ok(());
    }

    if !new_shortcut.is_empty() {
        if let Err(e) = register_blocking(app, new_shortcut) {
            // The old registration is untouched — nothing to roll back.
            let msg = format!("「{new_shortcut}」注册失败：{e}。可能已被其他应用占用，请换一个组合（如 ⌥Space、⌘⇧P）。");
            remember(state, &old, Some(msg.clone()));
            return Err(msg);
        }
    }
    if !old.is_empty() {
        if let Err(e) = unregister_blocking(app, &old) {
            // Both live now; log — the stale one disappears on next restart.
            tracing::warn!(error = %e, shortcut = %old, "unregister old composer shortcut failed");
        }
    }
    remember(state, new_shortcut, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_detection() {
        assert!(has_modifier("Alt+Space"));
        assert!(has_modifier("CommandOrControl+Shift+P"));
        assert!(has_modifier("ctrl+q"));
        assert!(has_modifier("Option+Space"));
        assert!(!has_modifier("Space"));
        assert!(!has_modifier("A"));
        assert!(!has_modifier(""));
    }
}
