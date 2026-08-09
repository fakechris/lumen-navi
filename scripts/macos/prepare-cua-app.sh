#!/usr/bin/env bash
# Build the product-neutral Lumen Cua helper and prepare its nested app bundle.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <target-triple>" >&2
  echo "  e.g. aarch64-apple-darwin | x86_64-apple-darwin" >&2
  exit 2
fi

target="$1"
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *)
    echo "Unsupported Lumen Cua target: $target" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/apps/desktop/src-tauri/helpers/Lumen Cua.app"
contents="$app/Contents"
macos_dir="$contents/MacOS"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

echo "Building lumen-cua for $target …"
cargo build -p lumen-cua --bin lumen-cua --release --target "$target" --manifest-path "$root/Cargo.toml"

src="$CARGO_TARGET_DIR/$target/release/lumen-cua"
if [[ ! -x "$src" ]]; then
  echo "Missing built Lumen Cua binary: $src" >&2
  exit 1
fi

resources_dir="$contents/Resources"
mkdir -p "$macos_dir" "$resources_dir"
cp "$root/apps/cua/Info.plist" "$contents/Info.plist"
version="$(node -p "require('$root/apps/desktop/src-tauri/tauri.conf.json').version")"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$contents/Info.plist"
cp "$src" "$macos_dir/lumen-cua"
chmod +x "$macos_dir/lumen-cua"

# App icon (System Settings / Finder). Source: Lumen Marks design system — CUA cursor.
# Info.plist must declare CFBundleIconFile=AppIcon; Resources/AppIcon.icns is the payload.
icon_icns="$root/apps/cua/icon/AppIcon.icns"
if [[ ! -f "$icon_icns" ]]; then
  echo "Missing Lumen Cua icon: $icon_icns" >&2
  echo "Canonical SVG: $root/apps/cua/icon/lumen-cua.svg (see apps/cua/icon/README.md)" >&2
  exit 1
fi
cp "$icon_icns" "$resources_dir/AppIcon.icns"
icon_key="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$contents/Info.plist" 2>/dev/null || true)"
if [[ "$icon_key" != "AppIcon" ]]; then
  echo "apps/cua/Info.plist must set CFBundleIconFile=AppIcon (got: ${icon_key:-<missing>})" >&2
  exit 1
fi


identity="${APPLE_SIGNING_IDENTITY:-$("$root/scripts/macos/resolve-identity.sh")}"
if [[ "$identity" == "-" ]]; then
  echo "Lumen Cua requires a certificate-backed identity; ad-hoc signing cannot preserve or authenticate TCC access." >&2
  echo "Run scripts/macos/ensure-local-identity.sh, or set APPLE_SIGNING_IDENTITY." >&2
  exit 1
fi
codesign --force --sign "$identity" --timestamp=none "$macos_dir/lumen-cua"
codesign --force --sign "$identity" --timestamp=none "$app"
codesign --verify --deep --strict --verbose=1 "$app"
requirement="$(codesign -d -r- "$app" 2>&1 | sed -n 's/^designated => //p')"
if [[ -z "$requirement" || "$requirement" == *cdhash* || "$requirement" != *certificate* ]]; then
  echo "Lumen Cua requires a certificate-backed designated requirement; got: ${requirement:-<none>}" >&2
  exit 1
fi
echo "Prepared $app with identity: $identity"
echo "  icon: $resources_dir/AppIcon.icns"
echo "Designated requirement: $requirement"
