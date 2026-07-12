//! Lumen Navi daemon — Observe (screen + mic) + OCR + local control API.
//!
//! Screen and audio never wait on each other. OCR never blocks capture.

mod control_server;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use lumen_api::{HealthResponse, SourceStatus};
use lumen_config::{AudioConfig, Config, PrivacyConfig};
use lumen_platform::{MicCapturer, MicOpenConfig, OcrEngine, PermissionProbe};
use lumen_platform_macos::{
    request_screen_recording, MacDisplays, MacFrontmost, MacMicCapturer, MacPermissions,
    MacScreenCapturer, MacScreenLock, MacVisionOcr,
};
use lumen_process::{OcrWorker, OcrWorkerConfig};
use lumen_sources_media::{AudioOrchestrator, CaptureOrchestrator, CapturedBatch};
use lumen_store::{EventStore, SCHEMA_VERSION, SqliteStore};
use lumen_types::{SourceEvent, SourceKind, TriggerReason};
use serde_json::json;
use tokio::sync::{mpsc, watch};
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
        phase = "S3-audio",
        "daemon starting"
    );

    let config = Config::load_or_default("navi.toml").unwrap_or_default();
    info!(
        data_dir = %config.data_dir.display(),
        screen = config.sources.screen,
        audio = config.sources.audio,
        audio_mode = %config.audio.mode,
        audio_chunk_ms = config.audio.chunk_ms,
        ocr = config.ocr.enabled,
        ticks_screen = config.capture.screen_ticks,
        ticks_audio = config.audio.ticks,
        api = config.api.enabled,
        api_bind = %config.api.bind,
        "config"
    );

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data_dir {}", config.data_dir.display()))?;

    let store = Arc::new(
        SqliteStore::open(&config.data_dir)
            .with_context(|| format!("open store {}", config.data_dir.display()))?,
    );
    let ocr_docs = store.ocr_doc_count().unwrap_or(0);
    info!(
        existing = store.len().await?,
        ocr_docs,
        schema = SCHEMA_VERSION,
        "durable store open"
    );

    store
        .append(vec![SourceEvent::new(
            SourceKind::Other("daemon".into()),
            "daemon.boot.v1",
            json!({
                "phase": "S3-audio",
                "observe": true,
                "screen": config.sources.screen,
                "audio": config.sources.audio,
                "ocr": config.ocr.enabled,
                "api": config.api.enabled,
            }),
        )])
        .await?;

    let perms = MacPermissions;
    let mut status = perms.status().await?;
    info!(
        screen = ?status.screen_recording,
        mic = ?status.microphone,
        "permissions"
    );
    if config.sources.screen && !status.can_capture_screen() {
        let _ = request_screen_recording();
        status = perms.status().await?;
        info!(screen = ?status.screen_recording, "after screen request");
    }

    // --- OCR worker ---
    let (ocr_cancel_tx, ocr_cancel_rx) = watch::channel(false);
    let ocr_handle = if config.ocr.enabled {
        let engine = Arc::new(MacVisionOcr::with_max_image_bytes(
            config.ocr.max_image_bytes as usize,
        ));
        if engine.is_supported() {
            let worker = Arc::new(OcrWorker::new(
                Arc::clone(&store),
                engine,
                OcrWorkerConfig {
                    languages: config.ocr.languages.clone(),
                    poll_interval: Duration::from_millis(config.ocr.poll_interval_ms),
                    batch_size: config.ocr.batch_size.max(1),
                    include_boxes: config.ocr.include_boxes,
                    boxes_when_empty_only: config.ocr.boxes_when_empty_only,
                    max_attempts: config.ocr.max_attempts as i64,
                    retry_base: Duration::from_millis(config.ocr.retry_base_ms),
                    retry_max: Duration::from_millis(config.ocr.retry_max_ms),
                    engine_timeout: Duration::from_millis(config.ocr.timeout_ms),
                    stale_running: Duration::from_millis(config.ocr.stale_running_ms),
                    max_image_bytes: config.ocr.max_image_bytes as usize,
                    max_text_chars: config.ocr.max_text_chars as usize,
                    shutdown_drain: Duration::from_millis(config.ocr.shutdown_drain_ms),
                },
            ));
            let _ = worker.reclaim_stale();
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
    let mut audio_status = SourceStatus {
        id: "audio".into(),
        enabled: config.sources.audio,
        running: false,
        last_error: None,
    };

    // --- Local control API ---
    let _api_handle = if config.api.enabled {
        control_server::spawn(
            &config.api.bind,
            control_server::ControlState {
                store: Arc::clone(&store),
                paused: config.privacy.paused,
                sources: vec![screen_status.clone(), audio_status.clone()],
            },
        )
    } else {
        None
    };

    // Shared cancel for long-running observe tasks.
    let (observe_cancel_tx, observe_cancel_rx) = watch::channel(false);

    // --- Audio (concurrent with screen) ---
    let audio_task = if config.sources.audio {
        audio_status.running = true;
        let store_a = Arc::clone(&store);
        let audio_cfg = config.audio.clone();
        let privacy = config.privacy.clone();
        let cancel = observe_cancel_rx.clone();
        Some(tokio::spawn(async move {
            run_audio_loop(store_a, audio_cfg, privacy, cancel).await
        }))
    } else {
        None
    };

    let mut ran_long_loop = false;
    let expect_long = (config.sources.screen && config.capture.screen_ticks == 0)
        || (config.sources.audio && config.audio.ticks == 0);

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
                                match store_w.enqueue_job(stored.id, "ocr_screen") {
                                    Ok(Some(_)) => {}
                                    Ok(None) => debug_skip_dup_ocr(),
                                    Err(e) => warn!(error = %e, "enqueue ocr_screen failed"),
                                }
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

        info!("observe screen loop running (Ctrl+C to stop if ticks=0)");
        if max_ticks == 0 {
            ran_long_loop = true;
        }

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
    } else if config.sources.audio && config.audio.ticks == 0 {
        // Audio-only continuous: wait until Ctrl+C (audio task runs in background).
        ran_long_loop = true;
        info!("audio-only observe running; Ctrl+C to stop");
        let _ = tokio::signal::ctrl_c().await;
        info!("Ctrl+C");
    } else if config.sources.audio && config.audio.ticks > 0 {
        // Finite audio smoke without screen: wait for audio task / cancel after grace.
        let wait_ms = config.audio.chunk_ms.saturating_mul(config.audio.ticks.saturating_add(2));
        tokio::time::sleep(Duration::from_millis(wait_ms.max(2_000))).await;
    }

    // Stop audio + OCR.
    let _ = observe_cancel_tx.send(true);
    if let Some(handle) = audio_task {
        match handle.await {
            Ok(Ok(st)) => {
                audio_status.running = false;
                info!(
                    emitted = st.chunks_emitted,
                    silent = st.chunks_dropped_silent,
                    pause = st.chunks_dropped_pause,
                    sessions_open = st.sessions_opened,
                    sessions_close = st.sessions_closed,
                    "audio stats"
                );
            }
            Ok(Err(e)) => {
                audio_status.running = false;
                audio_status.last_error = Some(e.to_string());
                warn!(error = %e, "audio task failed");
            }
            Err(e) => {
                audio_status.running = false;
                warn!(error = %e, "audio task join failed");
            }
        }
    }

    if let Some((worker, handle)) = ocr_handle {
        let _ = ocr_cancel_tx.send(true);
        let _ = handle.await;
        if config.capture.screen_ticks > 0 {
            let st = worker.drain(40).await;
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                skipped = st.skipped_existing,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "ocr stats"
            );
        } else {
            let st = worker.stats();
            info!(
                processed = st.processed,
                succeeded = st.succeeded,
                empty = st.empty,
                failed = st.failed,
                dead = st.dead,
                reclaimed = st.reclaimed,
                timed_out = st.timed_out,
                "ocr stats"
            );
        }
        if let Ok(counts) = store.job_counts_by_status("ocr_screen") {
            info!(?counts, "ocr job counts");
        }
    }

    // API-only keep-alive when no long observe ran.
    if config.api.enabled && expect_long && !ran_long_loop {
        info!(
            bind = %config.api.bind,
            "control API idle; Ctrl+C to stop"
        );
        tokio::signal::ctrl_c().await?;
        info!("Ctrl+C");
    } else if config.api.enabled
        && !expect_long
        && !config.sources.screen
        && !config.sources.audio
    {
        info!(
            bind = %config.api.bind,
            "control API only; Ctrl+C to stop"
        );
        tokio::signal::ctrl_c().await?;
        info!("Ctrl+C");
    }

    let health = HealthResponse::scaffold(
        vec![screen_status, audio_status],
        store.len().await?,
        config.privacy.paused,
        store.ocr_doc_count().unwrap_or(0),
        SCHEMA_VERSION,
    );
    info!(
        stored = health.stored_events,
        ocr_docs = health.ocr_docs,
        "health"
    );
    Ok(())
}

