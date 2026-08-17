//! Pull AX / OCR text for one closed 15-minute History slot.
//!
//! Capture never waits here. The fold only sees app / scene / title /
//! duration; this pass reads `derived` rows already written by the AX and
//! OCR workers and compresses them into a few untrusted snippets the
//! summarizer (or a deterministic fallback) can narrate from.
//!
//! Browsers skip the AX walk (their trees hang). For those apps the
//! screenshot `ocr.v1` body is the content source.

use std::collections::{BTreeMap, HashSet};

use lumen_api::{HistorySlotDto, SkillStepDto, SuggestedSkillDto};

use crate::slot_actions::SlotActionTrace;
use serde::Serialize;
use serde_json::Value;

const MAX_APPS: usize = 5;
const MAX_SNIPPETS_PER_APP: usize = 6;
const MAX_SNIPPET_CHARS: usize = 140;
const MAX_DOCS_PER_APP: usize = 4;
const MIN_FRAGMENT_CHARS: usize = 8;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SlotAppEvidence {
    pub app: String,
    /// `ax` when any kept snippet came from the accessibility tree.
    pub via: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub seen: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SlotEvidence {
    pub ax_docs: usize,
    pub ocr_docs: usize,
    pub apps: Vec<SlotAppEvidence>,
}

impl SlotEvidence {
    pub fn is_empty(&self) -> bool {
        self.apps.iter().all(|a| a.seen.is_empty())
    }

    pub fn to_facts(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone)]
pub struct DerivedDoc {
    pub app: String,
    pub window_title: String,
    pub url: String,
    pub path: String,
    pub kind: String,
    pub text: String,
    pub desynced: bool,
}

/// Compress AX/OCR documents into per-app snippets.
///
/// AX wins over OCR for the same screenshot. Empty / desynced AX falls
/// through to OCR. Chrome (menu bars, clocks) and secret-looking lines
/// are dropped.
pub fn compress_slot_docs(docs: &[DerivedDoc]) -> SlotEvidence {
    let mut ax_docs = 0usize;
    let mut ocr_docs = 0usize;
    let mut by_event: BTreeMap<(String, String, String), ChosenDoc<'_>> = BTreeMap::new();

    for doc in docs {
        match doc.kind.as_str() {
            "ax.v1" => ax_docs += 1,
            "ocr.v1" => ocr_docs += 1,
            _ => {}
        }
        if doc.desynced {
            continue;
        }
        let text = doc.text.trim();
        if text.is_empty() {
            continue;
        }
        let key = (
            doc.app.clone(),
            doc.window_title.clone(),
            first_chars(text, 80),
        );
        let rank = if doc.kind == "ax.v1" { 0 } else { 1 };
        match by_event.get(&key) {
            Some(existing) if existing.rank <= rank => {}
            _ => {
                by_event.insert(key, ChosenDoc { rank, doc });
            }
        }
    }

    let mut grouped: BTreeMap<String, AppAcc> = BTreeMap::new();
    for chosen in by_event.into_values() {
        let doc = chosen.doc;
        let app = if doc.app.is_empty() {
            "unknown".to_string()
        } else {
            doc.app.clone()
        };
        let acc = grouped.entry(app.clone()).or_insert_with(|| AppAcc {
            app,
            via_ax: false,
            window: None,
            path: None,
            host: None,
            seen: Vec::new(),
            seen_norm: HashSet::new(),
            docs: 0,
        });
        if acc.docs >= MAX_DOCS_PER_APP {
            continue;
        }
        acc.docs += 1;
        if chosen.rank == 0 {
            acc.via_ax = true;
        }
        if acc.window.is_none() && !doc.window_title.is_empty() {
            acc.window = Some(shorten(&doc.window_title, 48));
        }
        if acc.path.is_none() && !doc.path.is_empty() {
            acc.path = Some(shorten(&doc.path, 64));
        }
        if acc.host.is_none() {
            acc.host = host_only(&doc.url);
        }
        for frag in salient_fragments(&doc.text) {
            if acc.seen.len() >= MAX_SNIPPETS_PER_APP {
                break;
            }
            let norm = normalize(&frag);
            if norm.is_empty() || acc.seen_norm.contains(&norm) {
                continue;
            }
            acc.seen_norm.insert(norm);
            acc.seen.push(frag);
        }
    }

    let mut apps: Vec<SlotAppEvidence> = grouped
        .into_values()
        .filter(|a| !a.seen.is_empty())
        .map(|a| SlotAppEvidence {
            app: a.app,
            via: if a.via_ax { "ax".into() } else { "ocr".into() },
            window: a.window,
            path: a.path,
            host: a.host,
            seen: a.seen,
        })
        .collect();
    apps.sort_by(|a, b| {
        b.seen
            .len()
            .cmp(&a.seen.len())
            .then_with(|| a.app.cmp(&b.app))
    });
    if apps.len() > MAX_APPS {
        apps.truncate(MAX_APPS);
    }

    SlotEvidence {
        ax_docs,
        ocr_docs,
        apps,
    }
}

/// Overlay extracted snippets onto a folded card. Never touches a `ready`
/// narrative. Status becomes `extracted` so the UI shows the digest while
/// the LLM job can still replace it.
pub fn apply_slot_evidence(slot: &mut HistorySlotDto, ev: &SlotEvidence) {
    if ev.is_empty() || slot.narrative_status == "ready" {
        return;
    }
    if let Some(title) = evidence_title(ev, slot) {
        slot.title = title;
    }
    slot.body = evidence_body(ev, slot);
    if slot.narrative_status != "pending" {
        slot.narrative_status = "extracted".into();
    }
}

/// Cheap gate before asking the model for a CUA-replay chip.
///
/// Need a real HID sequence (focus + clicks/shortcuts), not just reading
/// a page. Messaging-only stretches stay chip-less.
pub fn slot_may_hold_skill(
    slot: &HistorySlotDto,
    ev: &SlotEvidence,
    actions: &SlotActionTrace,
) -> bool {
    if slot.active_ms < 2 * 60_000 {
        return false;
    }
    let hid_enough = actions.folded.len() >= 3 || actions.clicks >= 4 || actions.submits >= 2;
    if !hid_enough {
        return false;
    }
    let apps: Vec<&str> = ev
        .apps
        .iter()
        .map(|a| a.app.as_str())
        .chain(slot.apps.iter().map(|a| a.app_name.as_str()))
        .chain(actions.folded.iter().map(|a| a.app.as_str()))
        .collect();
    if !apps.is_empty() && apps.iter().all(|a| is_messaging_app(a)) {
        return false;
    }
    true
}

/// Keep a model suggestion only when it is a CUA-replayable step list.
pub fn sanitize_suggested_skill(raw: &SuggestedSkillDto) -> Option<SuggestedSkillDto> {
    let name = strip_skill_suffix(raw.name.trim());
    let trigger = raw.trigger.trim();
    let prompt = raw.prompt.trim();
    let verify = raw.verify.trim();
    let n = name.chars().count();
    if !(2..=24).contains(&n) {
        return None;
    }
    if trigger.chars().count() < 6 || prompt.chars().count() < 8 {
        return None;
    }
    if looks_secret(trigger) || looks_secret(prompt) || looks_secret(&name) || looks_secret(verify)
    {
        return None;
    }
    let steps: Vec<SkillStepDto> = raw
        .steps
        .iter()
        .filter_map(sanitize_step)
        .take(8)
        .collect();
    if steps.len() < 2 {
        return None;
    }
    Some(SuggestedSkillDto {
        kind: "cua".into(),
        name,
        trigger: trigger.to_string(),
        prompt: prompt.to_string(),
        verify: verify.to_string(),
        steps,
    })
}

fn sanitize_step(step: &SkillStepDto) -> Option<SkillStepDto> {
    let action = step.action.trim();
    if !matches!(
        action,
        "focus" | "click" | "shortcut" | "submit" | "type" | "context_menu" | "drag"
    ) {
        return None;
    }
    let app = step.app.trim();
    if app.is_empty() {
        return None;
    }
    if action == "shortcut" && step.keys.as_deref().unwrap_or("").trim().is_empty() {
        return None;
    }
    let target = step
        .target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !looks_secret(s))
        .map(|s| s.to_string());
    let note = step
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !looks_secret(s))
        .map(|s| s.to_string());
    Some(SkillStepDto {
        action: action.to_string(),
        app: app.to_string(),
        window: step
            .window
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        target,
        keys: step
            .keys
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        role: step
            .role
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        rel_x: step.rel_x.filter(|v| (0.0..=1.0).contains(v)),
        rel_y: step.rel_y.filter(|v| (0.0..=1.0).contains(v)),
        note,
    })
}

