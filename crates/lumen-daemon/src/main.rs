//! Lumen Navi local daemon.
//!
//! Phase S0: wire config + platform stubs + media source shells + store smoke.
//! Real capture loops and control-plane server land in S1–S2.

use std::sync::Arc;

use anyhow::Result;
use lumen_api::{HealthResponse, SourceStatus};
use lumen_config::Config;
use lumen_intake::{drain_once, Source};
use lumen_platform::{NullFrontmost, NullPermissions, PermissionProbe};
use lumen_platform_macos::MacScreenCapturer;
use lumen_sources_media::{AudioSource, ScreenSource};
use lumen_store::{EventStore, MemoryEventStore, StoreSink};
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
        "daemon starting (phase S0 skeleton)"
    );

    let config = Config::load_or_default("navi.toml").unwrap_or_default();
    info!(
        data_dir = %config.data_dir.display(),
        screen = config.sources.screen,
        audio = config.sources.audio,
        browser = config.sources.browser,
        "config loaded (media-first defaults; browser off)"
    );

    let perms = NullPermissions;
    let status = perms.status().await?;
    info!(
        screen = ?status.screen_recording,
        mic = ?status.microphone,
        "permission probe (stub until signed macOS capture)"
    );

    let store = Arc::new(MemoryEventStore::default());
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
        if let Some(st) = statuses.iter_mut().find(|s| s.id == source.id()) {
            st.running = false;
        }
    }

    let health = HealthResponse::scaffold(statuses, store.len().await?, false);
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
