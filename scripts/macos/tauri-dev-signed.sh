#!/usr/bin/env bash
# tauri-dev-signed.sh — build + sign + run the desktop app for dev.
#
# Standardized dev loop that avoids the three pitfalls of plain `tauri dev`:
#   1. White screen (vite dev server not running or killed)
#   2. Cua connection rejected (unsigned binaries → peer_auth drops silently)
#   3. Screen capture permanently off (daemon boots before Cua is ready)
#
# This script uses frontendDist (vite build → dist/) instead of vite dev URL,
# so there's no separate vite process to manage — no white screen risk.
#
# Usage: bash scripts/macos/tauri-dev-signed.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IDENTITY="${LUMEN_CODESIGN_IDENTITY:-Lumen Local Codesign}"
DESKTOP="$ROOT/target/debug/lumen-navi-desktop"
DAEMON="$ROOT/target/debug/lumen-daemon"
CUA_SOCKET="$HOME/Library/Application Support/Lumen/Cua/run/cua.sock"

cd "$ROOT"

# --- 1. Clean stale state ---
echo "=== cleaning stale processes ==="
pkill -f "lumen-navi-desktop" 2>/dev/null || true
pkill -f "target/debug/lumen-daemon" 2>/dev/null || true
pkill -f "lumen-cua serve" 2>/dev/null || true
pkill -f "vite.*1421" 2>/dev/null || true
lsof -ti :1421 2>/dev/null | xargs kill -9 2>/dev/null || true
lsof -ti :7420 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 2
rm -f "$CUA_SOCKET" 2>/dev/null || true

# --- 2. Build (Rust debug + frontend dist) ---
echo "=== building Rust (debug) ==="
cargo build -p lumen-daemon -p lumen-navi-desktop 2>&1 | tail -1

echo "=== building frontend (dist) ==="
cd "$ROOT/apps/desktop"
npm install --silent 2>/dev/null || true
npx vite build 2>&1 | tail -1
cd "$ROOT"

# --- 3. Sign dev binaries ---
echo "=== signing binaries with '$IDENTITY' ==="
bash "$ROOT/scripts/macos/sign-dev-binaries.sh" 2>/dev/null || {
  echo "sign-dev-binaries.sh failed; signing directly" >&2
  codesign --force --sign "$IDENTITY" --identifier "com.lumenopen.navi" "$DESKTOP"
  codesign --force --sign "$IDENTITY" --identifier "lumen-daemon" "$DAEMON"
}

# --- 4. Pre-launch Cua (must be ready before daemon boots) ---
echo "=== launching Lumen Cua ==="
open -n -g "/Applications/Lumen Cua.app" --args serve
# Wait for socket (up to 10s).
for i in $(seq 1 10); do
  [[ -S "$CUA_SOCKET" ]] && break
  sleep 1
done
if [[ ! -S "$CUA_SOCKET" ]]; then
  echo "WARNING: Cua socket not ready after 10s — screen capture may not work." >&2
  echo "         Time tracking and Dashboard will still function." >&2
fi

# --- 5. Launch desktop (uses frontendDist=../dist, no vite needed) ---
echo "=== launching desktop ==="
exec "$DESKTOP"
