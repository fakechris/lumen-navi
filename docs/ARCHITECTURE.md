# Lumen Navi — Target Architecture

> Preliminary. Evolve with the plan; keep the core seams stable.

## System shape

```
 ┌──────────────────────────────────────────────────────────────┐
 │                     Edge adapters                            │
 │  Chrome ext · coding-agent hooks · future IDE/IM plugins     │
 └───────────────┬──────────────────────────────┬───────────────┘
                 │  stable local protocol       │
                 ▼                              ▼
 ┌──────────────────────────────────────────────────────────────┐
 │                     lumen-daemon (Rust)                      │
 │  source registry · schedules · permissions · health          │
 │         │                                                    │
 │         ▼                                                    │
 │  lumen-intake  ──append──►  lumen-store  ◄──jobs── lumen-process
 │  (adapters)               (meta + blobs)         (enrichment)
 └──────────────────────────────────────────────────────────────┘
                 │
                 ▼
        UI / search / agent context (later)
```

## Crates

| Crate | Responsibility |
|-------|----------------|
| `lumen-types` | Domain types: source id, event envelope, artifact refs, process job kinds |
| `lumen-intake` | `Source` trait, intake runtime, backpressure, basic adapters |
| `lumen-store` | Persistence API: events, artifacts, retention, wipe |
| `lumen-process` | Processors & job orchestration (OCR/ASR/summary later) |
| `lumen-daemon` | Binary entry: config load, run sources, expose local API |

Future (not scaffolded yet): `lumen-platform-macos`, desktop app, shared media codecs.

## Event model (v0 intent)

Every source emits a **SourceEvent**:

- `id` — UUID
- `source` — enum/string (`screen`, `audio`, `video`, `browser`, `coding_agent`, …)
- `kind` — finer event type (`screenshot`, `page_visit`, `transcript_chunk`, …)
- `ts` — capture time (UTC)
- `session_id` — optional grouping
- `payload` — structured JSON metadata (URL, app bundle, titles, …)
- `artifacts` — zero or more blob references (image/audio/video/file)

**Rule:** processing never mutates the original event; it writes **derived records**.

## Intake design notes

- Sources run independently; one failing source must not stall others.
- Capture path should be cheap: write disk + metadata first, process async.
- Prefer content hashes for dedup of identical screenshots/pages.
- Sensitive apps / private browsing / pause switch must short-circuit before storage when configured.

## Browser extension path

```
Chrome MV3
  → (native messaging | loopback HTTPS)
  → daemon intake adapter
  → store
```

Extension responsibilities: observe navigation/focus/visibility with user consent.  
Daemon responsibilities: auth of local clients, schema validation, retention, linking to other sources by time.

## Coding-agent path

Agents are just another source:

- Ingest exported transcripts / session files / hooks
- Normalize to `SourceEvent{ source: coding_agent, kind: message|tool_call|... }`
- Later: reverse direction — export Navi context *into* agents (out of scope for Phase 0–2)

## Privacy seams

| Seam | Where |
|------|-------|
| Consent | OS permissions + per-source toggles |
| Capture filter | intake (deny list before write) |
| Redaction | process stage (before export/cloud) |
| Retention | store policies |
| Wipe | store + blob filesystem |

## What is intentionally not specified yet

- Exact SQLite schema
- Exact Chrome message schema
- Choice of OCR/ASR engines
- Desktop UI toolkit

Those land when the corresponding phase starts, with tests.
