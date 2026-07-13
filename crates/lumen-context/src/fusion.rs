use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::json;
use uuid::Uuid;

use crate::session::{ContextSource, SourceError, SourceInput};
use crate::{
    ArtifactPayload, ArtifactRef, AxNode, CaptureRequest, CapturedArtifact, OcrDocument, Rect,
    ScreenshotDocument, SourceCapture, SourceKind, VisibleTextBlock, VisibleTextDocument,
};

pub(crate) struct VisibleTextFusionSource;

const OPTIONAL_INPUTS: &[SourceKind] = &[
    SourceKind::EditorAx,
    SourceKind::AxVisible,
    SourceKind::Browser,
    SourceKind::ScreenshotElement,
    SourceKind::ScreenshotWindow,
    SourceKind::ScreenshotDisplays,
    SourceKind::OcrElement,
    SourceKind::OcrWindow,
    SourceKind::OcrDisplays,
];

#[async_trait]
impl ContextSource for VisibleTextFusionSource {
    fn kind(&self) -> SourceKind {
        SourceKind::VisibleTextFusion
    }

    fn optional_dependencies(&self) -> &'static [SourceKind] {
        OPTIONAL_INPUTS
    }

    async fn capture(
        &self,
        request: &CaptureRequest,
        input: SourceInput,
    ) -> Result<SourceCapture, SourceError> {
        let document = fuse_visible_text(&input, request.requested_at);
        let encoded = serde_json::to_vec(&document).map_err(|error| {
            SourceError::failed(
                "visible_text_serialize_failed",
                format!("failed to serialize Visible Text: {error}"),
                false,
            )
        })?;
        let artifact_id = Uuid::new_v4();
        let hash = blake3::hash(&encoded).to_hex().to_string();
        let empty = document.blocks.is_empty();
        Ok(SourceCapture {
            visible_text_fused: Some(document),
            artifacts: vec![CapturedArtifact {
                descriptor: ArtifactRef {
                    artifact_id,
                    source: SourceKind::VisibleTextFusion,
                    kind: "visible_text_v1".to_owned(),
                    content_hash: hash,
                    media_type: "application/json".to_owned(),
                    bytes: encoded.len() as u64,
                    metadata: json!({ "schema_version": 1 }),
                },
                payload: ArtifactPayload::Bytes {
                    media_type: "application/json".to_owned(),
                    bytes: Bytes::from(encoded),
                },
            }],
            empty,
            ..SourceCapture::default()
        })
    }
}

#[derive(Debug)]
struct Candidate {
    block: VisibleTextBlock,
    priority: u8,
    sequence: usize,
}

