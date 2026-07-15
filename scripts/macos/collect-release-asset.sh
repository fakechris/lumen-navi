#!/usr/bin/env bash
# Verify Tauri DMG contents and copy to a stable release asset name.
set -euo pipefail

if [[ $# -lt 3 || $# -gt 4 ]]; then
  echo "Usage: $0 <target> <vMAJOR.MINOR.PATCH> <bundle-dir> [output-dir]" >&2
  exit 2
fi

target="$1"
release_tag="$2"
bundle_dir="$3"
output_dir="${4:-dist}"

if [[ ! "$release_tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "Release tag must use the stable SemVer form vMAJOR.MINOR.PATCH: $release_tag" >&2
  exit 2
fi

case "$target" in
  aarch64-apple-darwin)
    expected_arch="arm64"
    asset_arch="arm64"
    ;;
  x86_64-apple-darwin)
    expected_arch="x86_64"
    asset_arch="x64"
    ;;
  *)
    echo "Unsupported macOS target: $target" >&2
    exit 2
    ;;
esac

dmgs=()
while IFS= read -r -d '' path; do
  dmgs+=("$path")
done < <(find "$bundle_dir/dmg" -maxdepth 1 -type f -name '*.dmg' -print0 2>/dev/null)

if [[ ${#dmgs[@]} -ne 1 ]]; then
  echo "Expected exactly one DMG in $bundle_dir/dmg, found ${#dmgs[@]}" >&2
  find "$bundle_dir" -type f 2>/dev/null | head -50 >&2 || true
  exit 1
fi

mount_dir="$(mktemp -d)"
mounted=0
cleanup() {
  if [[ "$mounted" -eq 1 ]]; then
    hdiutil detach "$mount_dir" >/dev/null || true
  fi
  rmdir "$mount_dir" 2>/dev/null || true
}
trap cleanup EXIT

hdiutil attach -readonly -nobrowse -mountpoint "$mount_dir" "${dmgs[0]}" >/dev/null
mounted=1

apps=()
while IFS= read -r -d '' path; do
  apps+=("$path")
done < <(find "$mount_dir" -maxdepth 1 -type d -name '*.app' -print0)

if [[ ${#apps[@]} -ne 1 ]]; then
  echo "Expected exactly one app bundle inside ${dmgs[0]}, found ${#apps[@]}" >&2
  exit 1
fi

app_path="${apps[0]}"
info_plist="$app_path/Contents/Info.plist"

codesign --verify --deep --strict --verbose=2 "$app_path"
signature_details="$(codesign -dv --verbose=4 "$app_path" 2>&1)"
printf '%s\n' "$signature_details"

if ! grep -Fq 'Signature=adhoc' <<<"$signature_details"; then
  echo "Expected an ad-hoc signature, but Signature=adhoc was not reported" >&2
  exit 1
fi

bundle_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$info_plist")"
expected_version="${release_tag#v}"
if [[ "$bundle_version" != "$expected_version" ]]; then
  echo "App version $bundle_version does not match tag version $expected_version" >&2
  exit 1
fi

executable="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$info_plist")"
if [[ "$executable" != "lumen-navi-desktop" ]]; then
  echo "Expected lumen-navi-desktop as the app entry point, found: $executable" >&2
  exit 1
fi
executable_path="$app_path/Contents/MacOS/$executable"
binary_archs="$(lipo -archs "$executable_path")"
if [[ "$binary_archs" != "$expected_arch" ]]; then
  echo "Expected $expected_arch executable, found: $binary_archs" >&2
  exit 1
fi

daemon_path="$app_path/Contents/MacOS/lumen-daemon"
if [[ ! -x "$daemon_path" ]]; then
  echo "Bundled lumen-daemon missing at $daemon_path" >&2
  ls -la "$app_path/Contents/MacOS/" >&2 || true
  exit 1
fi
daemon_archs="$(lipo -archs "$daemon_path")"
if [[ "$daemon_archs" != "$expected_arch" ]]; then
  echo "Expected $expected_arch daemon, found: $daemon_archs" >&2
  exit 1
fi

mkdir -p "$output_dir"
asset_path="$output_dir/Lumen-Navi-${release_tag}-${asset_arch}.dmg"
cp "${dmgs[0]}" "$asset_path"
echo "Prepared $asset_path"
