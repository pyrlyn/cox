#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Builds desktop/macos/build/Cox.app (T37.32.1, DT§7, A106): XcodeGen writes the thin
# Cox.xcodeproj from desktop/macos/project.yml, then xcodebuild builds its one app target, Debug
# and ad-hoc signed (`-`): no signing identity, no team, no Keychain. Needs
# desktop/macos/build/CoxFFI.xcframework first (`just desktop-xcframework`): CoxCore's binary
# target is resolved with the packages, before any build phase could make it.
#
#   just desktop-app
#   mise exec -- bash scripts/desktop/app.sh [xcodebuild setting …]
#   COX_CONFIGURATION=Release COX_CLI=<path to cox> mise exec -- bash scripts/desktop/app.sh
#
# Release (T37.32.2) is the shipped build: Hardened Runtime, and the `cox` CLI copied to
# Contents/Helpers/cox (DT§7), so the app's Install Command-Line Tool links the very binary
# the app was released with. Release refuses to build without COX_CLI; Debug leaves it out, so
# the dev loop never waits for a second Rust build.
#
# Plain `xcodegen` here, so the caller picks the version: the recipe and CI through `mise exec`
# (the mise.toml pin). Extra arguments go to xcodebuild; CI passes CODE_SIGNING_ALLOWED=NO.
#
# Output, all gitignored: desktop/macos/Cox.xcodeproj, and under desktop/macos/build/ the
# DerivedData and a copy of the app at Cox.app.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
macos="$root/desktop/macos"
build="$macos/build"
configuration="${COX_CONFIGURATION:-Debug}"

case "$configuration" in
  Debug) ;;
  Release)
    [ -x "${COX_CLI:-}" ] \
      || { echo "a Release build bundles the CLI: set COX_CLI to a built cox" >&2; exit 1; } ;;
  *) echo "COX_CONFIGURATION is Debug or Release, not $configuration" >&2; exit 1 ;;
esac

[ -d "$build/CoxFFI.xcframework" ] \
  || { echo "no $build/CoxFFI.xcframework: run \`just desktop-xcframework\` first" >&2; exit 1; }

xcodegen generate --quiet --spec "$macos/project.yml"

# The packages' SwiftLintPlugins build-tool plugin runs unattended: without the flag xcodebuild
# stops to ask whether the plugin may run. The system git fetches the remote packages as
# `swift build` does; Xcode's built-in one can sit waiting for account credentials.
xcodebuild build -quiet \
  -project "$macos/Cox.xcodeproj" -scheme Cox -configuration "$configuration" \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$build/DerivedData" \
  -skipPackagePluginValidation -scmProvider system \
  CODE_SIGN_IDENTITY=- "$@"

rm -rf "$build/Cox.app"
ditto "$build/DerivedData/Build/Products/$configuration/Cox.app" "$build/Cox.app"
if [ "$configuration" = Release ]; then
  # After xcodebuild's signature, which this breaks: dmg.sh signs the helper and reseals the app.
  mkdir -p "$build/Cox.app/Contents/Helpers"
  cp "$COX_CLI" "$build/Cox.app/Contents/Helpers/cox"
fi
echo "$build/Cox.app"
