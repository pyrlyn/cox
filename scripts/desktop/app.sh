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

[ -d "$build/CoxFFI.xcframework" ] \
  || { echo "no $build/CoxFFI.xcframework: run \`just desktop-xcframework\` first" >&2; exit 1; }

xcodegen generate --quiet --spec "$macos/project.yml"

# The packages' SwiftLintPlugins build-tool plugin runs unattended: without the flag xcodebuild
# stops to ask whether the plugin may run. The system git fetches the remote packages as
# `swift build` does; Xcode's built-in one can sit waiting for account credentials.
xcodebuild build -quiet \
  -project "$macos/Cox.xcodeproj" -scheme Cox -configuration Debug \
  -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$build/DerivedData" \
  -skipPackagePluginValidation -scmProvider system \
  CODE_SIGN_IDENTITY=- "$@"

rm -rf "$build/Cox.app"
ditto "$build/DerivedData/Build/Products/Debug/Cox.app" "$build/Cox.app"
echo "$build/Cox.app"
