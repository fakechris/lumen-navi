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

xattr -cr "$APP" 2>/dev/null || true

ARGS=(--force --deep --sign "$IDENTITY" --timestamp=none)

if [[ "${LUMEN_CODESIGN_HARDENED:-0}" == "1" ]]; then
  ARGS+=(--options runtime)
fi
if [[ -f "$ENTITLEMENTS" ]]; then
  ARGS+=(--entitlements "$ENTITLEMENTS")
fi

echo "Signing: $APP"
if [[ "$IDENTITY" == "-" ]]; then
  echo "  identity: ad-hoc (-)"
  echo "  WARN: ad-hoc changes cdhash every rebuild → Screen/Accessibility often need re-grant."
  echo "  Fix: trust \"Lumen Local Codesign\" in Keychain Access, or renew free Apple Development."
else
  echo "  identity: $IDENTITY"
fi

# codesign can hang on keychain UI — fail fast if possible
if ! codesign "${ARGS[@]}" "$APP"; then
  echo "ERROR: codesign failed for identity: $IDENTITY" >&2
  if [[ "$IDENTITY" != "-" ]]; then
    echo "Retrying ad-hoc so the app at least launches…" >&2
    codesign --force --deep --sign - --timestamp=none "$APP"
    IDENTITY="-"
  else
    exit 1
  fi
fi

echo "Verify:"
if codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | tail -8; then
  :
else
  echo "WARN: strict verify failed (common for ad-hoc); app may still run locally." >&2
fi
codesign -dv --verbose=2 "$APP" 2>&1 | grep -iE "Authority|Signature|TeamIdentifier|Identifier|Format|flags=" || true

if [[ "$IDENTITY" != "-" ]]; then
  REQ="$(codesign -d -r- "$APP" 2>&1 | grep 'designated =>' || true)"
  echo "Requirement: $REQ"
  echo "OK: signed with stable identity (TCC should survive rebuilds)."
else
  echo "OK: ad-hoc signed. Prefer stable identity for daily work — see docs/MACOS_LOCAL_SIGNING.md"
fi
