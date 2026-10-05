#!/usr/bin/env bash
# Packs desktop/macos/build/Cox.app into a DMG someone can download and run (T37.32.3, DT§7):
# signs the app with COX_SIGN_IDENTITY (a Developer ID Application identity; CI imports it with
# .github/actions/macos-signing) or ad-hoc when that is unset, checks the signature, and writes
# desktop/macos/build/Cox-<version>-<build>-debug-arm64.dmg holding the app and an
# /Applications link. Needs `just desktop-app` first; app.sh builds Debug only, hence the tag.
#
#   just desktop-dmg
#   COX_SIGN_IDENTITY=<sha1> SIGNING_KEYCHAIN=<path> bash scripts/desktop/dmg.sh
#
# The app is re-signed here rather than by xcodebuild: Xcode's identity lookup wants a team and
# its own certificate setup, while codesign takes the identity as given. --preserve-metadata
# keeps the identifier, entitlements and flags the build signed in, so the result carries
# exactly what project.yml asks for. Developer ID signing keeps the bundle's identity stable
# across builds, so notification, Keychain and folder permissions survive an update. A Debug
# build has no hardened runtime, so it is not notarized (T37.32.2): Gatekeeper asks once,
# through System Settings -> Privacy & Security -> Open Anyway.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
build="$root/desktop/macos/build"
app="$build/Cox.app"

[ -d "$app" ] || { echo "no $app: run \`just desktop-app\` first" >&2; exit 1; }

identity="${COX_SIGN_IDENTITY:--}"
opts=(--force --sign "$identity")
if [ -n "${SIGNING_KEYCHAIN:-}" ]; then opts+=(--keychain "$SIGNING_KEYCHAIN"); fi
# A timestamp needs Apple's server and means nothing on an ad-hoc signature.
if [ "$identity" = - ]; then opts+=(--timestamp=none); else opts+=(--timestamp); fi

sign() {
  codesign "${opts[@]}" --preserve-metadata=identifier,entitlements,flags,runtime "$1"
}

# Nested code first, deepest first: a bundle's signature seals what is inside it, so the outer
# one is made last. Resource bundles hold no code and are sealed as the app's resources.
while IFS= read -r -d '' item; do
  sign "$item"
done < <(find "$app/Contents" -depth \( -name '*.framework' -o -name '*.dylib' \
  -o -name '*.appex' -o -name '*.xpc' -o -name '*.app' \) -print0)
sign "$app"

codesign --verify --deep --strict --verbose=2 "$app"
if [ "$identity" != - ]; then
  codesign --display --verbose=2 "$app" 2>&1 | grep -q 'Authority=Developer ID Application' \
    || { echo "$app is not signed by a Developer ID Application identity" >&2; exit 1; }
fi

plist="$app/Contents/Info.plist"
version="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$plist")"
number="$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$plist")"
dmg="$build/Cox-$version-$number-debug-arm64.dmg"

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
