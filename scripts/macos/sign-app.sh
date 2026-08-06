#!/usr/bin/env bash
# Re-sign a built Lumen Navi.app with the best stable identity available.
# Usage: sign-app.sh [/path/to/Lumen Navi.app]

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${1:-$ROOT/target/aarch64-apple-darwin/release/bundle/macos/Lumen Navi.app}"

if [[ ! -d "$APP" ]]; then
  echo "ERROR: app not found: $APP" >&2
  exit 1
fi

# Try to ensure local identity exists (may exit 2 if trust GUI still needed — ignore).
"$ROOT/scripts/macos/ensure-local-identity.sh" >/dev/null 2>&1 || true

IDENTITY="$("$ROOT/scripts/macos/resolve-identity.sh")"
ENTITLEMENTS="${LUMEN_CODESIGN_ENTITLEMENTS:-$ROOT/scripts/macos/entitlements.dev.plist}"
if [[ "$IDENTITY" == "-" ]]; then
  echo "ERROR: Lumen Cua requires a certificate-backed identity; refusing ad-hoc fallback." >&2
  echo "Run scripts/macos/ensure-local-identity.sh, then trust Lumen Local Codesign." >&2
  exit 1
fi

xattr -cr "$APP" 2>/dev/null || true

SIGN_OPTIONS=(--force --timestamp=none)
if [[ "${LUMEN_CODESIGN_HARDENED:-0}" == "1" ]]; then
  SIGN_OPTIONS+=(--options runtime)
fi

echo "Signing: $APP"
echo "  identity: $IDENTITY"

sign_all() {
  local identity="$1"
  local cua_app="$APP/Contents/Resources/helpers/Lumen Cua.app"
  local cua_binary="$cua_app/Contents/MacOS/lumen-cua"
  local daemon="$APP/Contents/MacOS/lumen-daemon"

  # Nested code has its own identity and entitlement boundary. Sign from the
  # inside out; applying Navi's entitlements to Lumen Cua would blur TCC ownership.
  if [[ -x "$cua_binary" ]]; then
    codesign "${SIGN_OPTIONS[@]}" --sign "$identity" "$cua_binary"
    codesign "${SIGN_OPTIONS[@]}" --sign "$identity" "$cua_app"
  fi
  if [[ -x "$daemon" ]]; then
    codesign "${SIGN_OPTIONS[@]}" --sign "$identity" "$daemon"
  fi

  local outer_args=("${SIGN_OPTIONS[@]}" --sign "$identity")
  if [[ -f "$ENTITLEMENTS" ]]; then
    outer_args+=(--entitlements "$ENTITLEMENTS")
  fi
  codesign "${outer_args[@]}" "$APP"
}

# codesign can hang on keychain UI — fail fast if possible
if ! sign_all "$IDENTITY"; then
  echo "ERROR: codesign failed for identity: $IDENTITY" >&2
  exit 1
fi

echo "Verify:"
codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | tail -8
codesign -dv --verbose=2 "$APP" 2>&1 | grep -iE "Authority|Signature|TeamIdentifier|Identifier|Format|flags=" || true

REQ="$(codesign -d -r- "$APP" 2>&1 | grep 'designated =>' || true)"
echo "Requirement: $REQ"
echo "OK: signed with stable identity (TCC should survive rebuilds)."
