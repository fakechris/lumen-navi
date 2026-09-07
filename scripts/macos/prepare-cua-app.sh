#!/usr/bin/env bash
# Build the product-neutral Lumen Cua helper and prepare its nested app bundle.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <target-triple>" >&2
  echo "  e.g. aarch64-apple-darwin | x86_64-apple-darwin" >&2
  exit 2
fi

target="$1"
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *)
    echo "Unsupported Lumen Cua target: $target" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/apps/desktop/src-tauri/helpers/Lumen Cua.app"
contents="$app/Contents"
macos_dir="$contents/MacOS"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

echo "Building lumen-cua for $target …"
cargo build -p lumen-cua --bin lumen-cua --release --target "$target" --manifest-path "$root/Cargo.toml"

src="$CARGO_TARGET_DIR/$target/release/lumen-cua"
if [[ ! -x "$src" ]]; then
  echo "Missing built Lumen Cua binary: $src" >&2
  exit 1
fi

resources_dir="$contents/Resources"
mkdir -p "$macos_dir" "$resources_dir"
cp "$root/apps/cua/Info.plist" "$contents/Info.plist"
version="$(node -p "require('$root/apps/desktop/src-tauri/tauri.conf.json').version")"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$contents/Info.plist"
cp "$src" "$macos_dir/lumen-cua"
chmod +x "$macos_dir/lumen-cua"

# App icon (System Settings / Finder). Source: Lumen Marks design system — CUA cursor.
# Info.plist must declare CFBundleIconFile=AppIcon; Resources/AppIcon.icns is the payload.
icon_icns="$root/apps/cua/icon/AppIcon.icns"
if [[ ! -f "$icon_icns" ]]; then
  echo "Missing Lumen Cua icon: $icon_icns" >&2
  echo "Canonical SVG: $root/apps/cua/icon/lumen-cua.svg (see apps/cua/icon/README.md)" >&2
  exit 1
fi
cp "$icon_icns" "$resources_dir/AppIcon.icns"

# Optional MIT cua-driver (Act v2). Missing binary is a warning, not a failed
# helper build — HID replay does not need it. Never fetch cua-agent[omni].
helpers_dir="$contents/Helpers"
mkdir -p "$helpers_dir"
driver_src=""
if [[ -n "${CUA_DRIVER_BIN:-}" && -f "${CUA_DRIVER_BIN}" ]]; then
  driver_src="$CUA_DRIVER_BIN"
elif [[ -f "$root/apps/cua/vendor/cua-driver" ]]; then
  driver_src="$root/apps/cua/vendor/cua-driver"
elif [[ "${CUA_DRIVER_FETCH:-1}" == "1" ]]; then
  version="${CUA_DRIVER_VERSION:-0.23.2}"
  vendor="$root/apps/cua/vendor"
  mkdir -p "$vendor"
  tarball="$vendor/cua-driver-rs-${version}-darwin-universal-binary.tar.gz"
  tarball_name="cua-driver-rs-${version}-darwin-universal-binary.tar.gz"
  url="https://github.com/trycua/cua/releases/download/cua-driver-rs-v${version}/${tarball_name}"
  pin="$vendor/checksums-${version}.txt"
  expected_sha="${CUA_DRIVER_SHA256:-}"
  if [[ -z "$expected_sha" && -f "$pin" ]]; then
    expected_sha="$(awk -v f="$tarball_name" '$2 == f { print $1; exit }' "$pin")"
  fi
  if [[ -z "$expected_sha" ]]; then
    echo "ERROR: no SHA-256 pin for ${tarball_name}." >&2
    echo "Add $pin or set CUA_DRIVER_SHA256. Refusing to fetch an unsigned tarball." >&2
    exit 1
  fi
  if [[ ! -f "$vendor/cua-driver" ]]; then
    echo "Fetching MIT cua-driver ${version} …"
    if curl -fsSL --retry 3 -o "$tarball" "$url"; then
      actual_sha="$(shasum -a 256 "$tarball" | awk '{ print $1 }')"
      if [[ "$actual_sha" != "$expected_sha" ]]; then
        echo "ERROR: cua-driver tarball SHA-256 mismatch" >&2
        echo "  expected: $expected_sha" >&2
        echo "  actual:   $actual_sha" >&2
        rm -f "$tarball"
        exit 1
      fi
      tmp="$(mktemp -d)"
      tar -xzf "$tarball" -C "$tmp"
      found="$(find "$tmp" -type f -name cua-driver | head -1 || true)"
      if [[ -n "$found" ]]; then
        cp "$found" "$vendor/cua-driver"
        chmod +x "$vendor/cua-driver"
      else
        echo "WARNING: cua-driver tarball had no cua-driver binary" >&2
      fi
      rm -rf "$tmp"
    else
      echo "WARNING: could not fetch cua-driver from $url (Act v2 optional)" >&2
    fi
  fi
  if [[ -f "$vendor/cua-driver" ]]; then
    driver_src="$vendor/cua-driver"
  fi
fi
if [[ -n "$driver_src" ]]; then
  cp "$driver_src" "$helpers_dir/cua-driver"
  chmod +x "$helpers_dir/cua-driver"
  echo "Bundled cua-driver: $helpers_dir/cua-driver"
else
  echo "NOTE: cua-driver not bundled (set CUA_DRIVER_BIN or CUA_DRIVER_FETCH=1). HID replay still works."
fi
icon_key="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$contents/Info.plist" 2>/dev/null || true)"
if [[ "$icon_key" != "AppIcon" ]]; then
  echo "apps/cua/Info.plist must set CFBundleIconFile=AppIcon (got: ${icon_key:-<missing>})" >&2
  exit 1
fi


identity="${APPLE_SIGNING_IDENTITY:-$("$root/scripts/macos/resolve-identity.sh")}"
if [[ "$identity" == "-" ]]; then
  echo "Lumen Cua requires a certificate-backed identity; ad-hoc signing cannot preserve or authenticate TCC access." >&2
  echo "Run scripts/macos/ensure-local-identity.sh, or set APPLE_SIGNING_IDENTITY." >&2
  exit 1
fi
codesign --force --sign "$identity" --timestamp=none "$macos_dir/lumen-cua"
if [[ -x "$helpers_dir/cua-driver" ]]; then
  # Same flags as lumen-cua. Do not enable Hardened Runtime on the nested
  # binary unless the host has matching entitlements.
  codesign --force --sign "$identity" --timestamp=none "$helpers_dir/cua-driver"
fi
codesign --force --sign "$identity" --timestamp=none "$app"
codesign --verify --deep --strict --verbose=1 "$app"
requirement="$(codesign -d -r- "$app" 2>&1 | sed -n 's/^designated => //p')"
if [[ -z "$requirement" || "$requirement" == *cdhash* || "$requirement" != *certificate* ]]; then
  echo "Lumen Cua requires a certificate-backed designated requirement; got: ${requirement:-<none>}" >&2
  exit 1
fi
echo "Prepared $app with identity: $identity"
echo "  icon: $resources_dir/AppIcon.icns"
echo "Designated requirement: $requirement"
