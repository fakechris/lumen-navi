# Observe Capture — Product Design (S2 complete)

> Status: **implementation target** (2026-07-12)  
> Scope: screen observation only. **OCR is Phase S4 (next product step), not this doc.**  
> Repo: https://github.com/fakechris/lumen-navi

---

## 1. Problem

Continuous desktop context requires **smart screenshots**: enough fidelity to reconstruct “what the user was doing,” without flooding disk or thrashing CPU. Upstream Yansu (IDA-verified) and the old Go LumenNavi both orbit the same problem; this design freezes a **product-grade Observe plane** for Rust Navi — clean-slate, not a port.

---

## 2. Research synthesis

### 2.1 Upstream Yansu (IDA)

| Mechanism | Behavior |
|-----------|----------|
| Capture API | `activitymon.CaptureScreen` → C `capture_screen` + buffer pool; links `CGWindowListCreateImage` |
| Probe path | `checkVisualChange`: capture at **1/6 resolution**, `bgraToGray`, `frameComparer.distanceFromGray ≥ 0.05` |
| Debounce | `captureSnapshotDebounced`: default **1s**; churn (app switch style) **3s**; same-bundle skip unless **≥10s** |
| Gates | **ClosedEyes** atomic skip; **`is_screen_locked`** skip; channel **backpressure** (len ≥ cap → drop) |
| Force path | Focus/event → `captureSnapshotForEvent` (bypass pure interval logic) |
| Storage | Session + snapshot files; activity path prefers **JPEG** encode |
| OCR | Separate `ActivityOCRWorker` + Vision `darwinEngine` / `yansu-ocr-helper` (**out of this phase**) |

### 2.2 Old LumenNavi Go

- `CGWindowListCreateImage` full-screen + window capture helpers.  
- Same named APIs (debounce / visual change) but **visual change stubbed to always true**.  
- Session/snapshot/OCR tables exist; mon CGo partially stubbed.  

### 2.3 Prior Rust S2

- Fixed interval + PNG hash dedup + main display only.  
- Missing: multi-display, focus trigger, grayscale probe, lock/privacy, backpressure, session model.

### 2.4 cua-driver

**Not required for Observe.**  
cua-driver (MIT, trycua/cua) is the optional **Act** plane (click/type/launch without focus steal).  
Its `cheapWindowPixelHash` is for **settle / action verification**, not activity archival.  
Navi capture owns CG/Screen Recording itself.

---

## 3. Product principles

1. **Capture never waits on OCR** — write raw first; jobs later.  
2. **Cheap signals first** — focus poll + 1/N grayscale probe before full encode.  
3. **Multi-display is first-class** — every online display can produce a frame.  
4. **Privacy gates are hard stops** — lock / closed-eyes / global pause never write.  
5. **Backpressure drops probes, not silence** — metrics + skip; never block UI thread.  
6. **Sessions group work** — open on activity, close on idle; screenshots carry `session_id`.  
7. **No cua-driver on this path.**  
8. **Fail soft** — permission/locked/denied degrade with logs, daemon keeps running.

---

