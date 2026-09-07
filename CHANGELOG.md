# Changelog

## Unreleased

### Added

- Act v1 in Lumen Cua: L0 app probe, four delivery gates, window-level
  verify, stop-lines, and background `CGEventPostToPid` replay. Default
  replay does not steal focus. Desktop confirms each skill before Act.
- Act v2: optional MIT cua-driver nested in `Lumen Cua.app/Contents/Helpers`
  and spawned as a Cua child (`CUA_DRIVER_EMBEDDED=1`). Observe never
  starts it. Missing binary is a soft skip. Fetch is SHA-256 pinned.
- Act engine path: skill replay and `InputReplay` go through cua-driver
  with background-only policy. `launch_app`, window-local clicks,
  `page`, and the session cursor overlay are available. Noop no longer
  steals focus. Settings can copy/write a `computer-use` MCP snippet
  into Codex or Claude Code.
- Settings → 技能库: Act engine status and a one-shot 「启动 Act 引擎」.

- 设置 → 快捷键: quick-chat (快捷对话) global shortcut is now configurable
  (record-capture UI, live re-registration, conflict surfaced with the OS
  reason; empty disables). Stored in `navi.toml` `[shortcuts].composer`,
  default remains `Alt+Space`.
- Tray menu item 打开快捷对话 as the keyboard-free fallback when the
  shortcut is disabled or taken by another app.

### Changed

- Settings redesigned: one long page → six sections behind a secondary side
  nav (通用 / 采集 / 语音与转写 / AI 与划词 / 快捷键 / 技能库, last section
  remembered). Skill library no longer sits at the top of Settings and its
  list scrolls on its own. Settings UI moved from `App.tsx` (~1100 lines)
  into `views/SettingsView.tsx`.

### Fixed

- Dock icon badge displaying red "0" on macOS after health recovery: pass
  `undefined` instead of `0` to `setBadgeCount` to remove the badge.

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
