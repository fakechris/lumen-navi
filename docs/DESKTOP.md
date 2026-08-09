# Lumen Navi — Desktop shell (Tauri)

> macOS app shell for Observe control + local search.  
> Dictation remains [Lumen ASR](https://github.com/fakechris/lumen-asr) (separate product).

## Role

| Surface | Responsibility |
|---------|----------------|
| **Desktop (this)** | Permissions, start/stop Observe, privacy pause, timeline, FTS search |
| **`lumen-daemon`** | Capture + OCR + ASR workers (spawned by the app or run headless) |
| **Lumen ASR** | Hotkey dictation → correct → inject |

## Layout

```
apps/desktop/
  src/                 # React UI
  src-tauri/           # Tauri 2 + Rust commands
```

Data default: `~/Library/Application Support/LumenNavi/`  
(`navi.toml`, `meta/navi.db`, `blobs/`, `logs/`, `rules/`)

### Category rules (time tracking)

Classification uses a fixed engine + JSON under `rules/` (no rebuild to tune keywords).  
Full guide: [`crates/lumen-store/rules/README.md`](../crates/lumen-store/rules/README.md).  
Runtime overrides: `~/Library/Application Support/LumenNavi/rules/`.

## Dev

> **Read this entire section before running `tauri dev`.**
> Skipping these steps causes white screens, dead screen capture, and
> permission loops that look like bugs but are dev-mode setup gaps.

### Prerequisites (one-time)

```bash
# 1. Stable local signing identity (stable cdhash → TCC permissions survive rebuilds)
scripts/macos/ensure-local-identity.sh
# See docs/MACOS_LOCAL_SIGNING.md for details.

# 2. Build the Cua helper app (screen capture owner + Settings icon)
scripts/macos/prepare-cua-app.sh "$(rustc -vV | sed -n 's/^host: //p')"
# → apps/desktop/src-tauri/helpers/Lumen Cua.app
#    Contents/MacOS/lumen-cua
#    Contents/Resources/AppIcon.icns   # CUA cursor mark from apps/cua/icon/
# Navi installs the helper to /Applications/Lumen Cua.app on first use (own TCC identity).

# 3. Install frontend deps
cd apps/desktop && npm install
```

### Standard dev run (with screen capture + time tracking)

```bash
# One command: builds, signs dev binaries, starts Cua, launches the app.
bash scripts/macos/tauri-dev-signed.sh
```

This wrapper handles three things that plain `tauri dev` does NOT:

1. **Code-signs the freshly compiled binaries.** Cua's `peer_auth`
   (`crates/lumen-cua/src/peer_auth.rs`) validates that connecting clients
   (`lumen-daemon`, `lumen-navi-desktop`) are signed by the same
   `Lumen Local Codesign` identity. `tauri dev` recompiles them **unsigned**
   on every source change — an unsigned binary is silently rejected (the
   connection is dropped before any protocol response, appearing as
   `invalid response header`). The wrapper re-signs after compile and
   restarts so the signature is validated at launch.

2. **Pre-launches Cua before the daemon spawns.** The daemon checks Cua
   readiness **once at boot** (`main.rs` `cua_client_from_env` →
   `client.status()`). If Cua isn't ready at that moment, screen capture is
   permanently off for that daemon lifetime (no retry). The wrapper starts
   Cua first and waits for its socket.

3. **Cleans stale state.** Multiple Cua instances compete for one socket;
   stale socket files prevent new instances from binding. The wrapper kills
   stale Cua processes and removes the socket before launch.

### Frontend-only dev (no screen capture, no signing)

If you're only changing React/CSS and don't need screen capture:

```bash
cd apps/desktop
npm run dev          # vite on :1421 (HMR)
# In another terminal, run the already-compiled desktop binary:
cargo run -p lumen-navi-desktop
```

The webview connects to `http://localhost:1421` (vite HMR). Screen capture
won't work (unsigned binary → Cua rejects), but time tracking and all
Dashboard features work fine — they don't depend on screen capture.

> **Time tracking is independent of screen capture.** Activity tracking
> (`activity.focus.v1` events, `activity_segments` projection, Dashboard)
> uses `CGWindowListCopyWindowInfo` for frontmost detection and
> `CGEventSourceSecondsSinceLastEventType` for idle — neither requires
> Screen Recording permission. See `research/lumen-navi-time-tracking-gap-workplan.md`.

### Plain `npx tauri dev` — when it breaks

Plain `tauri dev` works for frontend changes but breaks screen capture
because it recompiles binaries unsigned. Symptoms and causes:

| Symptom | Cause |
|---------|-------|
| White screen | Desktop binary launched directly (no vite on :1421) |
| `invalid response header` / `IPC endpoint did not become ready` | Unsigned binary rejected by Cua `peer_auth` |
| `screen: running=false` persistently | Cua wasn't ready when daemon booted |
| `Window Server` as frontmost app | CGWindowList returns system windows; now filtered |
| Only `lumen-navi-desktop` in segments | `NSWorkspace.frontmostApplication()` from a daemon returns its own bundle; now uses `CGWindowListCopyWindowInfo` |

### Build artifacts

```bash
# daemon binary (placed for Tauri externalBin)
scripts/macos/prepare-daemon-binary.sh "$(rustc -vV | sed -n 's/^host: //p')"

# or placeholder for cargo check only
scripts/macos/ensure-daemon-binary-placeholder.sh
```


**Start Observe** resolves `lumen-daemon` in this order:

1. Next to the app binary (bundled in DMG via Tauri `externalBin`)
2. `target/release` / `target/debug` (dev workspace)
3. `$PATH`

## Release (GitHub Actions)

Push a **stable SemVer tag** to trigger [`.github/workflows/release-macos.yml`](../.github/workflows/release-macos.yml):

```bash
git tag v0.1.0
git push origin v0.1.0
```

Produces:

| Asset | Arch |
|-------|------|
| `Lumen-Navi-v0.1.0-arm64.dmg` | Apple Silicon |
| `Lumen-Navi-v0.1.0-x64.dmg` | Intel |
| `SHA256SUMS.txt` | checksums |

Local DMG smoke (Apple Silicon example):

```bash
scripts/macos/prepare-daemon-binary.sh aarch64-apple-darwin
scripts/macos/prepare-cua-app.sh aarch64-apple-darwin
# or for cargo check only: scripts/macos/ensure-daemon-binary-placeholder.sh
cd apps/desktop && npm ci
APPLE_SIGNING_IDENTITY="$(../../scripts/macos/resolve-identity.sh)" \
  npm run tauri -- build --target aarch64-apple-darwin --bundles dmg
```

Install notes: [`docs/MACOS_RELEASE_NOTES.md`](MACOS_RELEASE_NOTES.md).  
Local dev builds should use a **stable self-signed identity** so TCC
permissions survive rebuilds: [`docs/MACOS_LOCAL_SIGNING.md`](MACOS_LOCAL_SIGNING.md).

## Permissions (macOS)

| Permission | Why |
|------------|-----|
| Screen Recording (`Lumen Cua`) | Screenshots |
| Microphone | Audio chunks |
| Speech Recognition | Optional ASR engine / SenseVoice fallback |
| Accessibility | Selection popup (划词助手) — read selected text + mouse-up monitor |

Screen Recording is requested by the independently signed `/Applications/Lumen Cua.app`
(installed from a payload nested in Navi Resources; icon is the Lumen Marks **CUA
cursor** from `apps/cua/icon/AppIcon.icns`, copied by `prepare-cua-app.sh`). Click
**请求屏幕录制** in Navi — do not double-click Lumen Cua itself (it is an
`LSUIElement` helper; Navi starts it with `serve`). The Overview tab shows base
grant vs real capture readiness.

The other permissions belong to Lumen Navi.

## Tabs

1. **概览** — health counts, sources, start/stop Observe, privacy pause  
2. **搜索** — OCR + transcript FTS (same index as control API)  
3. **活动** — timeline with thumbnails, text preview, kind/app filters, day summary  
4. **设置** — live source toggles (restart Observe to apply), 划词助手, data dir, launch-on-start

## Selection popup (划词助手)

PopClip-style floating panel: select text with the mouse in any app → a
borderless panel appears near the selection → click **翻译** or ask a question
→ the selected text goes to an OpenAI-compatible chat LLM and the answer
streams back into the panel. Esc or clicking elsewhere dismisses it.

- **Off by default.** Enable in 设置 → 划词助手 (`assistant.popup_enabled`),
  then grant **Accessibility** (used to read the selection via AX API and to
  run the global mouse-up monitor; re-enable the toggle after granting).
- **Privacy:** selected text is sent to the configured LLM **only** when the
  user clicks an action. Nothing is captured, stored, or indexed; unrelated
  to Observe capture and its privacy gates.
- **Limits:** apps with no Accessibility-tree support at all (a few
  custom-rendered UIs / old terminals / some Java apps) don't trigger the
  panel. Chromium apps (Chrome, VS Code, Electron) build their AX tree
  lazily — the monitor nudges them (`AXManualAccessibility` /
  `AXEnhancedUserInterface`) and retries briefly after mouse-up. WebKit
  (Safari) doesn't vend `AXSelectedText`; the selected string is read via
  `AXSelectedTextMarkerRange` + `AXStringForTextMarkerRange` instead. There
  is no ⌘C pasteboard fallback in auto mode (it must not clobber the
  pasteboard); a Bob-style ⌘C-with-restore capture could be added later
  behind a hotkey.
- Config (`navi.toml`):

```toml
[assistant]
enabled = true            # master switch for LLM actions
popup_enabled = true      # mouse-selection auto popup
base_url = "https://api.openai.com/v1"   # any OpenAI-compatible endpoint
api_key = "sk-…"          # or env LUMEN_NAVI_LLM_API_KEY / OPENAI_API_KEY
model = "gpt-4o-mini"
target_lang = "中文"
max_selection_chars = 4000
```

Commands: `assistant_get_config`, `assistant_update_config`, `assistant_run`,
`assistant_cancel`, `request_accessibility_permission`,
`selection_popup_hide`, `selection_popup_current`.
Popup events: `selection-changed`, `assistant-stream`, `assistant-done`,
`assistant-error`.

Implementation: AX capture + CGEventTap live in
`crates/lumen-platform-macos/src/selection.rs`; window glue in
`apps/desktop/src-tauri/src/selection_popup.rs`; SSE client in
`apps/desktop/src-tauri/src/assistant.rs`; panel UI in
`apps/desktop/src/popup/` (vite multi-entry `popup.html`).

**Roadmap (Act plane):** the popup's action seam (`assistant_run` + the
`[assistant]` config) is designed to gain a
[cua-driver](https://github.com/trycua/cua)-backed engine later (MIT only —
never `cua-agent[omni]`/ultralytics, AGPL-3.0). That integration is computer-**use**
only: it must never sit on the Observe capture path, which stays Navi-owned.


