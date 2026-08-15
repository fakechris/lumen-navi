# Windows port — implementation plan

Goal: make Lumen Navi build, install and run Observe on Windows 10/11 x64,
without regressing macOS. Packaging follows the lumen-asr precedent
(NSIS current-user installer, WebView2 download bootstrapper, one tag-driven
release workflow that aggregates both platforms).

## Stage 1: Platform layer split
**Goal**: `lumen-platform` ports get a real Windows backend; no consumer keeps
a hard dependency on `lumen-platform-macos`.
**Success Criteria**:
- `crates/lumen-platform-cpal` owns the cross-platform mic (was macOS-only).
- `crates/lumen-platform-windows` implements displays, capture, frontmost,
  lock, permissions and OCR.
- `crates/lumen-platform-host` selects the backend per target.
- `cargo check --target x86_64-pc-windows-gnu -p lumen-platform-host` passes.
**Status**: Complete

## Stage 2: Consumer rewiring
**Goal**: daemon + desktop shell use the facade and Windows-correct paths.
**Success Criteria**:
- No `lumen_platform_macos` symbol outside the macOS backend.
- Data dir is `%LOCALAPPDATA%\LumenNavi`, models root `%LOCALAPPDATA%\Lumen\models`.
- Daemon sidecar resolves `lumen-daemon.exe` and spawns without a console window.
- `speech` ASR engine is not offered or auto-fallen-back to on Windows.
**Status**: Complete

## Stage 3: Packaging + CI
**Goal**: an installable unsigned Windows x64 build produced by CI.
**Success Criteria**:
- `tauri.windows.conf.json` (nsis, currentUser, WebView2 bootstrapper) and
  `tauri.macos.conf.json` overlays; base config is platform-neutral.
- `scripts/windows/*.ps1` mirror the macOS daemon/bundle scripts.
- `ci-windows.yml` builds crates + frontend + NSIS bundle on `windows-latest`.
- `release-desktop.yml` replaces `release-macos.yml` and publishes mac + win
  assets from a single aggregate job.
**Status**: Complete

## Stage 4: UX honesty
**Goal**: the UI never shows macOS-only instructions on Windows.
**Success Criteria**:
- `get_platform_info` exposes os + capability flags to the frontend.
- Permission cards, onboarding and the ASR engine picker are platform-aware.
- Selection popup is reported unsupported rather than silently dead.
**Status**: Complete

## Stage 5: On-Windows verification
**Goal**: real-hardware validation of everything cross-compilation cannot prove.
**Success Criteria**: see `docs/WINDOWS_PORT_STATUS.md` "Verification pending".
**Status**: In Progress — CI half done, hardware half not started.
- Done: `ci-windows.yml` green on `windows-latest`. MSVC compile, workspace
  tests, frontend build, daemon staging, NSIS bundle and extension tests all
  pass. See "What Windows CI proves".
- Remaining: the six runtime checks that need real hardware (install without
  WebView2, Observe capture/OCR/mic, mixed-DPI multi-monitor, lock + UAC,
  sleep/resume + RDP, denied-mic permission card).
