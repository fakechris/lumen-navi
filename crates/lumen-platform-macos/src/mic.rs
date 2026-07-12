//! Microphone capture via cpal.
//!
//! The cpal `Stream` is `!Send` on macOS, so it lives on a dedicated audio
//! thread. Fixed-duration chunks are pushed through a bounded channel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, StreamConfig};
use lumen_platform::{MicCapturer, MicOpenConfig, MicStream, PcmChunk, PlatformError};
use tracing::{debug, warn};

/// macOS / desktop mic capturer (cpal default host).
pub struct MacMicCapturer;

impl MicCapturer for MacMicCapturer {
    fn open(&self, cfg: MicOpenConfig) -> Result<MicStream, PlatformError> {
        open_mic(cfg)
    }
}

fn open_mic(cfg: MicOpenConfig) -> Result<MicStream, PlatformError> {
    let host = cpal::default_host();
    let device = select_device(&host, &cfg.device)?;
    let device_name = device.name().unwrap_or_else(|_| "default-input".into());

    let (tx, rx) = mpsc::sync_channel::<PcmChunk>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let epoch = Arc::new(AtomicU64::new(1));
    let epoch_t = Arc::clone(&epoch);
    let name_for_thread = device_name;
    let chunk_ms = cfg.chunk_ms.max(200);
    let preferred_rate = cfg.preferred_sample_rate;
    let preferred_channels = cfg.preferred_channels.max(1);

    let join = thread::Builder::new()
        .name("lumen-mic".into())
        .spawn(move || {
            let supported = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "no default input config");
                    return;
                }
            };

            let mut stream_cfg: StreamConfig = supported.config();
            stream_cfg.channels = preferred_channels.min(stream_cfg.channels.max(1));
            if preferred_rate > 0 {
                stream_cfg.sample_rate = cpal::SampleRate(preferred_rate);
            }

            let sample_format = supported.sample_format();
            let (stream, buffer, sample_rate, channels) = match try_build_stream(
                &device,
                &stream_cfg,
                sample_format,
                chunk_ms,
                &epoch_t,
                &tx,
                &name_for_thread,
            ) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "mic preferred config failed; using device default");
                    let Ok(def) = device.default_input_config() else {
                        return;
                    };
                    let mut def_cfg: StreamConfig = def.config();
                    def_cfg.channels = preferred_channels.min(def_cfg.channels.max(1));
                    match try_build_stream(
                        &device,
                        &def_cfg,
                        def.sample_format(),
                        chunk_ms,
                        &epoch_t,
                        &tx,
                        &name_for_thread,
                    ) {
                        Ok(v) => v,
                        Err(e2) => {
                            warn!(error = %e2, "mic stream open failed");
                            return;
                        }
                    }
                }
            };

            if let Err(e) = stream.play() {
                warn!(error = %e, "mic stream play failed");
                return;
            }
            debug!(
                device = %name_for_thread,
                sample_rate,
                channels,
                chunk_ms,
                "mic stream playing"
            );

            while !stop_t.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(50));
            }
            epoch_t.fetch_add(1, Ordering::SeqCst);
            drop(stream);

            let leftover = buffer
                .lock()
                .ok()
                .map(|mut guard| std::mem::take(&mut *guard))
                .unwrap_or_default();
            if !leftover.is_empty() {
                let mono = to_mono(leftover, channels);
                let chunk = PcmChunk::from_mono_i16(mono, sample_rate, name_for_thread);
                let _ = tx.try_send(chunk);
            }
        })
        .map_err(|e| PlatformError::Message(format!("spawn mic thread: {e}")))?;

    Ok(MicStream::new(rx, stop, join))
}

type BuiltStream = (
    cpal::Stream,
    Arc<Mutex<Vec<i16>>>,
    u32,
    u16,
);

