# Lumen Navi — Core Skeleton Architecture

> Status: **frozen skeleton** (2026-07-11)  
> Repo: https://github.com/fakechris/lumen-navi  
> Goal: seams stable enough that screen/media, later Chrome, agents, and UI grow *on top* without rewiring the core.

---

## 1. Product stance

| Decision | Choice |
|----------|--------|
| Core language | Rust daemon + library crates |
| First vertical | **Screen + media** (screenshot, audio, optional video) |
| Browser | **Later** (Chrome extension after media path is real) |
| Coding agents | Later edge adapters |
| Desktop UI | After daemon + media intake prove durable |
| Privacy | Local-first; pause / wipe / deny-list first-class |

Media-first is intentional: continuous capture is the reliability spine. Chrome is an edge protocol problem and waits until Observe→Memory is boring and stable.

---

## 2. External projects (not in-tree)

### 2.1 Lumen ASR — separate product

- **Repo:** https://github.com/fakechris/lumen-asr  
- **Role:** voice dictation (hotkey → ASR → correct → inject).  
- **To Navi:** remains a **separate product**. May later become an intake source or share *patterns* (sherpa-onnx, permission ports, SQLite discipline). **Do not merge monorepos.**

Patterns borrowed (not code-coupled):

- Core crates have **no UI deps**
- **Ports over providers**
- **Fail soft** on enrichment — never drop raw capture because OCR/ASR failed
- `platform` + `platform-macos` split

### 2.2 cua-driver — open-source Act plane only

