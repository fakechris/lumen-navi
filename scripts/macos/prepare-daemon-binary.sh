#!/usr/bin/env bash
# Build lumen-daemon for a target triple and place it for Tauri externalBin.
# Tauri expects: src-tauri/binaries/<name>-<target-triple>
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <target-triple>" >&2
  echo "  e.g. aarch64-apple-darwin | x86_64-apple-darwin" >&2
  exit 2
fi

target="$1"
root="$(cd "$(dirname "$0")/../.." && pwd)"
bin_dir="$root/apps/desktop/src-tauri/binaries"
mkdir -p "$bin_dir"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"

echo "Building lumen-daemon for $target …"
cargo build -p lumen-daemon --release --target "$target" --manifest-path "$root/Cargo.toml"

src="$CARGO_TARGET_DIR/$target/release/lumen-daemon"
if [[ ! -x "$src" ]]; then
  echo "Missing built daemon: $src" >&2
  exit 1
fi

dest="$bin_dir/lumen-daemon-$target"
cp "$src" "$dest"
chmod +x "$dest"
echo "Prepared $dest"
