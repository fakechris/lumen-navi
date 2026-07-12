# Observe Capture — Product Design

> Product design only. No reverse-engineering material in this repo.  
> Repo: https://github.com/fakechris/lumen-navi

---

## 1. Goal

Continuous desktop context via **smart screenshots**: enough fidelity to reconstruct “what the user was doing,” without flooding disk or thrashing CPU.

OCR is a **later process step** (S4). Capture never waits on OCR.

---

## 2. Principles

1. Capture never waits on OCR — write pixels first.  
2. Cheap signals first — focus poll + low-res grayscale probe before full encode.  
3. Multi-display first-class — every active display can produce a frame.  
4. Privacy gates are hard stops — lock / closed-eyes / pause never write.  
5. Backpressure drops work, does not hang — bounded queue + metrics.  
6. Sessions group work — open on activity, close on idle.  
7. Observe does **not** depend on cua-driver (Act plane only, optional later).  
8. Fail soft — permission/locked degrade; daemon keeps running.

---

## 3. Architecture

```
FocusPoll + IntervalTick
        │
 PrivacyGate (pause · closed_eyes · screen_lock)
        │
 Debounce / same-app throttle
        │
 VisualProbe (1/N gray distance)   [skip for force triggers]
        │
 FullCapture (all|main displays) → JPEG/PNG
        │
 SessionManager
        │
 bounded queue → SqliteStore + blobs
        │
 enqueue ocr_screen job (consumer later)
```

---

## 4. Triggers

| Reason | Force full capture? | Debounce profile |
|--------|---------------------|------------------|
| `interval` | No — needs visual change | default |
| `focus_change` | Yes | churn |
| `title_change` | Yes | churn |
| `manual` / `session_open` | Yes | none / none |

**Debounce defaults:** 1000 ms normal · 3000 ms after focus churn · same-app skip unless ≥ 10000 ms (non-force).

**Visual probe defaults:** scale divisor **6**, mean gray distance threshold **0.05**.

---

## 5. Multi-display

- `displays = "all"` (default): all active displays  
- `displays = "main"`: main only  

One batch = shared `session_id` + `capture_id`; **one event per display**.

---

## 6. Encode & storage

| Setting | Default |
|---------|---------|
| Format | JPEG quality 75 |
| Max edge | 1920 |
| Probe frames | memory only, not stored |

Payload `screenshot.v1` includes reason, app/bundle/title, display_id/index, size, probe_distance, capture_id, session_id.

---

## 7. Activity sessions (Observe-level)

Table `activity_sessions`: open on first capture, touch on each batch, close after idle (default 5 min) or privacy/shutdown.

Not full Timeline L2/L3 — only grouping for later OCR/timeline.

---

## 8. Privacy gates (order)

1. Global pause  
2. closed_eyes  
3. screen locked  
4. missing Screen Recording → request + degrade  

---

## 9. Config (`navi.toml`)

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
screen_ticks = 0
displays = "all"
encode = "jpeg"
jpeg_quality = 75
focus_poll_ms = 500

[privacy]
paused = false
closed_eyes = false
```

---

## 10. Non-goals (this surface)

- OCR implementation details  
- PII redaction  
- Timeline semantic layers  
- System audio / video  
- cua-driver Act integration  

---

## 11. Success criteria

1. Multi-monitor → distinct `display_id` frames  
2. App switch → capture without waiting for visual miss  
3. Static desktop → no full captures (probe under threshold)  
4. Lock / closed_eyes → zero new screenshots  
5. Focus flood → queue bounded, drops counted  
6. Session id stable across a work stretch  
