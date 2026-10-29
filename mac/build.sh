#!/bin/bash
# Builds "Image Vault.app" (release), signs it and installs it.
# Signing uses the local identity from setup-signing.sh (created on first run)
# so macOS keeps Screen Recording permission across rebuilds; ad-hoc otherwise.
#   ./build.sh            → /Applications (or ~/Applications if not writable)
#   ./build.sh --no-open  → build + install only
set -euo pipefail
cd "$(dirname "$0")"

# Stable signing identity (falls back to ad-hoc if it can't be set up).
SIGN_ID="-"
if ./setup-signing.sh >/dev/null 2>&1; then
  SIGN_ID="Image Vault Local Signing"
else
  echo "warning: signing identity unavailable, signing ad-hoc (permissions reset each build)" >&2
fi
sign() { codesign --force --timestamp=none --sign "$SIGN_ID" "$@" >/dev/null 2>&1; }

SCRATCH="${TMPDIR:-/tmp/}image-vault-mac-build"
swift build -c release --scratch-path "$SCRATCH"
BIN="$SCRATCH/release/ImageVault"

DEST="/Applications"
[ -w "$DEST" ] || DEST="$HOME/Applications"
mkdir -p "$DEST"
APP="$DEST/Image Vault.app"

# Quit a running copy so the binary can be replaced.
osascript -e 'tell application id "com.imagevault.mac" to quit' >/dev/null 2>&1 || true
pkill -x ImageVault >/dev/null 2>&1 || true

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ImageVault"
cp Resources/Info.plist "$APP/Contents/Info.plist"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

# Bundled Tesseract (Bangla and other scripts Vision can't read).
cp -R Vendor/tesseract "$APP/Contents/Resources/tesseract"

# Now-playing artwork helper, loaded into /usr/bin/perl (universal like perl).
clang -dynamiclib -O2 -arch arm64 -arch x86_64 -mmacosx-version-min=13.0 \
  -framework CoreFoundation -o "$APP/Contents/Resources/libivmr.dylib" Helpers/ivmr.c

# Sign nested code first; --deep doesn't reach binaries inside Resources.
for lib in "$APP"/Contents/Resources/tesseract/lib/*.dylib; do sign "$lib"; done
sign "$APP/Contents/Resources/tesseract/bin/tesseract"
sign "$APP/Contents/Resources/libivmr.dylib"
sign "$APP" || { echo "error: signing failed" >&2; exit 1; }

echo "Installed: $APP  (signed: $SIGN_ID)"
if [ "${1:-}" != "--no-open" ]; then open "$APP"; fi
