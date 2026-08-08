#!/usr/bin/env bash
# Sign the dev-mode debug binaries with the local Lumen codesigning identity.
#
# Cua's peer_auth requires clients (lumen-daemon, lumen-navi-desktop) to be
# signed by the same "Lumen Local Codesign" identity. Dev builds from
# `cargo build` are unsigned, so Cua rejects them ("invalid response header"
# — the connection is silently dropped before any protocol response).
#
# Run this after `npm run tauri dev` builds the desktop crate, then restart
# the app. The daemon in binaries/ should be signed once (it's not recompiled
# by tauri dev unless daemon source changed).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IDENTITY="${LUMEN_CODESIGN_IDENTITY:-Lumen Local Codesign}"

# Ensure the identity exists.
if ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"${IDENTITY}\""; then
  echo "Identity '${IDENTITY}' not found. Run scripts/macos/ensure-local-identity.sh first." >&2
  exit 1
fi

echo "Signing dev binaries with '${IDENTITY}'…"

# Daemon (used by tauri as externalBin, and the debug build).
for daemon in \
  "$ROOT/target/debug/lumen-daemon" \
  "$ROOT/apps/desktop/src-tauri/binaries/lumen-daemon-aarch64-apple-darwin"
do
  if [[ -f "$daemon" ]]; then
    codesign --force --sign "$IDENTITY" --identifier "lumen-daemon" "$daemon"
    echo "  signed $daemon"
  fi
done

# Desktop shell.
desktop="$ROOT/target/debug/lumen-navi-desktop"
if [[ -f "$desktop" ]]; then
  codesign --force --sign "$IDENTITY" --identifier "com.lumenopen.navi" "$desktop"
  echo "  signed $desktop"
fi

echo "Done. Restart the app for the signatures to take effect."
