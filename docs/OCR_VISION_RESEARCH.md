# Vision OCR Research — Yansu vs Go LumenNavi

> Date: 2026-07-12  
> Purpose: ground S4 OCR product design in real upstream + prior Go evidence  
> Sources: IDA on `/Applications/Yansu.app` (0.1.337), `yansu-ocr-helper` strings,  
> `yansu-reverse/wails-gui/internal/platform/ocr`, `activity_ocr_worker.go`, `HELPERS_DEEP_v337.md`

---

## 1. One-line answers

| Question | Answer |
|----------|--------|
| What is Yansu’s OCR engine? | **Apple Vision.framework** (`VNRecognizeTextRequest`) via Go CGo (`darwinEngine`) |
| Is OCR in-process only? | **No (0.1.337):** optional **out-of-process** `yansu-ocr-helper` (JSON stdin/stdout) |
| What did Go LumenNavi implement? | Same Vision CGo path (**in-process**), plus `ActivityOCRWorker` batch pipeline; helper split **planned, not productized** |
| Does capture need OCR? | **No** — capture writes pixels first; OCR is async process plane |
| Does OCR need cua-driver? | **No** |

---

## 2. Yansu actual scheme (product, IDA-verified)

### 2.1 Engine stack

```
image bytes (JPEG preferred)
    → CGImageSourceCreateWithData → CGImage
    → VNImageRequestHandler(initWithCGImage:)
    → VNRecognizeTextRequest
    → topCandidates(1) per VNRecognizedTextObservation
```

**Two recognition modes** (same engine, different request flags):

| API | recognitionLevel | usesLanguageCorrection | Output |
|-----|------------------|------------------------|--------|
| `RecognizeJPEG` / `ocr_recognize` | **Accurate** (flag bit) | **NO** when accurate | Text + avg confidence trailer `\n---\n{float}` |
| `RecognizeJPEGWithBoxes` / `ocr_recognize_with_boxes` | **Fast** (hardcoded) | **YES** | JSON array of boxes `{x,y,w,h,text,confidence}` (normalized bbox) |

Languages: dynamic list from Go (`prepareCLangs` / `getOCRLanguagePrefs` / `resolveOCRLanguagePrefs`).  
Supported set includes at least: `en-US`, `en-GB`, `zh-Hans`, `zh-Hant`, `ja-JP`, `ko-KR`, `fr-FR`, `de-DE`, `es-ES`, `it-IT`, `pt-BR`.

### 2.2 Process topology (0.1.337)

```
Main Yansu process
  ├─ ActivityOCRWorker (batch, 120s ticker + startup pass)
  ├─ captureAndOCR (interactive / spotlight path)
  ├─ getOrComputeOCRBoxes (cache + singleflight)
  └─ spawn yansu-ocr-helper  (optional isolation)

yansu-ocr-helper (~3.4MB arm64)
  env: YANSU_OCR_HELPER_PATH / YANSU_OCR_HELPER_CHILD / YANSU_OCR_HELPER_DISABLE
  protocol: decode request → Vision → encode response
  errors: "ocr helper request", "ocr helper failed", "ocr helper response", "ocr helper timed out after %s"
```

**Why helper:** isolate Vision/GPU work; crash isolation; independent codesign (same pattern as PII helper).  
**Disable path:** `YANSU_OCR_HELPER_DISABLE` falls back to in-process CGo (inferred from env names + dual symbols).

### 2.3 Pipeline roles (not one function)

| Path | Role |
|------|------|
| **Background batch** `ActivityOCRWorker` | Sessions with `ocr_status=pending` → sample snapshots → Recognize → `activity_ocr_frames` → FTS → optional embed |
| **Interactive** `captureAndOCR` | Capture → optional **focus-rect crop** → optional **2× upscale** → EncodeJPEG q≈80 → RecognizeJPEG with language prefs |
| **Highlight cache** `getOrComputeOCRBoxes` | Local path only → DB boxes_json cache → else compute under **singleflight** → store boxes |

