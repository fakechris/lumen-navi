use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use lumen_context::{
    BrowserCaptureRequest, BrowserSnapshotProvider, CaptureId, NativeBrowserBridgeConfig,
    NativeBrowserProvider,
};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("browser e2e server failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let root = PathBuf::from(argument("--root").ok_or("--root is required")?);
    let origin = argument("--origin").ok_or("--origin is required")?;
    let config = NativeBrowserBridgeConfig::new(
        "lumen-asr",
        root.join("bridge.sock"),
        root.join("bridge.token"),
        [origin],
    );
    let provider = NativeBrowserProvider::bind(config.clone())
        .await
        .map_err(|error| error.to_string())?;
    config
        .write_host_config(&root.join("host.browser-host.json"))
        .map_err(|error| error.to_string())?;
    println!("READY");

    let connected_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !provider.is_connected().await {
        if tokio::time::Instant::now() >= connected_deadline {
            return Err("native browser host did not connect".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    let capture_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_main_textarea = false;
    let mut saw_contenteditable = false;
    let mut saw_iframe_textarea = false;
    let mut saw_all_frames = false;
    let mut saw_navigation = false;
    let mut saw_secure_redaction = false;
    loop {
        let now = Utc::now();
        let request = BrowserCaptureRequest {
            request_id: Uuid::new_v4(),
            capture_id: CaptureId::new(),
            target_generation: 1,
            target_hint: None,
            requested_at: now,
            deadline: now + chrono::Duration::seconds(3),
            max_chars: 10_000,
            max_nodes: 500,
            allow_private_browsing: false,
            denied_bundle_ids: BTreeSet::new(),
            denied_domains: BTreeSet::new(),
        };
        match provider.capture(request.clone()).await {
            Ok(snapshot) => {
                saw_all_frames |= snapshot.frame_status.len() >= 2
                    && snapshot.frame_status.iter().all(|frame| frame.captured);
                let context = match snapshot.validate_for(&request) {
                    Ok(context) => context,
                    Err(error) => {
                        if tokio::time::Instant::now() >= capture_deadline {
                            return Err(error.to_string());
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let focused = context.focused_element.as_ref();
                saw_main_textarea |= context.frame_id == Some(0)
                    && focused.is_some_and(|element| element.tag.as_deref() == Some("textarea"));
                saw_contenteditable |=
                    focused.is_some_and(|element| element.contenteditable == Some(true));
                saw_iframe_textarea |= context.frame_id.is_some_and(|frame_id| frame_id != 0)
                    && focused.is_some_and(|element| element.tag.as_deref() == Some("textarea"));
                saw_secure_redaction |= focused.is_some_and(|element| {
                    element.input_type.as_deref() == Some("password")
                        && element.secure
                        && element.value.is_none()
                        && element.selection_start.is_none()
                        && element.selection_end.is_none()
                });
                saw_navigation |= context
                    .url
                    .as_deref()
                    .is_some_and(|url| url.contains("/navigated"));
                if saw_main_textarea
                    && saw_contenteditable
                    && saw_iframe_textarea
                    && saw_all_frames
                    && saw_navigation
                    && saw_secure_redaction
                    && !context.viewport_text_blocks.is_empty()
                {
                    println!("PASS");
                    return Ok(());
                }
            }
            Err(error) => {
                if tokio::time::Instant::now() >= capture_deadline {
                    return Err(error.to_string());
                }
            }
        }
        if tokio::time::Instant::now() >= capture_deadline {
            return Err(format!(
                "browser matrix incomplete: main_textarea={saw_main_textarea} \
                 contenteditable={saw_contenteditable} iframe_textarea={saw_iframe_textarea} \
                 all_frames={saw_all_frames} navigation={saw_navigation} \
                 secure_redaction={saw_secure_redaction}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
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
