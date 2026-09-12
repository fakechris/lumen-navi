//! Read-time accounting over immutable automatic intervals and manual overlays.
//! UTC half-open intervals are clipped before aggregation. Missing observations
//! are not extended to the current clock; manual time wins without double count.
use crate::{GroupBy, StoreError};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Timelike, Utc};
use lumen_api::{ActivitySegmentDto, AppTotal, CategoryTotal, DayStatsDto, HourCategoryTotal};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn day_bounds(day: &str) -> Result<(DateTime<Utc>, DateTime<Utc>), StoreError> {
    let date =
        NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|e| StoreError::Other(e.to_string()))?;
    let next = date
        .succ_opt()
        .ok_or_else(|| StoreError::Other("day out of range".into()))?;
    let midnight = |d: NaiveDate| {
        // A few zones skip midnight. Use the first representable minute of
        // that calendar date; repeated midnight starts at its earlier instant.
        (0..1440)
            .find_map(|minute| {
                Local
                    .from_local_datetime(
                        &(d.and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::minutes(minute)),
                    )
                    .earliest()
                    .map(|t| t.with_timezone(&Utc))
            })
            .ok_or_else(|| StoreError::Other("local day does not exist".into()))
    };
    Ok((midnight(date)?, midnight(next)?))
}

pub(crate) fn effective_segments(
    rows: Vec<ActivitySegmentDto>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    day: &str,
) -> Vec<ActivitySegmentDto> {
    // Sweep boundaries instead of summing overlapping durations. For legacy
    // overlaps, manual wins, then the later start, then stable segment identity.
    let mut boundaries = BTreeMap::<DateTime<Utc>, Vec<(bool, usize)>>::new();
    for (i, row) in rows.iter().enumerate() {
        let a = row.started_at.max(start);
        let b = row.ended_at.unwrap_or(row.started_at).min(end);
        if a < b {
            boundaries.entry(a).or_default().push((true, i));
            boundaries.entry(b).or_default().push((false, i));
        }
    }
    let mut active = BTreeSet::new();
    let mut previous = None;
    let mut out: Vec<ActivitySegmentDto> = Vec::new();
    for (at, changes) in boundaries {
        if let (Some(from), Some((_, _, _, i))) = (previous, active.last()) {
            let row: &ActivitySegmentDto = &rows[*i];
            if let Some(last) = out
                .last_mut()
                .filter(|last| last.seg_id == row.seg_id && last.ended_at == Some(from))
            {
                last.ended_at = Some(at);
                last.duration_ms += (at - from).num_milliseconds();
            } else {
                let mut part = row.clone();
                part.day = day.into();
                part.started_at = from;
                part.ended_at = Some(at);
                part.duration_ms = (at - from).num_milliseconds();
                out.push(part);
            }
        }
        if let Some(from) = previous.filter(|_| active.is_empty()) {
            if from < at {
                let mut gap = rows[0].clone();
                gap.seg_id = format!("gap:{}:{}", from.timestamp_millis(), at.timestamp_millis());
                gap.day = day.into();
                gap.started_at = from;
                gap.ended_at = Some(at);
                gap.duration_ms = (at - from).num_milliseconds();
                gap.app_name = None;
                gap.bundle_id = None;
                gap.window_title = None;
                gap.url = None;
                gap.category = None;
                gap.productivity_level = None;
                gap.scene_label = None;
                gap.is_idle = false;
                gap.is_locked = false;
                gap.source = "gap".into();
                gap.event_count = 0;
                out.push(gap);
            }
        }
        for (add, i) in changes {
            let r = &rows[i];
            let key = (r.source == "manual", r.started_at, r.seg_id.clone(), i);
            if add {
                active.insert(key);
            } else {
                active.remove(&key);
            }
        }
        previous = Some(at);
    }
    // A manual overlay can split one automatic segment into multiple visible
    // pieces. Give those pieces stable distinct view IDs; manual IDs remain
    // the stored identifiers accepted by the delete endpoint.
    let mut counts = BTreeMap::new();
    for row in &out {
        *counts.entry(row.seg_id.clone()).or_insert(0) += 1;
    }
    for row in &mut out {
        if counts[&row.seg_id] > 1 && row.source != "manual" {
            row.seg_id = format!("{}@{}", row.seg_id, row.started_at.timestamp_millis());
        }
    }
    out
}

fn underlying_segment_id(row: &ActivitySegmentDto) -> &str {
    if row.source == "auto" {
        row.seg_id.split('@').next().unwrap_or(&row.seg_id)
    } else {
        &row.seg_id
    }
}

fn is_active(row: &ActivitySegmentDto) -> bool {
    !row.is_idle
        && !row.is_locked
        && row.app_name.is_some()
        && row.source != "gap"
        && row.source != "paused"
}

