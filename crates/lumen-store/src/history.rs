//! Fold activity segments into 15-minute History cards.
//!
//! Wall-clock slots (`:00` / `:15` / `:30` / `:45`) with apps by **duration**,
//! not event count. Idle and lock time are excluded. Titles/bodies are
//! deterministic so Observe never waits on a model.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
use lumen_api::{ActivitySegmentDto, HistorySlotAppDto, HistorySlotDto, HistorySlotSceneDto};
use lumen_scene::stack_for;

const SLOT_MINUTES: i64 = 15;

#[derive(Default)]
struct SlotAcc {
    apps: BTreeMap<String, SlotApp>,
    scenes: BTreeMap<String, i64>,
    titles: BTreeMap<String, i64>,
    urls: BTreeMap<String, i64>,
    active_ms: i64,
}

struct SlotApp {
    app_name: String,
    bundle_id: Option<String>,
    ms: i64,
}

pub fn fold_history_slots<Tz: TimeZone>(
    segments: &[ActivitySegmentDto],
    tz: Tz,
) -> Vec<HistorySlotDto>
where
    Tz::Offset: std::fmt::Display,
{
    let mut slots: BTreeMap<DateTime<Utc>, SlotAcc> = BTreeMap::new();
    for seg in segments {
        if seg.is_idle || seg.is_locked {
            continue;
        }
        let end = seg
            .ended_at
            .unwrap_or_else(|| seg.started_at + Duration::milliseconds(seg.duration_ms.max(0)));
        if end <= seg.started_at {
            continue;
        }
        let mut cursor = floor_slot(seg.started_at, &tz);
        while cursor < end {
            let slot_end = cursor + Duration::minutes(SLOT_MINUTES);
            let overlap_start = seg.started_at.max(cursor);
            let overlap_end = end.min(slot_end);
            let ms = (overlap_end - overlap_start).num_milliseconds().max(0);
            if ms > 0 {
                add_overlap(slots.entry(cursor).or_default(), seg, ms);
            }
            cursor = slot_end;
        }
    }

    let mut out: Vec<HistorySlotDto> = slots
        .into_iter()
        .filter(|(_, acc)| acc.active_ms > 0)
        .map(|(start, acc)| finish_slot(start, acc))
        .collect();
    out.reverse();
    out
}

fn floor_slot<Tz: TimeZone>(ts: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
    let local = ts.with_timezone(tz);
    let minute = (local.minute() / 15) * 15;
    local
        .with_minute(minute)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(ts)
}

fn add_overlap(acc: &mut SlotAcc, seg: &ActivitySegmentDto, ms: i64) {
    acc.active_ms += ms;
    let app_name = seg
        .app_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let key = seg
        .bundle_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| app_name.clone());
    let entry = acc.apps.entry(key).or_insert_with(|| SlotApp {
        app_name: app_name.clone(),
        bundle_id: seg.bundle_id.clone(),
        ms: 0,
    });
    entry.ms += ms;

    let app = app_name.as_str();
    let bundle = seg.bundle_id.as_deref().unwrap_or("");
    let title = seg.window_title.as_deref().unwrap_or("");
    let label = stack_for(app, bundle, title, "", seg.url.as_deref()).label();
    *acc.scenes.entry(label).or_default() += ms;

    if let Some(t) = seg.window_title.as_ref().filter(|t| !t.is_empty()) {
        *acc.titles.entry(t.clone()).or_default() += ms;
    }
    if let Some(u) = seg.url.as_ref().filter(|u| !u.is_empty()) {
        *acc.urls.entry(u.clone()).or_default() += ms;
    }
}

fn finish_slot(start: DateTime<Utc>, acc: SlotAcc) -> HistorySlotDto {
    let mut apps: Vec<HistorySlotAppDto> = acc
        .apps
        .into_values()
        .map(|a| HistorySlotAppDto {
            pct: if acc.active_ms > 0 {
                (a.ms as f64) * 100.0 / (acc.active_ms as f64)
            } else {
                0.0
            },
            app_name: a.app_name,
            bundle_id: a.bundle_id,
            ms: a.ms,
        })
        .collect();
    apps.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.app_name.cmp(&b.app_name)));

    let mut scenes: Vec<HistorySlotSceneDto> = acc
        .scenes
        .into_iter()
        .map(|(label, ms)| HistorySlotSceneDto { label, ms })
        .collect();
    scenes.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.label.cmp(&b.label)));

    let mut titles: Vec<(String, i64)> = acc.titles.into_iter().collect();
    titles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let titles: Vec<String> = titles.into_iter().take(4).map(|(t, _)| t).collect();

    let mut urls: Vec<(String, i64)> = acc.urls.into_iter().collect();
    urls.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let urls: Vec<String> = urls.into_iter().take(4).map(|(u, _)| u).collect();

    let title = distinctive_title(&titles, &apps, &scenes);
    let body = narrative_body(&apps, &scenes);

    HistorySlotDto {
        slot_start: start,
        slot_end: start + Duration::minutes(SLOT_MINUTES),
        title,
        body,
        apps,
        scenes,
        titles,
        urls,
        active_ms: acc.active_ms,
        narrative_status: "none".into(),
    }
}