### 2.4 Interactive refinements (IDA `captureAndOCR`)

1. Optionally take frontmost window bounds.  
2. Encode raw BGRA → JPEG quality **80**.  
3. If focus rect valid: `cropImageToFocusRect` then re-encode.  
4. If flag set: `upscaleImage2x` then re-encode.  
5. `getOCRLanguagePrefs` → `darwinEngine.RecognizeJPEG`.  
6. Snapshot flag `ocr_used_focus_rect` recorded when crop used.

### 2.5 Background worker policy

From symbols + Go reconstruction aligned with binary:

- Interval ~**120s** + immediate run on start  
- Cap snapshots per session per run (Go: **20**)  
- Skip already-OCR’d snapshot ids  
- Prefer text via RecognizeJPEG; if empty → WithBoxes + reading-order sort  
- Session `ocr_status`: `pending` → `done` (or remain pending if deferred)  
- Sampling log: `session %s sampling %d/%d snapshots for background OCR`  
- Multi-language re-index migration resets done → pending  

### 2.6 Storage model (Yansu)

```sql
activity_ocr_frames (
  id, session_id, segment_id,  -- segment_id ↔ snapshot id
  frame_time, text, confidence,
  boxes_json,                  -- highlight boxes
  embedding, embed_hash,       -- vector search
  ...
)
activity_ocr_fts  -- FTS5 trigram preferred, unicode61 fallback
activity_sessions.ocr_status
activity_snapshots.ocr_used_focus_rect
```

Downstream consumers: keyword frames, daily OCR samples, triager, timeline summaries, semantic search.

### 2.7 Concurrency / safety

- `ocr.gate` semaphore (limit concurrent Vision calls — GPU thrash)  
- Engine mutex in Go wrappers  
- singleflight for box computation by file path  
- PII path can **drop frames** on OCR error during mask (`[pii-mask] OCR error (frame dropped)`)  

---

## 3. Prior Go LumenNavi scheme

### 3.1 Engine

**Same Vision stack**, full CGo in-repo:

- `internal/platform/ocr/ocr_darwin.go` — real `ocr_recognize` / `ocr_recognize_with_boxes`  
- Defaults: **`zh-Hans` first, then `en-US`** (product fix after English-first hurt Chinese UI)  
- Accurate mode for text path; Fast + language correction for boxes  

Note: `ocr.go` has a **stub** `darwinEngine.IsSupported() == false` documentation shell; **real path is `NewDarwinCGoEngine()`** used by `ocr_engine_adapter.go`.

### 3.2 Worker

`ActivityOCRWorker` mirrors Yansu:

| Piece | Value |
|-------|-------|
| Tick | 120s + startup |
| Input | snapshot file bytes (`readSnapshotFile`) |
| Primary | `RecognizeJPEG` |
| Fallback | `RecognizeJPEGWithBoxes` + Y-major then X-minor sort |
| Output | `SaveOCRFrame` + `UpdateOCRStatus` |
| Cap | 20 snapshots / session / run |

### 3.3 What Go did **not** fully productize vs Yansu 0.1.337

| Feature | Go LumenNavi | Yansu 0.1.337 |
|---------|--------------|---------------|
| Vision CGo | ✅ | ✅ |
| Batch worker | ✅ | ✅ |
| FTS trigram | ✅ (store) | ✅ |
| Out-of-process helper | ❌ (docs only) | ✅ `yansu-ocr-helper` |
| Focus-rect crop + 2× upscale OCR | partial / not full IDA parity | ✅ `captureAndOCR` |
| boxes_json + singleflight cache | weaker | ✅ |
| Embedding on OCR frames | partial | ✅ EmbedOCRFrames |
| PII-before-OCR gate | partial rules | ✅ helper + defer/drain |

### 3.4 Fidelity note