fn strip_skill_suffix(name: &str) -> String {
    let t = name
        .trim()
        .trim_end_matches(" skill")
        .trim_end_matches(" Skill")
        .trim_end_matches("技能")
        .trim_end_matches("自动化")
        .trim();
    t.to_string()
}

fn is_messaging_app(name: &str) -> bool {
    matches!(
        name,
        "Feishu"
            | "飞书"
            | "DingTalk"
            | "钉钉"
            | "WeChat"
            | "微信"
            | "Slack"
            | "Mail"
            | "Mail.app"
            | "Microsoft Outlook"
            | "Outlook"
            | "Messages"
            | "信息"
            | "Telegram"
            | "Discord"
    )
}

fn evidence_title(ev: &SlotEvidence, slot: &HistorySlotDto) -> Option<String> {
    for app in &ev.apps {
        for line in &app.seen {
            let t = line.trim();
            let n = t.chars().count();
            if (8..36).contains(&n) && !looks_like_app_name(t, slot) {
                return Some(t.to_string());
            }
        }
        if let Some(w) = &app.window {
            let t = shorten(w, 36);
            if t.chars().count() >= 6 && !looks_like_app_name(&t, slot) {
                return Some(t);
            }
        }
    }
    None
}

fn evidence_body(ev: &SlotEvidence, slot: &HistorySlotDto) -> String {
    let mut parts: Vec<String> = Vec::new();
    for app in ev.apps.iter().take(3) {
        let focus = app
            .seen
            .first()
            .map(|s| s.as_str())
            .or(app.window.as_deref())
            .unwrap_or(app.app.as_str());
        let extra: Vec<&str> = app
            .seen
            .iter()
            .skip(1)
            .take(2)
            .map(|s| s.as_str())
            .collect();
        if extra.is_empty() {
            parts.push(format!("在{}里看到「{}」", app.app, shorten(focus, 72)));
        } else {
            parts.push(format!(
                "在{}里看到「{}」，还有「{}」",
                app.app,
                shorten(focus, 56),
                shorten(extra[0], 48)
            ));
        }
    }
    if parts.is_empty() {
        return slot.body.clone();
    }
    match parts.len() {
        1 => format!("这段时间{}。", parts[0]),
        2 => format!("这段时间{}；{}。", parts[0], parts[1]),
        _ => format!("这段时间{}；{}；{}。", parts[0], parts[1], parts[2]),
    }
}

