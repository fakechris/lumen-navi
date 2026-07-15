//! Minimal PCM s16le WAV decode + linear resample for offline ASR.

use lumen_platform::PlatformError;

/// Decoded mono f32 samples in [-1, 1].
#[derive(Debug, Clone)]
pub struct DecodedPcm {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Parse a RIFF/WAVE PCM s16le blob (Navi store format). Multi-channel is averaged to mono.
pub fn decode_wav_pcm_s16le(audio: &[u8]) -> Result<DecodedPcm, PlatformError> {
    if audio.len() < 44 {
        return Err(PlatformError::Message("wav too short".into()));
    }
    if &audio[0..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
        return Err(PlatformError::Message("not a RIFF/WAVE blob".into()));
    }

    let mut offset = 12usize;
    let mut channels: u16 = 1;
    let mut sample_rate: u32 = 16_000;
    let mut bits_per_sample: u16 = 16;
    let mut audio_format: u16 = 1;
    let mut data: Option<&[u8]> = None;

    while offset + 8 <= audio.len() {
        let id = &audio[offset..offset + 4];
        let size = u32::from_le_bytes(audio[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body_start = offset + 8;
        let body_end = body_start.saturating_add(size);
        if body_end > audio.len() {
            return Err(PlatformError::Message("wav chunk overflow".into()));
        }
        let body = &audio[body_start..body_end];
        if id == b"fmt " {
            if body.len() < 16 {
                return Err(PlatformError::Message("fmt chunk too short".into()));
            }
            audio_format = u16::from_le_bytes(body[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(body[2..4].try_into().unwrap()).max(1);
            sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits_per_sample = u16::from_le_bytes(body[14..16].try_into().unwrap());
        } else if id == b"data" {
            data = Some(body);
        }
        // chunks are word-aligned
        offset = body_end + (size % 2);
    }

    if audio_format != 1 {
        return Err(PlatformError::Message(format!(
            "unsupported wav format {audio_format} (need PCM)"
        )));
    }
    if bits_per_sample != 16 {
        return Err(PlatformError::Message(format!(
            "unsupported bits_per_sample {bits_per_sample} (need 16)"
        )));
    }
    let data = data.ok_or_else(|| PlatformError::Message("wav missing data chunk".into()))?;
    if data.len() < 2 {
        return Err(PlatformError::Message("empty wav data".into()));
    }

    let frame_bytes = 2 * channels as usize;
    let frames = data.len() / frame_bytes;
    let mut samples = Vec::with_capacity(frames);
    for i in 0..frames {
        let base = i * frame_bytes;
        let mut acc = 0i32;
        for ch in 0..channels as usize {
            let o = base + ch * 2;
            let s = i16::from_le_bytes([data[o], data[o + 1]]);
            acc += s as i32;
        }
        let mono = (acc / channels as i32) as i16;
        samples.push(mono as f32 / 32768.0);
    }

    if sample_rate == 0 {
        sample_rate = 16_000;
    }

    Ok(DecodedPcm {
        samples,
        sample_rate,
    })
}

/// Linear resample mono f32 to `target_hz`.
pub fn resample_linear(samples: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if samples.is_empty() || from_hz == 0 || to_hz == 0 {
        return Vec::new();
    }
    if from_hz == to_hz {
        return samples.to_vec();
    }
    let ratio = from_hz as f64 / to_hz as f64;
    let out_len = ((samples.len() as f64) / ratio).floor().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let i0 = src.floor() as usize;
        let i1 = (i0 + 1).min(samples.len().saturating_sub(1));
        let t = (src - i0 as f64) as f32;
        let a = samples[i0.min(samples.len() - 1)];
        let b = samples[i1];
        out.push(a + (b - a) * t);
    }
    out
}

/// Decode WAV and resample to 16 kHz mono for offline engines.
pub fn prepare_for_offline_asr(audio: &[u8]) -> Result<DecodedPcm, PlatformError> {
    let decoded = decode_wav_pcm_s16le(audio)?;
    const TARGET: u32 = 16_000;
    let samples = resample_linear(&decoded.samples, decoded.sample_rate, TARGET);
    Ok(DecodedPcm {
        samples,
        sample_rate: TARGET,
    })
}

/// Encode mono f32 samples as WAV s16le (for HTTP multipart upload).
pub fn samples_to_wav_mono_i16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let sr = if sample_rate == 0 { 16_000 } else { sample_rate };
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_platform::pcm_s16le_to_wav;

    #[test]
    fn roundtrip_pcm_wav() {
        let pcm: Vec<i16> = (0..1600).map(|i| ((i % 100) * 30) as i16).collect();
        let wav = pcm_s16le_to_wav(&pcm, 16_000, 1);
        let dec = decode_wav_pcm_s16le(&wav).unwrap();
        assert_eq!(dec.sample_rate, 16_000);
        assert_eq!(dec.samples.len(), pcm.len());
    }

    #[test]
    fn resample_halves_length() {
        let samples: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let out = resample_linear(&samples, 32_000, 16_000);
        assert!((out.len() as i32 - 50).abs() <= 1);
    }
}
