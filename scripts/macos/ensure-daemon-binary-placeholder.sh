#!/usr/bin/env bash
# Ensure Tauri externalBin path exists for the current host (dev cargo check/build).
# CI uses prepare-daemon-binary.sh for real release binaries.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
bin_dir="$root/apps/desktop/src-tauri/binaries"
mkdir -p "$bin_dir"

host="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "$host" ]]; then
  echo "Could not detect rustc host triple" >&2
  exit 1
fi

dest="$bin_dir/lumen-daemon-$host"
if [[ -e "$dest" ]]; then
  echo "OK $dest"
  exit 0
fi

# Prefer copying a workspace build if available.
for cand in \
  "$root/target/release/lumen-daemon" \
  "$root/target/debug/lumen-daemon" \
  "$root/target/$host/release/lumen-daemon" \
  "$root/target/$host/debug/lumen-daemon"
do
  if [[ -x "$cand" ]]; then
    cp "$cand" "$dest"
    chmod +x "$dest"
    echo "Copied $cand → $dest"
    exit 0
  fi
done

cat > "$dest" << 'EOF'
#!/bin/sh
echo "stub lumen-daemon: run scripts/macos/prepare-daemon-binary.sh <target>" >&2
exit 127
EOF
chmod +x "$dest"
echo "Wrote stub $dest"
