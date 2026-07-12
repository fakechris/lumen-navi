# Lumen Navi

Local-first **continuous context** platform.

Lumen Navi continuously ingests multi-modal signals from the user’s digital life, stores them under clear privacy boundaries, and turns them into structured memory and actionable context for people and agents.

This repository is a **greenfield Rust workspace**. It is not a port of any previous prototype stack.

## Product one-liner

**Keep watching what matters — screen, sound, browser, and tools — then make that stream useful.**

## Workspace layout

```
lumen-navi/
├── crates/
│   ├── lumen-types      # Shared domain types & event envelope
│   ├── lumen-intake     # Source adapters + intake pipeline
│   ├── lumen-store      # Persistence (raw + derived)
│   ├── lumen-process    # Processing / enrichment jobs
│   └── lumen-daemon     # Long-running local daemon entrypoint
├── apps/                # Future desktop / CLI surfaces
├── extensions/          # Browser extensions (Chrome first)
└── docs/                # Product & architecture (no legacy research)
```

## Docs

| Doc | Purpose |
|-----|---------|
| [`docs/VISION.md`](docs/VISION.md) | Product vision and principles |
| [`docs/PLAN.md`](docs/PLAN.md) | Preliminary roadmap & scope |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Target system shape |

## Quick start

```bash
cargo build
cargo test
cargo run -p lumen-daemon
```

Requires Rust stable (edition 2021+).

## Related projects

- **Lumen ASR** (`~/source/lumen-asr`) — voice dictation product; may later become an *intake source* or share platform crates, but remains a separate product.

## Status

**Phase 0 — scaffolding & product framing.** Architecture is intentional; most adapters are stubs.
