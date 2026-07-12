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
| **Observe** | Multi-source intake | Media-first (screen / audio / video) |
| **Memory** | Durable store + async process | Core skeleton |
| **Act** | Optional computer-use | Later, via open-source **cua-driver** (MIT) |

Full write-up: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) · roadmap: [`docs/PLAN.md`](docs/PLAN.md) · vision: [`docs/VISION.md`](docs/VISION.md)

## Workspace

```
lumen-navi/
├── crates/
│   ├── lumen-types            # Event envelope & shared types
│   ├── lumen-config           # Config / flags / retention defaults
│   ├── lumen-platform         # OS capability ports
│   ├── lumen-platform-macos   # macOS implementations
│   ├── lumen-intake           # Source runtime, supervisor, policy
│   ├── lumen-sources-media    # Screen / audio / video adapters (first)
│   ├── lumen-store            # Events + blobs + jobs
│   ├── lumen-process          # Enrichment jobs
│   ├── lumen-api              # Versioned local control API schema
│   └── lumen-daemon           # Long-running entrypoint
├── apps/                      # Desktop later
├── extensions/                # Chrome later
└── docs/
```

## Quick start

```bash
cargo build
cargo test
cargo run -p lumen-daemon
```

Requires Rust stable (edition 2021+).

## Related projects

| Project | Link | Relationship |
|---------|------|----------------|
| **Lumen ASR** | https://github.com/fakechris/lumen-asr | Separate **voice dictation** product. May later become an intake source or share engine *patterns*; **not** merged into this monorepo. |
| **cua-driver** | https://github.com/trycua/cua | Open-source **MIT** computer-use driver for the optional **Act** plane only. Observe/capture is Navi-owned. **Do not** use `cua-agent[omni]` (AGPL). |

## Priority order

1. Stable core skeleton (this phase)  
2. Screen + audio durability  
3. Light processing (OCR / ASR jobs)  
4. Chrome extension & other edges  
5. Optional Act via cua-driver · desktop UI  

## Status

**Phase S2 complete — product Observe capture.** Multi-display, focus triggers, grayscale change detection, debounce, screen-lock / closed-eyes gates, backpressure, activity sessions. Design: [`docs/OBSERVE_CAPTURE.md`](docs/OBSERVE_CAPTURE.md).

```bash
cargo run -p lumen-daemon   # data/ · Ctrl+C to stop (screen_ticks=0 default)
# navi.toml: capture.displays=all|main, probe_scale, visual_change_threshold,
#            privacy.closed_eyes, encode=jpeg|png
```

Grant **Screen Recording** if capture fails. **OCR is the next step** (jobs already enqueued as `ocr_screen`). **cua-driver is not used for capture.**
