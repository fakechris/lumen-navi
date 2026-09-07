//! Map Lumen InputReplay steps onto embedded cua-driver tools.
//!
//! Delivery is always `background`. Noop never activates.

use anyhow::Result;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

use crate::act_driver;
use crate::protocol::{ActionEffect, ActionResult, ActionRoute, DeliveryMode, InputStep};
use crate::stoplines;
use crate::CuaPaths;

pub fn replay_via_driver(paths: &CuaPaths, steps: &[InputStep]) -> Result<Vec<ActionResult>> {
    if steps.is_empty() {
        anyhow::bail!("empty replay");
    }
    if steps.len() > 16 {
        anyhow::bail!("replay too long");
    }
    act_driver::ensure(paths)?;
    let session = steps
        .iter()
        .find_map(|s| s.session.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("lumen-{}", Uuid::new_v4().simple()));
    let _ = act_driver::call(paths, "start_session", json!({ "session": session }));
    let _ = act_driver::call(
        paths,
        "set_agent_cursor_enabled",
        json!({ "session": session, "enabled": true }),
    );

    let mut effects = Vec::with_capacity(steps.len());
    for step in steps {
        let app = step
            .bundle_id
            .as_deref()
            .or(step.window.as_deref())
            .unwrap_or("");
        if let Some(reason) = stoplines::refuse_replay_step(
            &step.action,
            app,
            step.bundle_id.as_deref(),
            step.keys.as_deref(),
            &[
                step.target.as_deref().unwrap_or(""),
                step.window.as_deref().unwrap_or(""),
            ],
        ) {
            effects.push(ActionResult::refused(reason, None));
            continue;
        }
        if step.dry {
            effects.push(ActionResult {
                effect: ActionEffect::Unverifiable,
                route: ActionRoute::Skipped,
                delivery: DeliveryMode::Background,
                reason: Some("dry_run".into()),
                gates: None,
            });
            continue;
        }
        effects.push(run_step(paths, step, &session));
        let wait = step.wait_ms.unwrap_or(180).min(2_000);
        thread::sleep(Duration::from_millis(wait));
    }
    let _ = act_driver::call(paths, "end_session", json!({ "session": session }));
    Ok(effects)
}

fn run_step(paths: &CuaPaths, step: &InputStep, session: &str) -> ActionResult {
    let step = resolve_target(paths, step, session);
    match map_step(&step, session) {
        Ok(calls) => {
            let mut last = ActionResult {
                effect: ActionEffect::Unverifiable,
                route: ActionRoute::Skipped,
                delivery: DeliveryMode::Background,
                reason: Some("no_driver_calls".into()),
                gates: None,
            };
            for (tool, mut args) in calls {
                if tool == "click" {
                    fill_click_pixels(paths, &mut args);
                }
                match act_driver::call(paths, &tool, args) {
                    Ok(value) => last = result_from_driver(&tool, &value),
                    Err(err) => {
                        return ActionResult::refused(err.to_string(), None);
                    }
                }
            }
            last
        }
        Err(reason) => ActionResult::refused(reason, None),
    }
}

fn resolve_target(paths: &CuaPaths, step: &InputStep, session: &str) -> InputStep {
    let mut step = step.clone();
    if step.pid.is_none() {
        if let Ok(calls) = map_step(
            &InputStep {
                action: "launch".into(),
                session: Some(session.into()),
                ..step.clone()
            },
            session,
        ) {
            if let Some((tool, args)) = calls.into_iter().next() {
                if let Ok(value) = act_driver::call(paths, &tool, args) {
                    step.pid = value.get("pid").and_then(|v| v.as_i64()).map(|p| p as i32);
                    if step.window_id.is_none() {
                        step.window_id = pick_window_id(&value, step.window.as_deref());
                    }
                }
            }
        }
    }
    if step.window_id.is_none() {
        if let Some(pid) = step.pid {
            if let Ok(value) = act_driver::call(paths, "list_windows", json!({ "pid": pid })) {
                step.window_id = pick_window_id(&value, step.window.as_deref());
            }
        }
    }
    step
}

fn pick_window_id(value: &Value, title: Option<&str>) -> Option<u64> {
    let windows = value
        .get("windows")
        .or_else(|| value.as_array().map(|_| value))
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())?;
    let want = title.unwrap_or("");
    let named = windows.iter().find(|w| {
        !want.is_empty()
            && w.get("title")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t.contains(want) || want.contains(t))
    });
    named
        .or(windows.first())
        .and_then(|w| w.get("window_id").and_then(|id| id.as_u64()))
}