pub(crate) fn categories(rows: &[ActivitySegmentDto]) -> Vec<CategoryTotal> {
    let mut acc = BTreeMap::new();
    for row in rows.iter().filter(|r| is_active(r)) {
        *acc.entry((
            row.category
                .clone()
                .unwrap_or_else(|| "Uncategorized".into()),
            row.productivity_level.clone(),
        ))
        .or_insert(0) += row.duration_ms;
    }
    let mut out: Vec<_> = acc
        .into_iter()
        .map(|((category, productivity_level), ms)| CategoryTotal {
            category,
            productivity_level,
            ms,
        })
        .collect();
    out.sort_by(|a, b| b.ms.cmp(&a.ms).then(a.category.cmp(&b.category)));
    out
}

pub(crate) fn pulse(rows: &[ActivitySegmentDto]) -> Option<f64> {
    let (weighted, total) = rows
        .iter()
        .filter(|r| is_active(r))
        .fold((0.0, 0i64), |(w, t), r| {
            let weight = match r.productivity_level.as_deref() {
                Some("productive") => 1.0,
                Some("neutral") => 0.5,
                Some("distracting") => 0.0,
                _ => return (w, t),
            };
            (w + weight * r.duration_ms as f64, t + r.duration_ms)
        });
    (total > 0).then(|| 100.0 * weighted / total as f64)
}

pub(crate) fn top_apps(rows: &[ActivitySegmentDto], group: GroupBy, limit: usize) -> Vec<AppTotal> {
    let mut groups: BTreeMap<String, Vec<&ActivitySegmentDto>> = BTreeMap::new();
    for row in rows.iter().filter(|r| is_active(r) && r.duration_ms > 0) {
        let key = if group == GroupBy::Site {
            row.url
                .as_deref()
                .and_then(crate::categorization::registrable_domain)
        } else {
            row.bundle_id.clone().or(row.app_name.clone())
        };
        if let Some(k) = key {
            groups.entry(k).or_default().push(row);
        }
    }
    let mut out: Vec<_> = groups
        .into_iter()
        .map(|(key, grouped)| {
            let names: Vec<&str> = grouped
                .iter()
                .filter_map(|r| r.app_name.as_deref())
                .collect();
            let best = grouped.iter().max_by_key(|r| r.duration_ms).unwrap();
            let mut classifications = BTreeMap::new();
            for row in &grouped {
                *classifications
                    .entry((row.category.clone(), row.productivity_level.clone()))
                    .or_insert(0i64) += row.duration_ms;
            }
            // Keep the pair together and choose the dominant total duration;
            // lexical order is only a deterministic tie-breaker.
            let ((category, productivity_level), _) = classifications
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)))
                .unwrap();
            AppTotal {
                app_name: if group == GroupBy::Site {
                    key
                } else {
                    crate::categorization::preferred_display_name(&names)
                },
                bundle_id: if group == GroupBy::Site {
                    None
                } else {
                    best.bundle_id.clone()
                },
                ms: grouped.iter().map(|r| r.duration_ms).sum(),
                category,
                productivity_level,
                segment_count: grouped
                    .iter()
                    .map(|r| underlying_segment_id(r))
                    .collect::<BTreeSet<_>>()
                    .len() as i64,
                title: if group == GroupBy::Site {
                    best.window_title.clone()
                } else {
                    None
                },
            }
        })
        .collect();
    out.sort_by(|a, b| b.ms.cmp(&a.ms).then(a.app_name.cmp(&b.app_name)));
    out.truncate(limit);
    out
}

