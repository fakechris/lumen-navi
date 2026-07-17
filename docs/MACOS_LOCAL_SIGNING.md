# macOS local signing (no paid Apple Developer Program)

> Daily development loop on one machine. Public/CI builds stay **ad-hoc**
> (`signingIdentity: "-"` in `tauri.conf.json`) — see
> [MACOS_RELEASE_NOTES.md](./MACOS_RELEASE_NOTES.md). A stable local identity
> keeps macOS TCC permissions (Screen Recording, Accessibility, Microphone)
> alive across rebuilds; ad-hoc breaks them every rebuild because each
> re-sign mints a new cdhash.

| Method | Needs | Share with others | Rebuild TCC |
|--------|-------|-------------------|-------------|
| **Ad-hoc** (`codesign -s -`) | nothing | Gatekeeper warning | **Breaks every rebuild** |
| **Self-signed "Lumen Local Codesign"** | one-time Keychain trust | local only | **Stable** |
| **Apple Development** (free Personal Team) | free Apple ID, yearly refresh | local | Stable while valid |
| **Developer ID + notarize** | $99/yr program | distribute | Stable |

## Identity resolution

`scripts/macos/resolve-identity.sh` picks, in order:

1. `$LUMEN_CODESIGN_IDENTITY` (explicit override)
2. **Lumen Local Codesign** — self-signed cert, shared with other Lumen apps
   (created by `ensure-local-identity.sh`, or Keychain Access → Certificate
   Assistant → Create a Certificate → Code Signing)
3. any valid `Apple Development: …`
4. ad-hoc `-`

## Usage

```bash
# one-time: create + trust the self-signed identity (GUI trust step if needed)
scripts/macos/ensure-local-identity.sh

# build a DMG signed with the stable identity
scripts/macos/prepare-daemon-binary.sh aarch64-apple-darwin
cd apps/desktop
APPLE_SIGNING_IDENTITY="$(../../scripts/macos/resolve-identity.sh)" \
  npm run tauri -- build --target aarch64-apple-darwin --bundles dmg

# or re-sign an already-built .app (e.g. after cargo-only rebuild)
scripts/macos/sign-app.sh
```

`tauri.conf.json` keeps `"signingIdentity": "-"` so CI/release DMGs need no
certificate; the env var only overrides local builds. Verify any build with:

```bash
codesign -dv --verbose=2 "target/aarch64-apple-darwin/release/bundle/macos/Lumen Navi.app" 2>&1 | grep Authority
# → Authority=Lumen Local Codesign
```
