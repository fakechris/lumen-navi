//! Lumen Navi local daemon.
//!
//! Phase S2: real macOS screen capture → durable store (interval + dedup).

use std::sync::Arc;

use anyhow::{Context, Result};
use lumen_api::{HealthResponse, SourceStatus};
use lumen_config::Config;
use lumen_intake::Source;
use lumen_platform::PermissionProbe;
use lumen_platform_macos::{
    request_screen_recording, MacFrontmost, MacPermissions, MacScreenCapturer,
};
use lumen_sources_media::{AudioSource, ScreenSource};
use lumen_store::{EventStore, SqliteStore};
use lumen_types::{SourceEvent, SourceKind};
use serde_json::json;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!(
        product = "lumen-navi",
        repo = "https://github.com/fakechris/lumen-navi",
        phase = "S2",
        "daemon starting"
    );

    let config = Config::load_or_default("navi.toml").unwrap_or_default();
    info!(
        data_dir = %config.data_dir.display(),
        screen = config.sources.screen,
        audio = config.sources.audio,
        browser = config.sources.browser,
        interval_ms = config.capture.screen_interval_ms,
        ticks = config.capture.screen_ticks,
        max_edge = config.capture.screen_max_edge,
        "config loaded"
    );

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data_dir {}", config.data_dir.display()))?;

    let store = Arc::new(
        SqliteStore::open(&config.data_dir)
            .with_context(|| format!("open store in {}", config.data_dir.display()))?,
    );
    info!(
        db = %config.data_dir.join("meta/navi.db").display(),
        existing_events = store.len().await?,
        "durable store open"
    );

    let boot = SourceEvent::new(
        SourceKind::Other("daemon".into()),
        "daemon.boot.v1",
        json!({
            "payload_version": 1,
            "phase": "S2",
            "note": "process start marker"
        }),
    );
    store.append(vec![boot]).await?;

    let perms = MacPermissions;
    let mut status = perms.status().await?;
    info!(
        screen = ?status.screen_recording,
        mic = ?status.microphone,
        accessibility = ?status.accessibility,
        "permission probe"
    );

    if config.sources.screen && !status.can_capture_screen() {
        info!("requesting Screen Recording access (system prompt may appear)");
        let _ = request_screen_recording();
        status = perms.status().await?;
        info!(screen = ?status.screen_recording, "permission after request");
    }

    let mut screen_status = SourceStatus {
        id: "screen".into(),
        enabled: config.sources.screen,
        running: false,
        last_error: None,
    };
    let mut audio_status = SourceStatus {
        id: "audio".into(),
        enabled: config.sources.audio,
        running: false,
        last_error: None,
    };

    if config.sources.screen {
        let capturer = MacScreenCapturer::with_max_edge(config.capture.screen_max_edge);
        let mut screen = ScreenSource::new(
            Arc::new(capturer),
            Arc::new(MacFrontmost),
            &config.capture,
        );
        screen.start().await?;
        screen_status.running = true;

        let interval = screen.interval();
        let max_ticks = config.capture.screen_ticks;
        info!(
            ?interval,
            max_ticks,
            "screen capture loop (ticks=0 means until Ctrl+C)"
        );

        let mut tick: u64 = 0;
        loop {
            if max_ticks > 0 && tick >= max_ticks {
                break;
            }
            tick += 1;

            match screen.capture_tick().await {
                Ok(batch) => {
                    for item in batch {
                        if let Some((media, bytes)) = item.blob {
                            let stored = store.put_and_append(item.event, media, &bytes)?;
                            info!(
                                id = %stored.id,
                                kind = %stored.kind,
                                artifacts = stored.artifacts.len(),
                                "stored screenshot"
                            );
                        } else {
                            store.append(vec![item.event]).await?;
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "screen capture tick failed");
                    screen_status.last_error = Some(e.to_string());
                    // Keep looping — permission may be granted mid-run after Settings toggle.
                }
            }

            if max_ticks > 0 && tick >= max_ticks {
                break;
            }
            // Sleep, but wake early on Ctrl+C when running forever.
            if max_ticks == 0 {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        info!("Ctrl+C — stopping capture loop");
                        break;
                    }
                    _ = tokio::time::sleep(interval) => {}
                }
            } else {
                tokio::time::sleep(interval).await;
            }
        }

        screen.stop().await?;
        screen_status.running = false;
    }

    if config.sources.audio {
        // S3 will run audio; for now just register status.
        let mut audio = AudioSource::new();
        audio.start().await?;
        audio_status.running = true;
        audio.stop().await?;
        audio_status.running = false;
    }

    if config.sources.browser {
        warn!("browser source enabled in config but not implemented until Phase B1");
    }

    let recent = store.list_recent(8).await?;
    for ev in &recent {
        info!(
            id = %ev.id,
            kind = %ev.kind,
            artifacts = ev.artifacts.len(),
            "recent event"
        );
    }

    let health = HealthResponse::scaffold(
        vec![screen_status, audio_status],
        store.len().await?,
        false,
    );
    info!(
        api_version = health.api_version,
        stored = health.stored_events,
        sources = health.sources.len(),
        "health snapshot"
    );

    info!("related: Lumen ASR https://github.com/fakechris/lumen-asr (separate product)");
    info!("act plane later: cua-driver MIT only — https://github.com/trycua/cua");
    info!("lumen-navi daemon exiting cleanly");
    Ok(())
}