/// Pure mapping used by tests. Live replay resolves pid/window via the driver.
pub fn map_step(step: &InputStep, session: &str) -> Result<Vec<(String, Value)>, String> {
    match step.action.as_str() {
        "launch" | "focus" | "activate" => {
            let mut args = json!({ "session": session });
            if let Some(bundle) = step.bundle_id.as_deref().filter(|s| !s.is_empty()) {
                args["bundle_id"] = json!(bundle);
            } else if let Some(window) = step.window.as_deref().filter(|s| !s.is_empty()) {
                args["name"] = json!(window);
            } else {
                return Err("launch needs bundle_id".into());
            }
            if let Some(urls) = &step.urls {
                if !urls.is_empty() {
                    args["urls"] = json!(urls);
                }
            }
            Ok(vec![("launch_app".into(), args)])
        }
        "click" => Ok(vec![("click".into(), click_args(step, session)?)]),
        "shortcut" | "submit" | "key" => {
            let keys = step
                .keys
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(if step.action == "submit" {
                    Some("return")
                } else {
                    None
                })
                .ok_or_else(|| "key step needs keys".to_string())?;
            let pid = require_pid(step)?;
            let mut args = json!({
                "pid": pid,
                "delivery_mode": "background",
                "session": session,
            });
            if keys.contains('+') || keys.contains("command") || keys.contains("cmd") {
                args["keys"] = json!(split_hotkey(keys));
                Ok(vec![("hotkey".into(), args)])
            } else {
                args["key"] = json!(keys);
                Ok(vec![("press_key".into(), args)])
            }
        }
        "type" => {
            let text = step
                .text
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "type step needs text".to_string())?;
            let pid = require_pid(step)?;
            let mut args = json!({
                "pid": pid,
                "text": text,
                "delivery_mode": "background",
                "session": session,
            });
            if let Some(token) = step.element_token.as_deref().filter(|s| !s.is_empty()) {
                args["element_token"] = json!(token);
            }
            Ok(vec![("type_text".into(), args)])
        }
        "scroll" => {
            let pid = require_pid(step)?;
            let mut args = json!({
                "pid": pid,
                "direction": step.target.as_deref().unwrap_or("down"),
                "amount": 3,
                "delivery_mode": "background",
                "session": session,
            });
            if let Some(token) = step.element_token.as_deref().filter(|s| !s.is_empty()) {
                args["element_token"] = json!(token);
            }
            Ok(vec![("scroll".into(), args)])
        }
        "page" => {
            let pid = require_pid(step)?;
            let mut args = json!({
                "pid": pid,
                "action": step.target.as_deref().unwrap_or("get_text"),
                "session": session,
            });
            if let Some(id) = step.window_id {
                args["window_id"] = json!(id);
            }
            if let Some(sel) = step.css_selector.as_deref().filter(|s| !s.is_empty()) {
                args["css_selector"] = json!(sel);
            }
            if let Some(js) = step.javascript.as_deref().or(step.text.as_deref()) {
                if !js.is_empty() && args["action"] == "execute_javascript" {
                    args["javascript"] = json!(js);
                }
            }
            Ok(vec![("page".into(), args)])
        }
        "cursor" => {
            let enabled = step
                .text
                .as_deref()
                .map(|s| s != "off" && s != "false" && s != "0")
                .unwrap_or(true);
            Ok(vec![(
                "set_agent_cursor_enabled".into(),
                json!({ "session": session, "enabled": enabled }),
            )])
        }
        other => Err(format!("unsupported replay action {other}")),
    }
}

fn click_args(step: &InputStep, session: &str) -> Result<Value, String> {
    let pid = require_pid(step)?;
    let mut args = json!({
        "pid": pid,
        "delivery_mode": "background",
        "session": session,
    });
    if let Some(token) = step.element_token.as_deref().filter(|s| !s.is_empty()) {
        args["element_token"] = json!(token);
        return Ok(args);
    }
    let window_id = step.window_id.ok_or_else(|| {
        "click needs window_id or element_token (driver path does not steal focus)".to_string()
    })?;
    args["window_id"] = json!(window_id);
    if let (Some(x), Some(y)) = (step.x, step.y) {
        args["x"] = json!(x);
        args["y"] = json!(y);
        return Ok(args);
    }
    if let (Some(nx), Some(ny)) = (step.nx, step.ny) {
        // Caller may pass 0–1 relative coords; driver wants screenshot pixels.
        // Width/height are filled by `fill_click_pixels` just before the call.
        args["nx"] = json!(nx.clamp(0.02, 0.98));
        args["ny"] = json!(ny.clamp(0.02, 0.98));
        return Ok(args);
    }
    Err("click needs element_token or window-local x/y".into())
}

fn require_pid(step: &InputStep) -> Result<i32, String> {
    step.pid
        .filter(|p| *p > 0)
        .ok_or_else(|| "step needs pid (launch_app first, or pass pid)".to_string())
}

