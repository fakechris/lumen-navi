# Audio — Product Spec (S3)

> Product documentation only. Mic-first Observe intake.

## Goal

Capture **microphone** audio into durable `audio_chunk.v1` events **concurrently with screen**, without blocking capture or OCR.

System audio loopback is **out of scope** for this phase.

## Pipeline

```
mic (cpal stream on audio thread)
        ↓
  chunk every chunk_ms (PCM → WAV)
        ↓
  privacy gate (pause)
        ↓
  session policy (continuous | session+VAD)
        ↓
  store event + audio/wav blob
```

Transcription (`transcribe_audio` jobs / Lumen ASR) is **not** required for S3 exit — only durable intake.

## Modes

| Mode | Behavior |
|------|----------|
| `continuous` | Fixed-duration chunks while running; one open audio session for the run |
| `session` | Open session when RMS ≥ threshold; close after `session_silence_ms` of quiet; optional drop of silent chunks |

## Payload `audio_chunk.v1`

```json
{
  "payload_version": 1,
  "device": "MacBook Pro Microphone",
  "sample_rate": 48000,
  "channels": 1,
  "duration_ms": 5000,
  "samples": 240000,
  "mode": "continuous",
  "rms": 0.02,
  "peak": 0.4,
  "format": "wav_s16le",
  "session_ordinal": 3
}
```

Artifact: `audio/wav` (mono s16le + standard RIFF header).

## Config

```toml
[sources]
audio = true

[audio]
mode = "continuous"          # continuous | session
sample_rate = 16000          # preferred; device may negotiate
channels = 1
chunk_ms = 5000
queue_capacity = 8
ticks = 0                    # 0 = until stop; >0 finite chunks (smoke)
session_silence_ms = 2500
vad_rms_threshold = 0.008
drop_silent_chunks = false   # session mode often true
device = ""                  # empty = default input
```

## Guarantees

| Property | Behavior |
|----------|----------|
| Non-blocking screen | Audio on own task + audio thread |
| Backpressure | Bounded queue; drop oldest / count drops |
| Restart recovery | Chunks already in store survive; new session on boot |
| Privacy pause | No new audio while `privacy.paused` |
| Permissions | Degrade if no mic; do not crash daemon |

## Non-goals (this ship)

- System audio / BlackHole / ScreenCaptureKit audio  
- On-device ASR inside Navi (see [Lumen ASR](https://github.com/fakechris/lumen-asr))  
- Multi-device mixing  
- Cloud upload  

## Exit criteria

- Mic chunks land as `audio_chunk.v1` + WAV blobs  
- Screen + audio can run together  
- Finite `ticks` smoke works without hang  
- Unit tests cover session VAD and WAV framing without a real mic  
