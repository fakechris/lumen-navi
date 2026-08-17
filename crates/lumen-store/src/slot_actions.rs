//! Fold HID interaction events into a CUA-shaped action trace.
//!
//! Memory (15-min cards) and Act (CUA chips) share this HID fold. Messaging
//! apps stay in the narrative lane. Replay steps skip chat/mail. Clicks are
//! hit-tested only against the same app's nearest AX snapshot. Typed text is
//! never stored (`record_text` is off). Capture does not wait.

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel_y: Option<f64>,
    /// Narrative verb for the 15-min card (`发了一条消息`, `关闭标签`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AxHitBox {
    pub role: String,
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Default)]
pub struct AxHitSet {
    pub app: String,
    pub window: Option<AxHitBox>,
    pub hits: Vec<AxHitBox>,
}

/// One AX walk, kept separate so a later app cannot label an earlier click.
#[derive(Debug, Clone, Default)]
pub struct AxSnapshot {
    pub ts_ms: Option<i64>,
    pub app: String,
    pub set: AxHitSet,
}

#[derive(Debug, Clone, Default)]
pub struct InteractionHit {
    pub kind: String,
    pub app: String,
    pub bundle_id: String,
    pub window: String,
    pub url: String,
    pub keys: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub ts_ms: Option<i64>,
}

impl SlotActionTrace {
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn to_facts(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Facts for the 15-min card: verbs + same-app labels, no pixel coords.
    pub fn to_interaction_facts(&self) -> Value {
        let folded: Vec<Value> = self
            .folded
            .iter()
            .map(|a| {
                serde_json::json!({
                    "verb": a.verb,
                    "action": a.action,
                    "app": a.app,
                    "window": a.window,
                    "target": a.target,
                    "keys": a.keys,
                    "count": a.count,
                })
            })
            .collect();
        serde_json::json!({
            "events": self.events,
            "clicks": self.clicks,
            "shortcuts": self.shortcuts,
            "submits": self.submits,
            "folded": folded,
        })
    }
}

/// Compress a time-ordered HID stream: insert `focus` on window change,
/// merge consecutive identical actions. Clicks are hit-tested against AX boxes.
pub fn fold_slot_actions(hits: &[InteractionHit]) -> SlotActionTrace {
    fold_slot_actions_with_ax(hits, &AxHitSet::default())
}

pub fn fold_slot_actions_with_ax(hits: &[InteractionHit], ax: &AxHitSet) -> SlotActionTrace {
    fold_slot_actions_inner(hits, AxSource::Merged(ax))
}

/// Same fold, but each AX walk stays scoped to its app and timestamp.
pub fn fold_slot_actions_with_snapshots(
    hits: &[InteractionHit],
    snaps: &[AxSnapshot],
) -> SlotActionTrace {
    fold_slot_actions_inner(hits, AxSource::Snapshots(snaps))
}

#[derive(Clone, Copy)]
enum AxSource<'a> {
    Merged(&'a AxHitSet),
    Snapshots(&'a [AxSnapshot]),
}

fn fold_slot_actions_inner(hits: &[InteractionHit], ax: AxSource<'_>) -> SlotActionTrace {
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
                    target: None,
                    role: None,
                    rel_x: None,
                    rel_y: None,
                    verb: Some(narrative_verb(&app, "focus", None, None)),
                },
            );
            last_focus = Some(focus_key);
        }
        let keys = hit.keys.as_deref().map(pretty_keys);
        let mapped = if action == "click" {
            match ax_for_hit(hit, ax) {
                Some(set) => map_click(hit, set),
                None => ClickMap::default(),
            }
        } else {
            ClickMap::default()
        };
        let verb = Some(narrative_verb(
            &app,
            action,
            keys.as_deref(),
            mapped.target.as_deref(),
        ));
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
                target: mapped.target,
                role: mapped.role,
                rel_x: mapped.rel_x,
                rel_y: mapped.rel_y,
                verb,
            },
        );
    }
    if out.folded.len() > MAX_FOLDED {
        out.folded.truncate(MAX_FOLDED);
    }
    out
}

