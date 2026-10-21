#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Writes the Sparkle appcast for one release DMG and checks it (T37.32.2, DT§7): Sparkle's own
# generate_appcast, signed with the EdDSA private key in SPARKLE_ED_KEY (as `generate_keys -x`
# exports it), writes desktop/macos/build/appcast.xml with one item whose enclosure is the DMG on
# the release's download page. Then the check: one item, the enclosure URL and short version
# match the tag, the DMG's EdDSA signature and the feed's own signature (the app requires a
# signed feed, SURequireSignedFeed) both verify with sign_update.
#
#   SPARKLE_ED_KEY=<key> bash scripts/desktop/appcast.sh desktop/macos/build/Cox-0.1.3-arm64.dmg v0.1.3
#
# The release uploads appcast.xml beside the DMG, and the app's SUFeedURL is
# /releases/latest/download/appcast.xml, so the feed is always the latest normal release's and
# a test release (never latest) is never offered. Sparkle's tools come from the Sparkle package
# app.sh resolved (DerivedData's SourcePackages), so they are the version the app links;
# SPARKLE_BIN overrides the directory. The key only ever goes to the tools on standard input.
set -euo pipefail

dmg="${1:?usage: appcast.sh <Cox-<version>-arm64.dmg> <tag>}"
tag="${2:?usage: appcast.sh <Cox-<version>-arm64.dmg> <tag>}"
[ -f "$dmg" ] || { echo "no $dmg" >&2; exit 1; }
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a release tag: $tag" >&2; exit 2; }
[ -n "${SPARKLE_ED_KEY:-}" ] || { echo "SPARKLE_ED_KEY (the EdDSA private key) is not set" >&2; exit 1; }

root="$(cd "$(dirname "$0")/../.." && pwd)"
build="$root/desktop/macos/build"
bin="${SPARKLE_BIN:-$build/DerivedData/SourcePackages/artifacts/sparkle/Sparkle/bin}"
for tool in generate_appcast sign_update; do
  [ -x "$bin/$tool" ] || { echo "no $bin/$tool: run app.sh first, or set SPARKLE_BIN" >&2; exit 1; }
done

repo=https://github.com/pyrlyn/cox
prefix="$repo/releases/download/$tag/"
out="${APPCAST_OUT:-$build/appcast.xml}"

# generate_appcast reads every archive in its directory, so it gets one holding this DMG alone.
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp "$dmg" "$stage/"
rm -f "$out"
printf '%s' "$SPARKLE_ED_KEY" | "$bin/generate_appcast" --ed-key-file - \
  --download-url-prefix "$prefix" --full-release-notes-url "$repo/releases/tag/$tag" \
  --link "$repo" --maximum-deltas 0 -o "$out" "$stage"

xpath() { xmllint --xpath "$1" "$out"; }
fail() { echo "$out: $1" >&2; exit 1; }
xmllint --noout "$out"
[ "$(xpath 'count(//item)')" = 1 ] || fail "expected one item"
url="$(xpath 'string(//item/enclosure/@url)')"
[ "$url" = "$prefix$(basename "$dmg")" ] || fail "enclosure URL is $url, not $prefix$(basename "$dmg")"
short="$(xpath "string(//item/*[local-name()='shortVersionString'])")"
[ "$short" = "${tag#v}" ] || fail "short version is $short, not ${tag#v}"
signature="$(xpath "string(//item/enclosure/@*[local-name()='edSignature'])")"
[ -n "$signature" ] || fail "the enclosure has no EdDSA signature"
printf '%s' "$SPARKLE_ED_KEY" | "$bin/sign_update" --verify --ed-key-file - "$dmg" "$signature" \
  || fail "the DMG's EdDSA signature does not verify"
printf '%s' "$SPARKLE_ED_KEY" | "$bin/sign_update" --verify --ed-key-file - "$out" \
  || fail "the feed's signature does not verify"

echo "$out"