fn split_hotkey(spec: &str) -> Vec<String> {
    spec.split('+')
        .map(|s| {
            let t = s.trim().to_ascii_lowercase();
            match t.as_str() {
                "command" => "cmd".into(),
                "option" => "alt".into(),
                "control" => "ctrl".into(),
                other => other.to_string(),
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn result_from_driver(tool: &str, value: &Value) -> ActionResult {
    let effect = match value.get("effect").and_then(|v| v.as_str()).unwrap_or("") {
        "confirmed" => ActionEffect::Confirmed,
        "partial" => ActionEffect::Partial,
        "suspected_noop" => ActionEffect::SuspectedNoop,
        "refused" => ActionEffect::Refused,
        _ => ActionEffect::Unverifiable,
    };
    let route = match value.get("path").and_then(|v| v.as_str()).unwrap_or("") {
        "ax" => ActionRoute::Accessibility,
        "cgevent" | "pixel" => ActionRoute::SyntheticEvents,
        _ if tool == "launch_app" => ActionRoute::Skipped,
        _ => ActionRoute::SyntheticEvents,
    };
    let reason = value
        .get("error")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            value
                .pointer("/escalation/reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    ActionResult {
        effect,
        route,
        delivery: DeliveryMode::Background,
        reason,
        gates: None,
    }
}

/// If the mapped click still has nx/ny, convert using get_window_state PNG size.
pub fn fill_click_pixels(paths: &CuaPaths, args: &mut Value) {
    let Some(nx) = args.get("nx").and_then(|v| v.as_f64()) else {
        return;
    };
    let Some(ny) = args.get("ny").and_then(|v| v.as_f64()) else {
        return;
    };
    let Some(pid) = args.get("pid").and_then(|v| v.as_i64()) else {
        return;
    };
    let Some(window_id) = args.get("window_id").and_then(|v| v.as_u64()) else {
        return;
    };
    let Ok(state) = act_driver::call(
        paths,
        "get_window_state",
        json!({
            "pid": pid,
            "window_id": window_id,
            "include_screenshot": true,
        }),
    ) else {
        return;
    };
    let width = state
        .get("screenshot_width")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let height = state
        .get("screenshot_height")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if width > 1.0 && height > 1.0 {
        args["x"] = json!((nx * width).round());
        args["y"] = json!((ny * height).round());
        args.as_object_mut().map(|o| {
            o.remove("nx");
            o.remove("ny");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_step() -> InputStep {
        InputStep {
            action: "click".into(),
            bundle_id: Some("com.apple.Safari".into()),
            window: Some("Inbox".into()),
            target: None,
            keys: None,
            nx: Some(0.4),
            ny: Some(0.6),
            wait_ms: None,
            text: None,
            dry: false,
            allow_foreground: true,
            window_id: Some(99),
            element_token: None,
            x: None,
            y: None,
            session: None,
            pid: Some(4242),
            urls: None,
            css_selector: None,
            javascript: None,
        }
    }

    #[test]
    fn click_never_requests_foreground() {
        let (tool, args) = &map_step(&click_step(), "s1").unwrap()[0];
        assert_eq!(tool, "click");
        assert_eq!(args["delivery_mode"], "background");
        assert_eq!(args["pid"], 4242);
        assert_eq!(args["window_id"], 99);
    }

    #[test]
    fn focus_maps_to_launch_app_not_activate() {
        let mut step = click_step();
        step.action = "focus".into();
        let (tool, args) = &map_step(&step, "s1").unwrap()[0];
        assert_eq!(tool, "launch_app");
        assert_eq!(args["bundle_id"], "com.apple.Safari");
        assert!(args.get("activate").is_none());
    }

    #[test]
    fn page_and_cursor_map() {
        let mut step = click_step();
        step.action = "page".into();
        step.target = Some("get_text".into());
        let (tool, _) = &map_step(&step, "s1").unwrap()[0];
        assert_eq!(tool, "page");
        step.action = "cursor".into();
        step.text = Some("on".into());
        let (tool, args) = &map_step(&step, "s1").unwrap()[0];
        assert_eq!(tool, "set_agent_cursor_enabled");
        assert_eq!(args["enabled"], true);
    }

    #[test]
    fn driver_effect_maps_honestly() {
        let r = result_from_driver(
            "click",
            &json!({"effect":"suspected_noop","path":"cgevent"}),
        );
        assert_eq!(r.effect, ActionEffect::SuspectedNoop);
        assert_eq!(r.delivery, DeliveryMode::Background);
        assert_eq!(r.route, ActionRoute::SyntheticEvents);
    }
}
