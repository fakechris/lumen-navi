//! Microphone Observe orchestrator (S3).
//!
//! Converts PCM chunks into durable `audio_chunk.v1` events + WAV bytes.
//! Never blocks the screen capture path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lumen_config::{AudioConfig, PrivacyConfig};
use lumen_platform::{pcm_rms_peak, pcm_s16le_to_wav, MicStream, PcmChunk};
use lumen_types::{event_kind, SourceEvent, SourceKind};
use serde_json::json;
use uuid::Uuid;

const VOICE_FRAME_MS: u64 = 20;
const MIN_VOICE_RUN_MS: u64 = 160;
const MAX_VOICE_ZERO_CROSSING_RATE: f32 = 0.35;
const DEFAULT_TRUNK_MAX_MS: u64 = 12_000;
const DEFAULT_TRUNK_PADDING_MS: u64 = 300;
/// Audio kept immediately before a VAD hit so utterance onsets ("今天…")
/// are not clipped while the frame gate warms up.
const DEFAULT_TRUNK_PREROLL_MS: u64 = 300;
/// The adaptive threshold must clear the measured noise floor by at least
/// this factor, otherwise ambient fluctuation opens a trunk every few
/// seconds and floods ASR with noise.
const NOISE_FLOOR_HEADROOM: f32 = 2.0;
const ASR_TARGET_RMS: f32 = 0.04;
const MAX_AUDIO_GAIN: f32 = 8.0;

/// One audio chunk ready to persist.
#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub event: SourceEvent,
    pub wav: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct AudioStats {
    pub chunks_received: u64,
    pub last_rms: f32,
    pub max_rms: f32,
    pub chunks_emitted: u64,
    pub chunks_dropped_silent: u64,
    pub chunks_dropped_queue: u64,
    pub chunks_dropped_pause: u64,
    pub chunks_dropped_oversized: u64,
    pub sessions_opened: u64,
    pub sessions_closed: u64,
}

/// Session + VAD policy over a live [`MicStream`].
pub struct AudioOrchestrator {
    config: AudioConfig,
    privacy: PrivacyConfig,
    session_id: Option<Uuid>,
    session_started: Option<Instant>,
    ordinal: u64,
    stats_emitted: AtomicU64,
    stats_received: AtomicU64,
    last_rms: f32,
    max_rms: f32,
    stats_silent: AtomicU64,
    stats_pause: AtomicU64,
    stats_oversized: AtomicU64,
    stats_sessions_open: AtomicU64,
    stats_sessions_close: AtomicU64,
    noise_floor_rms: Option<f32>,
    trunk_samples: Vec<i16>,
    trunk_sample_rate: u32,
    trunk_device: String,
    trunk_silent_ms: u64,
    trunk_pending_silence: Vec<i16>,
    /// Ring buffer of the most recent quiet audio, prepended to a trunk when
    /// voice is first detected (onset pre-roll).
    pre_roll_samples: Vec<i16>,
}

