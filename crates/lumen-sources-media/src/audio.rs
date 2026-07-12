//! Microphone Observe orchestrator (S3).
//!
//! Converts PCM chunks into durable `audio_chunk.v1` events + WAV bytes.
//! Never blocks the screen capture path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lumen_config::{AudioConfig, PrivacyConfig};
use lumen_platform::{pcm_s16le_to_wav, MicStream, PcmChunk};
use lumen_types::{event_kind, SourceEvent, SourceKind};
use serde_json::json;
use uuid::Uuid;

/// One audio chunk ready to persist.
#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub event: SourceEvent,
    pub wav: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct AudioStats {
    pub chunks_emitted: u64,
    pub chunks_dropped_silent: u64,
    pub chunks_dropped_queue: u64,
    pub chunks_dropped_pause: u64,
    pub sessions_opened: u64,
    pub sessions_closed: u64,
}

/// Session + VAD policy over a live [`MicStream`].
pub struct AudioOrchestrator {
    config: AudioConfig,
    privacy: PrivacyConfig,
    session_id: Option<Uuid>,
    session_started: Option<Instant>,
    last_voice: Option<Instant>,
    ordinal: u64,
    stats_emitted: AtomicU64,
    stats_silent: AtomicU64,
    stats_pause: AtomicU64,
    stats_sessions_open: AtomicU64,
    stats_sessions_close: AtomicU64,
}

