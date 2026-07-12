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

## Phase S2 — Screen source (first real) ✅

- [x] macOS Screen Recording permission probe + request  
- [x] CoreGraphics main-display capture → PNG (optional max-edge downscale)  
- [x] Interval capture loop + pixel_hash dedup window  
- [x] Frontmost app metadata (NSWorkspace / osascript fallback)  
- [x] PNG blob + `screenshot.v1` into durable store  
- [ ] Focus-change trigger (optional enhancement)  
- [ ] Long-run 30–60 min soak (manual / later CI)  

**Exit (S2 code):** daemon captures real frames when TCC granted.  
**Remaining soak / focus-change:** tracked as follow-ups, not blocking S3.

---

## Phase S3 — Audio source

- Mic path first; system audio later if needed  
- Session vs continuous modes; `session_id` grouping  
- Independent enable flag alongside screen  

**Exit:** concurrent screen + audio for 1h with restart recovery.

---

## Phase S4 — Process pipeline (light)

- Enqueue jobs on append  
- OCR processor for screenshots (pluggable)  
- Optional ASR for audio (design-compatible with lumen-asr engines; no hard dep)  
- Derived records queryable  

**Exit:** raw event → at least one derived record without blocking capture.

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

1. ~~S0~~ / ~~S1~~ / ~~S2 screen capture~~  
2. **S3 audio source** (mic first)  
3. S4 process pipeline (OCR job on screenshots)  