fn looks_like_app_name(t: &str, slot: &HistorySlotDto) -> bool {
    slot.apps.iter().any(|a| a.app_name.eq_ignore_ascii_case(t))
        || t.eq_ignore_ascii_case("herdr")
        || t.eq_ignore_ascii_case("untitled")
}

struct ChosenDoc<'a> {
    rank: u8,
    doc: &'a DerivedDoc,
}

struct AppAcc {
    app: String,
    via_ax: bool,
    window: Option<String>,
    path: Option<String>,
    host: Option<String>,
    seen: Vec<String>,
    seen_norm: HashSet<String>,
    docs: usize,
}

fn salient_fragments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for frag in split_fragments(text) {
        if !keep_fragment(&frag) {
            continue;
        }
        let clipped = shorten(&frag, MAX_SNIPPET_CHARS);
        let norm = normalize(&clipped);
        if norm.is_empty() || !seen.insert(norm) {
            continue;
        }
        out.push(clipped);
        if out.len() >= MAX_SNIPPETS_PER_APP + 2 {
            break;
        }
    }
    out
}

fn split_fragments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split(['\n', '\r']) {
        for part in line.split(" | ") {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if p.chars().count() > 160 {
                out.extend(split_sentences(p));
            } else {
                out.push(p.to_string());
            }
        }
    }
    out
}