- **Upstream:** [trycua/cua](https://github.com/trycua/cua) · **`cua-driver` MIT**  
- **Role:** optional computer-use *action* (click/type/launch without stealing focus).  
- **Not intake.** Observe (screenshots/audio) is Navi-owned; cua-driver is for *doing*, not *watching*.  
- **Integration (later):** bundle/spawn MIT binary or MCP/CLI; thin `lumen-act` client.  
- **License line:** **cua-driver only** — never `cua-agent[omni]` (AGPL contagion). OCR stays Navi-owned.

```
Observe plane (Navi, media-first)
     │ events + blobs
     ▼
Memory plane (store + process)
     │
     ▼ optional later
Act plane ──► open-source cua-driver (MIT)
```

---

## 3. Principles

1. **Three planes** — Observe / Memory / Act. Act is late and optional.  
2. **One event envelope** — every source → `SourceEvent`; processors never mutate raw.  
3. **Capture never waits on enrich** — blob + meta first; jobs async.  
4. **Sources are supervised peers** — one failure must not stall others.  
5. **Platform behind ports** — no `#[cfg]` soup in intake/store/process.  
6. **Policy before write** — pause, deny-list, retention gate intake.  
7. **Derived data is append-only** — OCR/transcripts/summaries = new records + jobs.  
8. **Edge adapters use a stable local API** — Chrome/agents later plug in without core rewrites.  
9. **Core has zero UI dependency**.  
10. **Cheap signals first** — frontmost app, hashes, VAD before heavy media when possible.

---

## 4. System shape

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Surfaces (later)  Desktop · CLI · Agent context export                   │
└───────────────────────────────┬──────────────────────────────────────────┘
                                │ local control API (UDS preferred)
┌───────────────────────────────▼──────────────────────────────────────────┐
│ lumen-daemon                                                             │
│  config · permissions · health · SourceSupervisor · JobRunner · PolicyGate│
└─────┬───────────────────────┬───────────────────────────┬────────────────┘
      ▼                       ▼                           ▼
┌─────────────┐       ┌───────────────┐           ┌────────────────┐
│ lumen-intake│       │ lumen-store   │           │ lumen-process  │
│ Source      │──────►│ EventStore    │◄──jobs───│ Processor      │
│ Supervisor  │ blobs │ BlobStore     │           │ JobQueue       │
│ PolicyGate  │──────►│ JobStore      │           │                │
└──────┬──────┘       └───────────────┘           └────────────────┘
       │
       ├─ lumen-sources-media (FIRST): screen · audio · video
       ├─ browser (LATER)
       ├─ coding_agent (LATER)
       └─ lumen_asr bridge (OPTIONAL later)
                                │
                                ▼ optional
                      lumen-act → cua-driver (MIT)
```

---

## 5. Crate map

### Core (change only with migration notes)

| Crate | Responsibility |
|-------|----------------|
| `lumen-types` | `SourceKind`, `SourceEvent`, `ArtifactRef`, job/derived types |
| `lumen-config` | Config load, feature flags, retention defaults |
| `lumen-platform` | Ports: permissions, frontmost app, screen/audio capturers |
| `lumen-platform-macos` | macOS implementations |
| `lumen-intake` | `Source`, sink, supervisor, policy gate |
| `lumen-store` | Event / blob / job persistence APIs |
| `lumen-process` | Processors + job orchestration |
| `lumen-api` | Versioned local control/RPC schema |
| `lumen-daemon` | Thin binary wiring |

### Growth (add without rewriting core)

| Crate | When |
|-------|------|
| `lumen-sources-media` | **Now** — screen / audio / video |
| `lumen-sources-browser` | Later |
| `lumen-sources-agent` | Later |
| `lumen-process-ocr` / `lumen-process-asr` | After media events exist |
| `lumen-act` | Optional act via cua-driver |
| `apps/desktop` | After media durability |

### Dependency direction

```
types ← config | platform | intake | store | process | api
intake ← sources-media
platform-macos → platform
daemon → config, platform-*, intake, sources-media, store, process, api
```

No cycles. Process depends on types (+ store APIs), **not** on sources.

---

## 6. Domain model (v1)

### SourceEvent

```text
SourceEvent {
  id, source, kind, ts, session_id?,
  payload: Json,          // per-kind, versioned (e.g. kind = "screenshot.v1")
  artifacts: [ArtifactRef]
}
```

### ArtifactRef

```text
id, media_type, path (relative), bytes?, content_hash?  // BLAKE3 preferred
```

### Blob layout

```text
$data_dir/
  blobs/<aa>/<bb>/<blake3-hex>   # content-addressed; aa/bb = first 4 hex chars
  meta/navi.db                   # events, artifacts, jobs, derived, kv
  tmp/                           # atomic write staging (*.part → rename)
```

Implemented by `lumen_store::SqliteStore` + `BlobStore` (Phase S1).

### Screen capture (Phase S2)

- Port: `ScreenCapturer` / `MacScreenCapturer` (`CGDisplayCreateImage` → PNG)
- Adapter: `lumen_sources_media::ScreenSource::capture_tick` (interval + `pixel_hash` dedup)
- Payload kind: `screenshot.v1` with frontmost app metadata
- **Not** cua-driver — observe plane only; Act plane remains optional/later

### Jobs & derived

```text
Job { id, event_id, kind, status, attempts, last_error, updated_at }
DerivedRecord { id, event_id, kind, body, created_at }
```

Media-first job kinds: `ocr_screen`, `transcribe_audio`, `segment_activity`, `redact` (later).

### Media payload intent

| Kind | Fields (intent) |
|------|-----------------|
| `screenshot.v1` | app_name, bundle_id, window_title, display_id, bounds, pixel_hash, reason |
| `audio_chunk.v1` / `audio_session.v1` | device, sample_rate, channels, duration_ms, mode, vad? |
| `video_segment.v1` | display_id, duration_ms, codec, linked_screenshot_ids? |

---

## 7. Runtime

### Boot

1. Load config + data dir  
2. Open store (migrate)  
3. Check permissions — degrade, don’t crash  
4. Build PolicyGate  
5. Register enabled sources (media first)  
6. Start supervisor + job runner  
7. Serve local API  
8. Shutdown: stop sources → flush → close DB  

### PolicyGate (before store)

1. Global pause → 2. Source disabled → 3. App deny-list → 4. Disk budget → 5. Cheap dedup (`pixel_hash` window)

### Local API (minimal)

`health` · `search_ocr` · `reindex_ocr` · `list_events` · `wipe` · `pause`/`resume` · `permissions`  

Transport **now**: loopback HTTP (`127.0.0.1:7420` by default). UDS preferred later for desktop UI. Chrome reuses the same host schema (`lumen-api`).

### Source style

- Simple/test sources: `poll`  
- Long-lived media: **push** via channel into supervisor (preferred for streams)

---

## 8. Success criteria (skeleton stable)

1. New source = adapter + config flag + register — **no** store/process rewrite.  
2. Screen + audio 1h unattended with restart recovery.  
3. Processors fail/retry without losing raw events.  
4. Chrome later = edge adapter + API client only.  
5. cua-driver act path wireable without touching Observe/Memory traits.

---

## 9. Non-goals (skeleton)

- Porting old Go desktop structure as architecture  
- Chrome in the first durable media milestone  
- Reverse-engineering docs in this repo  
- Full agent/memory product surfaces before intake reliability  
- AGPL computer-use stacks  
- Merging with https://github.com/fakechris/lumen-asr  
