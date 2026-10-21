#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Signs a built Cox.app inside-out and checks it (T37.32.2, T37.32.3, DT§7): with
# COX_SIGN_IDENTITY (a Developer ID Application identity; CI imports it with pyrlyn/ci's
# macos-keychain action, which also sets SIGNING_KEYCHAIN) or ad-hoc when that is unset. app.sh
# runs it after putting the CLI in the bundle, dmg.sh before packing. Its own script so the two
# share one signing order.
#
#   bash scripts/desktop/sign.sh desktop/macos/build/Cox.app
#
# The app is re-signed here rather than by xcodebuild: Xcode's identity lookup wants a team and
# its own certificate setup, while codesign takes the identity as given. --preserve-metadata
# keeps the identifier, entitlements and flags the build signed in, so the app and Sparkle's
# parts carry exactly what project.yml and Sparkle ask for, Hardened Runtime included in
# Release. The bundled CLI (Contents/Helpers/cox) was built by cargo, not Xcode, so it has none
# to keep: it always gets the runtime and scripts/cox.entitlements, as the release tarball's
# binary does (package.sh). Developer ID signing keeps the bundle's identity stable across
# builds, so notification, Keychain and folder permissions survive an update.
set -euo pipefail

app="${1:?usage: sign.sh <Cox.app>}"
# find's paths must match "$helper" below character for character.
app="${app%/}"
[ -d "$app/Contents" ] || { echo "$app is not an app bundle" >&2; exit 1; }
root="$(cd "$(dirname "$0")/../.." && pwd)"

identity="${COX_SIGN_IDENTITY:--}"
opts=(--force --sign "$identity")
if [ -n "${SIGNING_KEYCHAIN:-}" ]; then opts+=(--keychain "$SIGNING_KEYCHAIN"); fi
# A timestamp needs Apple's server and means nothing on an ad-hoc signature.
if [ "$identity" = - ]; then opts+=(--timestamp=none); else opts+=(--timestamp); fi

helper="$app/Contents/Helpers/cox"
cli_id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist").cli"

sign() {
  if [ "$1" = "$helper" ]; then
    codesign "${opts[@]}" --options runtime --identifier "$cli_id" \
      --entitlements "$root/scripts/cox.entitlements" "$1"
  else
    codesign "${opts[@]}" --preserve-metadata=identifier,entitlements,flags,runtime "$1"
  fi
}

# Nested code first, deepest first: a bundle's signature seals what is inside it, so the outer
# one is made last. Resource bundles hold no code and are sealed as the app's resources. Two
# bare executables sit outside any nested bundle: the CLI, and Sparkle's Autoupdate, which
# Sparkle's own re-signing steps sign before its framework.
while IFS= read -r -d '' item; do
  sign "$item"
done < <(find "$app/Contents" -depth \( -name '*.framework' -o -name '*.dylib' \
  -o -name '*.appex' -o -name '*.xpc' -o -name '*.app' \
  -o -path '*/Sparkle.framework/Versions/*/Autoupdate' -o -path "$helper" \) -print0)
sign "$app"

codesign --verify --deep --strict --verbose=2 "$app"
# Captured before matching: `codesign | grep -q` fails under pipefail whenever grep exits
# before codesign finishes writing.
if [ "$identity" != - ]; then
  shown="$(codesign --display --verbose=2 "$app" 2>&1)"
  grep -q 'Authority=Developer ID Application' <<<"$shown" \
    || { echo "$app is not signed by a Developer ID Application identity" >&2; exit 1; }
fi
if [ -e "$helper" ]; then
  shown="$(codesign --display --verbose=2 "$helper" 2>&1)"
  grep -q 'flags=.*(.*runtime' <<<"$shown" || { echo "$helper has no Hardened Runtime" >&2; exit 1; }
fi