fn split_sentences(s: &str) -> Vec<String> {
    let mut cur = String::new();
    let mut out = Vec::new();
    for ch in s.chars() {
        cur.push(ch);
        let n = cur.chars().count();
        let boundary = matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | '…');
        if (boundary && n >= MIN_FRAGMENT_CHARS) || n >= 96 {
            let t = cur.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            cur.clear();
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

fn keep_fragment(s: &str) -> bool {
    let t = s.trim();
    let n = t.chars().count();
    if n < MIN_FRAGMENT_CHARS || n > 240 {
        return false;
    }
    if is_chrome_line(t) || looks_secret(t) {
        return false;
    }
    let useful = t
        .chars()
        .filter(|c| c.is_alphanumeric() || is_cjk(*c))
        .count();
    useful >= 4
}

fn is_chrome_line(s: &str) -> bool {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "file"
            | "edit"
            | "view"
            | "window"
            | "help"
            | "history"
            | "bookmarks"
            | "develop"
            | "profiles"
            | "tab"
            | "safari"
            | "ghostty"
            | "firefox"
            | "chrome"
            | "comet"
            | "feishu"
            | "dingtalk"
            | "messenger"
            | "message"
            | "unread"
            | "edit view"
            | "window help"
            | "go window"
            | "command palette"
            | "new session"
            | "workspaces"
    ) {
        return true;
    }
    if is_clock(t) || t.chars().all(|c| !c.is_alphanumeric() && !is_cjk(c)) {
        return true;
    }
    // herdr / Ghostty pane chrome is mostly box-drawing.
    let boxes = t
        .chars()
        .filter(|c| {
            matches!(
                *c,
                '│' | '┃' | '─' | '▕' | '▌' | '▐' | '╭' | '╮' | '╰' | '╯'
            )
        })
        .count();
    if boxes >= 3 && boxes * 3 >= t.chars().count() {
        return true;
    }
    false
}

fn is_clock(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() == 4
        && b[1] == b':'
        && b[0].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
    {
        return true;
    }
    if b.len() == 5
        && b[2] == b':'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
    {
        return true;
    }
    false
}

fn looks_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    if lower.contains("sk-")
        || lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("password")
        || lower.contains("token=")
    {
        return true;
    }
    if s.contains('@') && s.contains('.') && s.chars().any(|c| c.is_ascii_alphabetic()) {
        // likely an email — drop the whole line
        return true;
    }
    false
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}'
    )
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || is_cjk(*c))
        .flat_map(|c| c.to_lowercase())
        .take(48)
        .collect()
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn shorten(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars().take(max.saturating_sub(1)).collect::<String>()
        )
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

