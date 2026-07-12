# Lumen Navi — Roadmap

> Greenfield Rust · media-first · https://github.com/fakechris/lumen-navi  
> Related: [Lumen ASR](https://github.com/fakechris/lumen-asr) (separate) · [cua-driver](https://github.com/trycua/cua) (optional Act, MIT only)

## Locked priorities

1. **Screen + media intake** before browser  
2. **Stable core skeleton** before product surfaces  
3. Chrome / coding agents / UI **after** Observe→Memory is durable  
4. Optional **Act** via open-source **cua-driver** (MIT) — never blocks capture  

---

## Phase S0 — Skeleton freeze ✅

- [x] Greenfield workspace + Phase 0 stubs  
- [x] Architecture freeze (three planes, media-first)  
- [x] External refs: lumen-asr GitHub, cua-driver MIT boundary  
- [x] Crate shells: config, platform, platform-macos, sources-media, api  
- [x] Shells wired into daemon smoke  

**Exit:** docs + compile-green skeleton matching `docs/ARCHITECTURE.md`.

---

## Phase S1 — Store durability ✅

- [x] SQLite: `events`, `artifacts`, `jobs`, `derived`, `kv` (`meta/navi.db`)  
- [x] Content-addressed blobs (`blobs/ab/cd/<blake3>`) + atomic temp→rename  
- [x] Transactional append; wipe; reopen/restart tests  
- [x] Daemon opens durable store, writes boot + smoke events  

**Exit:** synthetic screen events survive restart.

---

## Phase S2 — Screen Observe (product-complete) ✅

Design: [`docs/OBSERVE_CAPTURE.md`](OBSERVE_CAPTURE.md)

- [x] Multi-display (`all` | `main`) via `CGGetActiveDisplayList`  
- [x] Focus / title change triggers + adaptive debounce (1s / 3s, same-app 10s)  
- [x] Grayscale visual probe at 1/`probe_scale` (default 6), threshold 0.05  
- [x] Gates: pause, closed_eyes, screen lock  
- [x] Backpressure (bounded persist queue)  
- [x] Activity sessions (`activity_sessions` table) + idle close  
- [x] JPEG default encode (q=75) + max edge  
- [x] **No cua-driver** on Observe path  
- [ ] Long-run soak (manual)

**OCR / Vision = Phase S4 (next product step), deliberately not mixed into capture.**

---

## Phase S3 — Audio source ✅

Product: [`docs/AUDIO_PRODUCT.md`](AUDIO_PRODUCT.md)

- [x] Mic path via cpal (dedicated audio thread; stream `!Send` isolated)  
- [x] Continuous + session (VAD/RMS) modes; `session_id` grouping  
- [x] `audio_chunk.v1` + WAV CA blobs; independent `sources.audio`  
- [x] Concurrent with screen; privacy pause; bounded backpressure  
- [x] Unit tests with synthetic PCM (no live mic required)  
- [ ] Long-run soak (manual)  
- [ ] System audio loopback (later)  

**Exit:** concurrent screen + audio durable intake; restart keeps stored chunks.

---

## Phase S4 — Vision OCR (product) ✅

Product: [`docs/OCR_PRODUCT.md`](OCR_PRODUCT.md)

- [x] Vision engine with real errors, size guards, global serialization  
- [x] Job state machine: dedupe open jobs, backoff retry, stale reclaim, timeouts  
- [x] Idempotent `ocr.v1` derived upsert  
- [x] Config surface complete (batch, boxes policy, limits, drain)  
- [x] Unit tests for worker + store job semantics  
- [x] Schema v4 `ocr_docs` + FTS5; reindex; search API  
- [x] Local control API (loopback): health + OCR search + reindex  
- [ ] (S4.1) optional OCR helper process isolation  
- [ ] Desktop / timeline search UI (U1)  

**Exit:** production-hardened OCR path; capture remains non-blocking; OCR text searchable.

---

## Phase S5 — Video (optional / gated)

- Same store/job model; feature-flagged; cost controls  

---

## Phase B1 — Chrome (explicitly later)

- MV3 extension + native messaging / loopback  
- `SourceKind::Browser` only — **no core trait changes** if API stayed versioned  

---

## Phase A1 — Act (optional)

- Bundle/spawn **cua-driver** (MIT only from trycua/cua)  
- `lumen-act` thin client  
- Never block intake  

---

## Phase U1 — Surfaces

- Local timeline / search UI  
- Optional bridge: ingest from / export to [Lumen ASR](https://github.com/fakechris/lumen-asr)  
- Coding-agent transcript adapters  

---

## Repo boundaries

```
https://github.com/fakechris/lumen-navi   ← this product
https://github.com/fakechris/lumen-asr    ← dictation product (separate)
https://github.com/trycua/cua             ← cua-driver only (MIT Act plane)
```

**Hard rules**

- Product/architecture docs only — no reverse-engineering material.  
- No AGPL `cua-agent[omni]`.  
- No monorepo merge with lumen-asr by default.

---

## Defaults

| Topic | Default |
|-------|---------|
| Local API | Loopback HTTP `127.0.0.1:7420` (UDS later) |
| Hash | BLAKE3 |
| Image | JPEG default (q=75) |
| Screen trigger | 2–5s interval + focus change |
| Mic | Continuous chunks (default 5s); session+VAD optional |
| System audio | After mic |
| Desktop shell | After S3 |

---

## Next actions

1. ~~S0 / S1 / S2 / S3 audio / S4 OCR + FTS~~  
2. S4.1 OCR helper isolation (optional)  
3. Manual soak: screen + audio 1h  
4. U1 timeline / search UI (API ready)
