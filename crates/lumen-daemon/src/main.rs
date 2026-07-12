//! Lumen Navi daemon — Observe capture + async Vision OCR.
//!
//! Capture never waits on OCR. OCR consumes `ocr_screen` jobs into `derived` ocr.v1.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use lumen_api::{HealthResponse, SourceStatus};
use lumen_config::Config;
use lumen_platform::{OcrEngine, PermissionProbe};
use lumen_platform_macos::{
    request_screen_recording, MacDisplays, MacFrontmost, MacPermissions, MacScreenCapturer,
    MacScreenLock, MacVisionOcr,
};
use lumen_process::{OcrWorker, OcrWorkerConfig};
use lumen_sources_media::{AudioSource, CaptureOrchestrator, CapturedBatch};
use lumen_store::{EventStore, SqliteStore};
use lumen_types::{SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tokio::sync::mpsc;
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
        phase = "S4-ocr",
        "daemon starting"
    );

    let config = Config::load_or_default("navi.toml").unwrap_or_default();
    info!(
        data_dir = %config.data_dir.display(),
        displays = %config.capture.displays,
        ocr = config.ocr.enabled,
        ocr_langs = ?config.ocr.languages,
        ticks = config.capture.screen_ticks,
        "config"
    );

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data_dir {}", config.data_dir.display()))?;

    let store = Arc::new(
        SqliteStore::open(&config.data_dir)
            .with_context(|| format!("open store {}", config.data_dir.display()))?,
    );
    info!(existing = store.len().await?, "durable store open");

    store
        .append(vec![SourceEvent::new(
            SourceKind::Other("daemon".into()),
            "daemon.boot.v1",
            json!({ "phase": "S4-ocr", "observe": true, "ocr": config.ocr.enabled }),
        )])
        .await?;

    let perms = MacPermissions;
    let mut status = perms.status().await?;
    info!(screen = ?status.screen_recording, "permissions");
    if config.sources.screen && !status.can_capture_screen() {
        let _ = request_screen_recording();
        status = perms.status().await?;
        info!(screen = ?status.screen_recording, "after request");
    }

    // --- OCR worker (async; independent of capture) ---
    let (ocr_cancel_tx, ocr_cancel_rx) = tokio::sync::watch::channel(false);
    let ocr_handle = if config.ocr.enabled {
        let engine = Arc::new(MacVisionOcr::new());
        if engine.is_supported() {
            let worker = Arc::new(OcrWorker::new(
                Arc::clone(&store),
                engine,
                OcrWorkerConfig {
                    languages: config.ocr.languages.clone(),
                    poll_interval: Duration::from_millis(config.ocr.poll_interval_ms),
                    batch_size: config.ocr.batch_size,
                    include_boxes: config.ocr.include_boxes,
                    max_attempts: 5,
                },
            ));
            let w = Arc::clone(&worker);
            let rx = ocr_cancel_rx.clone();
            Some((
                worker,
                tokio::spawn(async move {
                    w.run_until_cancelled(rx).await;
                }),
            ))
        } else {
            warn!("Vision OCR not supported on this OS; worker not started");
            None
        }
    } else {
        info!("OCR disabled in config");
        None
    };

    let mut screen_status = SourceStatus {
        id: "screen".into(),
        enabled: config.sources.screen,
        running: false,
        last_error: None,
    };

    if config.sources.screen {
        let mut orch = CaptureOrchestrator::new(
            Arc::new(MacDisplays),
            Arc::new(MacScreenCapturer),
            Arc::new(MacFrontmost),
            Arc::new(MacScreenLock),
            config.capture.clone(),
            config.privacy.clone(),
        );

        let (tx, mut rx) = mpsc::channel::<CapturedBatch>(config.capture.queue_capacity);
        let store_w = Arc::clone(&store);
        let ocr_on = config.ocr.enabled;
        let persist = tokio::spawn(async move {
            while let Some(batch) = rx.recv().await {
                if let Some(ref closed) = batch.closed_session {
                    let _ = store_w.upsert_session(closed);
                }
                if let Some(ref open) = batch.open_session {
                    let _ = store_w.upsert_session(open);
                }
                for (event, frame) in batch.frames {
                    match store_w.put_and_append(
                        event,
                        frame.media_type.clone(),
                        &frame.png_or_jpeg_bytes,
                    ) {
                        Ok(stored) => {
                            if ocr_on {
                                let _ = store_w.enqueue_job(stored.id, "ocr_screen");
                            }
                            info!(
                                id = %stored.id,
                                kind = %stored.kind,
                                media = %frame.media_type,
                                bytes = frame.png_or_jpeg_bytes.len(),
                                "persisted screenshot"
                            );
                        }
                        Err(e) => warn!(error = %e, "persist failed"),
                    }
                }
            }
        });

        screen_status.running = true;
        let interval = Duration::from_millis(config.capture.screen_interval_ms);
        let focus_every = Duration::from_millis(config.capture.focus_poll_ms);
        let max_ticks = config.capture.screen_ticks;
        let mut full_ticks = 0u64;
        let mut interval_ticks = 0u64;
        let mut focus_tick = tokio::time::interval(focus_every);
        focus_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut capture_tick = tokio::time::interval(interval);
        capture_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        focus_tick.tick().await;
        capture_tick.tick().await;

        info!("observe loop running (Ctrl+C to stop if ticks=0)");

        loop {
            if max_ticks > 0 && (full_ticks >= max_ticks || interval_ticks >= max_ticks) {
                break;
            }

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl+C");
                    break;
                }
                _ = focus_tick.tick() => {
                    if let Some(reason) = orch.poll_focus_trigger().await {
                        match orch.capture_tick(reason).await {
                            Ok(Some(batch)) => {
                                full_ticks += 1;
                                if tx.try_send(batch).is_err() {
                                    orch.note_backpressure_drop();
                                    warn!("backpressure: drop capture batch");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(error = %e, "focus capture failed");
                                screen_status.last_error = Some(e);
                            }
                        }
                    }
                    if let Some(closed) = orch.close_idle_session() {
                        let _ = store.upsert_session(&closed);
                    }
                }
                _ = capture_tick.tick() => {
                    interval_ticks += 1;
                    match orch.capture_tick(TriggerReason::Interval).await {
                        Ok(Some(batch)) => {
                            full_ticks += 1;
                            if tx.try_send(batch).is_err() {
                                orch.note_backpressure_drop();
                                warn!("backpressure: drop capture batch");
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(error = %e, "interval capture failed");
                            screen_status.last_error = Some(e);
                        }
                    }
                }
            }
        }

        if let Some(s) = orch.force_close_session() {
            let _ = store.upsert_session(&s);
        }
        drop(tx);
        let _ = persist.await;
        screen_status.running = false;

        let st = orch.stats();
        info!(
            full = st.full_captures,
            probes = st.probes,
            skip_visual = st.skipped_visual,
            skip_debounce = st.skipped_debounce,
            skip_gate = st.skipped_gate,
            drop_bp = st.dropped_backpressure,
            "capture stats"
        );
    }

    // Let OCR drain remaining jobs briefly after capture stops (finite runs).
    if let Some((worker, handle)) = ocr_handle {
        if config.capture.screen_ticks > 0 {
            for _ in 0..15u32 {
                let n: usize = worker.tick_once().await.unwrap_or(0);
                if n == 0 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let n2: usize = worker.tick_once().await.unwrap_or(0);
                    if n2 == 0 {
                        break;
                    }
                }
            }
        }
        let _ = ocr_cancel_tx.send(true);
        let _ = handle.await;
        let st = worker.stats();
        info!(
            processed = st.processed,
            succeeded = st.succeeded,
            empty = st.empty,
            failed = st.failed,
            "ocr stats"
        );
    }

    if config.sources.audio {
        let mut a = AudioSource::new();
        use lumen_intake::Source;
        a.start().await?;
        a.stop().await?;
    }

    let health = HealthResponse::scaffold(
        vec![screen_status],
        store.len().await?,
        config.privacy.paused,
    );
    info!(stored = health.stored_events, "health");
    Ok(())
}
