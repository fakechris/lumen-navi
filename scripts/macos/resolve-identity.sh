#!/usr/bin/env bash
# Print the best available codesign identity for local builds.
# Exit 0 and print identity name, or print "-" (ad-hoc) with exit 0.
# Exit 1 only on hard failure.
#
# Same resolution order as lumen-asr: $LUMEN_CODESIGN_IDENTITY →
# "Lumen Local Codesign" (self-signed, shared across Lumen apps) →
# any "Apple Development: …" → ad-hoc "-".

set -euo pipefail

list_valid() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' || true
}

VALID="$(list_valid)"

pick() {
  local want="$1"
  while IFS= read -r line; do
    if [[ "$line" == "$want" ]]; then
      echo "$line"
      return 0
    fi
  done <<<"$VALID"
  return 1
}

# 1) explicit override
if [[ -n "${LUMEN_CODESIGN_IDENTITY:-}" ]]; then
  if [[ "$LUMEN_CODESIGN_IDENTITY" == "-" ]]; then
    echo "-"
    exit 0
  fi
  if pick "$LUMEN_CODESIGN_IDENTITY" >/dev/null; then
    echo "$LUMEN_CODESIGN_IDENTITY"
    exit 0
  fi
  # allow hash / partial — codesign will error if bad
  echo "$LUMEN_CODESIGN_IDENTITY"
  exit 0
fi

# 2) stable local self-signed
if pick "Lumen Local Codesign" >/dev/null; then
  echo "Lumen Local Codesign"
  exit 0
fi

# 3) free Personal Team Apple Development (any valid)
while IFS= read -r line; do
  case "$line" in
    "Apple Development:"*)
      echo "$line"
      exit 0
      ;;
  esac
done <<<"$VALID"

# 4) ad-hoc fallback
echo "-"
exit 0