/// Parse one `derived` body + screenshot payload into a document.
pub fn parse_derived_doc(payload: &Value, kind: &str, body: &str) -> Option<DerivedDoc> {
    let body_v: Value = serde_json::from_str(body).ok()?;
    let text = body_v
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let desynced = body_v
        .get("desynced")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let app = payload
        .get("app_name")
        .and_then(|v| v.as_str())
        .or_else(|| body_v.get("app_name").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let window_title = payload
        .get("window_title")
        .and_then(|v| v.as_str())
        .or_else(|| body_v.get("window_title").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let url = payload
        .get("url")
        .and_then(|v| v.as_str())
        .or_else(|| body_v.get("browser_url").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let path = body_v
        .get("document_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(DerivedDoc {
        app,
        window_title,
        url,
        path,
        kind: kind.to_string(),
        text,
        desynced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use crate::slot_actions::{fold_slot_actions, InteractionHit};
    use lumen_api::{HistorySlotAppDto, HistorySlotDto, SkillStepDto, SuggestedSkillDto};

    fn doc(app: &str, kind: &str, title: &str, text: &str) -> DerivedDoc {
        DerivedDoc {
            app: app.into(),
            window_title: title.into(),
            url: String::new(),
            path: String::new(),
            kind: kind.into(),
            text: text.into(),
            desynced: false,
        }
    }

    #[test]
    fn ax_preferred_over_ocr_same_window() {
        let docs = vec![
            doc(
                "Ghostty",
                "ocr.v1",
                "herdr",
                "Ghostty | File | Edit | View | Window | Help | herdr",
            ),
            doc(
                "Ghostty",
                "ax.v1",
                "herdr",
                "herdr\nobsidian-vault-pipeline\nWelcome to Kimi Code!\n目标是我们定的产品形态，不是去抄他们的",
            ),
        ];
        let ev = compress_slot_docs(&docs);
        assert_eq!(ev.ax_docs, 1);
        assert_eq!(ev.ocr_docs, 1);
        assert_eq!(ev.apps.len(), 1);
        assert_eq!(ev.apps[0].via, "ax");
        let joined = ev.apps[0].seen.join(" ");
        assert!(joined.contains("Kimi Code") || joined.contains("产品形态"));
        assert!(!joined.contains("File"));
    }

    #[test]
    fn browser_without_ax_uses_ocr() {
        let docs = vec![doc(
            "Safari",
            "ocr.v1",
            "DeepSeek Harness",
            "Safari | File | Edit | 任务转派与历史状态清理机制 | 为右侧条目添加跳转链接 | 4:15",
        )];
        let ev = compress_slot_docs(&docs);
        assert_eq!(ev.apps[0].via, "ocr");
        let joined = ev.apps[0].seen.join(" ");
        assert!(joined.contains("任务转派"));
        assert!(!joined.split_whitespace().any(|w| w == "File"));
    }

    #[test]
    fn drops_desynced_and_secrets() {
        let mut gone = doc("Ghostty", "ax.v1", "herdr", "should skip");
        gone.desynced = true;
        let secret = doc(
            "Safari",
            "ocr.v1",
            "Keys",
            "api_key=sk-abc123456789 and user@example.com sent a token",
        );
        let ev = compress_slot_docs(&[gone, secret]);
        assert!(ev.is_empty());
    }

    #[test]
    fn apply_sets_extracted_and_keeps_ready() {
        let start = Utc.with_ymd_and_hms(2026, 8, 16, 11, 15, 0).unwrap();
        let mut slot = HistorySlotDto {
            slot_start: start,
            slot_end: start + chrono::Duration::minutes(15),
            title: "herdr".into(),
            body: "这段时间在Ghostty → herdr上 6m 31s。".into(),
            apps: vec![HistorySlotAppDto {
                app_name: "Ghostty".into(),
                bundle_id: None,
                ms: 391_000,
                pct: 40.0,
            }],
            scenes: vec![],
            titles: vec!["herdr".into()],
            urls: vec![],
            active_ms: 391_000,
            narrative_status: "none".into(),
            suggested_skills: vec![],
            skill_checked: false,
        };
        let ev = compress_slot_docs(&[doc(
            "Ghostty",
            "ax.v1",
            "herdr",
            "Welcome to Kimi Code!\n任务转派与历史状态清理机制已经落地",
        )]);
        apply_slot_evidence(&mut slot, &ev);
        assert_eq!(slot.narrative_status, "extracted");
        assert!(!slot.body.contains("6m 31s"));
        assert!(slot.body.contains("Kimi") || slot.body.contains("任务转派"));

        slot.narrative_status = "ready".into();
        slot.title = "Wrote the PR".into();
        slot.body = "done".into();
        apply_slot_evidence(&mut slot, &ev);
        assert_eq!(slot.title, "Wrote the PR");
        assert_eq!(slot.body, "done");
    }

    #[test]
    fn messaging_only_stretch_is_not_a_skill_candidate() {
        let start = Utc.with_ymd_and_hms(2026, 8, 16, 11, 15, 0).unwrap();
        let slot = HistorySlotDto {
            slot_start: start,
            slot_end: start + chrono::Duration::minutes(15),
            title: "飞书".into(),
            body: "看群".into(),
            apps: vec![HistorySlotAppDto {
                app_name: "Feishu".into(),
                bundle_id: None,
                ms: 9 * 60_000,
                pct: 100.0,
            }],
            scenes: vec![],
            titles: vec![],
            urls: vec![],
            active_ms: 9 * 60_000,
            narrative_status: "none".into(),
            suggested_skills: vec![],
            skill_checked: false,
        };
        let ev = compress_slot_docs(&[doc(
            "Feishu",
            "ax.v1",
            "飞书",
            "GLM Coding 用户交流群\n又回到熟悉的M3老师了",
        )]);
        let actions = fold_slot_actions(&[hit("mouse.click.v1", "Feishu", "飞书")]);
        assert!(!slot_may_hold_skill(&slot, &ev, &actions));
    }

    #[test]
    fn harness_stretch_is_a_skill_candidate() {
        let start = Utc.with_ymd_and_hms(2026, 8, 16, 11, 15, 0).unwrap();
        let slot = HistorySlotDto {
            slot_start: start,
            slot_end: start + chrono::Duration::minutes(15),
            title: "Harness".into(),
            body: "任务转派".into(),
            apps: vec![HistorySlotAppDto {
                app_name: "Safari".into(),
                bundle_id: None,
                ms: 9 * 60_000,
                pct: 100.0,
            }],
            scenes: vec![],
            titles: vec![],
            urls: vec![],
            active_ms: 9 * 60_000,
            narrative_status: "none".into(),
            suggested_skills: vec![],
            skill_checked: false,
        };
        let ev = compress_slot_docs(&[doc(
            "Safari",
            "ocr.v1",
            "DeepSeek Harness",
            "任务转派与历史状态清理机制\n为右侧条目添加跳转链接",
        )]);
        let actions = fold_slot_actions(&[
            hit("mouse.click.v1", "Safari", "Harness"),
            hit("mouse.click.v1", "Safari", "Harness"),
            hit("keyboard.submit.v1", "Safari", "Harness"),
        ]);
        assert!(slot_may_hold_skill(&slot, &ev, &actions));
    }

    fn hit(kind: &str, app: &str, window: &str) -> InteractionHit {
        InteractionHit {
            kind: kind.into(),
            app: app.into(),
            bundle_id: String::new(),
            window: window.into(),
            url: String::new(),
            keys: None,
            x: None,
            y: None,
        }
    }

    fn step(action: &str, app: &str, window: &str, keys: Option<&str>) -> SkillStepDto {
        SkillStepDto {
            action: action.into(),
            app: app.into(),
            window: Some(window.into()),
            target: None,
            keys: keys.map(|s| s.into()),
            role: None,
            rel_x: None,
            rel_y: None,
            note: None,
        }
    }

    #[test]
    fn sanitize_drops_empty_and_secret_skills() {
        assert!(sanitize_suggested_skill(&SuggestedSkillDto {
            kind: "cua".into(),
            name: "x".into(),
            trigger: "too".into(),
            prompt: "short".into(),
            verify: String::new(),
            steps: vec![],
        })
        .is_none());
        assert!(sanitize_suggested_skill(&SuggestedSkillDto {
            kind: "cua".into(),
            name: "Deploy".into(),
            trigger: "when shipping".into(),
            prompt: "use api_key=sk-abc123456789 please".into(),
            verify: String::new(),
            steps: vec![
                step("focus", "Safari", "Harness", None),
                step("click", "Safari", "Harness", None),
            ],
        })
        .is_none());
        let ok = sanitize_suggested_skill(&SuggestedSkillDto {
            kind: "automation".into(),
            name: "Harness 任务转派复查 skill".into(),
            trigger: "下次改 TaskHistory 清理逻辑时".into(),
            prompt: "帮我在 Harness 窗口里对照失败测试点开任务转派。".into(),
            verify: "失败列表与上次一致".into(),
            steps: vec![
                step("focus", "Safari", "DeepSeek Harness", None),
                step("click", "Safari", "DeepSeek Harness", None),
                step("submit", "Safari", "DeepSeek Harness", Some("return")),
            ],
        })
        .unwrap();
        assert_eq!(ok.kind, "cua");
        assert_eq!(ok.name, "Harness 任务转派复查");
        assert_eq!(ok.steps.len(), 3);
        assert_eq!(ok.steps[0].action, "focus");
    }
}
