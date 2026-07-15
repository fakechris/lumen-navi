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

**Start Observe** in the app looks for `target/release/lumen-daemon` (then debug, then `PATH`).

## Permissions (macOS)

| Permission | Why |
|------------|-----|
| Screen Recording | Screenshots |
| Microphone | Audio chunks |
| Speech Recognition | Observe ASR (`transcript.v1`) |

Granted via System Settings after first use; the Overview tab shows probe status.

## Tabs

1. **概览** — health counts, sources, start/stop Observe, privacy pause  
2. **搜索** — OCR + transcript FTS (same index as control API)  
3. **活动** — recent events timeline  
4. **设置** — data dir, engine flags (read from `navi.toml`)

## Relationship to Lumen ASR desktop

Patterns borrowed (not code-coupled):

- Tauri 2 + Vite + React  
- Application Support data dir  
- macOS Info.plist usage strings  
- Warm light design tokens  

**Not** borrowed: hotkey dictation, inject, dictionary, capsule overlay.