fn ax_for_hit<'a>(hit: &InteractionHit, src: AxSource<'a>) -> Option<&'a AxHitSet> {
    match src {
        AxSource::Merged(ax) => Some(ax),
        AxSource::Snapshots(snaps) => pick_snapshot(hit, snaps).map(|s| &s.set),
    }
}

fn pick_snapshot<'a>(hit: &InteractionHit, snaps: &'a [AxSnapshot]) -> Option<&'a AxSnapshot> {
    let app = hit.app.trim();
    if app.is_empty() {
        return None;
    }
    snaps
        .iter()
        .filter(|s| !s.app.is_empty() && s.app.eq_ignore_ascii_case(app))
        .min_by_key(|s| match (hit.ts_ms, s.ts_ms) {
            (Some(h), Some(t)) => (h - t).unsigned_abs(),
            _ => u64::MAX / 2,
        })
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
        ts_ms: None,
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

#[derive(Default)]
struct ClickMap {
    target: Option<String>,
    role: Option<String>,
    rel_x: Option<f64>,
    rel_y: Option<f64>,
}

fn map_click(hit: &InteractionHit, ax: &AxHitSet) -> ClickMap {
    let (Some(x), Some(y)) = (hit.x, hit.y) else {
        return ClickMap::default();
    };
    let mut best: Option<&AxHitBox> = None;
    let mut best_area = f64::MAX;
    for box_ in &ax.hits {
        if contains(box_, x, y) {
            let area = (box_.w * box_.h).max(1.0);
            if area < best_area {
                best_area = area;
                best = Some(box_);
            }
        }
    }
    if let Some(el) = best {
        return ClickMap {
            target: Some(el.title.clone()),
            role: Some(el.role.clone()),
            rel_x: Some(((x - el.x) / el.w.max(1.0)).clamp(0.0, 1.0)),
            rel_y: Some(((y - el.y) / el.h.max(1.0)).clamp(0.0, 1.0)),
        };
    }
    if let Some(win) = &ax.window {
        if win.w > 1.0 && win.h > 1.0 {
            return ClickMap {
                target: None,
                role: None,
                rel_x: Some(((x - win.x) / win.w).clamp(0.0, 1.0)),
                rel_y: Some(((y - win.y) / win.h).clamp(0.0, 1.0)),
            };
        }
    }
    ClickMap::default()
}

fn contains(b: &AxHitBox, x: f64, y: f64) -> bool {
    x >= b.x && y >= b.y && x <= b.x + b.w && y <= b.y + b.h
}

pub fn parse_ax_hit_set(body: &Value) -> AxHitSet {
    let mut set = AxHitSet::default();
    set.app = body
        .get("app_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(win) = body.get("window_bounds") {
        set.window = parse_box(win);
    }
    if let Some(arr) = body.get("hits").and_then(|v| v.as_array()) {
        set.hits = arr.iter().filter_map(parse_box).collect();
    }
    set
}

pub fn parse_ax_snapshot(ts_ms: Option<i64>, body: &Value, fallback_app: &str) -> AxSnapshot {
    let set = parse_ax_hit_set(body);
    let app = if set.app.is_empty() {
        fallback_app.trim().to_string()
    } else {
        set.app.clone()
    };
    AxSnapshot { ts_ms, app, set }
}

fn parse_box(v: &Value) -> Option<AxHitBox> {
    Some(AxHitBox {
        role: v.get("role").and_then(|x| x.as_str()).unwrap_or("").into(),
        title: v.get("title").and_then(|x| x.as_str()).unwrap_or("").into(),
        x: v.get("x").and_then(|x| x.as_f64())?,
        y: v.get("y").and_then(|x| x.as_f64())?,
        w: v.get("w").and_then(|x| x.as_f64())?,
        h: v.get("h").and_then(|x| x.as_f64())?,
    })
}

