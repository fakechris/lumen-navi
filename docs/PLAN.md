# Lumen Navi — Preliminary Plan

> Recorded: 2026-07-11  
> Nature: working plan, not a frozen spec  
> Decision context: start a greenfield Rust project; multi-source continuous intake is the core

## 1. Strategic shift (product / engineering)

| Before (context only) | Now |
|-----------------------|-----|
| Exploratory / prototype-heavy work in other trees | **Serious product development** |
| Go desktop experiment as main line | **Rust core platform** |
| Single-app feature chasing | **Intake + process architecture first** |
| Entangled historical docs | **Clean repo; product docs only** |

This repo (`~/source/lumen-navi`) is the home for that new line of work.

## 2. What we are building first

### Must be true early

1. **A long-running local daemon** (`lumen-daemon`) that owns capture schedules and source lifecycle.
2. **A unified event envelope** (`lumen-types`) so every source looks the same downstream.
3. **Append-only-ish storage** for raw artifacts + metadata (`lumen-store`).
4. **Async processing workers** that can lag behind capture (`lumen-process`).
5. **At least one real source path end-to-end** (likely screen *or* browser first — decide in Phase 1).

### Explicitly multi-source from day one (design)

Even if implementation is staged, the **API assumes many sources**:

- Screen capture (screenshots)
- Video recording (optional / policy-gated)
- Audio recording
- Chrome extension (web navigation & page activity)
- Coding-agent conversation / session logs
- Future adapters without core rewrites

## 3. Phased roadmap

### Phase 0 — Scaffolding ✅ (this commit)

- [x] New repository workspace under `/Users/chris/source/lumen-navi`
- [x] Product vision + plan + architecture notes (no legacy research material)
- [x] Crate skeleton: types / intake / store / process / daemon
- [x] `cargo build` / `cargo test` green on stubs

### Phase 1 — Event core + store

- Stabilize `SourceEvent` / `ArtifactRef` schema
- SQLite (or equivalent) for metadata; filesystem for blobs
- Retention policy hooks (TTL, pause, wipe)
- Daemon boot, config, graceful shutdown
- In-memory or file-backed “source registry”

**Exit criteria:** daemon can append synthetic events and list them; restart preserves data.

### Phase 2 — First real intake path

Pick one primary slice (recommended order to validate architecture):

**Option A — Browser first (fast feedback)**  
Chrome MV3 extension → local daemon (HTTP/WebSocket/native messaging) → store.

**Option B — Screen first (platform hard parts early)**  
macOS screenshot schedule → store → optional OCR job later.

Decision rule: if the goal is **multi-source product story** and UI demo, prefer A; if the goal is **prove continuous media pipeline**, prefer B. Default recommendation: **A then B** (protocol first, media second).

**Exit criteria:** one real source writes events continuously for 30+ minutes without data loss.

### Phase 3 — Media sources (screen / audio / video)

- Screen: interval + idle/active heuristics; app denylist
- Audio: session-based and continuous modes; permission UX
- Video: optional; same retention and cost controls as screen
- Artifact lifecycle: write → index → process → compact/archive

**Exit criteria:** concurrent screen + audio capture with separate retention policies.

### Phase 4 — Processing layer

- Pluggable processors: OCR, ASR, summarization, entity extraction
- Job queue with retries / dead-letter
- Derived views: sessions, page visits, coding sessions
- Privacy processors (redaction) before any export or cloud path

**Exit criteria:** raw event → derived “activity segment” available via store API.

### Phase 5 — Surfaces & agents

- Local timeline / search UI (desktop later)
- Coding-agent adapters (export context in; ingest transcripts out)
- Optional integration with Lumen ASR as an audio/text source
- Policy for external LLM use (BYO keys, local-only mode)

## 4. Repo boundaries

```
~/source/lumen-navi     ← this product (Rust core + extensions)
~/source/lumen-asr      ← separate dictation product (may share ideas later)
```

**Hard rule for this repo**

- Product, architecture, and engineering docs only.
- No reverse-engineering notes, binary dumps, or competitor decompilation material.
- No carrying over old monorepo structure by default; design for Navi’s intake model.

## 5. Open decisions (to resolve soon)

| Topic | Options | Owner trigger |
|-------|---------|---------------|
| First real source | Browser vs screen | Phase 1 exit |
| Local IPC | HTTP loopback / UDS / native messaging | Before Chrome extension |
| Blob store layout | content-addressed vs time-partitioned | Before media capture |
| Desktop shell | Tauri later vs daemon-only first | After Phase 2 |
| Identity / “who” model | deferred vs early principal table | After multi-source demos |

## 6. Near-term engineering norms

- Small, compiling increments; tests for schema & store first.
- Feature flags for expensive capture modes.
- macOS permissions treated as product surface, not afterthought.
- Chinese + English product copy OK; code identifiers English.

## 7. Immediate next actions (when development resumes)

1. Freeze v0 `SourceEvent` fields with tests.
2. Implement store append + list + wipe.
3. Choose Phase 2 source (browser recommended) and define the wire protocol.
4. Scaffold `extensions/chrome` only after protocol draft exists.
