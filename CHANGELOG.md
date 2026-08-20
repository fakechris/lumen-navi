# Changelog

## 0.2.0 - 2026-08-19

First product-shaped release after v0.1.0 (2026-07-15). The desktop app now tracks time, narrates 15-minute stretches, searches OCR, and optionally talks about the day — still local-first.

### Added

- Time tab: 15-minute History cards (duration-ranked apps, LLM narrative, app marks, CUA-replay chips)
- Time tracking: frontmost app + URL, HID idle, screensaver, display-sleep assertions (video/calls stay present)
- Overview: capture health, channel toggles, day/week/month rollups
- AI tab: Roast + Chat with shared LLM config (OpenAI-compat + Anthropic), persisted threads
- Deep AX tree capture for recall (through Lumen Cua, correct TCC)
- Scene engine (external JSON rules) on the Time dashboard
- Windows 10/11 x64 Observe port (unsigned NSIS installer)
- Chrome Observe extension (metadata-first; optional daemon sync)
- Mic device picker + recording self-test; audio trunk quality (VAD floor, onset pre-roll)
- Daemon supervisor, health monitor, Unix socket + TCP for the extension
- Static-screen **liveness** overwrite: one last frame per display under `data_dir/liveness/`, not evidence

### Changed

- Screen capture is owned by the nested **Lumen Cua** helper; daemon owns policy and persistence
- 15-minute cards skip idle/lock time; messaging stays on the card, not as a CUA replay
- Sidecar freshness check: tauri build fails if bundled daemon/Cua binaries are stale

### Fixed

- AX crashes (UAF, retain rules, dedicated thread, skip browsers)
- Idle miscounts (HID via IOKit, power-assertion FFI, screensaver)
- Audio junk transcripts and single-syllable drops
- Timeline thumbs, zero-duration apps, cross-day slot narratives

## 0.1.0 - 2026-07-15

Initial desktop release: Observe screen + mic, OCR search, Tauri macOS shell.