impl AudioOrchestrator {
    pub fn new(config: AudioConfig, privacy: PrivacyConfig) -> Self {
        Self {
            config,
            privacy,
            session_id: None,
            session_started: None,
            last_voice: None,
            ordinal: 0,
            stats_emitted: AtomicU64::new(0),
            stats_silent: AtomicU64::new(0),
            stats_pause: AtomicU64::new(0),
            stats_sessions_open: AtomicU64::new(0),
            stats_sessions_close: AtomicU64::new(0),
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.privacy.paused = paused;
    }

    pub fn stats(&self) -> AudioStats {
        AudioStats {
            chunks_emitted: self.stats_emitted.load(Ordering::Relaxed),
            chunks_dropped_silent: self.stats_silent.load(Ordering::Relaxed),
            chunks_dropped_queue: 0,
            chunks_dropped_pause: self.stats_pause.load(Ordering::Relaxed),
            sessions_opened: self.stats_sessions_open.load(Ordering::Relaxed),
            sessions_closed: self.stats_sessions_close.load(Ordering::Relaxed),
        }
    }

    /// Process one PCM chunk according to mode / privacy / VAD.
    pub fn on_chunk(&mut self, chunk: PcmChunk) -> Option<CapturedAudio> {
        if self.privacy.paused {
            self.stats_pause.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let voice = chunk.rms >= self.config.vad_rms_threshold;
        let now = Instant::now();

        if self.config.is_session_mode() {
            if voice {
                if self.session_id.is_none() {
                    self.open_session();
                }
                self.last_voice = Some(now);
            } else {
                // Silence: maybe close session; never emit if drop_silent.
                if let Some(last) = self.last_voice {
                    if now.duration_since(last)
                        >= Duration::from_millis(self.config.session_silence_ms)
                    {
                        self.close_session();
                    }
                }
                if self.config.drop_silent_chunks || self.session_id.is_none() {
                    self.stats_silent.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            }
        } else {
            // continuous: ensure a session id for grouping
            if self.session_id.is_none() {
                self.open_session();
            }
            if self.config.drop_silent_chunks && !voice {
                self.stats_silent.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }

        let session_id = self.session_id?;
        self.ordinal = self.ordinal.saturating_add(1);
        let wav = pcm_s16le_to_wav(&chunk.samples, chunk.sample_rate, 1);
        let event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({
                "payload_version": 1,
                "device": chunk.device_name,
                "sample_rate": chunk.sample_rate,
                "channels": 1,
                "duration_ms": chunk.duration_ms,
                "samples": chunk.samples.len(),
                "mode": self.config.mode,
                "rms": chunk.rms,
                "peak": chunk.peak,
                "format": "wav_s16le",
                "session_ordinal": self.ordinal,
                "voice": voice,
            }),
        )
        .with_session(session_id);

        self.stats_emitted.fetch_add(1, Ordering::Relaxed);
        Some(CapturedAudio {
            event,
            wav,
            media_type: "audio/wav".into(),
        })
    }

    /// Drain pending chunks from the mic stream (non-blocking).
    pub fn drain_ready(&mut self, stream: &MicStream) -> Vec<CapturedAudio> {
        let mut out = Vec::new();
        loop {
            match stream.try_recv() {
                Ok(Some(chunk)) => {
                    if let Some(c) = self.on_chunk(chunk) {
                        out.push(c);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // session timeout check even without new audio
        if self.config.is_session_mode() {
            if let Some(last) = self.last_voice {
                if Instant::now().duration_since(last)
                    >= Duration::from_millis(self.config.session_silence_ms)
                {
                    self.close_session();
                }
            }
        }
        out
    }

    pub fn force_close_session(&mut self) {
        self.close_session();
    }

    fn open_session(&mut self) {
        self.session_id = Some(Uuid::new_v4());
        self.session_started = Some(Instant::now());
        self.ordinal = 0;
        self.stats_sessions_open.fetch_add(1, Ordering::Relaxed);
    }

    fn close_session(&mut self) {
        if self.session_id.take().is_some() {
            self.stats_sessions_close.fetch_add(1, Ordering::Relaxed);
        }
        self.session_started = None;
        self.last_voice = None;
    }
}

/// Build a synthetic mono tone chunk (tests).
pub fn synthetic_tone_chunk(
    sample_rate: u32,
    duration_ms: u64,
    amplitude: f32,
    device: &str,
) -> PcmChunk {
    let n = (u64::from(sample_rate) * duration_ms / 1000) as usize;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sample_rate as f32;
        let s = (t * 440.0 * std::f32::consts::TAU).sin() * amplitude;
        samples.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    PcmChunk::from_mono_i16(samples, sample_rate, device)
}

/// Silence chunk (tests).
pub fn synthetic_silence_chunk(sample_rate: u32, duration_ms: u64) -> PcmChunk {
    let n = (u64::from(sample_rate) * duration_ms / 1000) as usize;
    PcmChunk::from_mono_i16(vec![0; n], sample_rate, "silence")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_emits_chunks() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                drop_silent_chunks: false,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        let c = synthetic_tone_chunk(16_000, 100, 0.2, "test");
        let out = orch.on_chunk(c).expect("chunk");
        assert_eq!(out.event.kind, event_kind::AUDIO_CHUNK_V1);
        assert_eq!(out.media_type, "audio/wav");
        assert!(out.wav.starts_with(b"RIFF"));
        assert!(out.event.session_id.is_some());
        assert_eq!(orch.stats().chunks_emitted, 1);
    }

    #[test]
    fn pause_drops_chunks() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig::default(),
            PrivacyConfig {
                paused: true,
                closed_eyes: false,
            },
        );
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.2, "t"))
            .is_none());
        assert_eq!(orch.stats().chunks_dropped_pause, 1);
    }

    #[test]
    fn session_mode_opens_on_voice_closes_on_silence() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "session".into(),
                vad_rms_threshold: 0.01,
                session_silence_ms: 50,
                drop_silent_chunks: true,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        // silence alone → nothing
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .is_none());
        // voice → open + emit
        let a = orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.5, "t"))
            .expect("voice");
        let sid = a.event.session_id.unwrap();
        assert_eq!(orch.stats().sessions_opened, 1);
        // more voice same session
        let b = orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.5, "t"))
            .expect("voice2");
        assert_eq!(b.event.session_id, Some(sid));
        // wait silence threshold
        std::thread::sleep(Duration::from_millis(60));
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .is_none());
        assert_eq!(orch.stats().sessions_closed, 1);
        // new voice → new session
        let c = orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.5, "t"))
            .expect("voice3");
        assert_ne!(c.event.session_id, Some(sid));
    }

    #[test]
    fn drop_silent_in_continuous() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                drop_silent_chunks: true,
                vad_rms_threshold: 0.05,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 50))
            .is_none());
        assert_eq!(orch.stats().chunks_dropped_silent, 1);
    }
}