## Tray

Menu bar icon:

- Show window  
- Start / Stop Observe  
- Toggle privacy pause  
- Quit (stops child daemon)

## First-run onboarding

Stored in `shell.toml` (desktop-only; not product `navi.toml`):

| Field | Meaning |
|-------|---------|
| `onboarding_completed` / `skipped` | Wizard done |
| `onboarding_step` | Resume mid-wizard |
| `launch_observe` | Auto-start daemon on app launch |

Wizard steps:

1. Welcome  
2. Screen Recording  
3. Microphone (+ Speech settings link for fallback)  
4. **Local ASR model** — choose engine, pick existing dir, or **download SenseVoice**  
5. Ready / launch Observe  

Model selection writes product `navi.toml` (`asr.engine`, `asr.model_dir`, optional `asr.models_root`).  
**Download installs to the shared cluster path**  
`~/Library/Application Support/Lumen/models/sensevoice/` (shared with Lumen ASR / future apps).  
Users may pick any ready directory (shared, legacy per-app, or custom).

Commands: `check_asr_model_status`, `use_existing_asr_model`, `set_asr_models_root`,  
`start_asr_model_download`, `cancel_asr_model_download`, `set_asr_engine_preference`.  
Event: `asr-download-progress`.

## Relationship to Lumen ASR desktop

Patterns borrowed (not code-coupled):

- Tauri 2 + Vite + React  
- Application Support data dir  
- macOS Info.plist usage strings  
- Warm light design tokens  
- Tray + first-run onboarding flow  
- SenseVoice package download + candidate scan  

**Not** borrowed: hotkey dictation, inject, dictionary, capsule overlay.
