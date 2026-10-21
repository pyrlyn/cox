#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Builds desktop/macos/build/Cox.app (T37.32.1, T37.32.2, DT§7, A106): XcodeGen writes the thin
# Cox.xcodeproj from desktop/macos/project.yml, then xcodebuild builds its one app target, ad-hoc
# signed (`-`): no signing identity, no team, no Keychain. Debug unless COX_CONFIGURATION is
# Release, the configuration with the Hardened Runtime. Needs
# desktop/macos/build/CoxFFI.xcframework first (`just desktop-xcframework`): CoxCore's binary
# target is resolved with the packages, before any build phase could make it.
#
#   just desktop-app
#   COX_CONFIGURATION=Release COX_CLI=<cox binary> mise exec -- bash scripts/desktop/app.sh [xcodebuild setting …]
#
# With COX_CLI set, that cox binary goes in as Contents/Helpers/cox (DT§7), so app and CLI never
# drift in version or database schema, and sign.sh re-signs the bundle around it. It is copied
# in after xcodebuild rather than by a build phase so the Xcode build never compiles Rust; the
# release build passes the binary it already built for the tarball.
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
  Debug | Release) ;;
  *) echo "COX_CONFIGURATION must be Debug or Release, not $configuration" >&2; exit 2 ;;
esac
# The app's version is the workspace's, as the CLI's is, so a release names both alike: the DMG,
# the cask and the appcast all take it from the app. A caller's MARKETING_VERSION= still wins.
version="$(sed -nE '/^\[workspace\.package\]/,/^\[/s/^version = "([^"]+)"/\1/p' "$root/Cargo.toml")"
[ -n "$version" ] || { echo "no [workspace.package] version in $root/Cargo.toml" >&2; exit 1; }
if [ -n "${COX_CLI:-}" ]; then
  [ -f "$COX_CLI" ] && [ -x "$COX_CLI" ] || { echo "COX_CLI=$COX_CLI is not an executable" >&2; exit 1; }
  lipo -archs "$COX_CLI" | grep -qw arm64 || { echo "$COX_CLI has no arm64 slice" >&2; exit 1; }
fi

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
  CODE_SIGN_IDENTITY=- MARKETING_VERSION="$version" "$@"

rm -rf "$build/Cox.app"
ditto "$build/DerivedData/Build/Products/$configuration/Cox.app" "$build/Cox.app"
if [ -n "${COX_CLI:-}" ]; then
  mkdir -p "$build/Cox.app/Contents/Helpers"
  install -m 755 "$COX_CLI" "$build/Cox.app/Contents/Helpers/cox"
  bash "$root/scripts/desktop/sign.sh" "$build/Cox.app"
fi
echo "$build/Cox.app"
