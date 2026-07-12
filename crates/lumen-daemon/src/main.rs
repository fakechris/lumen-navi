//! Lumen Navi local daemon.
//!
//! Phase S1: durable SQLite + CA blob store under `data_dir`.
//! Real capture loops land in S2+.

use std::sync::Arc;

use anyhow::{Context, Result};
use lumen_api::{HealthResponse, SourceStatus};
use lumen_config::Config;
use lumen_intake::{drain_once, Source};
use lumen_platform::{NullFrontmost, NullPermissions, PermissionProbe};
use lumen_platform_macos::MacScreenCapturer;
use lumen_sources_media::{AudioSource, ScreenSource};
use lumen_store::{EventStore, SqliteStore, StoreSink};
use lumen_types::{event_kind, SourceEvent, SourceKind};
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
        phase = "S1",
        "daemon starting"
    );

    let config = Config::load_or_default("navi.toml").unwrap_or_default();
    info!(
        data_dir = %config.data_dir.display(),
        screen = config.sources.screen,
        audio = config.sources.audio,
        browser = config.sources.browser,
        "config loaded (media-first defaults; browser off)"
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

    // Boot marker proves durability across process restarts (survives in navi.db).
    let boot = SourceEvent::new(
        SourceKind::Other("daemon".into()),
        "daemon.boot.v1",
        json!({
            "payload_version": 1,
            "phase": "S1",
            "note": "process start marker"
        }),
    );
    store.append(vec![boot]).await?;
    // Tiny CA blob attached to a synthetic screen-shaped event (pipeline check).
    let smoke = SourceEvent::new(
        SourceKind::Screen,
        event_kind::SCREENSHOT_V1,
        json!({
            "payload_version": 1,
            "reason": "daemon_smoke",
        }),
    );
    let smoke = store.put_and_append(smoke, "application/octet-stream", b"lumen-navi-s1-smoke")?;
    store.enqueue_job(smoke.id, "ocr_screen")?;

    let perms = NullPermissions;
    let status = perms.status().await?;
    info!(
        screen = ?status.screen_recording,
        mic = ?status.microphone,
        "permission probe (stub until signed macOS capture)"
    );

    let sink = StoreSink::new(Arc::clone(&store));
    let mut sources: Vec<Box<dyn Source>> = Vec::new();
    let mut statuses: Vec<SourceStatus> = Vec::new();

    if config.sources.screen {
        let screen = ScreenSource::new(
            Arc::new(MacScreenCapturer),
            Arc::new(NullFrontmost),
            &config.capture,
        );
        sources.push(Box::new(screen));
        statuses.push(SourceStatus {
            id: "screen".into(),
            enabled: true,
            running: false,
            last_error: None,
        });
    }

    if config.sources.audio {
        sources.push(Box::new(AudioSource::new()));
        statuses.push(SourceStatus {
            id: "audio".into(),
            enabled: true,
            running: false,
            last_error: None,
        });
    }

    if config.sources.browser {
        warn!("browser source enabled in config but not implemented until Phase B1");
    }

    for source in sources.iter_mut() {
        source.start().await?;
        let n = drain_once(source.as_mut(), &sink).await?;
        info!(source = source.id(), events = n, "poll complete");
        source.stop().await?;
    }

    let recent = store.list_recent(5).await?;
    for ev in &recent {
        info!(
            id = %ev.id,
            kind = %ev.kind,
            artifacts = ev.artifacts.len(),
            "recent event"
        );
    }

    let health = HealthResponse::scaffold(statuses, store.len().await?, false);
    info!(
        api_version = health.api_version,
        stored = health.stored_events,
        sources = health.sources.len(),
        jobs = store.list_jobs(10)?.len(),
        "health snapshot"
    );

    info!("related: Lumen ASR https://github.com/fakechris/lumen-asr (separate product)");
    info!("act plane later: cua-driver MIT only — https://github.com/trycua/cua");
    info!("lumen-navi daemon exiting cleanly");
    Ok(())
}