pub(crate) fn day_stats(day: &str, rows: &[ActivitySegmentDto], group: GroupBy) -> DayStatsDto {
    let mut by_hour = [0; 24];
    let mut hours: BTreeMap<(u8, String), i64> = BTreeMap::new();
    for row in rows.iter().filter(|r| is_active(r)) {
        let mut at = row.started_at;
        let end = row.ended_at.unwrap_or(at);
        // UTC minute boundaries handle fractional-offset zones and both DST
        // directions without assuming that a civil hour is always 3600 seconds.
        while at < end {
            let next_ms = (at.timestamp_millis().div_euclid(60_000) + 1) * 60_000;
            let next = DateTime::from_timestamp_millis(next_ms).unwrap().min(end);
            let hour = at.with_timezone(&Local).hour() as u8;
            let ms = (next - at).num_milliseconds();
            by_hour[hour as usize] += ms;
            *hours
                .entry((
                    hour,
                    row.category
                        .clone()
                        .unwrap_or_else(|| "Uncategorized".into()),
                ))
                .or_default() += ms;
            at = next;
        }
    }
    DayStatsDto {
        day: day.into(),
        total_active_ms: rows
            .iter()
            .filter(|r| is_active(r))
            .map(|r| r.duration_ms)
            .sum(),
        total_idle_ms: rows
            .iter()
            .filter(|r| r.is_idle || r.is_locked)
            .map(|r| r.duration_ms)
            .sum(),
        pulse_score: pulse(rows),
        context_switches: rows
            .iter()
            .filter(|r| is_active(r))
            .map(|r| underlying_segment_id(r))
            .collect::<BTreeSet<_>>()
            .len() as i64,
        by_category: categories(rows),
        top_apps: top_apps(rows, group, 20),
        by_hour,
        by_hour_category: hours
            .into_iter()
            .map(|((hour, category), ms)| HourCategoryTotal { hour, category, ms })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: &str, from: i64, to: i64, manual: bool) -> ActivitySegmentDto {
        serde_json::from_value(serde_json::json!({"seg_id":id,"day":"2026-01-01","app_name":id,"bundle_id":null,"window_title":null,"started_at":DateTime::from_timestamp_millis(from).unwrap(),"ended_at":DateTime::from_timestamp_millis(to).unwrap(),"duration_ms":to-from,"is_idle":false,"is_locked":false,"category":"Development","productivity_level":"productive","event_count":1,"source":if manual{"manual"}else{"auto"}})).unwrap()
    }
    #[test]
    fn manual_union_masks_auto_and_removal_restores_it() {
        let auto = row("auto", 0, 100, false);
        let m1 = row("m1", 20, 60, true);
        let m2 = row("m2", 40, 80, true);
        let fold = |rows| {
            effective_segments(
                rows,
                DateTime::from_timestamp_millis(0).unwrap(),
                DateTime::from_timestamp_millis(100).unwrap(),
                "1970-01-01",
            )
        };
        let out = fold(vec![auto.clone(), m1, m2]);
        assert_eq!(out.iter().map(|r| r.duration_ms).sum::<i64>(), 100);
        assert_eq!(
            out.iter()
                .filter(|r| r.source == "manual")
                .map(|r| r.duration_ms)
                .sum::<i64>(),
            60
        );
        assert_eq!(fold(vec![auto])[0].duration_ms, 100);
    }
    #[test]
    fn clipping_and_reordering_do_not_double_count() {
        let a = row("a", 0, 50, false);
        let b = row("b", 30, 90, false);
        let fold = |rows| {
            effective_segments(
                rows,
                DateTime::from_timestamp_millis(20).unwrap(),
                DateTime::from_timestamp_millis(80).unwrap(),
                "1970-01-01",
            )
        };
        let x = fold(vec![a.clone(), b.clone()]);
        let y = fold(vec![b, a]);
        assert_eq!(
            serde_json::to_value(&x).unwrap(),
            serde_json::to_value(&y).unwrap()
        );
        assert_eq!(x.iter().map(|r| r.duration_ms).sum::<i64>(), 60);
        assert_eq!(pulse(&x), Some(100.0));
    }
    #[test]
    fn top_app_classification_uses_total_duration_and_keeps_pair() {
        let mut a = row("a", 0, 40, false);
        let mut b = row("b", 40, 80, false);
        let mut c = row("c", 80, 140, false);
        for r in [&mut a, &mut b, &mut c] {
            r.bundle_id = Some("same.app".into());
        }
        for r in [&mut a, &mut b] {
            r.category = Some("Entertainment".into());
            r.productivity_level = Some("distracting".into());
        }
        c.category = Some("Writing".into());
        c.productivity_level = Some("productive".into());
        let result = top_apps(&[a, b, c], GroupBy::App, 10);
        assert_eq!(result[0].category.as_deref(), Some("Entertainment"));
        assert_eq!(result[0].productivity_level.as_deref(), Some("distracting"));
        assert_eq!(result[0].ms, 140);
    }

    #[test]
    fn gaps_are_internal_and_do_not_invent_unobserved_day_tails() {
        let fold = |rows| {
            effective_segments(
                rows,
                DateTime::from_timestamp_millis(0).unwrap(),
                DateTime::from_timestamp_millis(100).unwrap(),
                "1970-01-01",
            )
        };
        assert!(fold(vec![]).is_empty());
        let rows = fold(vec![row("a", 20, 40, false), row("b", 60, 80, false)]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].source, "gap");
        assert_eq!(rows[1].duration_ms, 20);
        assert_eq!(rows[0].started_at.timestamp_millis(), 20);
        assert_eq!(rows[2].ended_at.unwrap().timestamp_millis(), 80);
    }

    #[cfg(unix)]
    #[test]
    fn dst_day_lengths_and_hour_sums() {
        if std::env::var("LUMEN_TIME_TEST_TZ").is_err() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "time_accounting::tests::dst_day_lengths_and_hour_sums",
                    "--nocapture",
                ])
                .env("TZ", "America/Los_Angeles")
                .env("LUMEN_TIME_TEST_TZ", "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        for (day, hours) in [("2026-03-08", 23), ("2026-11-01", 25)] {
            let (start, end) = day_bounds(day).unwrap();
            assert_eq!((end - start).num_hours(), hours);
            let rows = effective_segments(
                vec![row(
                    "a",
                    start.timestamp_millis(),
                    end.timestamp_millis(),
                    false,
                )],
                start,
                end,
                day,
            );
            let stats = day_stats(day, &rows, GroupBy::App);
            assert_eq!(stats.total_active_ms, hours * 3_600_000);
            assert_eq!(stats.by_hour.iter().sum::<i64>(), stats.total_active_ms);
            assert_eq!(
                stats.by_hour_category.iter().map(|r| r.ms).sum::<i64>(),
                stats.total_active_ms
            );
        }
    }
}
