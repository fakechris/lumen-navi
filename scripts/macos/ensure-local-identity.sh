#!/usr/bin/env bash
# Ensure a *stable* local code-signing identity for day-to-day testing.
#
# Preferred order of identities (picked by sign-app.sh / resolve-identity.sh):
#   1. $LUMEN_CODESIGN_IDENTITY
#   2. "Lumen Local Codesign" (self-signed, shared with other Lumen apps)
#   3. Valid "Apple Development: …" (free Personal Team — renew yearly in Xcode)
#   4. ad-hoc `-` (always works; TCC Screen/AX grants often break every rebuild)
#
# Why not only ad-hoc?
#   Ad-hoc has no certificate. Every re-sign → new cdhash → macOS TCC treats the
#   app as a different binary → Screen Recording / Accessibility must be re-granted.
#
# No paid Apple Developer Program / Team ID required for options 2–4.

set -euo pipefail

CERT_NAME="${LUMEN_CODESIGN_IDENTITY:-Lumen Local Codesign}"
KEYCHAIN="${LUMEN_CODESIGN_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
P12_PASS="${LUMEN_CODESIGN_P12_PASS:-lumen-local-dev}"

have_valid_identity() {
  local name="$1"
  security find-identity -v -p codesigning 2>/dev/null \
    | grep -F "\"${name}\"" >/dev/null 2>&1
}

have_any_identity_named() {
  # Includes NOT_TRUSTED / expired entries
  security find-identity -p codesigning 2>/dev/null \
    | grep -F "\"${1}\"" >/dev/null 2>&1
}

if have_valid_identity "$CERT_NAME"; then
  echo "OK: valid codesigning identity: ${CERT_NAME}"
  security find-identity -v -p codesigning 2>/dev/null | grep -F "\"${CERT_NAME}\"" || true
  exit 0
fi

# If an untrusted copy already exists, don't re-import forever — guide the user.
if have_any_identity_named "$CERT_NAME"; then
  cat <<EOF
FOUND (but not trusted yet): "${CERT_NAME}"

One-time trust (GUI — macOS blocks silent trust for code signing):

  1. Open "Keychain Access"
  2. login → Certificates → double-click "${CERT_NAME}"
  3. expand Trust → "Code Signing" = Always Trust
  4. close window; enter keychain password if asked
  5. re-run:  security find-identity -v -p codesigning | grep Lumen

Or create a clean Code Signing cert from the menu (often more reliable):

  Keychain Access → Certificate Assistant → Create a Certificate…
    Name: ${CERT_NAME}
    Identity Type: Self Signed Root
    Certificate Type: Code Signing
    ✔ Let me override defaults → Validity Period: 3650 days

EOF
  exit 2
fi

echo "Creating self-signed codesigning identity: ${CERT_NAME}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/lumen-codesign.XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

OPENSSL_BIN="${OPENSSL_BIN:-}"
if [[ -z "$OPENSSL_BIN" ]]; then
  if [[ -x /opt/homebrew/bin/openssl ]]; then
    OPENSSL_BIN=/opt/homebrew/bin/openssl
  else
    OPENSSL_BIN="$(command -v openssl)"
  fi
fi

cat >"$WORKDIR/openssl.cnf" <<EOF
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_codesign
prompt = no

[req_distinguished_name]
CN = ${CERT_NAME}
O = Lumen Local Dev
C = US

[v3_codesign]
basicConstraints = CA:TRUE
keyUsage = critical, digitalSignature, keyCertSign
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF

"$OPENSSL_BIN" genrsa -out "$WORKDIR/key.pem" 2048 2>/dev/null
"$OPENSSL_BIN" req -new -x509 -days 3650 \
  -key "$WORKDIR/key.pem" \
  -out "$WORKDIR/cert.pem" \
  -config "$WORKDIR/openssl.cnf" \
  -extensions v3_codesign

# OpenSSL 3 default PKCS#12 is rejected by macOS (MAC verification failed).
P12_ARGS=(pkcs12 -export
  -out "$WORKDIR/identity.p12"
  -inkey "$WORKDIR/key.pem"
  -in "$WORKDIR/cert.pem"
  -name "$CERT_NAME"
  -passout "pass:${P12_PASS}"
)
if "$OPENSSL_BIN" pkcs12 -export -help 2>&1 | grep -q -- '-legacy'; then
  P12_ARGS=(pkcs12 -export -legacy
    -out "$WORKDIR/identity.p12"
    -inkey "$WORKDIR/key.pem"
    -in "$WORKDIR/cert.pem"
    -name "$CERT_NAME"
    -passout "pass:${P12_PASS}"
  )
else
  P12_ARGS+=( -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg SHA1 )
fi
"$OPENSSL_BIN" "${P12_ARGS[@]}"

security import "$WORKDIR/identity.p12" \
  -k "$KEYCHAIN" \
  -P "$P12_PASS" \
  -T /usr/bin/codesign \
  -T /usr/bin/security \
  -T /usr/bin/productsign

# Best-effort ACL so codesign can use the key without a modal (needs unlocked keychain).
security set-key-partition-list \
  -S "apple-tool:,apple:,codesign:" \
  -s -k "" \
  "$KEYCHAIN" >/dev/null 2>&1 || true

# Best-effort auto-trust (often requires interactive auth; ignore failure).
security add-trusted-cert -d -r trustRoot -p codeSign \
  -k "$KEYCHAIN" "$WORKDIR/cert.pem" >/dev/null 2>&1 || true

if have_valid_identity "$CERT_NAME"; then
  echo "OK: created and trusted: ${CERT_NAME}"
  security find-identity -v -p codesigning 2>/dev/null | grep -F "\"${CERT_NAME}\"" || true
  exit 0
fi

cat <<EOF
Imported "${CERT_NAME}" into login keychain, but it is not yet trusted for code signing.

Finish once in Keychain Access:

  1. Open Keychain Access (Spotlight: Keychain Access)
  2. login → Certificates → double-click "${CERT_NAME}"
  3. Trust → Code Signing = Always Trust
  4. Close; unlock keychain if prompted
  5. Verify:
       security find-identity -v -p codesigning | grep '${CERT_NAME}'

Then rebuild:
  APPLE_SIGNING_IDENTITY="\$(./scripts/macos/resolve-identity.sh)" \\
    npm --prefix apps/desktop run tauri -- build --target aarch64-apple-darwin --bundles dmg
EOF
exit 2