pub fn steps_from_actions(trace: &SlotActionTrace) -> Vec<lumen_api::SkillStepDto> {
    trace
        .folded
        .iter()
        .filter(|a| !is_messaging_app(&a.app))
        .take(8)
        .map(|a| lumen_api::SkillStepDto {
            action: a.action.clone(),
            app: a.app.clone(),
            window: a.window.clone(),
            target: a.target.clone(),
            keys: a.keys.clone(),
            role: a.role.clone(),
            rel_x: a.rel_x,
            rel_y: a.rel_y,
            note: None,
        })
        .collect()
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
        .replace("0x4c", "return")
        .replace("0x7b", "left")
        .replace("0x7c", "right");
    mapped
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppKind {
    Messaging,
    Mail,
    Terminal,
    Browser,
    Other,
}

pub fn is_messaging_app(name: &str) -> bool {
    matches!(app_kind(name), AppKind::Messaging | AppKind::Mail)
}

fn app_kind(name: &str) -> AppKind {
    match name {
        "Feishu" | "飞书" | "Lark" | "DingTalk" | "钉钉" | "WeChat" | "微信" | "Slack"
        | "Messages" | "信息" | "Telegram" | "Discord" => AppKind::Messaging,
        "Mail" | "Mail.app" | "Microsoft Outlook" | "Outlook" | "Mailspring" => AppKind::Mail,
        "Ghostty" | "Terminal" | "iTerm" | "iTerm2" | "Warp" | "kitty" | "Alacritty" => {
            AppKind::Terminal
        }
        "Comet" | "Safari" | "Chrome" | "Google Chrome" | "Firefox" | "Arc" | "Edge"
        | "Microsoft Edge" | "Brave" | "Chromium" => AppKind::Browser,
        _ => AppKind::Other,
    }
}

fn narrative_verb(app: &str, action: &str, keys: Option<&str>, target: Option<&str>) -> String {
    let kind = app_kind(app);
    let keys = keys.unwrap_or("");
    match (kind, action) {
        (AppKind::Messaging, "focus") => "切到聊天".into(),
        (AppKind::Messaging, "click") => {
            if target_looks_like_session(target) {
                "打开会话".into()
            } else {
                "点了会话列表".into()
            }
        }
        (AppKind::Messaging, "submit") => "发了一条消息".into(),
        (AppKind::Messaging, "shortcut") if is_session_nav(keys) => "切换会话".into(),
        (AppKind::Mail, "submit") => "发送邮件".into(),
        (AppKind::Terminal, "submit") => "回车执行".into(),
        (AppKind::Browser, "shortcut") if keys.contains("command") && keys.ends_with("+w") => {
            "关闭标签".into()
        }
        (_, "focus") => "切到窗口".into(),
        (_, "click") => "点击".into(),
        (_, "submit") => "确认提交".into(),
        (_, "shortcut") => "快捷键".into(),
        (_, "drag") => "拖拽".into(),
        (_, "type") => "输入".into(),
        _ => action.to_string(),
    }
}

fn is_session_nav(keys: &str) -> bool {
    keys.contains("left")
        || keys.contains("right")
        || keys.contains("0x7b")
        || keys.contains("0x7c")
}

fn target_looks_like_session(target: Option<&str>) -> bool {
    let Some(t) = target.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    !matches!(
        t,
        "Weixin"
            | "Contacts"
            | "Favorites"
            | "Discover"
            | "More"
            | "Mobile"
            | "Messages"
            | "Chat History"
            | "Send File"
            | "New chat"
            | "Pull requests"
    )
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
            ts_ms: None,
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
        assert_eq!(actions, ["focus", "click", "focus", "shortcut", "submit"]);
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
        assert_eq!(
            folded.folded.last().unwrap().keys.as_deref(),
            Some("command+tab")
        );
    }

    #[test]
    fn click_maps_to_smallest_ax_hit_and_relative_coords() {
        let hits = vec![hit("mouse.click.v1", "Safari", "Harness", None)];
        let mut click = hits[0].clone();
        click.x = Some(25.0);
        click.y = Some(45.0);
        let ax = AxHitSet {
            app: "Safari".into(),
            window: Some(AxHitBox {
                role: "AXWindow".into(),
                title: "Harness".into(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            }),
            hits: vec![AxHitBox {
                role: "AXButton".into(),
                title: "任务转派".into(),
                x: 20.0,
                y: 40.0,
                w: 20.0,
                h: 10.0,
            }],
        };
        let t = fold_slot_actions_with_ax(&[click], &ax);
        let click = t.folded.iter().find(|a| a.action == "click").unwrap();
        assert_eq!(click.target.as_deref(), Some("任务转派"));
        assert_eq!(click.role.as_deref(), Some("AXButton"));
        assert!((click.rel_x.unwrap() - 0.25).abs() < 0.01);
        assert!((click.rel_y.unwrap() - 0.5).abs() < 0.01);
    }

    fn box_at(role: &str, title: &str, x: f64, y: f64, w: f64, h: f64) -> AxHitBox {
        AxHitBox {
            role: role.into(),
            title: title.into(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn click_does_not_take_another_app_ax_label() {
        let mut click = hit("mouse.click.v1", "WeChat", "Weixin", None);
        click.x = Some(216.6);
        click.y = Some(169.5);
        click.ts_ms = Some(1_000);
        let wechat = AxSnapshot {
            ts_ms: Some(1_100),
            app: "WeChat".into(),
            set: AxHitSet {
                app: "WeChat".into(),
                window: Some(box_at("AXWindow", "Weixin", 0.0, 56.0, 1068.0, 841.0)),
                hits: vec![
                    box_at("AXWindow", "Weixin", 0.0, 56.0, 1068.0, 841.0),
                    box_at(
                        "AXStaticText",
                        "联合国常任理事国首脑交流群",
                        60.0,
                        108.0,
                        240.0,
                        65.0,
                    ),
                ],
            },
        };
        let chatgpt = AxSnapshot {
            ts_ms: Some(600_000),
            app: "ChatGPT".into(),
            set: AxHitSet {
                app: "ChatGPT".into(),
                window: None,
                hits: vec![box_at("AXButton", "Pull requests", 8.0, 154.0, 259.0, 30.0)],
            },
        };
        let t = fold_slot_actions_with_snapshots(&[click], &[chatgpt, wechat]);
        let click = t.folded.iter().find(|a| a.action == "click").unwrap();
        assert_eq!(click.target.as_deref(), Some("联合国常任理事国首脑交流群"));
        assert_ne!(click.target.as_deref(), Some("Pull requests"));
        assert_eq!(click.verb.as_deref(), Some("打开会话"));
    }

    #[test]
    fn wechat_submit_is_send_message_not_command() {
        let t = fold_slot_actions(&[hit(
            "keyboard.submit.v1",
            "WeChat",
            "Weixin",
            Some("return"),
        )]);
        let submit = t.folded.iter().find(|a| a.action == "submit").unwrap();
        assert_eq!(submit.verb.as_deref(), Some("发了一条消息"));
        let ghostty = fold_slot_actions(&[hit(
            "keyboard.submit.v1",
            "Ghostty",
            "herdr",
            Some("return"),
        )]);
        let run = ghostty
            .folded
            .iter()
            .find(|a| a.action == "submit")
            .unwrap();
        assert_eq!(run.verb.as_deref(), Some("回车执行"));
    }

    #[test]
    fn replay_steps_keep_browser_and_drop_chat() {
        let hits = vec![
            hit("mouse.click.v1", "Comet", "Home / X", None),
            hit("keyboard.submit.v1", "WeChat", "Weixin", Some("return")),
            hit(
                "keyboard.shortcut.v1",
                "Comet",
                "Home / X",
                Some("command+w"),
            ),
        ];
        let t = fold_slot_actions(&hits);
        let steps = steps_from_actions(&t);
        assert!(steps.iter().all(|s| s.app != "WeChat"));
        assert!(steps.iter().any(|s| s.app == "Comet"));
        let facts = t.to_interaction_facts();
        let blob = facts.to_string();
        assert!(blob.contains("WeChat"));
        assert!(blob.contains("发了一条消息"));
        assert!(!blob.contains("rel_x"));
    }

    #[test]
    fn session_arrows_are_pretty_and_named() {
        let t = fold_slot_actions(&[hit(
            "keyboard.shortcut.v1",
            "WeChat",
            "Weixin",
            Some("command+0x7b"),
        )]);
        let sc = t.folded.iter().find(|a| a.action == "shortcut").unwrap();
        assert_eq!(sc.keys.as_deref(), Some("command+left"));
        assert_eq!(sc.verb.as_deref(), Some("切换会话"));
    }
}
