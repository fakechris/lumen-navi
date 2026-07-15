# Lumen Navi

Local-first **continuous context** platform.

Lumen Navi continuously ingests multi-modal signals (screen, audio, later browser & tools), stores them under clear privacy boundaries, and turns them into structured memory and actionable context.

**Greenfield Rust workspace** — https://github.com/fakechris/lumen-navi

## One-liner

**Keep watching what matters — screen and sound first — then make that stream useful.**

## Architecture (summary)

Three planes:

| Plane | Role | Status |
|-------|------|--------|
| **Observe** | Multi-source intake | Screen + mic productized |
| **Memory** | Durable store + async process | SQLite + FTS + jobs |
| **Act** | Optional computer-use | Later, via open-source **cua-driver** (MIT) |

Full write-up: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) · roadmap: [`docs/PLAN.md`](docs/PLAN.md) · vision: [`docs/VISION.md`](docs/VISION.md)

## Status (current)

| Phase | Status |
|-------|--------|
| S0–S1 skeleton + store | ✅ |
| S2 screen Observe | ✅ (manual soak open) |
| S3 audio + Observe ASR | ✅ (16 kHz / 3s chunks; Speech → `transcript.v1`) |
| S4 Vision OCR + FTS API | ✅ |
| **U1 Tauri Mac app** | ✅ shell (control + search + start/stop daemon) |
| S4.1 OCR helper isolation | optional later |
| System audio / Chrome / Act | later |

## Workspace

```
lumen-navi/
├── crates/          # daemon + libraries
├── apps/desktop/    # Tauri 2 Mac shell
├── extensions/      # Chrome later
└── docs/
```

## Quick start (daemon)

```bash
cargo build
cargo test
cargo run -p lumen-daemon
```

Requires Rust stable (edition 2021+). Grant **Screen Recording** / **Microphone** / **Speech Recognition** as needed.

```bash
# search while daemon is up
curl -s 'http://127.0.0.1:7420/v1/ocr/search?q=关键词&limit=5' | jq .
```

## Desktop (Mac app)

```bash
cargo build -p lumen-daemon --release
cd apps/desktop && npm install && npm run build
cargo run -p lumen-navi-desktop
# or: cd apps/desktop && npx tauri dev
```

See [`docs/DESKTOP.md`](docs/DESKTOP.md).

## Related projects

| Project | Link | Relationship |
|---------|------|----------------|
| **Lumen ASR** | https://github.com/fakechris/lumen-asr | Separate **voice dictation** product. Share patterns only; **not** merged. |
| **cua-driver** | https://github.com/trycua/cua | Open-source **MIT** computer-use for optional **Act**. Never for Observe. |

## Config highlights

| Key | Default |
|-----|---------|
| `capture.*` | multi-display, probe, debounce — `docs/OBSERVE_CAPTURE.md` |
| `audio.sample_rate` / `chunk_ms` | 16000 / 3000 |
| `asr.enabled` / `locale` | true / `zh-CN` |
| `ocr.enabled` | true |
| `api.bind` | `127.0.0.1:7420` |

**cua-driver is not used for capture/OCR/ASR.**
