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

## Phase S3 — Audio source

- Mic path first; system audio later if needed  
- Session vs continuous modes; `session_id` grouping  
- Independent enable flag alongside screen  

**Exit:** concurrent screen + audio for 1h with restart recovery.

---

## Phase S4 — Vision OCR (product step)

Product intent: [`docs/OCR_PRODUCT.md`](OCR_PRODUCT.md)  
*(Engine research notes stay outside this repo.)*

- [ ] In-process Vision engine (quality text + layout boxes)  
- [ ] Job worker for `ocr_screen` → `derived` `ocr.v1`  
- [ ] Languages default `zh-Hans` + `en-US`; concurrency gate  
- [ ] Never block capture  
- [ ] (S4.1) optional OCR helper process for isolation  

**Exit:** screenshot events get OCR text without slowing Observe loop.

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
| Local API | UDS (+ optional loopback HTTP later) |
| Hash | BLAKE3 |
| Image | PNG first |
| Screen trigger | 2–5s interval + focus change |
| System audio | After mic |
| Desktop shell | After S3 |

---

## Next actions

1. ~~S0 / S1 / S2 Observe capture product path~~  
2. **S4 Vision OCR** (job consumer for `ocr_screen` — complete product step)  
3. S3 audio (can parallelize after OCR or before)  
