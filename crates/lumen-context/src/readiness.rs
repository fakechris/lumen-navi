use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AxNode, ContextSnapshot, SourceKind, SourceState};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityObservation {
    pub target_present: bool,
    pub target_bundle_id_present: bool,
    pub target_pid_present: bool,
    pub target_window_present: bool,
    pub target_window_title_present: bool,
    pub target_window_bounds_present: bool,
    pub editor_present: bool,
    pub editor_role_present: bool,
    pub editor_editable: Option<bool>,
    pub editor_secure: bool,
    pub editor_selection_present: bool,
    pub editor_selected_chars: usize,
    pub editor_prefix_chars: usize,
    pub editor_suffix_chars: usize,
    pub editor_full_field_chars: usize,
    pub editor_nearby_before_chars: usize,
    pub editor_nearby_after_chars: usize,
    pub editor_truncated: bool,
    pub ax_visible_present: bool,
    pub ax_visited_nodes: usize,
    pub ax_text_nodes: usize,
    pub ax_text_chars: usize,
    pub ax_truncated_nodes: usize,
    pub ax_truncated_chars: usize,
    pub browser_present: bool,
    pub browser_url_present: bool,
    pub browser_focused_element_present: bool,
    pub browser_focused_secure: bool,
    pub browser_focused_value_chars: usize,
    pub browser_viewport_blocks: usize,
    pub browser_viewport_chars: usize,
    pub browser_truncated: bool,
    pub screenshot_documents: usize,
    pub ocr_documents: usize,
    pub ocr_text_chars: usize,
    pub ocr_boxes: usize,
    pub visible_text_blocks: usize,
    pub visible_text_chars: usize,
    pub artifact_descriptors: usize,
    pub payload_count: usize,
    pub payload_bytes: u64,
    pub raw_text_allowed: bool,
    pub screenshots_allowed: bool,
    pub applied_privacy_gates: usize,
}