impl AudioOrchestrator {
    pub fn new(config: AudioConfig, privacy: PrivacyConfig) -> Self {
        Self {
            config,
            privacy,
            session_id: None,
            session_started: None,
            ordinal: 0,
            stats_emitted: AtomicU64::new(0),
            stats_received: AtomicU64::new(0),
            last_rms: 0.0,
            max_rms: 0.0,
            stats_silent: AtomicU64::new(0),
            stats_pause: AtomicU64::new(0),
            stats_oversized: AtomicU64::new(0),
            stats_sessions_open: AtomicU64::new(0),
            stats_sessions_close: AtomicU64::new(0),
            noise_floor_rms: None,
            trunk_samples: Vec::new(),
            trunk_sample_rate: 0,
            trunk_device: String::new(),
            trunk_silent_ms: 0,
            trunk_pending_silence: Vec::new(),
            pre_roll_samples: Vec::new(),
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.privacy.paused = paused;
    }

    pub fn stats(&self) -> AudioStats {
        AudioStats {
            chunks_received: self.stats_received.load(Ordering::Relaxed),
            last_rms: self.last_rms,
            max_rms: self.max_rms,
            chunks_emitted: self.stats_emitted.load(Ordering::Relaxed),
            chunks_dropped_silent: self.stats_silent.load(Ordering::Relaxed),
            chunks_dropped_queue: 0,
            chunks_dropped_pause: self.stats_pause.load(Ordering::Relaxed),
            chunks_dropped_oversized: self.stats_oversized.load(Ordering::Relaxed),
            sessions_opened: self.stats_sessions_open.load(Ordering::Relaxed),
            sessions_closed: self.stats_sessions_close.load(Ordering::Relaxed),
        }
    }

    /// Process one capture quantum according to VAD / trunk / privacy policy.
    pub fn on_chunk(&mut self, chunk: PcmChunk) -> Option<CapturedAudio> {
        self.stats_received.fetch_add(1, Ordering::Relaxed);
        self.last_rms = chunk.rms;
        self.max_rms = self.max_rms.max(chunk.rms);
        if self.privacy.paused {
            self.stats_pause.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // Enforce max chunk duration (trim tail if needed).
        let chunk = clamp_chunk_duration(chunk, self.config.max_chunk_ms);

        self.update_noise_floor(chunk.rms);
        let voice = is_probable_voice(&chunk, self.effective_vad_threshold());
        let now = Instant::now();

        if self.config.is_session_mode() {
            // Force-close long sessions before accepting more voice.
            if let (Some(started), Some(_)) = (self.session_started, self.session_id) {
                if now.duration_since(started) >= Duration::from_millis(self.config.max_session_ms)
                {
                    self.close_session();
                }
            }
            if voice {
                if self.session_id.is_none() {
                    self.open_session();
                }
            }
        } else {
            if voice && self.session_id.is_none() {
                self.open_session();
            } else if voice {
                // continuous: roll session id every max_session_ms for grouping hygiene
                if self.session_started.is_some_and(|started| {
                    now.duration_since(started) >= Duration::from_millis(self.config.max_session_ms)
                }) {
                    self.close_session();
                    self.open_session();
                }
            }
        }

        if voice {
            self.append_voice_to_trunk(&chunk);
            if self.trunk_reached_max() {
                return self.flush_trunk();
            }
            return None;
        }

        self.stats_silent.fetch_add(1, Ordering::Relaxed);
        if self.trunk_samples.is_empty() || self.session_id.is_none() {
            // No open trunk: keep a short tail of quiet audio so the next
            // utterance can be prepended with its onset.
            if self.trunk_samples.is_empty() && chunk.sample_rate > 0 {
                let cap =
                    (u64::from(chunk.sample_rate) * DEFAULT_TRUNK_PREROLL_MS / 1_000) as usize;
                self.pre_roll_samples.extend_from_slice(&chunk.samples);
                if self.pre_roll_samples.len() > cap {
                    let excess = self.pre_roll_samples.len() - cap;
                    self.pre_roll_samples.drain(..excess);
                }
            }
            return None;
        }

        self.trunk_pending_silence.extend_from_slice(&chunk.samples);
        self.trunk_silent_ms = self
            .trunk_silent_ms
            .saturating_add(chunk.duration_ms.max(1));
        if self.trunk_silent_ms >= self.trunk_silence_ms() {
            let output = self.flush_trunk();
            if self.config.is_session_mode() {
                self.close_session();
            }
            return output;
        }

        // Keep silence in a side buffer while the hangover window is open.
        // This lets us preserve only a short tail, instead of persisting every
        // long silent capture quantum.
        if self.trunk_reached_max() {
            return self.flush_trunk();
        }
        None
    }

    fn trunk_silence_ms(&self) -> u64 {
        self.config.session_silence_ms.max(20)
    }

    /// The configured threshold is a ceiling, not a guaranteed microphone
    /// level. Some CoreAudio devices deliver speech around 0.006 RMS while
    /// the default 0.01 floor is calibrated for louder microphones. The
    /// adaptive floor must still clear the measured ambient noise by a
    /// comfortable margin — tracking the floor too closely (×1.05) let
    /// ordinary noise fluctuation open trunks every few seconds.
    fn effective_vad_threshold(&self) -> f32 {
        let configured = self.config.vad_rms_threshold.max(0.001);
        self.noise_floor_rms
            .map(|floor| (floor * NOISE_FLOOR_HEADROOM).max(floor + 0.001).min(configured))
            .unwrap_or(configured)
    }

    fn update_noise_floor(&mut self, rms: f32) {
        let configured = self.config.vad_rms_threshold.max(0.001);
        if !rms.is_finite() || rms <= 0.0 || rms >= configured {
            return;
        }
        self.noise_floor_rms = Some(match self.noise_floor_rms {
            Some(previous) => previous * 0.9 + rms * 0.1,
            None => rms,
        });
    }

    fn trunk_max_ms(&self) -> u64 {
        let configured = if self.config.trunk_max_ms == 0 {
            DEFAULT_TRUNK_MAX_MS
        } else {
            self.config.trunk_max_ms
        };
        configured.max(self.config.chunk_ms.max(200))
    }

    fn trunk_padding_ms(&self) -> u64 {
        let configured = if self.config.trunk_padding_ms == 0 {
            DEFAULT_TRUNK_PADDING_MS
        } else {
            self.config.trunk_padding_ms
        };
        configured.min(self.trunk_max_ms().saturating_sub(1))
    }

    fn append_voice_to_trunk(&mut self, chunk: &PcmChunk) {
        if self.trunk_samples.is_empty() {
            self.trunk_sample_rate = chunk.sample_rate;
            self.trunk_device = chunk.device_name.clone();
            // Prepend the retained quiet onset so the trunk starts before
            // the first frame that cleared the VAD gate.
            self.trunk_samples.append(&mut self.pre_roll_samples);
        }
        self.trunk_samples
            .extend_from_slice(&self.trunk_pending_silence);
        self.trunk_pending_silence.clear();
        self.trunk_samples.extend_from_slice(&chunk.samples);
        self.trunk_silent_ms = 0;
    }

    fn trunk_reached_max(&self) -> bool {
        self.trunk_sample_rate > 0
            && self.trunk_samples.len() as u64 * 1_000
                >= u64::from(self.trunk_sample_rate) * self.trunk_max_ms()
    }

    fn flush_trunk(&mut self) -> Option<CapturedAudio> {
        let session_id = self.session_id?;
        if self.trunk_samples.is_empty() || self.trunk_sample_rate == 0 {
            return None;
        }

        let mut samples = std::mem::take(&mut self.trunk_samples);
        let padding_samples =
            (u64::from(self.trunk_sample_rate) * self.trunk_padding_ms() / 1_000) as usize;
        let padding_samples = padding_samples.min(self.trunk_pending_silence.len());
        if padding_samples > 0 {
            let start = self.trunk_pending_silence.len() - padding_samples;
            samples.extend_from_slice(&self.trunk_pending_silence[start..]);
        }
        self.trunk_pending_silence.clear();
        let sample_rate = self.trunk_sample_rate;
        let device = std::mem::take(&mut self.trunk_device);
        self.trunk_sample_rate = 0;
        self.trunk_silent_ms = 0;
        let (input_rms, input_peak) = pcm_rms_peak(&samples);
        let gain = boost_quiet_audio(&mut samples);
        let (rms, peak) = pcm_rms_peak(&samples);
        self.ordinal = self.ordinal.saturating_add(1);
        let wav = pcm_s16le_to_wav(&samples, sample_rate, 1);
        if wav.len() as u64 > self.config.max_audio_bytes {
            self.stats_oversized.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let duration_ms = samples.len() as u64 * 1_000 / u64::from(sample_rate);
        let event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({
                "payload_version": 1,
                "device": device,
                "sample_rate": sample_rate,
                "channels": 1,
                "duration_ms": duration_ms,
                "samples": samples.len(),
                "mode": self.config.mode,
                "rms": rms,
                "peak": peak,
                "input_rms": input_rms,
                "input_peak": input_peak,
                "gain": gain,
                "format": "wav_s16le",
                "session_ordinal": self.ordinal,
                "voice": true,
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
    pub fn drain_ready(
        &mut self,
        stream: &MicStream,
    ) -> Result<Vec<CapturedAudio>, lumen_platform::PlatformError> {
        let mut out = Vec::new();
        loop {
            match stream.try_recv() {
                Ok(Some(chunk)) => {
                    if let Some(c) = self.on_chunk(chunk) {
                        out.push(c);
                    }
                }
                Ok(None) => break,
                Err(error) => return Err(error),
            }
        }
        self.apply_idle_session_timeouts();
        Ok(out)
    }

    fn apply_idle_session_timeouts(&mut self) {
        let now = Instant::now();
        if let Some(started) = self.session_started {
            if now.duration_since(started) >= Duration::from_millis(self.config.max_session_ms) {
                self.close_session();
                return;
            }
        }
    }

    pub fn force_close_session(&mut self) {
        self.close_session();
    }

    /// Flush a voice trunk when the capture loop is shutting down.
    pub fn take_pending_audio(&mut self) -> Option<CapturedAudio> {
        self.flush_trunk()
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
    }
}

/// Bring a quiet but already-voiced trunk into a useful playback/ASR range.
/// Silence never reaches this function because trunks are opened by VAD.
fn boost_quiet_audio(samples: &mut [i16]) -> f32 {
    let (rms, _) = pcm_rms_peak(samples);
    if !rms.is_finite() || rms <= 0.0 {
        return 1.0;
    }
    let gain = (ASR_TARGET_RMS / rms).clamp(1.0, MAX_AUDIO_GAIN);
    if gain > 1.0 {
        for sample in samples {
            *sample = ((*sample as f32 * gain).clamp(-32768.0, 32767.0)) as i16;
        }
    }
    gain
}

/// Truncate PCM so duration ≤ max_ms (product hard cap).
fn clamp_chunk_duration(mut chunk: PcmChunk, max_ms: u64) -> PcmChunk {
    if max_ms == 0 || chunk.sample_rate == 0 {
        return chunk;
    }
    let max_samples = (u64::from(chunk.sample_rate) * max_ms / 1000) as usize;
    if chunk.samples.len() > max_samples && max_samples > 0 {
        chunk.samples.truncate(max_samples);
        chunk.duration_ms = max_ms;
        let (rms, peak) = lumen_platform::pcm_rms_peak(&chunk.samples);
        chunk.rms = rms;
        chunk.peak = peak;
    }
    chunk
}

/// Cheap pre-persist speech gate. Inspect short frames instead of requiring the
/// whole capture window to clear the threshold: a short Observe quantum may
/// contain only a short utterance whose overall RMS is below the configured
/// floor. Require a sustained active region and reject noise-like
/// zero-crossing rates before allocating or storing WAV bytes.
fn is_probable_voice(chunk: &PcmChunk, rms_threshold: f32) -> bool {
    if chunk.sample_rate == 0
        || chunk.samples.is_empty()
        || !chunk.rms.is_finite()
        || !rms_threshold.is_finite()
        || rms_threshold < 0.0
    {
        return false;
    }

    let frame_samples = ((u64::from(chunk.sample_rate) * VOICE_FRAME_MS) / 1_000).max(1) as usize;
    // The configured RMS threshold is already the product's noise floor.
    // Raising it again here to 2x caused normal quiet speech (roughly
    // 0.01–0.02 RMS on the user's input device) to be rejected frame by frame.
    let active_threshold = rms_threshold;
    let mut longest_active_frames = 0usize;
    let mut active_frames = 0usize;

    for frame in chunk.samples.chunks(frame_samples) {
        let (frame_rms, _) = lumen_platform::pcm_rms_peak(frame);
        if frame_rms >= active_threshold {
            active_frames += 1;
            longest_active_frames = longest_active_frames.max(active_frames);
        } else {
            active_frames = 0;
        }
    }

    // Short test/probe chunks should still be classifiable. Product capture
    // chunks are normally 3 seconds and require the full sustained run.
    let required_run_ms = MIN_VOICE_RUN_MS.min((chunk.duration_ms / 2).max(60));
    let sustained = longest_active_frames as u64 * VOICE_FRAME_MS >= required_run_ms;
    if !sustained {
        return false;
    }

    let crossings = chunk
        .samples
        .windows(2)
        .filter(|pair| (pair[0] < 0 && pair[1] >= 0) || (pair[0] >= 0 && pair[1] < 0))
        .count();
    let zero_crossing_rate = crossings as f32 / (chunk.samples.len() - 1).max(1) as f32;
    zero_crossing_rate <= MAX_VOICE_ZERO_CROSSING_RATE
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
                session_silence_ms: 100,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        let c = synthetic_tone_chunk(16_000, 100, 0.2, "test");
        assert!(orch.on_chunk(c).is_none());
        let out = orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .expect("trunk");
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
                app_blocklist: Vec::new(),
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
        // voice → open + accumulate
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.5, "t"))
            .is_none());
        // endpoint silence → flush one trunk
        let a = orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .expect("trunk");
        let sid = a.event.session_id.unwrap();
        assert_eq!(orch.stats().sessions_opened, 1);
        // new voice → new session; it is accumulated until endpoint silence
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 100, 0.5, "t"))
            .is_none());
        assert_eq!(orch.stats().sessions_closed, 1);
        let c = orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .expect("trunk2");
        assert_ne!(c.event.session_id, Some(sid));
        assert_eq!(orch.stats().sessions_opened, 2);
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 100))
            .is_none());
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
        assert!(orch.on_chunk(synthetic_silence_chunk(16_000, 50)).is_none());
        assert_eq!(orch.stats().chunks_dropped_silent, 1);
    }

    #[test]
    fn merges_contiguous_voice_into_one_trunk() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                session_silence_ms: 200,
                trunk_padding_ms: 500,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 500, 0.2, "t"))
            .is_none());
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 500, 0.2, "t"))
            .is_none());
        let trunk = orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .expect("merged trunk");
        let duration_ms = trunk
            .event
            .payload
            .get("duration_ms")
            .and_then(|value| value.as_u64())
            .expect("duration");
        assert_eq!(duration_ms, 1_500);
        assert_eq!(orch.stats().chunks_emitted, 1);
    }

    #[test]
    fn adapts_to_quiet_microphone_above_noise_floor() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                session_silence_ms: 500,
                vad_rms_threshold: 0.01,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        // Model the user's device: ambient input sits around 0.004 RMS and
        // speech rises to roughly 0.014 RMS.
        for _ in 0..3 {
            assert!(orch
                .on_chunk(synthetic_tone_chunk(16_000, 500, 0.006, "quiet-mic"))
                .is_none());
        }
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 500, 0.02, "quiet-mic"))
            .is_none());
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .is_some());
    }

    #[test]
    fn ambient_noise_fluctuation_never_opens_a_trunk() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                session_silence_ms: 500,
                vad_rms_threshold: 0.01,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        // Sustained ambient noise around 0.004 RMS with mild fluctuation:
        // the adaptive threshold (2x floor) must keep classifying it as
        // silence instead of emitting a junk trunk every few seconds.
        for amplitude in [0.006, 0.005, 0.006, 0.007, 0.006, 0.005] {
            assert!(orch
                .on_chunk(synthetic_tone_chunk(16_000, 500, amplitude, "noisy-room"))
                .is_none());
        }
        assert_eq!(orch.stats().chunks_emitted, 0);
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .is_none());
        assert_eq!(orch.stats().chunks_emitted, 0);
    }

    #[test]
    fn trunk_includes_preroll_from_preceding_quiet_audio() {
        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                session_silence_ms: 500,
                trunk_padding_ms: 0,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        // 1s of quiet audio, then 500ms of voice, then endpoint silence.
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .is_none());
        assert!(orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .is_none());
        assert!(orch
            .on_chunk(synthetic_tone_chunk(16_000, 500, 0.2, "t"))
            .is_none());
        let trunk = orch
            .on_chunk(synthetic_silence_chunk(16_000, 500))
            .expect("trunk");
        let duration_ms = trunk
            .event
            .payload
            .get("duration_ms")
            .and_then(|value| value.as_u64())
            .expect("duration");
        // 300ms pre-roll cap (of the 1s quiet run-up) + 500ms voice + the
        // 300ms default tail padding applied from the endpoint silence.
        assert_eq!(duration_ms, 1_100);
    }

    #[test]
    fn boosts_quiet_voiced_audio_without_clipping() {
        let mut samples = synthetic_tone_chunk(16_000, 500, 0.01, "quiet-mic").samples;
        let (before_rms, _) = pcm_rms_peak(&samples);
        let gain = boost_quiet_audio(&mut samples);
        let (after_rms, after_peak) = pcm_rms_peak(&samples);
        assert!(gain > 1.0);
        assert!(after_rms > before_rms);
        assert!(after_rms > 0.02);
        assert!(after_peak < 1.0);
    }

    #[test]
    fn accepts_quiet_speech_above_configured_rms() {
        let chunk = synthetic_tone_chunk(16_000, 3_000, 0.02, "quiet-voice");
        assert!(is_probable_voice(&chunk, 0.01));
    }

    #[test]
    fn accepts_short_speech_inside_quiet_observe_window() {
        let sample_rate = 16_000;
        let mut samples = vec![0i16; sample_rate as usize * 3];
        let start = 1_200 * sample_rate as usize / 1_000;
        let end = start + 300 * sample_rate as usize / 1_000;
        for (offset, sample) in samples[start..end].iter_mut().enumerate() {
            let t = offset as f32 / sample_rate as f32;
            let value = (t * 220.0 * std::f32::consts::TAU).sin() * 0.02;
            *sample = (value * 32767.0) as i16;
        }
        let chunk = PcmChunk::from_mono_i16(samples, sample_rate, "short-voice");
        assert!(
            chunk.rms < 0.01,
            "fixture must model a quiet utterance in a 3s window"
        );
        assert!(is_probable_voice(&chunk, 0.01));
    }

    #[test]
    fn drops_low_level_impulses_before_wav_persist() {
        let sample_rate = 16_000;
        let mut samples = vec![0i16; sample_rate as usize * 3];
        for start_ms in [620usize, 890, 1_160, 1_430] {
            let start = start_ms * sample_rate as usize / 1_000;
            let end = start + 120 * sample_rate as usize / 1_000;
            for (offset, sample) in samples[start..end].iter_mut().enumerate() {
                let t = offset as f32 / sample_rate as f32;
                let value = (t * 240.0 * std::f32::consts::TAU).sin() * 0.08;
                *sample = (value * 32767.0) as i16;
            }
        }
        let chunk = PcmChunk::from_mono_i16(samples, sample_rate, "impulse-noise");
        assert!(
            chunk.rms >= 0.01,
            "fixture must defeat the old RMS-only gate"
        );

        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                drop_silent_chunks: true,
                vad_rms_threshold: 0.01,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        assert!(orch.on_chunk(chunk).is_none());
        assert_eq!(orch.stats().chunks_dropped_silent, 1);
    }

    #[test]
    fn drops_sustained_white_noise_before_wav_persist() {
        let sample_rate = 16_000;
        let mut state = 0x1234_5678u32;
        let samples = (0..sample_rate as usize * 3)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let unit = ((state >> 16) as f32 / u16::MAX as f32) * 2.0 - 1.0;
                (unit * 0.08 * 32767.0) as i16
            })
            .collect();
        let chunk = PcmChunk::from_mono_i16(samples, sample_rate, "white-noise");
        assert!(
            chunk.rms >= 0.01,
            "fixture must defeat the old RMS-only gate"
        );

        let mut orch = AudioOrchestrator::new(
            AudioConfig {
                mode: "continuous".into(),
                drop_silent_chunks: true,
                vad_rms_threshold: 0.01,
                ..AudioConfig::default()
            },
            PrivacyConfig::default(),
        );
        assert!(orch.on_chunk(chunk).is_none());
        assert_eq!(orch.stats().chunks_dropped_silent, 1);
    }

    #[test]
    fn keeps_a_sustained_quiet_voice_like_signal() {
        let sample_rate = 16_000;
        let mut samples = vec![0i16; sample_rate as usize * 3];
        let start = 900 * sample_rate as usize / 1_000;
        let end = start + 400 * sample_rate as usize / 1_000;
        for (offset, sample) in samples[start..end].iter_mut().enumerate() {
            let t = offset as f32 / sample_rate as f32;
            let envelope = (offset as f32 / (end - start) as f32 * std::f32::consts::PI).sin();
            let value = (t * 180.0 * std::f32::consts::TAU).sin() * envelope * 0.08;
            *sample = (value * 32767.0) as i16;
        }
        let chunk = PcmChunk::from_mono_i16(samples, sample_rate, "quiet-voice");
        assert!(is_probable_voice(&chunk, 0.01));
    }
}
