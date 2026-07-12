//! Lumen Navi local daemon entrypoint.
//!
//! Phase 0: boots logging, constructs store + null sources, and exits cleanly
//! after a smoke cycle. Real capture loops land in later phases.

use std::sync::Arc;

use anyhow::Result;
use lumen_intake::{drain_once, NullSource, Source};
use lumen_store::{EventStore, MemoryEventStore, StoreSink};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("lumen-navi daemon starting (phase 0 scaffold)");

    let store = Arc::new(MemoryEventStore::default());
    let sink = StoreSink::new(Arc::clone(&store));

    // Placeholder sources — real adapters (browser, screen, audio, …) replace these.
    let mut sources: Vec<Box<dyn Source>> = vec![
        Box::new(NullSource::new("screen")),
        Box::new(NullSource::new("browser")),
        Box::new(NullSource::new("audio")),
    ];

    for source in sources.iter_mut() {
        source.start().await?;
        let n = drain_once(source.as_mut(), &sink).await?;
        info!(source = source.id(), events = n, "poll complete");
        source.stop().await?;
    }

    info!(
        stored = store.len().await?,
        "smoke cycle done — no real capture enabled yet"
    );
    info!("lumen-navi daemon exiting cleanly");
    Ok(())
}
