#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Notarizes a Release DMG from dmg.sh, staples the ticket to it and has Gatekeeper assess the
# result (T37.32.2, DT§7): `notarytool submit --wait` with an App Store Connect API key, the
# log on a rejection, `stapler staple`, then `spctl --assess` on the DMG and on the app inside,
# both of which must name a notarized Developer ID. The same three secrets as build-cli.yml's
# CLI notarization, in the same form:
#
#   NOTARY_KEY      the .p8 key, base64
#   NOTARY_KEY_ID   its key id
#   NOTARY_ISSUER   the issuer id
#
#   bash scripts/desktop/notarize.sh desktop/macos/build/Cox-<version>-arm64.dmg
#
# The DMG is what is notarized and stapled, not the app first: Gatekeeper checks the app
# against the DMG's ticket when it is opened from it, and Sparkle and the cask both download
# the DMG, so one submission covers every way the app arrives. The key file lives in a private
# temporary directory for the length of the run only.
set -euo pipefail

dmg="${1:?usage: notarize.sh <Cox-…-arm64.dmg>}"
case "$dmg" in *.dmg) ;; *) echo "$dmg is not a DMG" >&2; exit 2 ;; esac
[ -f "$dmg" ] || { echo "no $dmg" >&2; exit 1; }
case "$dmg" in *-debug-*) echo "$dmg is a Debug build, which has no Hardened Runtime" >&2; exit 2 ;; esac
if [ -z "${NOTARY_KEY:-}" ] || [ -z "${NOTARY_KEY_ID:-}" ] || [ -z "${NOTARY_ISSUER:-}" ]; then
  echo "NOTARY_KEY (the .p8 as base64), NOTARY_KEY_ID and NOTARY_ISSUER must all be set" >&2
  exit 1
fi

work="$(mktemp -d)"
mnt="$work/mnt"
cleanup() {
  if [ -d "$mnt" ]; then hdiutil detach -quiet -force "$mnt" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
key="$work/key.p8"
(umask 077 && printf '%s' "$NOTARY_KEY" | base64 --decode > "$key")
auth=(--key "$key" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")

xcrun notarytool submit "$dmg" "${auth[@]}" --wait --output-format json > "$work/result.json" || true
cat "$work/result.json"
status="$(plutil -extract status raw -o - "$work/result.json" 2>/dev/null || true)"
if [ "$status" != Accepted ]; then
  id="$(plutil -extract id raw -o - "$work/result.json" 2>/dev/null || true)"
  if [ -n "$id" ]; then xcrun notarytool log "$id" "${auth[@]}" >&2 || true; fi
  echo "notarization of $dmg ended as '${status:-no status}'" >&2
  exit 1
fi

xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"

notarized() {
  local out
  out="$(spctl "$@" 2>&1)" || { echo "$out" >&2; return 1; }
  echo "$out"
  grep -q 'source=Notarized Developer ID' <<<"$out"
}
notarized --assess --type open --context context:primary-signature --verbose=2 "$dmg" \
  || { echo "Gatekeeper does not accept $dmg as notarized" >&2; exit 1; }
mkdir "$mnt"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mnt" "$dmg"
notarized --assess --type execute --verbose=2 "$mnt/Cox.app" \
  || { echo "Gatekeeper does not accept the Cox.app in $dmg as notarized" >&2; exit 1; }
