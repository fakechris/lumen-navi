# OCR — Product Intent (S4)

> Product intent only. Implementation research stays **outside** this repository.

## Goal

Turn stored screenshots into searchable/on-device text **without slowing capture**.

## Constraints

- Engine: **on-device** macOS Vision (or equivalent local OCR)  
- Async only: consume `ocr_screen` jobs after Observe write  
- Default languages: Simplified Chinese + English  
- Dual outputs: plain text (quality path) + optional boxes (layout path)  
- Concurrency limited (start at 1–2)  
- Crash isolation preferred longer-term (helper process); in-process OK for MVP  

## Outputs

Derived record `ocr.v1` (or dedicated table later):

- text, confidence, languages, mode  
- optional boxes `{x,y,w,h,text,confidence}`  

## Non-goals (first OCR ship)

- Blocking capture on OCR  
- Cloud OCR  
- Full PII pipeline (follow-up)  
- FTS/timeline UX (can follow once text exists)  

## Exit criteria

Screenshot events gain OCR text while Observe loop FPS/latency stays unaffected.

## Implementation status (S4 MVP)

- In-process macOS Vision bridge (`MacVisionOcr`)
- Async `OcrWorker` claims `ocr_screen` jobs → writes `derived` rows `kind=ocr.v1`
- Default languages: `zh-Hans`, `en-US`
- Quality text path + optional layout boxes
- Capture path only enqueues jobs; never calls Vision

```toml
[ocr]
enabled = true
languages = ["zh-Hans", "en-US"]
poll_interval_ms = 2000
batch_size = 4
include_boxes = true
```
