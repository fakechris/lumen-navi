#!/usr/bin/env bash
# Fail the desktop build if any bundled sidecar binary is missing or older
# than its sources.
#
# Why this exists: `prepare-daemon-binary.sh` rebuilds the daemon, but the
# Lumen Cua helper was only rebuilt when someone remembered to run
# `prepare-cua-app.sh` by hand. Skipping it silently shipped a stale helper —
# in production this caused a daemon↔cua protocol mismatch where every
# capture failed until the helper was rebuilt from the same source as the
# daemon. This check makes `tauri build` fail loudly instead.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
staged="$root/apps/desktop/src-tauri"

newest_mtime() { # dir -> newest file mtime (epoch), 0 if empty
  find "$1" -type f -exec stat -f '%m' {} + 2>/dev/null | sort -n | tail -1
}

file_mtime() { stat -f '%m' "$1" 2>/dev/null || echo 0; }

# Both sidecars are built from the workspace crates; the cua helper bundle
# additionally embeds apps/cua resources (Info.plist, icon).
crate_newest="$(newest_mtime "$root/crates")"
cua_src_newest="$crate_newest"
apps_cua_newest="$(newest_mtime "$root/apps/cua")"
if (( apps_cua_newest > cua_src_newest )); then
  cua_src_newest="$apps_cua_newest"
fi

fail=0
check_binary() { # label binary src_newest
  local label="$1" bin="$2" src_newest="$3"
  if [[ ! -f "$bin" ]]; then
    echo "ERROR: bundled $label is missing: $bin" >&2
    fail=1
    return
  fi
  local bin_mtime
  bin_mtime="$(file_mtime "$bin")"
  if (( bin_mtime < src_newest )); then
    echo "ERROR: bundled $label is STALE (binary $(date -r "$bin_mtime" '+%F %T') < newest source $(date -r "$src_newest" '+%F %T')): $bin" >&2
    fail=1
  fi
}

daemon_found=0
for bin in "$staged"/binaries/lumen-daemon-*; do
  [[ -e "$bin" ]] || continue
  daemon_found=1
  check_binary "lumen-daemon ($(basename "$bin"))" "$bin" "$crate_newest"
done
if (( ! daemon_found )); then
  echo "ERROR: no bundled lumen-daemon found under $staged/binaries/" >&2
  fail=1
fi

check_binary "Lumen Cua helper" "$staged/helpers/Lumen Cua.app/Contents/MacOS/lumen-cua" "$cua_src_newest"

if (( fail )); then
  cat >&2 <<'EOF'

Bundled sidecar binaries are missing or older than their sources. Rebuild:
  scripts/macos/prepare-daemon-binary.sh aarch64-apple-darwin
  scripts/macos/prepare-cua-app.sh aarch64-apple-darwin
Or use the one-shot release script (always rebuilds everything):
  scripts/macos/build-desktop-release.sh aarch64-apple-darwin
EOF
  exit 1
fi

echo "bundled helpers are fresh (daemon + cua match their sources)"
