# Lumen Navi

Local-first **continuous context** platform.

Lumen Navi continuously ingests multi-modal signals (screen, audio, later browser & tools), stores them under clear privacy boundaries, and turns them into structured memory and actionable context.

**Greenfield Rust workspace** — https://github.com/fakechris/lumen-navi

## One-liner

**Keep watching what matters — screen and sound first — then make that stream useful.**

## Architecture (summary)

Three planes:

| Plane | Role | Status |
|-------|------|--------|
| **Observe** | Multi-source intake | Screen + mic productized |
| **Memory** | Durable store + async process | SQLite + FTS + jobs |
| **Act** | Optional computer-use | Later, via open-source **cua-driver** (MIT) |

Full write-up: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) · roadmap: [`docs/PLAN.md`](docs/PLAN.md) · vision: [`docs/VISION.md`](docs/VISION.md)

## Status (current)

| Phase | Status |
|-------|--------|
| S0–S1 skeleton + store | ✅ |
| S2 screen Observe | ✅ (manual soak open) |
| S3 audio + Observe ASR | ✅ (16 kHz / 3s; SenseVoice default + Whisper/Speech/Qwen HTTP) |
| S4 Vision OCR + FTS API | ✅ |
| **U1 Tauri Mac app** | ✅ shell (control + search + start/stop daemon) |
| S4.1 OCR helper isolation | optional later |
| Chrome Observe MVP | ✅ implementation; manual soak pending |
| System audio / Act | later |

## Workspace

```
lumen-navi/
├── crates/          # daemon + libraries
├── apps/desktop/    # Tauri 2 Mac shell
├── extensions/      # Chrome Browser Observe MVP
└── docs/
```

## Quick start (daemon)

```bash
cargo build
cargo test
cargo run -p lumen-daemon
```

Requires Rust stable (edition 2021+). Start Observe from the desktop app so its
bundled **Lumen Cua** helper can request Screen Recording. Microphone and optional
Speech Recognition permissions remain owned by Lumen Navi.

Default continuous ASR is **SenseVoice** (local sherpa-onnx). Models live under the **shared Lumen cluster** path  
`~/Library/Application Support/Lumen/models/` (override with `LUMEN_MODELS_DIR` / `asr.models_root`) so navi and asr share one download.  
Pick any ready folder via `asr.model_dir` or onboarding. Optional engines: `whisper`, `speech`, OpenAI-compatible HTTP (`qwen`). See [`docs/AUDIO_PRODUCT.md`](docs/AUDIO_PRODUCT.md).

```bash
# search while daemon is up
curl -s 'http://127.0.0.1:7420/v1/ocr/search?q=关键词&limit=5' | jq .
```

## Desktop (Mac app)

```bash
cargo build -p lumen-daemon --release
cd apps/desktop && npm install && npm run build
cargo run -p lumen-navi-desktop
# or: cd apps/desktop && npx tauri dev
```

See [`docs/DESKTOP.md`](docs/DESKTOP.md).

### Time-tracking categories (rules, not code)

App categories use a **fixed match engine** + **editable JSON rules** (no rebuild to tune keywords):

- Spec & usage: [`crates/lumen-store/rules/README.md`](crates/lumen-store/rules/README.md)
- Defaults: `category_mapping.v1.json` (text / iTunes / LS), `app_catalog.v1.json` (known apps)
- Live overrides: `~/Library/Application Support/LumenNavi/rules/`

## Browser Observe

The Chrome MV3 extension records a privacy-gated lifecycle stream into its own local IndexedDB archive and can optionally sync a transport copy into the local daemon. Standalone capture needs no token or daemon. Page content remains metadata-only unless its host appears in an explicit extension or daemon allow-list. The extension includes a local full-page archive for activity, domain, attention, and source-detail review. No HTML, input values, selections, clipboard data, or DOM link lists are collected.

Setup, build, API, and privacy contract: [`docs/BROWSER_CAPTURE.md`](docs/BROWSER_CAPTURE.md).

### Release DMG

```bash
git tag v0.1.0
git push origin v0.1.0
# → GitHub Actions builds arm64 + x64 DMGs and publishes a Release
```

Install notes: [`docs/MACOS_RELEASE_NOTES.md`](docs/MACOS_RELEASE_NOTES.md).

## Related projects

| Project | Link | Relationship |
|---------|------|----------------|
| **Lumen ASR** | https://github.com/fakechris/lumen-asr | Separate **voice dictation** product. Share patterns only; **not** merged. |
| **cua-driver** | https://github.com/trycua/cua | Open-source **MIT** computer-use for optional **Act**. Never for Observe. |

## Config highlights

| Key | Default |
|-----|---------|
| `capture.*` | multi-display, probe, debounce — `docs/OBSERVE_CAPTURE.md` |
| `audio.sample_rate` / `chunk_ms` | 16000 / 3000 |
| `asr.enabled` / `locale` | true / `zh-CN` |
| `ocr.enabled` | true |
| `api.bind` | `127.0.0.1:7420` |
| `sources.browser` | `false` |
| `browser.content_allow_hosts` | `[]` (metadata only) |

**cua-driver is not used for capture/OCR/ASR.**

## License

Lumen Navi is licensed under the [GNU General Public License v3.0 only](LICENSE).
