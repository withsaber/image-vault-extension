#!/bin/bash
# Creates a local, self-signed code-signing identity for Image Vault, once.
#
# Ad-hoc signatures change on every build, so macOS forgets Screen Recording
# (and other privacy) permissions after each rebuild. Signing every build with
# the same certificate gives the app a stable identity, so permissions stick.
#
# The certificate lives in its own keychain (~/.imagevault-signing), not your
# login keychain, so builds never trigger keychain password prompts. It is only
# meaningful on this Mac; it is not an Apple Developer ID and doesn't notarize.
#
#   ./setup-signing.sh          create it if missing (safe to run again)
#   ./setup-signing.sh --print  print the codesign arguments build.sh uses
set -euo pipefail

NAME="Image Vault Local Signing"
DIR="$HOME/.imagevault-signing"
KEYCHAIN="$DIR/signing.keychain-db"
PASSFILE="$DIR/keychain-password"

unlock() {
  security unlock-keychain -p "$(cat "$PASSFILE")" "$KEYCHAIN" >/dev/null 2>&1
  security set-keychain-settings "$KEYCHAIN" >/dev/null 2>&1   # no auto-lock timeout
}

# codesign only finds identities in keychains on the user search list, so add
# ours after the existing ones (login stays first and the default).
add_to_search_list() {
  local list=() line
  while IFS= read -r line; do
    line="${line#*\"}"; line="${line%\"*}"
    [ "$line" = "$KEYCHAIN" ] && return 0
    [ -n "$line" ] && list+=("$line")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s ${list[@]+"${list[@]}"} "$KEYCHAIN"
}

if [ -f "$KEYCHAIN" ] && [ -f "$PASSFILE" ]; then
  unlock
  add_to_search_list
  if security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1; then
    [ "${1:-}" = "--print" ] && echo "$KEYCHAIN"
    [ "${1:-}" = "--print" ] || echo "Signing identity already set up: $NAME"
    exit 0
  fi
fi

mkdir -p "$DIR"
chmod 700 "$DIR"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Random password for the private keychain (never leaves this folder).
[ -f "$PASSFILE" ] || { /usr/bin/openssl rand -hex 24 > "$PASSFILE"; chmod 600 "$PASSFILE"; }
PASS="$(cat "$PASSFILE")"

cat > "$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -sha256 \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/cert.cnf" >/dev/null 2>&1
/usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -name "$NAME" -out "$WORK/identity.p12" -passout pass:"$PASS" >/dev/null 2>&1

rm -f "$KEYCHAIN"
security create-keychain -p "$PASS" "$KEYCHAIN"
unlock
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign >/dev/null
# Let codesign use the key without asking.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PASS" "$KEYCHAIN" >/dev/null
add_to_search_list

echo "Created signing identity: $NAME"
echo "Keychain: $KEYCHAIN"
[ "${1:-}" = "--print" ] && echo "$KEYCHAIN"
exit 0