Go `ocr_darwin.go` comments mark **IDA-VERIFIED** alignment for Accurate/Fast flags and CGImageSource path — engine-level fidelity is high. Gaps are **process topology + interactive crop path + helper isolation**.

---

## 4. Comparison matrix

| Dimension | Yansu | Go LumenNavi | New Rust Navi (today) |
|-----------|-------|--------------|------------------------|
| Engine | Vision | Vision | ❌ not yet |
| Decode | CGImageSource | CGImageSource | — |
| Async vs capture | Yes | Yes | Jobs enqueued only |
| Helper process | Yes (optional) | No | TBD S4 |
| Languages | Prefs + multi | zh-Hans+en-US default | TBD |
| Boxes | Yes + cache | Yes | TBD |
| FTS | Yes | Yes | TBD (`derived` / table) |
| Focus crop OCR | Yes | Incomplete | TBD (optional) |
| Gate / concurrency | Yes | Designed | TBD |

---

## 5. Implications for Rust S4 (product recommendation)

### 5.1 Non-negotiables (match both codebases)

1. **Vision only for on-device OCR** (no cloud, no cua-agent vision).  
2. **Never block Observe capture** — consume `ocr_screen` jobs only.  
3. **JPEG-friendly input** (we already archive JPEG by default).  
4. **Two modes:** Accurate text + Fast boxes.  
5. **Languages:** configurable; default **`zh-Hans` + `en-US`**.  
6. **Concurrency gate** (start with 1–2 concurrent Vision requests).  
7. **Idempotent** per event/artifact id.

### 5.2 Architecture for Navi

```
CaptureOrchestrator
  → store event + blob
  → enqueue job kind=ocr_screen (already done)

OcrWorker (S4)
  → claim pending jobs
  → load blob bytes
  → VisionEngine.recognize_text / recognize_boxes
  → write derived record + optional ocr_frames table
  → job done | retry
```

**Process placement (product choice):**

| Option | Pros | Cons | Recommendation |
|--------|------|------|----------------|
| **A. In-process** (objc/Vision in macOS crate) | Fast to ship, simple | Vision crash kills daemon | **MVP S4** |
| **B. Helper process** (like yansu-ocr-helper) | Isolation, sign separate | IPC, packaging | **S4.1 after MVP** |

### 5.3 Explicitly out of first OCR MVP

- PII redaction on OCR text (follow-up; Yansu treats as hard gate for LLM)  
- Embedding OCR frames  
- Focus-rect crop path (nice-to-have after baseline quality)  
- Timeline keyword frames (consumes OCR later)

### 5.4 Suggested schema (fits Navi envelope)

Prefer **derived** first (already in store):

```json
// derived.kind = "ocr.v1"
{
  "event_id": "...",
  "text": "...",
  "confidence": 0.91,
  "languages": ["zh-Hans","en-US"],
  "mode": "accurate",
  "boxes": [ {"x":0.1,"y":0.2,"w":0.3,"h":0.05,"text":"...","confidence":0.9} ]
}
```

Optional later: dedicated `ocr_results` + FTS5 for search UX.

---

## 6. Open implementation questions (resolve at S4 start)

1. Rust binding approach: `objc2` + Vision APIs vs small ObjC static lib (closer to Go CGo).  
2. Helper MVP timing: same PR as engine vs immediately after.  
3. Whether boxes are stored for every frame or on-demand (Yansu caches on demand).  

---

## 7. Bottom line

- **Yansu = Vision + dual-mode API + async worker + optional helper + rich cache/crop/FTS.**  
- **Go LumenNavi = same Vision engine + async worker; missing helper isolation and full interactive crop path.**  
- **Rust Navi should copy the engine semantics and async boundary first; helper second; FTS/PII third.**

Next engineering step: implement S4 MVP as **in-process Vision engine + job worker + derived ocr.v1**, languages `zh-Hans`+`en-US`, gate concurrency=1.