impl CapabilityObservation {
    pub fn from_snapshot(snapshot: &ContextSnapshot) -> Self {
        let manifest = &snapshot.manifest;
        let target = manifest.target.as_ref();
        let editor = manifest.editor.as_ref();
        let browser = manifest.browser.as_ref();
        let mut ax_text_nodes = 0;
        let mut ax_text_chars = 0;
        if let Some(ax) = manifest.ax_visible.as_ref() {
            for root in &ax.roots {
                observe_ax_node(root, &mut ax_text_nodes, &mut ax_text_chars);
            }
        }
        Self {
            target_present: target.is_some(),
            target_bundle_id_present: target.and_then(|value| value.bundle_id.as_ref()).is_some(),
            target_pid_present: target.and_then(|value| value.pid).is_some(),
            target_window_present: target.and_then(|value| value.window_id).is_some(),
            target_window_title_present: target
                .and_then(|value| value.window_title.as_ref())
                .is_some(),
            target_window_bounds_present: target
                .and_then(|value| value.window_bounds_global.as_ref())
                .is_some(),
            editor_present: editor.is_some(),
            editor_role_present: editor.and_then(|value| value.role.as_ref()).is_some(),
            editor_editable: editor.and_then(|value| value.editable),
            editor_secure: editor.is_some_and(|value| value.secure),
            editor_selection_present: editor
                .and_then(|value| value.selection_range.as_ref())
                .is_some(),
            editor_selected_chars: option_chars(
                editor.and_then(|value| value.selected_text.as_deref()),
            ),
            editor_prefix_chars: option_chars(
                editor.and_then(|value| value.cursor_prefix.as_deref()),
            ),
            editor_suffix_chars: option_chars(
                editor.and_then(|value| value.cursor_suffix.as_deref()),
            ),
            editor_full_field_chars: option_chars(
                editor.and_then(|value| value.full_field_text.as_deref()),
            ),
            editor_nearby_before_chars: option_chars(
                editor.and_then(|value| value.nearby_before.as_deref()),
            ),
            editor_nearby_after_chars: option_chars(
                editor.and_then(|value| value.nearby_after.as_deref()),
            ),
            editor_truncated: editor.is_some_and(|value| value.truncated),
            ax_visible_present: manifest.ax_visible.is_some(),
            ax_visited_nodes: manifest
                .ax_visible
                .as_ref()
                .map_or(0, |value| value.visited_nodes),
            ax_text_nodes,
            ax_text_chars,
            ax_truncated_nodes: manifest
                .ax_visible
                .as_ref()
                .map_or(0, |value| value.truncated_nodes),
            ax_truncated_chars: manifest
                .ax_visible
                .as_ref()
                .map_or(0, |value| value.truncated_chars),
            browser_present: browser.is_some(),
            browser_url_present: browser.and_then(|value| value.url.as_ref()).is_some(),
            browser_focused_element_present: browser
                .and_then(|value| value.focused_element.as_ref())
                .is_some(),
            browser_focused_secure: browser
                .and_then(|value| value.focused_element.as_ref())
                .is_some_and(|value| value.secure),
            browser_focused_value_chars: option_chars(
                browser
                    .and_then(|value| value.focused_element.as_ref())
                    .and_then(|value| value.value.as_deref()),
            ),
            browser_viewport_blocks: browser.map_or(0, |value| value.viewport_text_blocks.len()),
            browser_viewport_chars: browser.map_or(0, |value| {
                value
                    .viewport_text_blocks
                    .iter()
                    .map(|block| block.text.chars().count())
                    .sum()
            }),
            browser_truncated: browser.is_some_and(|value| value.truncated),
            screenshot_documents: manifest.screenshots.len(),
            ocr_documents: manifest.ocr_documents.len(),
            ocr_text_chars: manifest
                .ocr_documents
                .iter()
                .map(|document| document.text.chars().count())
                .sum(),
            ocr_boxes: manifest
                .ocr_documents
                .iter()
                .map(|document| document.boxes.len())
                .sum(),
            visible_text_blocks: manifest
                .visible_text_fused
                .as_ref()
                .map_or(0, |document| document.blocks.len()),
            visible_text_chars: manifest.visible_text_fused.as_ref().map_or(0, |document| {
                document
                    .blocks
                    .iter()
                    .map(|block| block.text.chars().count())
                    .sum()
            }),
            artifact_descriptors: manifest.artifacts.len(),
            payload_count: snapshot.payloads.len(),
            payload_bytes: manifest.diagnostics.payload_bytes,
            raw_text_allowed: manifest.privacy.raw_text_allowed,
            screenshots_allowed: manifest.privacy.screenshots_allowed,
            applied_privacy_gates: manifest.privacy.applied_gates.len(),
        }
    }
}

fn observe_ax_node(node: &AxNode, text_nodes: &mut usize, text_chars: &mut usize) {
    let chars = [
        &node.title,
        &node.value,
        &node.description,
        &node.placeholder,
    ]
    .into_iter()
    .flatten()
    .map(|value| value.chars().count())
    .sum::<usize>();
    if chars > 0 {
        *text_nodes += 1;
        *text_chars += chars;
    }
    for child in &node.children {
        observe_ax_node(child, text_nodes, text_chars);
    }
}

