# Lumen Cua app icon

Source mark from the Lumen Marks design system (**CUA — cursor**, palette cyan `#4fb2c4` on espresso tile).

| File | Role |
|------|------|
| `lumen-cua.svg` | Canonical mark (128²) |
| `AppIcon.png` | 512² PNG preview |
| `AppIcon.icns` | macOS bundle icon (`CFBundleIconFile`) |

Regenerate `AppIcon.icns` after editing the SVG:

```bash
# from repo root
SVG=apps/cua/icon/lumen-cua.svg
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"
for pair in 16 32 32 64 128 256 256 512 512 1024; do :; done
# or use the one-shot generator used in development:
rsvg-convert -w 1024 -h 1024 "$SVG" -o /tmp/cua-1024.png
# then sips/iconutil into AppIcon.icns, copy here.

cp apps/cua/icon/lumen-cua.svg apps/desktop/public/marks/lumen-cua.svg
```

`scripts/macos/prepare-cua-app.sh` copies `AppIcon.icns` into
`Lumen Cua.app/Contents/Resources/` so System Settings → Screen Recording
shows the cursor mark instead of a blank default.
