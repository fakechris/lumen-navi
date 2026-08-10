# Windows port status

Updated: 2026-08-05

Tracks the deliberate compatibility choices of the Windows port, and what
still needs real hardware to confirm.

## Compatibility policy

- macOS stays a fully supported target. No Windows change may degrade it.
- Platform code lives in a backend crate, not in `#[cfg]` blocks scattered
  through product code. `lumen-platform-host` is the only crate that knows
  which OS it is compiled for.
- Where Windows cannot do something, the capability model says so and the UI
  explains it. Silently returning empty results is not acceptable.

No intentional macOS behaviour break has been introduced.

## Architecture

```text
lumen-platform            ports (traits + DTOs), no OS knowledge
      ├── lumen-platform-cpal       mic (CoreAudio / WASAPI)
      ├── lumen-platform-macos      macOS-only; empty off macOS
      └── lumen-platform-windows    Win32 + WinRT
                    │
          lumen-platform-host       target selection + HostCapabilities
                    │
        lumen-daemon / apps/desktop (no #[cfg])
```

## Implemented

- **Screen capture** — GDI `BitBlt` off the virtual-screen DC, per monitor.
  The process opts into per-monitor-v2 DPI awareness so rects and blits are in
  physical pixels on mixed-scaling setups. Display ids are an FNV-1a hash of
  the GDI device name (`\\.\DISPLAY1`), so a stored `display_id` still means
  something after a daemon restart; `HMONITOR` would not.
- **Foreground app** — `GetForegroundWindow` + `QueryFullProcessImageNameW`
  under `PROCESS_QUERY_LIMITED_INFORMATION`. `bundle_id` carries the
  lowercased executable name so per-app privacy rules keep working.
- **Screen lock** — `OpenInputDesktop(DESKTOP_SWITCHDESKTOP)`. Fails closed:
  a desktop this process cannot open means the lock screen or the UAC secure
  desktop, and capture is skipped.
- **OCR** — `Windows.Media.Ocr`, fully offline, serialized behind a mutex like
  the macOS Vision path. Line boxes are the union of the word rects,
  normalized to Vision's bottom-left convention.
- **Microphone** — the existing cpal capture moved to a shared crate; on
  Windows it runs on WASAPI unchanged.
- **Permissions** — `screen_recording` is `Granted` (Windows does not gate
  desktop capture), microphone comes from `AppCapability` for packaged builds
  and a WASAPI device probe otherwise, and `accessibility` is `Restricted`
  because there is no analogue and the features behind it are not implemented.
- **Paths** — data in `%LOCALAPPDATA%\LumenNavi`, shared models in
  `%LOCALAPPDATA%\Lumen\models` (same root Lumen ASR downloads into, so the
  ~163 MB SenseVoice package is fetched once per machine).
- **Sidecar** — resolves `lumen-daemon.exe` and spawns it with
  `CREATE_NO_WINDOW`, so the console-subsystem daemon does not flash a black
  console window on every start.
- **Shell integration** — `ShellExecuteW` for the data folder and
  `ms-settings:` deep links, instead of `cmd /c start`.
- **ASR** — Windows ships no recognizer worth using, so the `speech` engine is
  hidden in the UI and the "fall back to Speech" path is disabled. A failing
  local engine now surfaces its real error instead of silently swapping in an
  unsupported one.
- **Packaging** — `tauri.windows.conf.json` selects an NSIS current-user
  installer and the WebView2 download bootstrapper;
  `tauri.macos.conf.json` holds what used to be inline macOS bundle config.
- **CI** — `ci-windows.yml` builds and tests on `windows-latest`;
  `release-desktop.yml` replaces `release-macos.yml` so one tag-driven
  workflow publishes both platforms from a single aggregate job.

## Known Windows limitations

- **No text selection capture.** The 划词弹窗 needs UI Automation
  (`TextPattern` / `ValuePattern`) plus a low-level mouse hook. Until then
  `HostCapabilities::text_selection` is false, the monitor is never started,
  and the settings page says so.
- **OCR reports no confidence.** `Windows.Media.Ocr` exposes no score, so
  results carry `confidence: 0.0` and the UI hides the chip rather than
  showing a fabricated number.
- **OCR needs language packs.** Recognition only works for languages whose
  optional OCR feature is installed (Settings → Time & language → Language &
  region). With none installed, `is_supported()` is false and the worker does
  not start.
- **GDI capture excludes hardware-overlay content.** Protected video paths and
  some fullscreen-exclusive games blit black. Windows Graphics Capture would
  fix this at the cost of a D3D device; not required for Observe's cadence.
- **Unsigned installer.** SmartScreen warnings are expected until a trusted
  code-signing identity is configured. There is no Windows equivalent of
  macOS ad-hoc signing.
