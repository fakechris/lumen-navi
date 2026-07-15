# Audio + Observe ASR — Product Spec (S3)

> Product documentation only. Mic-first Observe intake + async multi-engine transcription.

## Goal

1. Capture **microphone** audio into durable `audio_chunk.v1` events **concurrently with screen**.
2. Transcribe chunks **asynchronously** into `transcript.v1` (same job pattern as OCR).
3. Make transcripts **searchable** via the existing FTS / control API.

System audio loopback and **dictation UI** are out of scope.

| Product | Role |
|---------|------|
| **Lumen Navi** (this repo) | Continuous Observe: store audio + background ASR enrichment |
| **[Lumen ASR](https://github.com/fakechris/lumen-asr)** | Separate dictation product (hotkey → correct → inject) |

Do **not** merge monorepos. Navi **reuses patterns** (16 kHz mono, sherpa SenseVoice/Whisper, OpenAI-compatible HTTP) via crate `lumen-asr-engine`, and owns its `AsrEngine` port (`WAV → text`).

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
  AsrEngine (sensevoice | whisper | speech | openai_audio/qwen)
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
  "engine": "sensevoice",
  "audio_bytes": 12345,
  "audio_blake3": "..."
}
```

`engine` values: `sensevoice` | `whisper` | `speech` | `openai_audio` | `qwen_asr` | `stub`.

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
system_audio = false         # reserved (ScreenCaptureKit); mic-only for now

[asr]
enabled = true
# sensevoice | whisper | speech | openai_audio | qwen
engine = "sensevoice"
model_dir = ""               # empty = auto-resolve
locale = "zh-CN"
fallback_speech = true       # if offline model missing → Speech.framework
# --- HTTP engines (openai_audio / qwen) ---
http_base_url = ""           # e.g. https://dashscope.aliyuncs.com/compatible-mode/v1
http_api_key = ""            # or env LUMEN_NAVI_ASR_API_KEY / OPENAI_API_KEY
http_model = "qwen3-asr-0.8b"
http_engine_label = ""       # empty = auto (qwen_asr if dashscope/qwen)
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

## Engines

Default: **SenseVoice** (local sherpa-onnx) — same model family as Lumen ASR.

| Engine | Backend | When to use |
|--------|---------|-------------|
| `sensevoice` | sherpa-onnx SenseVoice | Default continuous Observe (zh/en/ja/ko/yue) |
| `whisper` | sherpa-onnx Whisper | English-heavy offline |
| `speech` | macOS Speech.framework | No local model; Apple permission |
| `openai_audio` / `qwen` | HTTP `POST …/audio/transcriptions` | Qwen ASR 0.8B, Whisper API, local OpenAI-compat server |

### Model paths (SenseVoice / Whisper)

Resolution order:

1. `asr.model_dir` if set  
2. `LUMEN_NAVI_SENSEVOICE_DIR` / `LUMEN_SENSEVOICE_DIR` (or Whisper equivalents)  
3. `~/Library/Application Support/LumenNavi/models/sensevoice`  
4. Shared caches: LumenAsr app models, `~/.coli/models/sherpa-onnx-sense-voice-…`

SenseVoice package (int8, from sherpa releases):

```
https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
```

Unpack so the directory contains `model.int8.onnx` (or `model.onnx`) + `tokens.txt`.

### Desktop onboarding (model select / download)

First-run wizard includes a **本地 ASR 模型** step (after mic permission), patterned after Lumen ASR:

| Action | Effect |
|--------|--------|
| Choose engine | `sensevoice` / `whisper` / `speech` → written to `navi.toml` |
| Pick detected candidate | Sets `asr.model_dir` + engine |
| Paste path + validate | Same, after `model*.onnx` / Whisper layout check |
| **下载 SenseVoice** | curl + tar into `~/Library/Application Support/LumenNavi/models/sensevoice/` |
| Skip | Continue without local model (`fallback_speech` still available) |

Progress events: Tauri event `asr-download-progress`. Cancel via `cancel_asr_model_download`.

### Qwen ASR 0.8B (HTTP)

There is no sherpa-onnx Qwen port in-tree. Use an **OpenAI-compatible** transcription endpoint:

```toml
[asr]
engine = "qwen"   # or openai_audio
http_base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
http_model = "qwen3-asr-0.8b"   # or your server's model id
# http_api_key via config or LUMEN_NAVI_ASR_API_KEY
```

Any server that implements `POST /v1/audio/transcriptions` (multipart file + model) works — including a local process wrapping Qwen ASR 0.8B.

### Fallback

If `engine = sensevoice|whisper` and the model is missing, `fallback_speech = true` (default) starts **macOS Speech** so continuous capture still produces transcripts.

## Search

`insert_derived(..., "transcript.v1")` upserts the same `ocr_docs` + FTS5 index as OCR.

```bash
curl -s 'http://127.0.0.1:7420/v1/ocr/search?q=会议&limit=10' | jq .
```

## Guarantees

| Property | Behavior |
|----------|----------|
| Non-blocking screen | Audio + ASR on own tasks / threads |
| Non-blocking capture | Capture only enqueues; never calls ASR engine |
| Idempotency | One open `transcribe_audio` job per event; one `transcript.v1` |
| Retry | Exponential backoff via `available_at` |
| Stuck jobs | Reclaim `running` older than `stale_running_ms` |
| Privacy pause | No new audio while `privacy.paused` |

## System audio (P1 reserved)

`audio.system_audio = true` is accepted in config / desktop settings but **not captured yet**.  
Planned path: ScreenCaptureKit shareable content (macOS 13+), independent of mic stream.

## Non-goals (this ship)

- BlackHole / third-party loopback drivers  
- Dictation hotkey / inject (Lumen ASR)  
- Speaker diarization  
- Bundling multi-hundred-MB models inside the DMG  

## Exit criteria

- Mic chunks land as `audio_chunk.v1` + WAV blobs  
- Chunks enqueue `transcribe_audio`; worker writes `transcript.v1`  
- Engine selectable: SenseVoice (default) / Whisper / Speech / OpenAI-compatible (Qwen)  
- Transcripts appear in FTS search  
- Screen + audio + OCR + ASR can run together without blocking  
- Unit tests cover VAD/session, WAV framing, and ASR job path without live Speech  