fn try_build_stream(
    device: &cpal::Device,
    stream_cfg: &StreamConfig,
    sample_format: SampleFormat,
    chunk_ms: u64,
    epoch: &Arc<AtomicU64>,
    tx: &mpsc::SyncSender<PcmChunk>,
    device_name: &str,
) -> Result<BuiltStream, PlatformError> {
    let channels = stream_cfg.channels.max(1);
    let sample_rate = stream_cfg.sample_rate.0;
    let samples_per_chunk =
        ((u64::from(sample_rate) * chunk_ms) / 1000).max(1) as usize * channels as usize;

    let buffer: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(samples_per_chunk)));
    let buf_cb = Arc::clone(&buffer);
    let epoch_cb = Arc::clone(epoch);
    let my_epoch = epoch.load(Ordering::SeqCst);
    let tx_cb = tx.clone();
    let name_cb = device_name.to_string();
    let err_fn = |e| warn!(error = %e, "mic stream error");

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                stream_cfg,
                move |data: &[f32], _| {
                    if epoch_cb.load(Ordering::Relaxed) != my_epoch {
                        return;
                    }
                    append_samples(&buf_cb, data.iter().map(|s| s.to_sample::<i16>()));
                    flush_if_ready(
                        &buf_cb,
                        samples_per_chunk,
                        sample_rate,
                        channels,
                        &name_cb,
                        &tx_cb,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| PlatformError::Message(format!("build_input_stream: {e}")))?,
        SampleFormat::I16 => device
            .build_input_stream(
                stream_cfg,
                move |data: &[i16], _| {
                    if epoch_cb.load(Ordering::Relaxed) != my_epoch {
                        return;
                    }
                    append_samples(&buf_cb, data.iter().copied());
                    flush_if_ready(
                        &buf_cb,
                        samples_per_chunk,
                        sample_rate,
                        channels,
                        &name_cb,
                        &tx_cb,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| PlatformError::Message(format!("build_input_stream: {e}")))?,
        SampleFormat::U16 => device
            .build_input_stream(
                stream_cfg,
                move |data: &[u16], _| {
                    if epoch_cb.load(Ordering::Relaxed) != my_epoch {
                        return;
                    }
                    append_samples(
                        &buf_cb,
                        data.iter().map(|s| (*s as i32 - 32768) as i16),
                    );
                    flush_if_ready(
                        &buf_cb,
                        samples_per_chunk,
                        sample_rate,
                        channels,
                        &name_cb,
                        &tx_cb,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| PlatformError::Message(format!("build_input_stream: {e}")))?,
        other => {
            return Err(PlatformError::Message(format!(
                "unsupported sample format: {other:?}"
            )));
        }
    };

    Ok((stream, buffer, sample_rate, channels))
}

fn select_device(host: &cpal::Host, preferred: &str) -> Result<cpal::Device, PlatformError> {
    if !preferred.is_empty() {
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                if let Ok(name) = d.name() {
                    if name == preferred || name.contains(preferred) {
                        return Ok(d);
                    }
                }
            }
        }
        return Err(PlatformError::Message(format!(
            "input device not found: {preferred}"
        )));
    }
    host.default_input_device()
        .ok_or_else(|| PlatformError::Message("no default input device".into()))
}

fn append_samples<I>(buf: &Mutex<Vec<i16>>, iter: I)
where
    I: IntoIterator<Item = i16>,
{
    if let Ok(mut g) = buf.lock() {
        g.extend(iter);
    }
}

fn flush_if_ready(
    buf: &Mutex<Vec<i16>>,
    samples_per_chunk: usize,
    sample_rate: u32,
    channels: u16,
    device_name: &str,
    tx: &mpsc::SyncSender<PcmChunk>,
) {
    let mut g = match buf.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    while g.len() >= samples_per_chunk {
        let raw: Vec<i16> = g.drain(..samples_per_chunk).collect();
        let mono = to_mono(raw, channels);
        let chunk = PcmChunk::from_mono_i16(mono, sample_rate, device_name);
        let _ = tx.try_send(chunk);
    }
}

fn to_mono(interleaved: Vec<i16>, channels: u16) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved;
    }
    let frames = interleaved.len() / ch;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let mut acc = 0i32;
        for c in 0..ch {
            acc += interleaved[i * ch + c] as i32;
        }
        out.push((acc / ch as i32) as i16);
    }
    out
}

/// True when a default input device is enumerable (weak permission signal).
pub fn default_input_available() -> bool {
    let host = cpal::default_host();
    host.default_input_device()
        .and_then(|d| d.default_input_config().ok())
        .is_some()
}
