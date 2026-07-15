# Soak checklist (P0)

Manual long-run verification for Observe reliability. Target: **≥ 1 hour** unattended with screen + mic + OCR + ASR enabled.

## Prep

```bash
cargo build -p lumen-daemon --release
# or use desktop app "Start Observe" with release daemon nearby
```

Config (`~/Library/Application Support/LumenNavi/navi.toml` or cwd `navi.toml`):

```toml
[sources]
screen = true
audio = true

[capture]
screen_ticks = 0

[audio]
ticks = 0
chunk_ms = 3000

[ocr]
enabled = true

[asr]
enabled = true

[privacy]
paused = false
closed_eyes = false
```

Permissions: Screen Recording, Microphone, Speech Recognition.

## Run

1. Start Observe (desktop or `cargo run -p lumen-daemon --release` with data_dir set).
2. Use the machine normally for **60+ minutes** (app switches, meetings optional).
3. Leave overnight optional.

## Pass criteria

| Check | How |
|-------|-----|
| Process still alive | desktop tray shows Running / no crash loop in logs |
| Events grow | Overview Events count increases |
| Screenshots land | Activity filter `screenshot` has rows + thumbnails |
| Audio chunks land | filter `audio_chunk` |
| OCR text | search finds recent on-screen words |
| Transcripts | search finds spoken phrases (if Speech authorized) |
| Disk not unbounded | `data_dir` size reasonable; no runaway logs |
| No permanent job death flood | `logs/daemon.stderr.log` — few `dead` OCR/ASR |

## Logs

- Desktop: `~/Library/Application Support/LumenNavi/logs/daemon.*.log`
- Counts: Overview cards + Activity filters

## After soak

- Generate day summary from Activity tab → appears as `summary.v1` + searchable text.
- Note any drop rates / permission prompts / heat for follow-up.
