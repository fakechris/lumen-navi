use std::time::{Duration, Instant};

use chrono::Utc;
use lumen_context::{
    CapabilityObservation, CaptureId, CaptureProfile, CaptureRequest, CaptureTrigger,
    ContextCollector, ContextConfig, PrivacyPolicy, ReadinessAccumulator, ReadinessSample,
    SourceKind, SourceSelection, TriggerKind,
};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("context readiness probe failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let iterations = argument("--iterations")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 1_000_000);
    let duration = argument("--duration-secs")
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(1, 24 * 60 * 60)));
    let checkpoint_every = argument("--checkpoint-every")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| if duration.is_some() { 1_000 } else { 0 });
    let interval = Duration::from_millis(
        argument("--interval-ms")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            .min(60_000),
    );
    let observe_fields = has_argument("--observe-fields");
    let all_displays = has_argument("--all-displays");
    let mut privacy_policy = PrivacyPolicy {
        capture_raw_text: !has_argument("--disable-raw-text"),
        capture_screenshots: !has_argument("--disable-screenshots"),
        ..PrivacyPolicy::default()
    };
    if let Some(bundle_id) = argument("--deny-bundle-id") {
        privacy_policy.denied_bundle_ids.insert(bundle_id);
    }
    let profile = argument("--profile").unwrap_or_else(|| "metadata".to_owned());
    let deadline_ms = argument("--deadline-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2_000)
        .clamp(1, 60_000);
    let late_deadline_ms = argument("--late-deadline-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| deadline_ms.saturating_mul(2))
        .clamp(1, 60_000);
    let capture_profile = match profile.as_str() {
        "visible" => CaptureProfile::Visible,
        "vision" => CaptureProfile::Vision,
        "metadata" => CaptureProfile::Metadata,
        _ => return Err("--profile must be metadata, visible, or vision".to_owned()),
    };
    let sources = match capture_profile {
        CaptureProfile::Visible => SourceSelection::from_sources([
            SourceKind::Target,
            SourceKind::EditorAx,
            SourceKind::AxVisible,
            SourceKind::VisibleTextFusion,
        ]),
        CaptureProfile::Vision if all_displays => SourceSelection::from_sources([
            SourceKind::Target,
            SourceKind::ScreenshotDisplays,
            SourceKind::OcrDisplays,
            SourceKind::VisibleTextFusion,
        ]),
        CaptureProfile::Vision => SourceSelection::from_sources([
            SourceKind::Target,
            SourceKind::ScreenshotWindow,
            SourceKind::OcrWindow,
            SourceKind::VisibleTextFusion,
        ]),
        _ => SourceSelection::from_sources([SourceKind::Target]),
    };
    let collector = ContextCollector::new(
        ContextConfig {
            capture_all_displays: all_displays,
            screenshot_max_edge: 1_280,
            ocr_helper_path: std::env::var_os("LUMEN_CONTEXT_OCR_HELPER").map(Into::into),
            ..ContextConfig::default()
        },
        None,
    )
    .map_err(|error| error.to_string())?;
    let run_started = Instant::now();
    let mut accumulator = ReadinessAccumulator::default();
    let mut observation = None;
    let mut generation = 0_usize;
    loop {
        if generation > 0 {
            if duration.is_some_and(|limit| run_started.elapsed() >= limit) {
                break;
            }
            if duration.is_none() && generation >= iterations {
                break;
            }
        }
        generation += 1;
        let now = Utc::now();
        let begin_started = Instant::now();
        let session = collector
            .begin(CaptureRequest {
                capture_id: CaptureId::new(),
                consumer_session_id: Uuid::new_v4(),
                target_generation: generation as u64,
                profile: capture_profile,
                sources: sources.clone(),
                trigger: CaptureTrigger {
                    kind: TriggerKind::Test,
                    pressed_at: now,
                    released_at: None,
                },
                requested_at: now,
                target_hint: None,
                privacy_policy: privacy_policy.clone(),
            })
            .map_err(|error| error.to_string())?;
        let begin_micros = begin_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        let mut snapshot = session
            .snapshot(Instant::now() + Duration::from_millis(deadline_ms))
            .await;
        let late_after_freeze = !snapshot.manifest.all_requested_sources_terminal();
        if late_after_freeze {
            snapshot = session
                .snapshot(Instant::now() + Duration::from_millis(late_deadline_ms))
                .await;
        }
        if observe_fields {
            observation = Some(CapabilityObservation::from_snapshot(&snapshot));
        }
        let mut sample = ReadinessSample::from_snapshot(&snapshot, begin_micros);
        sample.late_after_freeze = late_after_freeze;
        accumulator.push(&sample);
        if checkpoint_every > 0 && generation.is_multiple_of(checkpoint_every) {
            eprintln!(
                "readiness checkpoint samples={} elapsed_ms={}",
                accumulator.sample_count(),
                run_started.elapsed().as_millis()
            );
        }
        if !interval.is_zero() {
            tokio::time::sleep(interval).await;
        }
    }
    let report = accumulator.finish();
    let output = if let Some(observation) = observation {
        serde_json::json!({ "report": report, "observation": observation })
    } else {
        serde_json::to_value(report).map_err(|error| error.to_string())?
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn argument(name: &str) -> Option<String> {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments.next();
        }
        if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
            return Some(value.to_owned());
        }
    }
    None
}

fn has_argument(name: &str) -> bool {
    std::env::args().skip(1).any(|argument| argument == name)
}
