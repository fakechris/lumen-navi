# Browser Observe MVP

The bundled Chrome MV3 extension stores versioned browser observations in its own IndexedDB archive. It works standalone and may optionally sync a transport copy to Lumen Navi, where the daemon persists append-only `SourceEvent` records. Content capture defaults to metadata-only until a host is explicitly added to the extension or daemon allow-list.

## Privacy contract

- Incognito, browser-internal, extension, localhost, single-label/internal-network, literal private-IP, prefetch-only, and excluded-host pages are not observed.
- URL fragments are removed. Query values named `token`, `session`, `auth`, `code`, `email`, `key`, `signature`, and their supported variants are removed before persistence.
- Input values, form submissions, selection, clipboard activity, and DOM link lists are never read or sent by this MVP.
- Raw HTML and structural HTML are not accepted by the daemon.
- Readability Markdown is captured only when the host appears in an explicit extension or daemon `content_allow_hosts` entry and the page has no password input, editable region, `noindex`, or sensitive path. A newsletter email field alone does not block article extraction; input values are never read.
- Pages that are eligible for observation but not content capture store metadata and extraction status only.
- Extension pause is always a hard local capture/write gate. When Navi is connected and its policy is reachable, global pause, `closed_eyes`, and screen lock also gate capture. If Navi is unavailable, the extension continues in standalone mode using its own privacy policy.
- A batch rejected by Navi remains in the local archive with a rejected sync status, but is removed from the transport queue instead of being replayed indefinitely.
- At the per-artifact size limit or blob-retention ceiling, the daemon rejects only the Markdown asset and persists its associated metadata with `extraction_status = artifact_too_large` or `retention_blocked`.

## Standalone use

No token or daemon is required. The extension popup provides:

- local capture and storage status;
- pause/resume capture;
- exclude the current host;
- an editable content allow-list;
- reversible **标记此页** (`flag`) and **不相关** (`dismiss`) facts;
- **查看浏览记录**, which opens the full-page local archive.

The archive combines lifecycle events into visits and exposes a 28-day heatmap, attention-ranked domains, title/domain/URL search, reading-depth filters, and source detail with captured Markdown when available.

## Optional Navi sync

Browser intake is disabled by default. Generate a local token and add the browser sections to `navi.toml`:

```bash
openssl rand -hex 32
```

```toml
[sources]
screen = true
audio = true
video = false
browser = true

[browser]
ingest_token = "replace-with-the-generated-token"
content_allow_hosts = ["example.test"]
excluded_hosts = [
  "mail.google.com",
  "outlook.office.com",
  "slack.com",
  "discord.com",
  "web.whatsapp.com",
  "web.telegram.org",
]
max_batch_size = 100
max_artifact_bytes = 2097152
```

`LUMEN_NAVI_BROWSER_TOKEN` overrides `browser.ingest_token`. Restart the daemon after changing policy. Enter the same token in the extension popup only when Navi sync is wanted. The extension then synchronizes additional allow/exclude policy and system-level privacy gates from the daemon while preserving its local archive.

## Build and load the extension

```bash
cd extensions/browser
npm install
npm test
npm run typecheck
npm run build
```

In Chrome, open `chrome://extensions`, enable Developer mode, choose **Load unpacked**, and select `extensions/browser/.output/chrome-mv3`.

In Comet, use `comet://extensions/` instead of `chrome://extensions/`.

## Wire contract

`POST /v1/browser/batches` requires `Authorization: Bearer <token>` and accepts:

- `installation_id`;
- `schema_version`;
- `capture_profile_version` and `config_hash`;
- replay-safe observations with UUID event ids;
- optional `text/markdown` artifacts attached to `browser.document_ready.v1`.

Supported event kinds:

- `browser.navigation_committed.v1`
- `browser.document_ready.v1`
- `browser.visibility_focus_change.v1`
- `browser.feedback.v1`
- `browser.visit_closed.v1`
- `browser.health.v1`
- `browser.gap.v1`

The content script sends document-ready metadata before scheduling Readability extraction, so enrichment does not delay lifecycle capture. IndexedDB is the durable standalone archive. A separate transport queue batches at 10 events by default, flushes after 30 seconds, and keeps at most 500 observations or 4 MiB of Markdown. These are configurable capture-profile defaults. Queue overflow creates a transport gap record without deleting the corresponding local archive observations.

## Local API

All browser endpoints except general daemon health require the installation token:

| Endpoint | Purpose |
|---|---|
| `GET /v1/browser/policy` | Read the daemon-owned capture gate and allow/exclude policy |
| `POST /v1/browser/batches` | Idempotent observation and Markdown intake |
| `GET /v1/browser/export?after=0&limit=1000` | Cursor-based NDJSON event and visit export |
| `POST /v1/control` | Browser pause/resume with `source: "browser"` |

The export is read-only and includes `export_header`, raw `event`, completed `visit_projection`, and `export_cursor` records. Its header records the Navi version, record count, and BLAKE3 integrity checksum. The rebuildable visit projection includes content identity, visible/background timing, revisit index, navigation provenance, and snapshot hashes. Browser health and gap events use the same stream, so an importer can distinguish no activity from collector failure.

## Verification

```bash
cargo test -p lumen-sources-browser
cargo test -p lumen-store
cargo test -p lumen-daemon
```

Use only synthetic `example.test` pages for automated or manual contract checks.
