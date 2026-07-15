# Audio + Observe ASR — Product Spec (S3)

> Product documentation only. Mic-first Observe intake + async transcription.

## Goal

1. Capture **microphone** audio into durable `audio_chunk.v1` events **concurrently with screen**.
2. Transcribe chunks **asynchronously** into `transcript.v1` (same job pattern as OCR).
3. Make transcripts **searchable** via the existing FTS / control API.

System audio loopback and **dictation UI** are out of scope.

| Product | Role |
|---------|------|
| **Lumen Navi** (this repo) | Continuous Observe: store audio + background ASR enrichment |
| **[Lumen ASR](https://github.com/fakechris/lumen-asr)** | Separate dictation product (hotkey → correct → inject) |

Do **not** merge monorepos. Navi may later share *patterns* (16 kHz mono, sherpa) but owns its `AsrEngine` port.

## Timing alignment (reference)

Defaults match the product reference capture path (native 16 kHz mono / Lumen ASR sample rate family):

| Knob | Default | Notes |
|------|---------|--------|
| `sample_rate` | **16000** | Preferred mic open; device may negotiate |
| `channels` | **1** | Mono |
| `chunk_ms` | **3000** | Continuous Observe window (ASR-friendly) |
| `max_chunk_ms` | **30000** | Hard cap per chunk |
| `session_silence_ms` | **1200** | End utterance / session after quiet |
| `max_session_ms` | **600000** | 10 min force-close / roll session id |
| `vad_rms_threshold` | **0.01** | Energy VAD |
| `max_audio_bytes` | **8 MiB** | Intake + ASR size guard |

## Pipeline

```
mic (cpal stream on audio thread)
        ↓
  chunk every chunk_ms (PCM → WAV)  + privacy / VAD / size gates
        ↓
  store event + audio/wav blob
        ↓
  enqueue job transcribe_audio (deduped if open)
        ↓
  TranscribeWorker (async, never on capture path)
        ↓
  derived transcript.v1 → ocr_docs FTS (searchable)
```

## Modes

| Mode | Behavior |
|------|----------|
| `continuous` | Fixed-duration chunks; session id rolls every `max_session_ms` |
| `session` | Open on voice (RMS ≥ threshold); close after silence or `max_session_ms` |

## Payloads

### `audio_chunk.v1`

```json
{
  "payload_version": 1,
  "device": "MacBook Pro Microphone",
  "sample_rate": 48000,
  "channels": 1,
  "duration_ms": 3000,
  "samples": 144000,
  "mode": "continuous",
  "rms": 0.02,
  "peak": 0.4,
  "format": "wav_s16le",
  "session_ordinal": 3,
  "voice": true
}
```

Artifact: `audio/wav` (mono s16le + RIFF header).

### `transcript.v1`

```json
{
  "payload_version": 1,
  "event_id": "...",
  "text": "...",
  "confidence": 0.0,
  "language": "zh-CN",
  "engine": "speech",
  "audio_bytes": 12345,
  "audio_blake3": "..."
}
```

## Config

```toml
[sources]
audio = true

[audio]
mode = "continuous"          # continuous | session
sample_rate = 16000
channels = 1
chunk_ms = 3000
max_chunk_ms = 30000
queue_capacity = 8
ticks = 0                    # 0 = until stop; >0 finite chunks (smoke)
session_silence_ms = 1200
max_session_ms = 600000
vad_rms_threshold = 0.01
drop_silent_chunks = false
max_audio_bytes = 8388608
device = ""
enqueue_transcribe = true

[asr]
enabled = true
locale = "zh-CN"
poll_interval_ms = 1500
batch_size = 1
max_attempts = 5
retry_base_ms = 2000
retry_max_ms = 60000
timeout_ms = 120000
stale_running_ms = 300000
max_audio_bytes = 8388608
max_text_chars = 200000
shutdown_drain_ms = 30000
```

## Engine

Default: **macOS Speech.framework** (`MacSpeechAsr`) — file-based recognition, serialized, size-guarded.

- Fail soft: raw audio is kept even if ASR fails.
- Authorization: system Speech Recognition permission (separate from mic).
- Tests use `StubAsr` (no live mic / Speech).

## Search

`insert_derived(..., "transcript.v1")` upserts the same `ocr_docs` + FTS5 index as OCR.

```bash
curl -s 'http://127.0.0.1:7420/v1/ocr/search?q=会议&limit=10' | jq .
```

## Guarantees

| Property | Behavior |
|----------|----------|
| Non-blocking screen | Audio + ASR on own tasks / threads |
| Non-blocking capture | Capture only enqueues; never calls Speech |
| Idempotency | One open `transcribe_audio` job per event; one `transcript.v1` |
| Retry | Exponential backoff via `available_at` |
| Stuck jobs | Reclaim `running` older than `stale_running_ms` |
| Privacy pause | No new audio while `privacy.paused` |

## Non-goals (this ship)

- System audio / BlackHole / ScreenCaptureKit audio  
- Dictation hotkey / inject (Lumen ASR)  
- Cloud ASR  
- Speaker diarization  

## Exit criteria

- Mic chunks land as `audio_chunk.v1` + WAV blobs  
- Chunks enqueue `transcribe_audio`; worker writes `transcript.v1`  
- Transcripts appear in FTS search  
- Screen + audio + OCR + ASR can run together without blocking  
- Unit tests cover VAD/session, WAV framing, and ASR job path without live Speech  