fn option_chars(value: Option<&str>) -> usize {
    value.map_or(0, |value| value.chars().count())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessSourceSample {
    pub state: SourceState,
    pub queue_wait_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub truncated_nodes: u64,
    pub truncated_chars: u64,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessSample {
    pub begin_duration_micros: u64,
    pub total_duration_ms: u64,
    pub payload_bytes: u64,
    pub terminal: bool,
    pub late_after_freeze: bool,
    pub sources: BTreeMap<SourceKind, ReadinessSourceSample>,
}

impl ReadinessSample {
    pub fn from_snapshot(snapshot: &ContextSnapshot, begin_duration_micros: u64) -> Self {
        Self {
            begin_duration_micros,
            total_duration_ms: snapshot
                .manifest
                .diagnostics
                .total_duration_ms
                .unwrap_or_default(),
            payload_bytes: snapshot.manifest.diagnostics.payload_bytes,
            terminal: snapshot.manifest.all_requested_sources_terminal(),
            late_after_freeze: false,
            sources: snapshot
                .manifest
                .source_status
                .iter()
                .map(|(source, status)| {
                    (
                        *source,
                        ReadinessSourceSample {
                            state: status.state,
                            queue_wait_ms: status.queue_wait_ms,
                            duration_ms: status.duration_ms,
                            truncated_nodes: status.truncated_nodes,
                            truncated_chars: status.truncated_chars,
                            reason_code: status.reason_code.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadinessReport {
    pub sample_count: usize,
    pub terminal_count: usize,
    pub late_drain_count: usize,
    pub begin_micros_p50: u64,
    pub begin_micros_p95: u64,
    pub begin_micros_max: u64,
    pub total_ms_p50: u64,
    pub total_ms_p95: u64,
    pub total_ms_max: u64,
    pub payload_bytes_max: u64,
    pub source_states: BTreeMap<String, u64>,
    pub source_reasons: BTreeMap<String, u64>,
    pub source_duration_ms_p95: BTreeMap<String, u64>,
    pub source_queue_wait_ms_p95: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
pub struct ReadinessAccumulator {
    sample_count: usize,
    terminal_count: usize,
    late_drain_count: usize,
    begin: BoundedSamples,
    total: BoundedSamples,
    payload_bytes_max: u64,
    states: BTreeMap<String, u64>,
    reasons: BTreeMap<String, u64>,
    durations: BTreeMap<String, BoundedSamples>,
    queues: BTreeMap<String, BoundedSamples>,
}

const READINESS_SAMPLE_RESERVOIR: usize = 8_192;

#[derive(Debug, Default)]
struct BoundedSamples {
    values: Vec<u64>,
    seen: u64,
    max: u64,
}

impl BoundedSamples {
    fn push(&mut self, value: u64) {
        self.seen = self.seen.saturating_add(1);
        self.max = self.max.max(value);
        if self.values.len() < READINESS_SAMPLE_RESERVOIR {
            self.values.push(value);
            return;
        }
        let slot = splitmix64(self.seen) % self.seen;
        if slot < READINESS_SAMPLE_RESERVOIR as u64 {
            self.values[slot as usize] = value;
        }
    }

    fn percentile(mut self, percentile_value: usize) -> u64 {
        self.values.sort_unstable();
        percentile(&self.values, percentile_value)
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

impl ReadinessAccumulator {
    pub fn push(&mut self, sample: &ReadinessSample) {
        self.sample_count += 1;
        self.terminal_count += usize::from(sample.terminal);
        self.late_drain_count += usize::from(sample.late_after_freeze);
        self.begin.push(sample.begin_duration_micros);
        self.total.push(sample.total_duration_ms);
        self.payload_bytes_max = self.payload_bytes_max.max(sample.payload_bytes);
        for (source, status) in &sample.sources {
            *self
                .states
                .entry(format!("{source:?}.{:?}", status.state).to_ascii_lowercase())
                .or_default() += 1;
            if let Some(reason) = status.reason_code.as_ref() {
                *self
                    .reasons
                    .entry(format!("{source:?}.{reason}").to_ascii_lowercase())
                    .or_default() += 1;
            }
            if let Some(duration) = status.duration_ms {
                self.durations
                    .entry(format!("{source:?}").to_ascii_lowercase())
                    .or_default()
                    .push(duration);
            }
            if let Some(queue) = status.queue_wait_ms {
                self.queues
                    .entry(format!("{source:?}").to_ascii_lowercase())
                    .or_default()
                    .push(queue);
            }
        }
    }

    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    pub fn finish(self) -> ReadinessReport {
        let begin_max = self.begin.max;
        let total_max = self.total.max;
        let mut begin_values = self.begin.values;
        let mut total_values = self.total.values;
        begin_values.sort_unstable();
        total_values.sort_unstable();
        ReadinessReport {
            sample_count: self.sample_count,
            terminal_count: self.terminal_count,
            late_drain_count: self.late_drain_count,
            begin_micros_p50: percentile(&begin_values, 50),
            begin_micros_p95: percentile(&begin_values, 95),
            begin_micros_max: begin_max,
            total_ms_p50: percentile(&total_values, 50),
            total_ms_p95: percentile(&total_values, 95),
            total_ms_max: total_max,
            payload_bytes_max: self.payload_bytes_max,
            source_states: self.states,
            source_reasons: self.reasons,
            source_duration_ms_p95: aggregate_percentiles(self.durations),
            source_queue_wait_ms_p95: aggregate_percentiles(self.queues),
        }
    }
}

pub fn build_readiness_report(samples: &[ReadinessSample]) -> ReadinessReport {
    let mut accumulator = ReadinessAccumulator::default();
    for sample in samples {
        accumulator.push(sample);
    }
    accumulator.finish()
}

fn aggregate_percentiles(values: BTreeMap<String, BoundedSamples>) -> BTreeMap<String, u64> {
    values
        .into_iter()
        .map(|(source, values)| (source, values.percentile(95)))
        .collect()
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_percentiles_are_deterministic_and_redacted() {
        let samples = (1..=20)
            .map(|value| ReadinessSample {
                begin_duration_micros: value,
                total_duration_ms: value * 10,
                payload_bytes: value * 100,
                terminal: true,
                late_after_freeze: false,
                sources: BTreeMap::new(),
            })
            .collect::<Vec<_>>();
        let report = build_readiness_report(&samples);
        assert_eq!(report.sample_count, 20);
        assert_eq!(report.begin_micros_p50, 10);
        assert_eq!(report.begin_micros_p95, 19);
        assert_eq!(report.total_ms_max, 200);
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("text"));
        assert!(!encoded.contains("title"));
    }

    #[test]
    fn streaming_accumulator_matches_batch_report() {
        let samples = (1..=50)
            .map(|value| ReadinessSample {
                begin_duration_micros: value * 2,
                total_duration_ms: value,
                payload_bytes: value * 3,
                terminal: value % 7 != 0,
                late_after_freeze: value % 7 == 0,
                sources: BTreeMap::new(),
            })
            .collect::<Vec<_>>();
        let expected = build_readiness_report(&samples);
        let mut accumulator = ReadinessAccumulator::default();
        for sample in &samples {
            accumulator.push(sample);
        }
        assert_eq!(accumulator.sample_count(), samples.len());
        assert_eq!(
            serde_json::to_value(accumulator.finish()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn streaming_reservoir_is_bounded_and_deterministic() {
        let mut first = BoundedSamples::default();
        let mut second = BoundedSamples::default();
        for value in 0..(READINESS_SAMPLE_RESERVOIR as u64 * 4) {
            first.push(value);
            second.push(value);
        }

        assert_eq!(first.values.len(), READINESS_SAMPLE_RESERVOIR);
        assert_eq!(first.values, second.values);
        assert_eq!(first.max, READINESS_SAMPLE_RESERVOIR as u64 * 4 - 1);
    }

    #[test]
    fn capability_observation_contains_only_presence_and_counts() {
        let manifest: crate::ContextManifest =
            serde_json::from_str(include_str!("../tests/fixtures/context_snapshot_v1.json"))
                .unwrap();
        let observation = CapabilityObservation::from_snapshot(&ContextSnapshot {
            manifest,
            payloads: Vec::new(),
        });
        assert!(observation.target_present);
        assert!(observation.target_bundle_id_present);
        let encoded = serde_json::to_string(&observation).unwrap();
        assert!(!encoded.contains("Fixture"));
        assert!(!encoded.contains("org.lumen.fixture"));
    }
}
