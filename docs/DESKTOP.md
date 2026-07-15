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
(`navi.toml`, `meta/navi.db`, `blobs/`, `logs/`)

## Dev

```bash
# workspace
cargo build -p lumen-daemon --release
cargo build -p lumen-navi-desktop

# frontend
cd apps/desktop
npm install
npm run build

# run UI (debug)
cd apps/desktop
npx tauri dev
# or
cargo run -p lumen-navi-desktop
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
# or for cargo check only: scripts/macos/ensure-daemon-binary-placeholder.sh
cd apps/desktop && npm ci && npm run tauri -- build --target aarch64-apple-darwin --bundles dmg
```

Install notes: [`docs/MACOS_RELEASE_NOTES.md`](MACOS_RELEASE_NOTES.md).

## Permissions (macOS)

| Permission | Why |
|------------|-----|
| Screen Recording | Screenshots |
| Microphone | Audio chunks |
| Speech Recognition | Optional ASR engine / SenseVoice fallback |

Granted via System Settings after first use; the Overview tab shows probe status.

## Tabs

1. **概览** — health counts, sources, start/stop Observe, privacy pause  
2. **搜索** — OCR + transcript FTS (same index as control API)  
3. **活动** — timeline with thumbnails, text preview, kind/app filters, day summary  
4. **设置** — live source toggles (restart Observe to apply), data dir, launch-on-start

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