async fn run_audio_loop(
    store: Arc<SqliteStore>,
    config: AudioConfig,
    privacy: PrivacyConfig,
    mut cancel: watch::Receiver<bool>,
) -> Result<lumen_sources_media::AudioStats> {
    let open_cfg = MicOpenConfig {
        preferred_sample_rate: config.sample_rate,
        preferred_channels: config.channels,
        chunk_ms: config.chunk_ms,
        device: config.device.clone(),
    };
    let capturer = MacMicCapturer;
    let stream = tokio::task::spawn_blocking(move || capturer.open(open_cfg))
        .await
        .context("join mic open")?
        .context("open microphone")?;

    info!(
        mode = %config.mode,
        chunk_ms = config.chunk_ms,
        ticks = config.ticks,
        "audio observe started"
    );

    let mut orch = AudioOrchestrator::new(config.clone(), privacy);
    let max_ticks = config.ticks;
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if *cancel.borrow() {
            break;
        }
        if max_ticks > 0 && orch.stats().chunks_emitted >= max_ticks {
            break;
        }

        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            _ = poll.tick() => {
                let batch = orch.drain_ready(&stream);
                for cap in batch {
                    let bytes = cap.wav.len();
                    match store.put_and_append(cap.event, cap.media_type, &cap.wav) {
                        Ok(stored) => {
                            info!(
                                id = %stored.id,
                                kind = %stored.kind,
                                bytes,
                                session = ?stored.session_id,
                                "persisted audio chunk"
                            );
                        }
                        Err(e) => warn!(error = %e, "audio persist failed"),
                    }
                    if max_ticks > 0 && orch.stats().chunks_emitted >= max_ticks {
                        break;
                    }
                }
            }
        }
    }

    orch.force_close_session();
    stream.stop();
    Ok(orch.stats())
}

#[inline]
fn debug_skip_dup_ocr() {
    // open job already exists — normal under burst captures
}
