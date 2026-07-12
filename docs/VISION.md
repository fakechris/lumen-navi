# Lumen Navi — Vision

> Recorded: 2026-07-11  
> Status: preliminary product framing (greenfield)

## Why this exists

Digital work is fragmented across screens, tabs, meetings, and tools. Useful context is created continuously and lost almost immediately. Lumen Navi’s job is to **capture the stream locally**, with user control, and turn it into durable, queryable, privacy-respecting context.

This is a **serious product codebase**, started clean on Rust. Design choices are product-first and architecture-first — not constrained by any prior implementation language or structure.

## Core idea

```
 continuous multi-source intake
            │
            ▼
     normalize + store (raw)
            │
            ▼
   process / enrich / index
            │
            ▼
   memory, timeline, agents, UI
```

The **intake layer is the foundation**. Everything else — memory, search, agents, UI — is a consumer of a reliable multi-source event stream.

## Primary product loop

1. **Observe** — keep receiving signals the user allowed.
2. **Retain** — store raw + metadata with retention policies.
3. **Refine** — OCR, ASR, entity extraction, summarization, linking.
4. **Serve** — timeline, search, memory, agent context injection.

## Intake sources (initial intent)

| Source | What it captures | Notes |
|--------|------------------|-------|
| **Screen** | Periodic or event-driven screenshots | Continuous visual context |
| **Video** | Optional continuous or session video | Higher fidelity, higher cost |
| **Audio** | Mic / system audio as permitted | Meetings, dictation, ambient |
| **Browser** | Chrome extension: navigation, tabs, page signals | First-class web activity source |
| **Coding agents** | Conversation / tool transcripts | Claude Code, Codex, local agents… |
| **Future sources** | Calendar, IM, IDE, files, OS notifications… | Plug-in source model |

Sources are **pluggable**. Shipping a source does not require redesigning the core.

## Non-goals (for now)

- Cloning any specific competitor feature-for-feature
- Cloud-default storage of raw screen/audio without explicit product design
- Premature multi-platform shipping (design for macOS first, keep ports in mind)
- Building every processing feature before intake reliability is real

## Principles

1. **Local-first** — raw sensitive media stays on device by default.
2. **Consent-shaped** — every source is opt-in; stop must be easy.
3. **Source-agnostic core** — one event model, many adapters.
4. **Cheap signals first** — metadata and hashes before heavy media when possible.
5. **Process later** — raw capture should not block on enrichment.
6. **Privacy as product** — redaction / sensitive-app rules are first-class, not add-ons.
7. **Rust for the core** — intake daemon, store, and processing pipeline in Rust.
8. **Extensions at the edge** — browser / IDE adapters may use JS/TS, talking a stable protocol to the daemon.

## Relationship to Lumen ASR

Lumen ASR is a focused dictation product. Lumen Navi is a broader **context platform**. They may share ideas or crates later (audio pipelines, platform permissions), but Navi does **not** inherit ASR’s product scope, and ASR does not block Navi’s architecture.

## Success criteria (directional)

- A user can enable 2+ sources and see a coherent local timeline of “what I was doing”.
- Adding a new source (e.g. a coding-agent adapter) is a weekend-sized task against a stable intake API.
- Heavy processing failures never drop raw capture.
- The user can pause, wipe, and export their data.