pub fn history_slot_key(start: DateTime<Utc>) -> String {
    format!("history.slot.{}", start.to_rfc3339())
}

/// Overlay a persisted narrative onto a freshly folded card.
///
/// `ready` is the LLM card. `extracted` is the AX/OCR digest written
/// before the model runs — the UI should show that instead of the
/// duration laundry list. `pending` / `failed` keep fold copy.
pub fn overlay_slot_narrative(slot: &mut HistorySlotDto, persisted: &HistorySlotDto) {
    match persisted.narrative_status.as_str() {
        "ready" | "extracted" => {
            slot.title = persisted.title.clone();
            slot.body = persisted.body.clone();
            slot.narrative_status = persisted.narrative_status.clone();
        }
        "pending" | "failed" => {
            slot.narrative_status = persisted.narrative_status.clone();
        }
        _ => {}
    }
}

fn distinctive_title(
    titles: &[String],
    apps: &[HistorySlotAppDto],
    scenes: &[HistorySlotSceneDto],
) -> String {
    if let Some(raw) = titles.first() {
        let t = shorten_title(raw);
        if t.chars().count() >= 4 && !apps.iter().any(|a| a.app_name == t) {
            return t;
        }
    }
    if let Some(s) = scenes.first() {
        if s.label.chars().count() >= 4 {
            return shorten_title(&s.label);
        }
    }
    if apps.len() >= 2 {
        format!("{} + {}", apps[0].app_name, apps[1].app_name)
    } else {
        apps.first()
            .map(|a| a.app_name.clone())
            .unwrap_or_else(|| "活动".into())
    }
}

fn narrative_body(apps: &[HistorySlotAppDto], scenes: &[HistorySlotSceneDto]) -> String {
    if apps.is_empty() {
        return String::new();
    }
    let bits: Vec<String> = apps
        .iter()
        .take(3)
        .map(|a| {
            let label = scenes
                .iter()
                .find(|s| s.label.starts_with(&a.app_name))
                .map(|s| s.label.as_str());
            match label {
                Some(l) if l != a.app_name => format!("在{}上 {}", l, fmt_ms(a.ms)),
                _ => format!("在{}上 {}", a.app_name, fmt_ms(a.ms)),
            }
        })
        .collect();
    match bits.len() {
        0 => String::new(),
        1 => format!("这段时间{}。", bits[0]),
        2 => format!("这段时间{}，随后{}。", bits[0], bits[1]),
        _ => format!("这段时间{}，随后{}，以及{}。", bits[0], bits[1], bits[2]),
    }
}

fn shorten_title(raw: &str) -> String {
    let t = raw.trim();
    let t = t
        .split(" - ")
        .next()
        .unwrap_or(t)
        .split(" — ")
        .next()
        .unwrap_or(t)
        .trim();
    let n = t.chars().count();
    if n <= 42 {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(40).collect::<String>())
    }
}

