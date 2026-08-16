//! Fold HID interaction events into a CUA-shaped action trace.
//!
//! A suggested skill is a replay draft: focus a window, then click / shortcut /
//! submit. Coordinates are kept as fallback only — layout changes. Typed text
//! is never stored here (`record_text` is off). Capture does not wait.

use serde::Serialize;
use serde_json::Value;

const MAX_FOLDED: usize = 16;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SlotActionTrace {
    pub events: usize,
    pub clicks: u32,
    pub shortcuts: u32,
    pub submits: u32,
    pub folded: Vec<FoldedAction>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FoldedAction {
    pub action: String,
    pub app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct InteractionHit {
    pub kind: String,
    pub app: String,
    pub bundle_id: String,
    pub window: String,
    pub url: String,
    pub keys: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

impl SlotActionTrace {
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn to_facts(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Compress a time-ordered HID stream: insert `focus` on window change,
/// merge consecutive identical actions.
pub fn fold_slot_actions(hits: &[InteractionHit]) -> SlotActionTrace {
    let mut out = SlotActionTrace {
        events: hits.len(),
        ..SlotActionTrace::default()
    };
    let mut last_focus: Option<(String, String)> = None;
    for hit in hits {
        let Some(action) = map_action(&hit.kind) else {
            continue;
        };
        match action {
            "click" => out.clicks += 1,
            "shortcut" => out.shortcuts += 1,
            "submit" => out.submits += 1,
            _ => {}
        }
        let app = if hit.app.is_empty() {
            "unknown".to_string()
        } else {
            hit.app.clone()
        };
        let window = nonempty(&hit.window);
        let focus_key = (app.clone(), window.clone().unwrap_or_default());
        if last_focus.as_ref() != Some(&focus_key) {
            push_folded(
                &mut out.folded,
                FoldedAction {
                    action: "focus".into(),
                    app: app.clone(),
                    bundle_id: nonempty(&hit.bundle_id),
                    window: window.clone(),
                    host: host_only(&hit.url),
                    count: 1,
                    keys: None,
                    x: None,
                    y: None,
                },
            );
            last_focus = Some(focus_key);
        }
        let keys = hit.keys.as_deref().map(pretty_keys);
        push_folded(
            &mut out.folded,
            FoldedAction {
                action: action.into(),
                app,
                bundle_id: nonempty(&hit.bundle_id),
                window,
                host: host_only(&hit.url),
                count: 1,
                keys,
                x: hit.x.map(|v| v.round() as i64),
                y: hit.y.map(|v| v.round() as i64),
            },
        );
    }
    if out.folded.len() > MAX_FOLDED {
        out.folded.truncate(MAX_FOLDED);
    }
    out
}

pub fn parse_interaction_hit(kind: &str, payload: &Value) -> InteractionHit {
    let kb = payload.get("keyboard");
    let mouse = payload.get("mouse");
    let key = kb
        .and_then(|v| v.get("key_equivalent"))
        .and_then(|v| v.as_str());
    let mods = kb
        .and_then(|v| v.get("modifiers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("+")
        })
        .filter(|s| !s.is_empty());
    let keys = match (mods, key) {
        (Some(m), Some(k)) => Some(format!("{m}+{k}")),
        (None, Some(k)) => Some(k.to_string()),
        _ => None,
    };
    InteractionHit {
        kind: kind.to_string(),
        app: payload
            .get("app_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        bundle_id: payload
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        window: payload
            .get("window_title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url: payload
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        keys,
        x: mouse.and_then(|v| v.get("x")).and_then(|v| v.as_f64()),
        y: mouse.and_then(|v| v.get("y")).and_then(|v| v.as_f64()),
    }
}

fn push_folded(out: &mut Vec<FoldedAction>, next: FoldedAction) {
    if let Some(last) = out.last_mut() {
        if last.action == next.action
            && last.app == next.app
            && last.window == next.window
            && last.keys == next.keys
            && last.action != "focus"
        {
            last.count = last.count.saturating_add(next.count);
            last.x = next.x.or(last.x);
            last.y = next.y.or(last.y);
            return;
        }
    }
    out.push(next);
}

fn map_action(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "mouse.click.v1" => "click",
        "mouse.context_menu.v1" => "context_menu",
        "mouse.drag.v1" => "drag",
        "keyboard.shortcut.v1" => "shortcut",
        "keyboard.submit.v1" => "submit",
        "keyboard.text_input.v1" => "type",
        _ => return None,
    })
}

fn pretty_keys(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let mapped = lower
        .replace("0x30", "tab")
        .replace("0x31", "space")
        .replace("0x33", "delete")
        .replace("0x35", "escape")
        .replace("0x24", "return")
        .replace("0x4c", "return");
    mapped
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn host_only(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("").split('?').next()?;
    if host.is_empty() || host == "127.0.0.1" || host.starts_with("localhost") {
        None
    } else {
        Some(host.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(kind: &str, app: &str, window: &str, keys: Option<&str>) -> InteractionHit {
        InteractionHit {
            kind: kind.into(),
            app: app.into(),
            bundle_id: String::new(),
            window: window.into(),
            url: String::new(),
            keys: keys.map(|s| s.into()),
            x: Some(10.0),
            y: Some(20.0),
        }
    }

    #[test]
    fn focus_then_merge_clicks_then_switch_window() {
        let hits = vec![
            hit("mouse.click.v1", "Safari", "Harness", None),
            hit("mouse.click.v1", "Safari", "Harness", None),
            hit("mouse.click.v1", "Safari", "Harness", None),
            hit(
                "keyboard.shortcut.v1",
                "Ghostty",
                "herdr",
                Some("command+s"),
            ),
            hit("keyboard.submit.v1", "Ghostty", "herdr", Some("return")),
        ];
        let t = fold_slot_actions(&hits);
        assert_eq!(t.clicks, 3);
        assert_eq!(t.shortcuts, 1);
        assert_eq!(t.submits, 1);
        let actions: Vec<_> = t.folded.iter().map(|a| a.action.as_str()).collect();
        assert_eq!(
            actions,
            ["focus", "click", "focus", "shortcut", "submit"]
        );
        assert_eq!(t.folded[1].count, 3);
        assert_eq!(t.folded[1].app, "Safari");
        assert_eq!(t.folded[2].window.as_deref(), Some("herdr"));
        assert_eq!(t.folded[3].keys.as_deref(), Some("command+s"));
    }

    #[test]
    fn parse_click_and_shortcut_payloads() {
        let click = parse_interaction_hit(
            "mouse.click.v1",
            &json!({
                "app_name": "Ghostty",
                "bundle_id": "com.mitchellh.ghostty",
                "window_title": "herdr",
                "mouse": {"button": "left", "x": 147.5, "y": 733.3}
            }),
        );
        assert_eq!(click.app, "Ghostty");
        assert_eq!(click.x, Some(147.5));
        let sc = parse_interaction_hit(
            "keyboard.shortcut.v1",
            &json!({
                "app_name": "Safari",
                "window_title": "Harness",
                "keyboard": {"key_equivalent": "s", "modifiers": ["command"]}
            }),
        );
        assert_eq!(sc.keys.as_deref(), Some("command+s"));
        let tab = parse_interaction_hit(
            "keyboard.shortcut.v1",
            &json!({
                "app_name": "Comet",
                "window_title": "Grok",
                "keyboard": {"key_equivalent": "0x30", "modifiers": ["command"]}
            }),
        );
        let folded = fold_slot_actions(&[tab]);
        assert_eq!(folded.folded.last().unwrap().keys.as_deref(), Some("command+tab"));
    }
}
