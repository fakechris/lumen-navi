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
│   ├── lumen-context          # Shared local context capture library
│   ├── lumen-platform         # OS capability ports
│   ├── lumen-platform-macos   # macOS implementations
│   ├── lumen-intake           # Source runtime, supervisor, policy
│   ├── lumen-sources-media    # Screen / audio / video adapters (first)
│   ├── lumen-store            # Events + blobs + jobs
│   ├── lumen-process          # Enrichment jobs
│   ├── lumen-api              # Versioned local control API schema
│   └── lumen-daemon           # Long-running entrypoint
├── apps/                      # Desktop later
├── extensions/                # Chromium/Safari context bridge
└── docs/
```

## Shared context library and ASR dependency

`crates/lumen-context` is the single shared context-capture library. It owns the versioned capture
contract and the reusable macOS AX, screenshot, Vision OCR, browser bridge, Visible Text fusion,
privacy-policy, and encryption primitives.

The relationship is intentionally one-way:

```text
lumen-navi repository
└── crates/lumen-context
        ▲
        └── Lumen ASR build-time Git dependency (pinned commit)
```

- Navi builds the crate directly as a workspace member.
- Lumen ASR downloads the crate from this repository at the exact commit recorded in the ASR
  `Cargo.toml` and `Cargo.lock`.
- ASR does **not** require the Navi app, daemon, database, socket, or a sibling Navi checkout at
  runtime or for a normal build.
- Navi does not depend on ASR. Each application owns its configuration, persistence, browser
  credentials, and Safari App Group.

The current shared-library release marker is `lumen-context-v0.1.0`. The pinned commit, rather than
the movable branch name, is the build source of truth.

## Quick start

```bash
cargo build --workspace --locked
cargo test --workspace --locked
cargo run --locked -p lumen-daemon
```

Build only the shared library and its helper binaries:

```bash
cargo build -p lumen-context --locked
cargo build -p lumen-context --bins --locked
```

Build and test the optional browser extension:

```bash
cd extensions/context-browser
npm ci
npm test
npm run check
```

Requires Rust stable (edition 2021+). macOS capture builds require Xcode Command Line Tools; the
browser extension additionally requires Node.js/npm.

When changing the shared contract, publish Navi first, then update ASR to the new full Git commit
and regenerate the ASR `Cargo.lock`. Do not point ASR at a branch or require a sibling checkout.

## Related projects

| Project | Link | Relationship |
|---------|------|----------------|
| **Lumen ASR** | https://github.com/fakechris/lumen-asr | Separate **voice dictation** product. It consumes `lumen-context` as an exact build-time Git dependency, with no Navi runtime dependency; it is **not** merged into this monorepo. |
| **cua-driver** | https://github.com/trycua/cua | Open-source **MIT** computer-use driver for the optional **Act** plane only. Observe/capture is Navi-owned. **Do not** use `cua-agent[omni]` (AGPL). |

## Priority order

1. Stable core skeleton (this phase)  
2. Screen + audio durability  
3. Light processing (OCR / ASR jobs)  
4. Chrome extension & other edges  
5. Optional Act via cua-driver · desktop UI  

## Status

**S2 Observe + S4 OCR MVP.** Capture is productized; Vision OCR runs **async** on `ocr_screen` jobs → `derived`/`ocr.v1`.

```bash
cargo run -p lumen-daemon   # data/ · Ctrl+C to stop (screen_ticks=0 default)
```

| Config | Notes |
|--------|--------|
| `capture.*` | multi-display, probe, debounce — see `docs/OBSERVE_CAPTURE.md` |
| `ocr.enabled` | default true; `ocr.languages` default `zh-Hans` + `en-US` |
| `privacy.closed_eyes` | hard stop on screen capture |

Grant **Screen Recording** if capture fails. **cua-driver is not used for capture/OCR.**
