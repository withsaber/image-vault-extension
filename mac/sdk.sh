#!/bin/bash
# Sourced by build.sh and release.sh. Picks an SDK swift build can use.
#
# From the macOS 27 SDK on, SwiftUI's @State is a macro whose plugin
# (libSwiftUIMacros) ships only with full Xcode. With just the Command Line
# Tools, building against that SDK fails with "plugin for module
# 'SwiftUIMacros' not found", so fall back to the newest macOS 26 SDK.
# Set SDKROOT yourself to override.
if [ -z "${SDKROOT:-}" ]; then
  _dev="$(xcode-select -p 2>/dev/null || true)"
  if [ ! -e "$_dev/usr/lib/swift/host/plugins/libSwiftUIMacros.dylib" ] \
     && [ ! -e "$_dev/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/host/plugins/libSwiftUIMacros.dylib" ]; then
    _sdk="$(ls -d "$_dev"/SDKs/MacOSX26*.sdk 2>/dev/null | sort -V | tail -1)"
    if [ -n "$_sdk" ]; then
      export SDKROOT="$_sdk"
      echo "note: no SwiftUIMacros plugin (Command Line Tools only); building with $(basename "$_sdk")" >&2
    fi
  fi
  unset _dev _sdk
fi
