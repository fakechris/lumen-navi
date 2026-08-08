#!/usr/bin/env bash
# tauri-dev-signed.sh — run `npm run tauri dev` with post-build code signing.
#
# Problem: `tauri dev` recompiles the desktop + daemon binaries on every source
# change. Cua's peer_auth rejects unsigned clients (the desktop app probes Cua
# status every few seconds from the desktop process itself). An unsigned dev
# build can never connect.
#
# This wrapper: (1) kills stale Cua instances + stale socket, (2) launches
# `tauri dev`, (3) after the desktop crate finishes compiling, signs the
# freshly-built binaries with Lumen Local Codesign, (4) restarts the desktop
# binary so the signature takes effect (code signatures are validated at
# launch, not mid-run).
#
# Usage: scripts/macos/tauri-dev-signed.sh
# (equivalent to `npm run tauri dev` in apps/desktop, but signed)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IDENTITY="${LUMEN_CODESIGN_IDENTITY:-Lumen Local Codesign}"
DESKTOP="$ROOT/target/debug/lumen-navi-desktop"
DAEMON="$ROOT/target/debug/lumen-daemon"
CUA_SOCKET="$HOME/Library/Application Support/Lumen/Cua/run/cua.sock"

cd "$ROOT/apps/desktop"

# 1. Clean stale state.
pkill -f "lumen-cua serve" 2>/dev/null || true
rm -f "$CUA_SOCKET" 2>/dev/null || true

# 2. Pre-sign existing binaries (in case of incremental rebuild skip).
for bin in "$DESKTOP" "$DAEMON"; do
  [[ -f "$bin" ]] && codesign --force --sign "$IDENTITY" --identifier \
    "$( [[ "$bin" == *desktop* ]] && echo com.lumenopen.navi || echo lumen-daemon )" \
    "$bin" 2>/dev/null || true
done

echo "=== launching tauri dev (build + vite) ==="
npm run tauri dev &
TURI_PID=$!

# 3. Wait for the desktop binary to be freshly compiled.
echo "=== waiting for desktop binary to compile ==="
for i in $(seq 1 120); do
  if ! kill -0 $TURI_PID 2>/dev/null; then
    echo "tauri dev exited early"; wait $TURI_PID; exit $?
  fi
  # Once the desktop process is running, the binary is compiled.
  if pgrep -f "target/debug/lumen-navi-desktop" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

sleep 2  # let the process fully initialize

# 4. Re-sign the binaries that tauri dev just rebuilt.
echo "=== signing freshly built binaries ==="
codesign --force --sign "$IDENTITY" --identifier "com.lumenopen.navi" "$DESKTOP" 2>/dev/null && echo "  signed desktop" || true
codesign --force --sign "$IDENTITY" --identifier "lumen-daemon" "$DAEMON" 2>/dev/null && echo "  signed daemon" || true

# 5. Restart the desktop binary so the signature is picked up.
echo "=== restarting desktop with valid signature ==="
pkill -f "target/debug/lumen-navi-desktop" 2>/dev/null || true
pkill -f "target/debug/lumen-daemon" 2>/dev/null || true
sleep 2

# Pre-launch Cua so it's ready when desktop connects.
rm -f "$CUA_SOCKET" 2>/dev/null || true
open -n -g "/Applications/Lumen Cua.app" --args serve 2>/dev/null || true
sleep 3

# Run the signed desktop binary directly (vite is still serving on 1421).
exec "$DESKTOP"