fn fmt_ms(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else {
        let m = secs / 60;
        let rem = secs % 60;
        if rem == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {rem}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_api::ActivitySegmentDto;

    fn seg(
        app: &str,
        bundle: &str,
        title: &str,
        start: DateTime<Utc>,
        minutes: i64,
    ) -> ActivitySegmentDto {
        let end = start + Duration::minutes(minutes);
        ActivitySegmentDto {
            seg_id: format!("{app}-{}", start.timestamp()),
            day: start.format("%Y-%m-%d").to_string(),
            app_name: Some(app.into()),
            bundle_id: Some(bundle.into()),
            window_title: Some(title.into()),
            url: None,
            started_at: start,
            ended_at: Some(end),
            duration_ms: minutes * 60_000,
            is_idle: false,
            is_locked: false,
            category: None,
            productivity_level: None,
            event_count: 1,
            source: "auto".into(),
            scene_label: None,
        }
    }

    #[test]
    fn safari_then_terminal_in_one_slot() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 15, 0).unwrap();
        let segs = vec![
            seg("Safari", "com.apple.Safari", "Inbox", start, 7),
            seg(
                "Ghostty",
                "com.mitchellh.ghostty",
                "herdr",
                start + Duration::minutes(7),
                3,
            ),
        ];
        let slots = fold_history_slots(&segs, Utc);
        assert_eq!(slots.len(), 1);
        let slot = &slots[0];
        assert_eq!(slot.slot_start, start);
        assert_eq!(slot.apps.len(), 2);
        assert_eq!(slot.apps[0].app_name, "Safari");
        assert_eq!(slot.apps[0].ms, 7 * 60_000);
        assert_eq!(slot.apps[1].app_name, "Ghostty");
        assert_eq!(slot.apps[1].ms, 3 * 60_000);
        assert!(slot.title.contains("Inbox") || slot.title.contains("Safari"));
        assert!(slot.body.contains("Safari"));
        assert!(slot.body.contains("Ghostty"));
        assert!(slot.body.starts_with("这段时间"));
    }

    #[test]
    fn idle_and_lock_are_excluded() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 20, 0).unwrap();
        let mut idle = seg("Safari", "com.apple.Safari", "Inbox", start, 10);
        idle.is_idle = true;
        let mut locked = seg("Safari", "com.apple.Safari", "Inbox", start, 10);
        locked.is_locked = true;
        assert!(fold_history_slots(&[idle, locked], Utc).is_empty());
    }

    #[test]
    fn long_segment_spans_two_slots() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 10, 0).unwrap();
        let segs = vec![seg("Safari", "com.apple.Safari", "Doc", start, 20)];
        let slots = fold_history_slots(&segs, Utc);
        assert_eq!(slots.len(), 2);
        let total: i64 = slots.iter().map(|s| s.active_ms).sum();
        assert_eq!(total, 20 * 60_000);
        assert_eq!(
            slots[0].slot_start,
            Utc.with_ymd_and_hms(2026, 8, 15, 4, 15, 0).unwrap()
        );
        assert_eq!(
            slots[1].slot_start,
            Utc.with_ymd_and_hms(2026, 8, 15, 4, 0, 0).unwrap()
        );
    }

    #[test]
    fn overlay_ready_replaces_title_body() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 20, 0).unwrap();
        let mut slot = fold_history_slots(
            &[seg("Safari", "com.apple.Safari", "Inbox", start, 10)],
            Utc,
        )
        .remove(0);
        let mut persisted = slot.clone();
        persisted.title = "Wrote the PR".into();
        persisted.body = "Safari for ten minutes on Inbox.".into();
        persisted.narrative_status = "ready".into();
        overlay_slot_narrative(&mut slot, &persisted);
        assert_eq!(slot.title, "Wrote the PR");
        assert_eq!(slot.body, "Safari for ten minutes on Inbox.");
        assert_eq!(slot.narrative_status, "ready");
    }

    #[test]
    fn overlay_extracted_replaces_laundry_list() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 20, 0).unwrap();
        let mut slot = fold_history_slots(
            &[seg("Safari", "com.apple.Safari", "Inbox", start, 10)],
            Utc,
        )
        .remove(0);
        let mut persisted = slot.clone();
        persisted.title = "Inbox triage".into();
        persisted.body = "在 Safari 里看到「未读 12」.".into();
        persisted.narrative_status = "extracted".into();
        overlay_slot_narrative(&mut slot, &persisted);
        assert_eq!(slot.title, "Inbox triage");
        assert_eq!(slot.body, "在 Safari 里看到「未读 12」.");
        assert_eq!(slot.narrative_status, "extracted");
    }

    #[test]
    fn overlay_failed_keeps_deterministic_copy() {
        let start = Utc.with_ymd_and_hms(2026, 8, 15, 4, 20, 0).unwrap();
        let mut slot = fold_history_slots(
            &[seg("Safari", "com.apple.Safari", "Inbox", start, 10)],
            Utc,
        )
        .remove(0);
        let original_title = slot.title.clone();
        let mut persisted = slot.clone();
        persisted.title = "ignored".into();
        persisted.narrative_status = "failed".into();
        overlay_slot_narrative(&mut slot, &persisted);
        assert_eq!(slot.title, original_title);
        assert_eq!(slot.narrative_status, "failed");
    }
}
