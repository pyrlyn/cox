#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Writes the Sparkle appcast for one release DMG (T37.32.2, DT§7): Sparkle's own
# generate_appcast signs the DMG and, because the app's Info.plist sets SURequireSignedFeed, the
# feed too, with the EdDSA private key read from SPARKLE_ED_KEY on stdin. The result is checked
# the way the app checks it: the feed's signature, the DMG's signature from its enclosure, and
# the enclosure URL pointing at the release asset.
#
#   SPARKLE_ED_KEY=<private key> bash scripts/desktop/appcast.sh <dmg> <tag>
#
# The feed holds the newest release only: the app reads it from /releases/latest/download/, so
# each release uploads its own. The tools come from the Sparkle package app.sh resolved
# (SPARKLE_BIN overrides). Output: appcast.xml next to the DMG.
set -euo pipefail

if [ $# -ne 2 ] || [ ! -f "$1" ]; then
  echo "usage: $0 <dmg> <tag>" >&2
  exit 2
fi
dmg="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
tag="$2"
# The tag becomes part of a URL in the feed.
if ! printf '%s' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
  echo "not a release tag: $tag" >&2
  exit 2
fi
if [ -z "${SPARKLE_ED_KEY:-}" ]; then
  echo "SPARKLE_ED_KEY (the EdDSA private key Sparkle's generate_keys -x exports) must be set" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${SPARKLE_BIN:-$root/desktop/macos/build/DerivedData/SourcePackages/artifacts/sparkle/Sparkle/bin}"
[ -x "$bin/generate_appcast" ] || { echo "no Sparkle tools in $bin: build the app first" >&2; exit 1; }
prefix="https://github.com/pyrlyn/cox/releases/download/$tag/"
feed="$(dirname "$dmg")/appcast.xml"

# generate_appcast reads every archive in its directory and moves older ones aside, so it sees
# a copy-on-write clone of this DMG alone.
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -c "$dmg" "$stage/"
printf '%s' "$SPARKLE_ED_KEY" | "$bin/generate_appcast" --ed-key-file - \
  --download-url-prefix "$prefix" --link https://github.com/pyrlyn/cox -o "$feed" "$stage"

printf '%s' "$SPARKLE_ED_KEY" | "$bin/sign_update" --verify --ed-key-file - "$feed"
url="$(xmllint --xpath 'string(//item/enclosure/@url)' "$feed")"
[ "$url" = "$prefix$(basename "$dmg")" ] \
  || { echo "the appcast points at $url, not $prefix$(basename "$dmg")" >&2; exit 1; }
signature="$(xmllint --xpath 'string(//item/enclosure/@*[local-name()="edSignature"])' "$feed")"
printf '%s' "$SPARKLE_ED_KEY" | "$bin/sign_update" --verify --ed-key-file - "$dmg" "$signature"
echo "$feed"
