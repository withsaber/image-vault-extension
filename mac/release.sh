#!/bin/bash
# Builds a release of "Image Vault.app" signed with your Apple Developer ID,
# notarized by Apple and stapled, packaged as a DMG in dist/.
#
# One-time setup:
#   1. Import your "Developer ID Application" certificate (with its private key)
#      into the login keychain. Check: security find-identity -v -p codesigning
#   2. Store notarization credentials in the keychain (prompts for the
#      app-specific password from appleid.apple.com):
#        xcrun notarytool store-credentials image-vault-notary \
#          --apple-id you@example.com --team-id ABCDE12345
#
#   ./release.sh               → dist/Image Vault <version>.dmg
#   ./release.sh --no-notarize → signed only, for a quick local check
#   ./release.sh --unsigned    → ad-hoc signed, no notarization (no Developer ID
#                                needed; users approve it in Privacy & Security)
#
# Environment overrides:
#   SIGN_IDENTITY   codesign identity (default: the only "Developer ID Application" one)
#   NOTARY_PROFILE  notarytool keychain profile (default: image-vault-notary)
set -euo pipefail
cd "$(dirname "$0")"

NOTARIZE=1
UNSIGNED=0
[ "${1:-}" = "--no-notarize" ] && NOTARIZE=0
if [ "${1:-}" = "--unsigned" ]; then UNSIGNED=1; NOTARIZE=0; SIGN_IDENTITY="-"; fi
NOTARY_PROFILE="${NOTARY_PROFILE:-image-vault-notary}"

if [ -z "${SIGN_IDENTITY:-}" ]; then
  ids="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p' | sort -u)"
  count="$(printf '%s' "$ids" | grep -c . || true)"
  if [ "$count" -ne 1 ]; then
    echo "error: found $count \"Developer ID Application\" identities; import yours or set SIGN_IDENTITY" >&2
    [ -n "$ids" ] && printf '  %s\n' "$ids" >&2
    exit 1
  fi
  SIGN_IDENTITY="$ids"
fi
echo "Signing as: $SIGN_IDENTITY"

PLIST=Resources/Info.plist
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST")"

# Hardened runtime and a secure timestamp are required for notarization.
# Ad-hoc builds skip both: with no Team ID, the hardened runtime's library
# validation would stop the bundled tesseract from loading its own dylibs.
if [ "$UNSIGNED" = 1 ]; then
  sign() { codesign --force --sign - "$@"; }
else
  sign() { codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$@"; }
fi

notarize() {
  [ "$NOTARIZE" = 1 ] || return 0
  echo "Notarizing $(basename "$1") (usually a few minutes)…"
  xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait
}

source ./sdk.sh
SCRATCH="${TMPDIR:-/tmp/}image-vault-mac-release"
swift build -c release --scratch-path "$SCRATCH"
BIN="$SCRATCH/release/ImageVault"

DIST="dist"
STAGE="$DIST/stage"
APP="$STAGE/Image Vault.app"
DMG="$DIST/Image Vault $VERSION.dmg"
rm -rf "$STAGE" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ImageVault"
cp "$PLIST" "$APP/Contents/Info.plist"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp -R Vendor/tesseract "$APP/Contents/Resources/tesseract"
chmod +x "$APP/Contents/Resources/tesseract/bin/tesseract"  # git may drop the bit
clang -dynamiclib -O2 -arch arm64 -arch x86_64 -mmacosx-version-min=13.0 \
  -framework CoreFoundation -o "$APP/Contents/Resources/libivmr.dylib" Helpers/ivmr.c

# Sign nested code first; --deep doesn't reach binaries inside Resources.
for lib in "$APP"/Contents/Resources/tesseract/lib/*.dylib; do sign "$lib"; done
sign "$APP/Contents/Resources/tesseract/bin/tesseract"
sign "$APP/Contents/Resources/libivmr.dylib"
sign "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

# Notarize the app and staple it, so the copy people drag out of the DMG
# carries its own ticket and opens offline.
if [ "$NOTARIZE" = 1 ]; then
  ZIP="$DIST/ImageVault-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  notarize "$ZIP"
  rm -f "$ZIP"
  xcrun stapler staple "$APP"
fi

ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Image Vault" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
[ "$UNSIGNED" = 1 ] || codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi
rm -rf "$STAGE"

echo "Release: $DMG  ($BUNDLE_ID $VERSION, arm64, macOS 13+)"
