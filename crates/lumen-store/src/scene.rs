//! Fold activity segments into scene episodes / rollups.
//!
//! Uses capture-time title + url (and bundle) via `lumen_scene::stack_for`.
//! AX is not required: herdr tabs are inferred from the title pattern.
//! Idle segments are skipped. Consecutive same-label active runs merge.

use std::collections::HashMap;

use lumen_api::{ActivitySegmentDto, SceneDayDto, SceneEpisodeDto, SceneRollupDto};
use lumen_scene::stack_for;

pub fn fold_scene_day(day: &str, segments: &[ActivitySegmentDto]) -> SceneDayDto {
    let mut episodes: Vec<SceneEpisodeDto> = Vec::new();
    for seg in segments {
        if seg.is_idle || seg.is_locked || seg.duration_ms <= 0 {
            continue;
        }
        let app = seg.app_name.as_deref().unwrap_or("unknown");
        let bundle = seg.bundle_id.as_deref().unwrap_or("");
        let title = seg.window_title.as_deref().unwrap_or("");
        let stack = stack_for(app, bundle, title, "", seg.url.as_deref());
        let label = stack.label();
        if let Some(last) = episodes.last_mut() {
            if last.label == label {
                last.ended_at = seg.ended_at.or(Some(seg.started_at));
                last.duration_ms += seg.duration_ms;
                last.segment_count += 1;
                continue;
            }
        }
        episodes.push(SceneEpisodeDto {
            day: day.to_string(),
            kind: stack.kind.as_str().to_string(),
            app_name: stack.app,
            bundle_id: seg.bundle_id.clone(),
            shell: stack.shell,
            leaf: stack.leaf,
            label,
            started_at: seg.started_at,
            ended_at: seg.ended_at,
            duration_ms: seg.duration_ms,
            segment_count: 1,
        });
    }

    let mut by_label: HashMap<String, SceneRollupDto> = HashMap::new();
    for ep in &episodes {
        let entry = by_label.entry(ep.label.clone()).or_insert_with(|| SceneRollupDto {
            kind: ep.kind.clone(),
            app_name: ep.app_name.clone(),
            bundle_id: ep.bundle_id.clone(),
            shell: ep.shell.clone(),
            leaf: ep.leaf.clone(),
            label: ep.label.clone(),
            ms: 0,
            episode_count: 0,
        });
        entry.ms += ep.duration_ms;
        entry.episode_count += 1;
    }
    let mut rollups: Vec<SceneRollupDto> = by_label.into_values().collect();
    rollups.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.label.cmp(&b.label)));

    SceneDayDto {
        day: day.to_string(),
        episodes,
        rollups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use lumen_api::ActivitySegmentDto;

    fn seg(
        app: &str,
        bundle: &str,
        title: &str,
        url: Option<&str>,
        start_min: i64,
        dur_min: i64,
    ) -> ActivitySegmentDto {
        let start = Utc.with_ymd_and_hms(2026, 8, 12, 10, 0, 0).unwrap()
            + chrono::Duration::minutes(start_min);
        let end = start + chrono::Duration::minutes(dur_min);
        ActivitySegmentDto {
            seg_id: format!("{app}-{start_min}"),
            day: "2026-08-12".into(),
            app_name: Some(app.into()),
            bundle_id: Some(bundle.into()),
            window_title: Some(title.into()),
            url: url.map(str::to_string),
            started_at: start,
            ended_at: Some(end),
            duration_ms: dur_min * 60_000,
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
    fn merges_same_herdr_tab_and_splits_on_leaf_change() {
        let segs = vec![
            seg(
                "Ghostty",
                "com.mitchellh.ghostty",
                "writing · 调研 · a0b45a82-37e5-43",
                None,
                0,
                10,
            ),
            seg(
                "Ghostty",
                "com.mitchellh.ghostty",
                "writing · 另一句 · a0b45a82-37e5-43",
                None,
                10,
                5,
            ),
            seg(
                "Ghostty",
                "com.mitchellh.ghostty",
                "source · 解压缩 · dfa2e527-3211-49",
                None,
                15,
                8,
            ),
        ];
        let day = fold_scene_day("2026-08-12", &segs);
        assert_eq!(day.episodes.len(), 2);
        assert_eq!(day.episodes[0].label, "Ghostty → herdr → writing");
        assert_eq!(day.episodes[0].duration_ms, 15 * 60_000);
        assert_eq!(day.episodes[1].label, "Ghostty → herdr → source");
        assert_eq!(day.rollups[0].label, "Ghostty → herdr → writing");
    }

    #[test]
    fn safari_url_becomes_domain_and_loopback_is_dev() {
        let segs = vec![
            seg(
                "Safari",
                "com.apple.Safari",
                "Kimi AI with K3",
                Some("https://www.kimi.com/chat"),
                0,
                4,
            ),
            seg(
                "Safari",
                "com.apple.Safari",
                "DeepSeek Harness",
                Some("http://127.0.0.1:3080/"),
                4,
                20,
            ),
        ];
        let day = fold_scene_day("2026-08-12", &segs);
        assert_eq!(day.episodes[0].label, "Safari → Kimi");
        assert_eq!(day.episodes[0].kind, "browser");
        assert_eq!(day.episodes[1].label, "Safari → 127.0.0.1");
        assert_eq!(day.episodes[1].kind, "development");
    }

    #[test]
    fn skips_idle() {
        let mut idle = seg("Safari", "com.apple.Safari", "Kimi", Some("https://kimi.com"), 0, 30);
        idle.is_idle = true;
        let day = fold_scene_day("2026-08-12", &[idle]);
        assert!(day.episodes.is_empty());
        assert!(day.rollups.is_empty());
    }
}