## 4. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│ CaptureOrchestrator (lumen-sources-media)                        │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────────┐  │
│  │ FocusPoller │  │ IntervalTick │  │ PrivacyGate             │  │
│  │ (bundle/    │  │              │  │ pause · closed_eyes ·   │  │
│  │  title)     │  │              │  │ screen_lock             │  │
│  └──────┬──────┘  └──────┬───────┘  └────────────┬────────────┘  │
│         │                │                       │               │
│         └────────┬───────┘                       │               │
│                  ▼                               │               │
│         TriggerRequest ──(debounce/same-app)─────┘               │
│                  │                                               │
│                  ▼                                               │
│         VisualProbe (1/scale gray distance)  [skip if force]      │
│                  │                                               │
│                  ▼                                               │
│         FullCapture all|main displays → encode JPEG/PNG          │
│                  │                                               │
│                  ▼                                               │
│         SessionManager (open/close idle)                         │
│                  │                                               │
│                  ▼ bounded channel (backpressure)                │
│         PersistWorker → SqliteStore + BlobStore                  │
└──────────────────────────────────────────────────────────────────┘
         platform ports only (no #[cfg] in orchestrator)
```

---

## 5. Triggers & policy

### 5.1 Trigger reasons

| Reason | Force full capture? | Debounce profile |
|--------|---------------------|------------------|
| `interval` | No — requires visual change | default |
| `focus_change` | **Yes** | churn |
| `title_change` | Yes (same as focus) | churn |
| `manual` | Yes | none |
| `session_open` | Yes | none |

### 5.2 Debounce (aligned with Yansu)

- Default min interval: **1000 ms**  
- During focus churn (after app_switch / title_change): **3000 ms**  
- Same bundle + not focus force: skip unless **≥ 10000 ms** since last capture for that bundle  

### 5.3 Visual probe

1. Capture **each configured display** at scale `1/probe_scale` (default **6**).  
2. Convert BGRA → grayscale luminance.  
3. Mean absolute difference vs last probe buffer / pixel ∈ [0,1].  
4. Threshold default **0.05** (5%).  
5. If below threshold and reason is `interval` → **no full capture**.  
6. Focus/manual always proceed to full capture (still subject to privacy gates + debounce).

### 5.4 Privacy / environment gates (ordered)

1. Global **pause**  
2. **closed_eyes** (product privacy mode — no screen pixels leave device path)  
3. **screen locked**  
4. Screen Recording permission missing → request once + degrade  

### 5.5 Backpressure

- Persist queue capacity default **8** capture batches.  
- If full: drop new batch, increment `dropped_backpressure`, log warn.  
- Never block capture probe longer than one encode; probe is sync on blocking pool.

---

## 6. Multi-display

| Setting | Behavior |
|---------|----------|
| `displays = "main"` | Only main display |
| `displays = "all"` (default) | All **active** displays via `CGGetActiveDisplayList` |

Per capture cycle:

- One **logical batch** (same `session_id`, same `capture_id` / timestamp group)  
- **One `SourceEvent` per display** (`screenshot.v1`) with `display_id`, `display_index`, `is_main`, bounds  

Probe state is **per display_id** (independent gray buffers).

---

## 7. Encode & storage

| | Default | Notes |
|--|---------|-------|
| Full frame format | **JPEG q=75** | Yansu activity path; smaller than PNG |
| Probe frames | never stored | memory only |
| Max edge | 1920 | after capture, before encode |
| Dedup | visual probe primary; optional content_hash on blob still via CA store | |

Payload `screenshot.v1` fields:

```json
{
  "payload_version": 1,
  "reason": "focus_change|interval|...",
  "app_name": "...",
  "bundle_id": "...",
  "window_title": "...",
  "display_id": 1,
  "display_index": 0,
  "is_main": true,
  "width": 1920,
  "height": 1080,
  "probe_distance": 0.12,
  "capture_id": "uuid-shared-across-displays-in-batch",
  "session_id": "uuid"
}
```

---

## 8. Activity session model (Observe-level)

Lightweight (not full Timeline L2/L3):

```
activity_sessions (
  id TEXT PK,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  primary_app TEXT,
  primary_bundle TEXT,
  trigger TEXT,
  snapshot_count INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL  -- open|closed
)
```

Rules:

- **Open** on first successful full capture when no open session.  
- **Touch** `snapshot_count` / primary app on each batch.  
- **Close** when idle ≥ `idle_session_ms` (default 5 min) or pause/closed_eyes/shutdown.  
- Events carry `session_id` for later Timeline/OCR.

---

## 9. Platform ports

```
trait DisplayEnumerator { list_displays() -> Vec<DisplayInfo> }
trait ScreenCapturer {
  capture_display(id, max_edge) -> ScreenshotFrame  // encoded-ready RGBA/PNG path or raw
  capture_display_raw(id, scale_div) -> RawFrame    // BGRA for probe
}
trait ScreenLockProbe { is_locked() -> bool }
trait FrontmostAppProbe { frontmost() -> Option<FrontmostApp> }  // existing
trait PermissionProbe { ... }  // existing
```

macOS impl:

- Displays: `CGGetActiveDisplayList` + `CGDisplayBounds` / main flag  
- Image: prefer `CGDisplayCreateImage` per display id (stable multi-monitor)  
- Lock: session dictionary `CGSSessionScreenIsLocked` / equivalent  
- Frontmost: NSWorkspace (existing)  
- **No** ScreenCaptureKit stream required for stills (SCK optional later for video)

---

## 10. Config (`navi.toml`)

```toml
[capture]
screen_interval_ms = 3000
probe_scale = 6
visual_change_threshold = 0.05
debounce_default_ms = 1000
debounce_churn_ms = 3000
same_app_min_ms = 10000
idle_session_ms = 300000
queue_capacity = 8
screen_max_edge = 1920
screen_ticks = 0          # 0 = until Ctrl+C
displays = "all"          # all | main
encode = "jpeg"           # jpeg | png
jpeg_quality = 75
focus_poll_ms = 500

[privacy]
paused = false
closed_eyes = false
```

---

## 11. Non-goals (this phase)

- Vision OCR / ocr-helper  
- PII redaction  
- Timeline runs / semantic events  
- System audio / video segments  
- cua-driver act integration  
- AX deep window title (best-effort frontmost only; AX optional later)  

---

## 12. Success criteria

1. Dual-monitor machine stores frames for **both** displays with distinct `display_id`.  
2. Switching apps produces a capture within debounce+1s without waiting full interval visual miss.  
3. Static desktop for 30s → **no** new full captures (probe distance &lt; threshold).  
4. Screen lock → zero new screenshots.  
5. `closed_eyes=true` → zero new screenshots.  
6. Flood of focus events → queue does not grow unbounded; drops counted.  
7. Session id stable across a work stretch; new session after idle.  
8. No dependency on cua-driver binary.  

---

## 13. Implementation map

| Piece | Crate |
|-------|-------|
| Types / reasons / session ids | `lumen-types` |
| Config | `lumen-config` |
| Ports | `lumen-platform` |
| macOS CG/NS | `lumen-platform-macos` |
| Gray compare + orchestrator + session | `lumen-sources-media` |
| `activity_sessions` table | `lumen-store` schema v2 |
| Run loop | `lumen-daemon` |

---

## 14. Next product step (not now)

**S4 OCR:** job `ocr_screen` on stored JPEG/PNG via Vision (process plane), never blocking capture.
