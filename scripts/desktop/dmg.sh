#!/usr/bin/env bash
# Packs desktop/macos/build/Cox.app into a DMG someone can download and run (T37.32.3, T37.32.2,
# DT§7): signs the app through sign.sh (COX_SIGN_IDENTITY, or ad-hoc when that is unset), and
# writes desktop/macos/build/Cox-<version>-<build>-debug-arm64.dmg for a Debug build or
# Cox-<version>-arm64.dmg for a Release one (COX_CONFIGURATION, as app.sh took it), holding the
# app and an /Applications link. Needs `just desktop-app` (or `just desktop-release`) first.
#
#   just desktop-dmg
#   COX_SIGN_IDENTITY=<sha1> SIGNING_KEYCHAIN=<path> bash scripts/desktop/dmg.sh
#
# The Release name carries no build number because the Homebrew cask and the appcast name the
# file from the version alone. A Debug build has no Hardened Runtime, so it is not notarized:
# Gatekeeper asks once, through System Settings -> Privacy & Security -> Open Anyway. A Release
# DMG goes on to notarize.sh.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
build="$root/desktop/macos/build"
app="$build/Cox.app"
configuration="${COX_CONFIGURATION:-Debug}"
case "$configuration" in
  Debug | Release) ;;
  *) echo "COX_CONFIGURATION must be Debug or Release, not $configuration" >&2; exit 2 ;;
esac

[ -d "$app" ] || { echo "no $app: run \`just desktop-app\` first" >&2; exit 1; }

bash "$root/scripts/desktop/sign.sh" "$app"

identity="${COX_SIGN_IDENTITY:--}"
opts=(--force --sign "$identity")
if [ -n "${SIGNING_KEYCHAIN:-}" ]; then opts+=(--keychain "$SIGNING_KEYCHAIN"); fi
if [ "$identity" = - ]; then opts+=(--timestamp=none); else opts+=(--timestamp); fi

plist="$app/Contents/Info.plist"
version="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$plist")"
number="$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$plist")"
if [ "$configuration" = Release ]; then
  dmg="$build/Cox-$version-arm64.dmg"
else
  dmg="$build/Cox-$version-$number-debug-arm64.dmg"
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/Cox.app"
ln -s /Applications "$stage/Applications"
rm -f "$build"/Cox-*.dmg
hdiutil create -quiet -volname Cox -srcfolder "$stage" -format UDZO -ov "$dmg"
if [ "$identity" != - ]; then
  codesign "${opts[@]}" "$dmg"
  codesign --verify --strict "$dmg"
fi
hdiutil verify -quiet "$dmg"

echo "$dmg"
