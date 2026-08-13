#!/usr/bin/env bash
# One-shot signed desktop release build (macOS).
#
# Always rebuilds EVERY bundled sidecar (lumen-daemon + Lumen Cua helper)
# from source before packaging, so a stale helper can never be shipped.
# Usage:
#   scripts/macos/build-desktop-release.sh [target-triple] [bundles]
#   e.g. scripts/macos/build-desktop-release.sh aarch64-apple-darwin dmg
set -euo pipefail

target="${1:-aarch64-apple-darwin}"
bundles="${2:-dmg}"
root="$(cd "$(dirname "$0")/../.." && pwd)"

"$root/scripts/macos/prepare-daemon-binary.sh" "$target"
"$root/scripts/macos/prepare-cua-app.sh" "$target"

cd "$root/apps/desktop"
if [[ ! -d node_modules ]]; then
  npm ci
fi

identity="${APPLE_SIGNING_IDENTITY:-$("$root/scripts/macos/resolve-identity.sh")}"
APPLE_SIGNING_IDENTITY="$identity" \
  npm run tauri -- build --target "$target" --bundles "$bundles"