- **x64 only.** `sherpa-onnx-sys` 1.13.4 maps a Windows x64 prebuilt archive
  but no ARM64 one, so no ARM64 build is promised.
- **No MSIX / Store package yet.** Lumen ASR ships one; navi does not, because
  Observe's capture model needs a privacy review before Store submission.

## What Windows CI proves

`ci-windows.yml` is green on `windows-latest` (PR #7). It establishes:

- The whole workspace **compiles under MSVC**, not just under the mingw
  target used for local cross-checks. No `windows` crate API differences
  between the two toolchains surfaced — the feature set in
  `lumen-platform-windows/Cargo.toml` is sufficient for both. The only
  linker note is a benign `LNK4098 defaultlib 'LIBCMT' conflicts` warning
  from the statically linked sherpa-onnx prebuilt.
- `cargo test --workspace --exclude lumen-navi-desktop` **passes on Windows**,
  so the platform-layer split behaves identically on both hosts for
  everything that is unit-testable.
- `sherpa-onnx-sys` really does fetch and link its Windows x64 prebuilt; the
  archive path documented below is not just a resolved name.
- The frontend builds, `prepare-daemon-binary.ps1` produces
  `lumen-daemon-x86_64-pc-windows-msvc.exe`, and `tauri build --bundles nsis`
  consumes it as an `externalBin` sidecar and emits a ~25 MB
  `Lumen Navi_0.1.0_x64-setup.exe`.
- The browser extension test suite passes on Windows.

## Local Windows hardware verification

The current-user NSIS package was also built and exercised on Windows 11 x64:

- The installer completed without elevation, the app opened a responsive main
  window, and the packaged `lumen-daemon.exe` sidecar was present.
- A native display capture produced JPEG data, `Windows.Media.Ocr` returned
  text, and the result was persisted and found again through SQLite FTS.
- The browser API path accepted an event and exercised dedupe, export and
  sanitization behavior end to end.

This covers the primary single-display screen/OCR/search path. It does not
replace the edge-case and audio checks below.

## Verification pending

CI proves the code compiles, links, tests and packages. It does not launch
the app. These still need real Windows 10/11 x64 hardware:

1. Install the NSIS package on a clean machine without WebView2 and confirm
   the bootstrapper runs.
2. Start Observe with microphone capture enabled and confirm mic chunks
   transcribe through the configured local ASR engine.
3. Confirm foreground app and window-title enrichment across protected and
   elevated applications.
4. Multi-monitor with mixed DPI (100% + 150%), monitor hot-unplug during
   capture, and a display arrangement change.
5. Lock the machine and trigger a UAC prompt; confirm no frames are captured.
6. Sleep/resume and RDP reconnect.
7. Microphone denied in Settings → confirm the permission card reports it
   rather than failing silently.

Record failures here until they are resolved.

## Cross-platform gotchas found while greening CI

- **Line endings are part of the build contract.** Windows git defaults to
  `core.autocrlf=true`, so the runner rewrote every checked-out text file to
  CRLF. That broke `shared_model_contract_matches_cluster_v1`, which hashes
  the raw bytes of `docs/SHARED_MODELS_CONTRACT.md` — the doc was correct,
  the working tree was not. A repo-root `.gitattributes` now pins
  `* text=auto eol=lf`, so any byte- or hash-sensitive fixture means the same
  thing on both platforms. Keep new binary asset extensions listed there.
- **Generated type stubs need a `postinstall`.** `extensions/browser`'s
  `tsconfig.json` extends the wxt-generated `./.wxt/tsconfig.json`, which is
  gitignored. Nothing regenerated it after a clean `npm ci`, so vitest could
  not resolve the extends chain. This was never Windows-specific — it failed
  identically on macOS — it just took a fresh CI checkout to notice.
- **Tests must not read live host state.** `ControlState::new` wires
  `screen_locked` to the real OS probe, so the browser-gate fixture inherited
  whatever the machine's screen was doing and returned 423 instead of 200 on a
  locked desktop. Also predates the port (main has the same line against the
  macOS backend), but the Windows runner can report a locked desktop too. The
  fixture now pins the probe. Anything reached through `lumen-platform-host`
  should be injected in tests, never called for real.

## What was verified from macOS

- `cargo check --target x86_64-pc-windows-gnu` passes for the whole workspace
  except the Tauri shell (which needs the MSVC toolchain and WebView2). This
  turned out to be a good proxy for MSVC: it caught every `windows` crate
  issue before CI saw it.
- The full macOS test suite passes, so the platform-layer refactor did not
  regress the supported platform.
- `sherpa-onnx-sys` 1.13.4 resolves and requests
  `sherpa-onnx-v1.13.4-win-x64-static-MT-Release-lib.tar.bz2`, confirming the
  Windows x64 prebuilt path exists.