fn fuse_visible_text(
    input: &SourceInput,
    generated_at: chrono::DateTime<chrono::Utc>,
) -> VisibleTextDocument {
    let mut candidates = Vec::new();
    let mut sequence = 0;
    if let Some(browser) = &input.browser {
        if let Some(value) = browser
            .focused_element
            .as_ref()
            .and_then(|element| element.value.as_ref())
        {
            push_candidate(
                &mut candidates,
                value,
                vec!["browser.focused_element.value".to_owned()],
                browser
                    .focused_element
                    .as_ref()
                    .and_then(|element| element.bounding_rect.clone()),
                browser
                    .focused_element
                    .as_ref()
                    .and_then(|element| element.coordinate_space.as_deref()),
                Some("editor".to_owned()),
                None,
                0,
                &mut sequence,
            );
        }
        for (index, block) in browser.viewport_text_blocks.iter().enumerate() {
            let mut block = block.clone();
            block
                .source_refs
                .push(format!("browser.viewport_text_blocks[{index}]"));
            candidates.push(Candidate {
                block,
                priority: 0,
                sequence,
            });
            sequence += 1;
        }
    }
    if let Some(editor) = &input.editor {
        if let Some(value) = &editor.full_field_text {
            push_candidate(
                &mut candidates,
                value,
                vec!["editor.full_field_text".to_owned()],
                editor.bounds_global.clone(),
                Some("global_points"),
                Some("editor".to_owned()),
                None,
                1,
                &mut sequence,
            );
        }
    }
    if let Some(visible) = &input.ax_visible {
        for root in &visible.roots {
            collect_ax(root, &mut candidates, &mut sequence);
        }
    }
    for document in &input.ocr_documents {
        collect_ocr(document, &input.screenshots, &mut candidates, &mut sequence);
    }

    candidates.sort_by_key(|candidate| (candidate.priority, candidate.sequence));
    let mut blocks: Vec<VisibleTextBlock> = Vec::new();
    let mut by_text: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for candidate in candidates {
        let normalized = normalize_text(&candidate.block.text);
        if normalized.is_empty() {
            continue;
        }
        let duplicate_index = by_text.get(&normalized).and_then(|indices| {
            indices.iter().copied().find(|index| {
                compatible_bounds(
                    blocks[*index].global_bounds.as_ref(),
                    candidate.block.global_bounds.as_ref(),
                )
            })
        });
        if let Some(index) = duplicate_index {
            let existing = &mut blocks[index];
            let mut refs: BTreeSet<_> = existing.source_refs.iter().cloned().collect();
            refs.extend(candidate.block.source_refs);
            existing.source_refs = refs.into_iter().collect();
            existing.confidence = max_confidence(existing.confidence, candidate.block.confidence);
            existing.duplicate_group_id.get_or_insert_with(|| {
                format!(
                    "dup:{}",
                    &blake3::hash(normalize_text(&existing.text).as_bytes()).to_hex()[..12]
                )
            });
        } else {
            let index = blocks.len();
            blocks.push(candidate.block);
            by_text.entry(normalized).or_default().push(index);
        }
    }
    mark_conflicts(&mut blocks);
    for (order, block) in blocks.iter_mut().enumerate() {
        block.order = order;
    }
    VisibleTextDocument {
        blocks,
        generated_at: Some(generated_at),
        policy_version: 1,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_candidate(
    candidates: &mut Vec<Candidate>,
    text: &str,
    source_refs: Vec<String>,
    bounds: Option<Rect>,
    coordinate_space: Option<&str>,
    semantic_role: Option<String>,
    confidence: Option<f64>,
    priority: u8,
    sequence: &mut usize,
) {
    if text.trim().is_empty() {
        return;
    }
    candidates.push(Candidate {
        block: VisibleTextBlock {
            text: text.to_owned(),
            source_refs,
            global_bounds: bounds,
            coordinate_space: coordinate_space.map(str::to_owned),
            semantic_role,
            order: 0,
            confidence,
            duplicate_group_id: None,
            conflict_group_id: None,
        },
        priority,
        sequence: *sequence,
    });
    *sequence += 1;
}

fn collect_ax(node: &AxNode, candidates: &mut Vec<Candidate>, sequence: &mut usize) {
    let fields = [
        ("title", node.title.as_deref()),
        ("value", node.value.as_deref()),
        ("description", node.description.as_deref()),
        ("placeholder", node.placeholder.as_deref()),
    ];
    for (field, value) in fields {
        if let Some(value) = value {
            push_candidate(
                candidates,
                value,
                vec![format!("ax:{}:{field}", node.stable_path)],
                node.bounds_global.clone(),
                Some("global_points"),
                node.role.clone(),
                None,
                1,
                sequence,
            );
        }
    }
    for child in &node.children {
        collect_ax(child, candidates, sequence);
    }
}

fn collect_ocr(
    document: &OcrDocument,
    screenshots: &[ScreenshotDocument],
    candidates: &mut Vec<Candidate>,
    sequence: &mut usize,
) {
    let screenshot = screenshots
        .iter()
        .find(|screenshot| screenshot.artifact_id == document.screenshot_artifact_id);
    if document.boxes.is_empty() {
        push_candidate(
            candidates,
            &document.text,
            vec![format!("ocr:{}:text", document.document_id)],
            screenshot.and_then(|screenshot| screenshot.global_bounds.clone()),
            Some("global_points"),
            Some("ocr_text".to_owned()),
            Some(document.confidence),
            2,
            sequence,
        );
        return;
    }
    for (index, region) in document.boxes.iter().enumerate() {
        let bounds = screenshot
            .and_then(|screenshot| screenshot.global_bounds.as_ref())
            .map(|image| Rect {
                x: image.x + region.normalized_box.x * image.width,
                y: image.y
                    + (1.0 - region.normalized_box.y - region.normalized_box.height) * image.height,
                width: region.normalized_box.width * image.width,
                height: region.normalized_box.height * image.height,
            });
        push_candidate(
            candidates,
            &region.text,
            vec![format!("ocr:{}:boxes[{index}]", document.document_id)],
            bounds,
            Some("global_points"),
            Some("ocr_text".to_owned()),
            Some(region.confidence),
            2,
            sequence,
        );
    }
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn compatible_bounds(left: Option<&Rect>, right: Option<&Rect>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => intersection_over_union(left, right) >= 0.5,
        _ => false,
    }
}

fn intersection_over_union(left: &Rect, right: &Rect) -> f64 {
    let x1 = left.x.max(right.x);
    let y1 = left.y.max(right.y);
    let x2 = (left.x + left.width).min(right.x + right.width);
    let y2 = (left.y + left.height).min(right.y + right.height);
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = left.width * left.height + right.width * right.height - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn max_confidence(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn mark_conflicts(blocks: &mut [VisibleTextBlock]) {
    const BUCKET_POINTS: f64 = 128.0;
    let mut buckets: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (index, block) in blocks.iter().enumerate() {
        let Some(bounds) = block.global_bounds.as_ref() else {
            continue;
        };
        let center_x = bounds.x + bounds.width / 2.0;
        let center_y = bounds.y + bounds.height / 2.0;
        buckets
            .entry((
                (center_x / BUCKET_POINTS).floor() as i64,
                (center_y / BUCKET_POINTS).floor() as i64,
            ))
            .or_default()
            .push(index);
    }
    let mut pairs = BTreeSet::new();
    for (&(bucket_x, bucket_y), indices) in &buckets {
        for neighbor_x in bucket_x - 1..=bucket_x + 1 {
            for neighbor_y in bucket_y - 1..=bucket_y + 1 {
                let Some(neighbors) = buckets.get(&(neighbor_x, neighbor_y)) else {
                    continue;
                };
                for &left_index in indices {
                    for &right_index in neighbors {
                        if left_index != right_index {
                            pairs
                                .insert((left_index.min(right_index), left_index.max(right_index)));
                        }
                    }
                }
            }
        }
    }
    for (left_index, right_index) in pairs {
        if normalize_text(&blocks[left_index].text) == normalize_text(&blocks[right_index].text)
            || intersection_over_optional(
                blocks[left_index].global_bounds.as_ref(),
                blocks[right_index].global_bounds.as_ref(),
            ) < 0.5
        {
            continue;
        }
        let mut texts = [
            normalize_text(&blocks[left_index].text),
            normalize_text(&blocks[right_index].text),
        ];
        texts.sort();
        let id = format!(
            "conflict:{}",
            &blake3::hash(texts.join("\n").as_bytes()).to_hex()[..12]
        );
        blocks[left_index].conflict_group_id = Some(id.clone());
        blocks[right_index].conflict_group_id = Some(id);
    }
}

fn intersection_over_optional(left: Option<&Rect>, right: Option<&Rect>) -> f64 {
    match (left, right) {
        (Some(left), Some(right)) => intersection_over_union(left, right),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AxVisibleContext, BrowserContext, BrowserElementContext, EditorContext, OcrRegion,
    };
    use chrono::TimeZone;

    #[test]
    fn fusion_is_deterministic_and_preserves_provenance() {
        let bounds = Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 30.0,
        };
        let input = SourceInput {
            editor: Some(EditorContext {
                full_field_text: Some("Hello world".to_owned()),
                bounds_global: Some(bounds.clone()),
                ..EditorContext::default()
            }),
            browser: Some(BrowserContext {
                focused_element: Some(BrowserElementContext {
                    value: Some("Hello   world".to_owned()),
                    bounding_rect: Some(bounds.clone()),
                    ..BrowserElementContext::default()
                }),
                ..BrowserContext::default()
            }),
            ax_visible: Some(AxVisibleContext::default()),
            ..SourceInput::default()
        };
        let timestamp = chrono::Utc.with_ymd_and_hms(2026, 7, 12, 12, 0, 0).unwrap();
        let first = fuse_visible_text(&input, timestamp);
        let second = fuse_visible_text(&input, timestamp);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(first.blocks.len(), 1);
        assert_eq!(first.blocks[0].source_refs.len(), 2);
        assert!(first.blocks[0].duplicate_group_id.is_some());
    }

    #[test]
    fn ocr_boxes_map_into_global_coordinates() {
        let screenshot_id = Uuid::new_v4();
        let input = SourceInput {
            screenshots: vec![ScreenshotDocument {
                artifact_id: screenshot_id,
                kind: crate::ScreenshotKind::ActiveWindow,
                display_id: Some(1),
                window_id: Some(2),
                global_bounds: Some(Rect {
                    x: 100.0,
                    y: 200.0,
                    width: 400.0,
                    height: 300.0,
                }),
                pixel_bounds: None,
                scale: 2.0,
                width: 800,
                height: 600,
                color_space: None,
                media_type: "image/png".to_owned(),
                content_hash: "fixture".to_owned(),
                captured_at: chrono::Utc::now(),
                duration_ms: 1,
                occluded: None,
                cropped: true,
                capture_method: Some("fixture".to_owned()),
                capture_fallback_reason: None,
            }],
            ocr_documents: vec![OcrDocument {
                document_id: Uuid::new_v4(),
                screenshot_artifact_id: screenshot_id,
                engine: "fixture".to_owned(),
                engine_version: None,
                binary_hash: None,
                mode: "fast".to_owned(),
                languages: vec![],
                custom_words: vec![],
                language_correction: None,
                text: "Box".to_owned(),
                confidence: 0.9,
                boxes: vec![OcrRegion {
                    text: "Box".to_owned(),
                    confidence: 0.9,
                    normalized_box: Rect {
                        x: 0.25,
                        y: 0.5,
                        width: 0.5,
                        height: 0.25,
                    },
                    pixel_box: None,
                }],
                reading_order: vec![0],
                duration_ms: 1,
                queue_wait_ms: 0,
                captured_at: chrono::Utc::now(),
                completed_at: chrono::Utc::now(),
            }],
            ..SourceInput::default()
        };
        let fused = fuse_visible_text(&input, chrono::Utc::now());
        let bounds = fused.blocks[0].global_bounds.as_ref().unwrap();
        assert_eq!(bounds.x, 200.0);
        assert_eq!(bounds.y, 275.0);
        assert_eq!(bounds.width, 200.0);
        assert_eq!(bounds.height, 75.0);
    }
}
