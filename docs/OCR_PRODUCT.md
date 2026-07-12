# OCR — Product Spec (S4)

> Product documentation only.

## Goal

Turn stored screenshots into on-device text **without slowing capture**.

## Pipeline

```
capture → store event + blob → enqueue job ocr_screen (deduped if open)
                ↓
         OcrWorker (async)
                ↓
    claim due pending → Vision (serialized, timeout)
                ↓
         derived kind=ocr.v1 (upsert) → job done
                ↓
         ocr_docs + FTS5 index (searchable)
                ↓
    on failure: backoff pending | dead after max_attempts
```

## Guarantees

| Property | Behavior |
|----------|----------|
| Non-blocking capture | Capture only enqueues; never calls Vision |
| Idempotency | One open job per event; one `ocr.v1` derived per event |
| Retry | Exponential backoff via `available_at` |
| Stuck jobs | Reclaim `running` older than `stale_running_ms` |
| Timeouts | Per-call `timeout_ms` |
| Size limits | Reject oversized images permanently |
| Languages | Default `zh-Hans` + `en-US` |
| Layout boxes | Optional; by default only when text empty |

## Config

```toml
[ocr]
enabled = true
languages = ["zh-Hans", "en-US"]
poll_interval_ms = 1500
batch_size = 2
include_boxes = true
boxes_when_empty_only = true
max_attempts = 5
retry_base_ms = 2000
retry_max_ms = 60000
timeout_ms = 90000
stale_running_ms = 300000
max_image_bytes = 26214400
max_text_chars = 500000
shutdown_drain_ms = 30000
```

## Derived payload `ocr.v1`

```json
{
  "payload_version": 1,
  "event_id": "...",
  "text": "...",
  "confidence": 0.0,
  "languages": ["zh-Hans", "en-US"],
  "mode": "accurate",
  "boxes": [],
  "image_bytes": 12345,
  "image_blake3": "...",
  "engine": "vision"
}
```

## Search index

Schema v4 adds `ocr_docs` (one row per event) plus external-content FTS5 (`ocr_fts`).

| Path | Behavior |
|------|----------|
| Write | `insert_derived(..., "ocr.v1", body)` upserts `ocr_docs` in the same transaction |
| Migrate | Opening an older DB backfills from existing `derived` ocr.v1 rows |
| Reindex | `SqliteStore::reindex_ocr_docs()` rebuilds from all ocr.v1 bodies |
| Query | `SqliteStore::search_ocr(q, limit)` — FTS MATCH, LIKE fallback |

## Local control API (loopback)

Default bind: `127.0.0.1:7420` (`[api]` in config).

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` or `/v1/health` | Events count, ocr_docs, schema version, sources |
| GET | `/v1/ocr/search?q=…&limit=20` | OCR full-text search |
| POST | `/v1/control` | JSON `ControlRequest` (`health`, `search_ocr`, `reindex_ocr`, `list_events`, …) |

Example:

```bash
curl -s 'http://127.0.0.1:7420/v1/ocr/search?q=发票&limit=5' | jq .
curl -s -X POST http://127.0.0.1:7420/v1/control \
  -H 'content-type: application/json' \
  -d '{"op":"reindex_ocr"}'
```

```toml
[api]
enabled = true
bind = "127.0.0.1:7420"
```

## Non-goals (this ship)

- Cloud OCR  
- PII redaction of OCR text  
- Desktop/timeline search UI (API only)  
- Separate OCR helper process (S4.1 optional)  

## Exit criteria

- Screenshots consistently produce `ocr.v1` under normal load  
- Capture loop remains responsive with OCR enabled  
- Crash/restart does not lose work permanently (pending reclaim + reopen)  
- Duplicate captures do not spawn duplicate open OCR jobs  
- OCR text is queryable via FTS / local control API  
