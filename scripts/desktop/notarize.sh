#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Notarizes a Release DMG from dmg.sh (T37.32.2, DT§7): `notarytool submit --wait` with the App
# Store Connect API key, Apple's log when it is not Accepted, then the ticket stapled to the DMG
# and Gatekeeper's verdict checked. One submission covers the app inside, so the app itself is
# not stapled: that would change the DMG after notarization, and Gatekeeper finds the app's
# ticket online the first time it opens.
#
#   NOTARY_KEY=<.p8 as base64> NOTARY_KEY_ID=<id> NOTARY_ISSUER=<uuid> \
#     bash scripts/desktop/notarize.sh desktop/macos/build/Cox-<version>-<build>-arm64.dmg
#
# The same three secrets as build-cli.yml's Notarise step. The key goes to a 0600 file in a
# private temporary directory that is removed on exit, and is never echoed.
set -euo pipefail

if [ $# -ne 1 ] || [ ! -f "$1" ]; then
  echo "usage: $0 <dmg>" >&2
  exit 2
fi
dmg="$1"
if [ -z "${NOTARY_KEY:-}" ] || [ -z "${NOTARY_KEY_ID:-}" ] || [ -z "${NOTARY_ISSUER:-}" ]; then
  echo "NOTARY_KEY (the .p8 as base64), NOTARY_KEY_ID and NOTARY_ISSUER must all be set" >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
key="$work/key.p8"
(umask 077 && printf '%s' "$NOTARY_KEY" | base64 --decode > "$key")
auth=(--key "$key" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")

xcrun notarytool submit "$dmg" "${auth[@]}" --wait --output-format json > "$work/result.json" || true
status="$(plutil -extract status raw -o - "$work/result.json" 2>/dev/null || echo unknown)"
if [ "$status" != Accepted ]; then
  echo "notarization of $(basename "$dmg"): $status" >&2
  cat "$work/result.json" >&2
  if id="$(plutil -extract id raw -o - "$work/result.json" 2>/dev/null)"; then
    xcrun notarytool log "$id" "${auth[@]}" >&2 || true
  fi
  exit 1
fi

xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg" 2>&1 \
  | tee /dev/stderr | grep -q 'source=Notarized Developer ID' \
  || { echo "Gatekeeper does not accept $dmg as notarized" >&2; exit 1; }
echo "$dmg"
