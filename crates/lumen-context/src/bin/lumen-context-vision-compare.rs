use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use chrono::Utc;
use lumen_context::{
    ArtifactPayload, CaptureId, CaptureProfile, CaptureRequest, CaptureTrigger, ContextCollector,
    ContextConfig, OcrEngine, PrivacyPolicy, SourceKind, SourceSelection, TriggerKind,
};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Vision comparison failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let collector = ContextCollector::new(
        ContextConfig {
            capture_all_displays: false,
            screenshot_max_edge: 1_280,
            ..ContextConfig::default()
        },
        None,
    )
    .map_err(|error| error.to_string())?;
    let now = Utc::now();
    let session = collector
        .begin(CaptureRequest {
            capture_id: CaptureId::new(),
            consumer_session_id: Uuid::new_v4(),
            target_generation: 1,
            profile: CaptureProfile::Vision,
            sources: SourceSelection::from_sources([SourceKind::ScreenshotWindow]),
            trigger: CaptureTrigger {
                kind: TriggerKind::Test,
                pressed_at: now,
                released_at: None,
            },
            requested_at: now,
            target_hint: None,
            privacy_policy: PrivacyPolicy::default(),
        })
        .map_err(|error| error.to_string())?;
    let snapshot = session
        .snapshot(Instant::now() + Duration::from_secs(10))
        .await;
    let artifact = snapshot
        .payloads
        .iter()
        .find(|artifact| artifact.descriptor.media_type.starts_with("image/"))
        .ok_or_else(|| {
            let status = &snapshot.manifest.source_status[&SourceKind::ScreenshotWindow];
            format!(
                "screenshot unavailable: state={:?} reason={}",
                status.state,
                status.reason_code.as_deref().unwrap_or("none")
            )
        })?;
    let ArtifactPayload::Bytes { bytes, .. } = &artifact.payload else {
        return Err("screenshot payload was not imported into memory".to_owned());
    };
    let languages = lumen_context::macos::default_ocr_languages();
    let engine = lumen_context::macos::MacVisionOcr::new();

    let started = Instant::now();
    let fast = engine
        .recognize_text_fast(bytes, &languages)
        .await
        .map_err(|error| error.to_string())?;
    let fast_ms = elapsed_ms(started);
    let started = Instant::now();
    let accurate = engine
        .recognize_text(bytes, &languages)
        .await
        .map_err(|error| error.to_string())?;
    let accurate_ms = elapsed_ms(started);
    let started = Instant::now();
    let layout = engine
        .recognize_boxes(bytes, &languages)
        .await
        .map_err(|error| error.to_string())?;
    let layout_ms = elapsed_ms(started);

    let output = serde_json::json!({
        "image_bytes": bytes.len(),
        "fast": observation(&fast, fast_ms),
        "accurate": observation(&accurate, accurate_ms),
        "accurate_layout": observation(&layout, layout_ms),
        "fast_accurate_token_overlap": token_overlap(&fast.text, &accurate.text),
        "accurate_layout_token_overlap": token_overlap(&accurate.text, &layout.text),
        "text_redacted": true
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn observation(result: &lumen_context::OcrResult, duration_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "mode": result.mode,
        "duration_ms": duration_ms,
        "text_chars": result.text.chars().count(),
        "confidence": result.confidence,
        "boxes": result.boxes.len()
    })
}

fn token_overlap(left: &str, right: &str) -> f64 {
    let left = tokens(left);
    let right = tokens(right);
    let union = left.union(&right).count();
    if union == 0 {
        return 1.0;
    }
    left.intersection(&right).count() as f64 / union as f64
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .map(|token| token.to_lowercase())
        .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
