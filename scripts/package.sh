#!/bin/bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Build one target and produce the release tarball for it.
#
# Used by both the release workflow and CI, so packaging breaks on a pull
# request rather than at publish time, when the release is already half out.
#
#   scripts/package.sh <rust-target> <output-dir>
#
# The asset name is `cox-<target>.tar.xz`, which is what install.sh and
# `cox self update` both look for. Renaming it breaks upgrades for everyone
# already installed.
#
# With COX_SIGN_IDENTITY set, the binary is code-signed with that identity
# before it is packed. Unset, it is packed as built — which is what CI does on
# a pull request, where no certificate is available.

set -euo pipefail

TARGET="${1:?usage: package.sh <rust-target> <output-dir>}"
OUT_DIR="${2:?usage: package.sh <rust-target> <output-dir>}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> building $TARGET"
rustup target add "$TARGET" >/dev/null
cargo build --profile dist --locked --target "$TARGET"

# Cargo can be configured to share a target directory outside the checkout.
# Ask Cargo for the resolved path instead of assuming the default `target/`.
TARGET_DIR="$(cargo metadata --no-deps --format-version=1 | sed -nE 's/.*"target_directory":"([^"]+)".*/\1/p')"
if [ -z "$TARGET_DIR" ]; then
  echo "could not determine Cargo's target directory" >&2
  exit 1
fi
if [ -f "$TARGET_DIR/$TARGET/dist/cox.exe" ]; then
  BINARY="$TARGET_DIR/$TARGET/dist/cox.exe"
  PACKED_NAME="cox.exe"
elif [ -f "$TARGET_DIR/$TARGET/dist/cox" ]; then
  BINARY="$TARGET_DIR/$TARGET/dist/cox"
  PACKED_NAME="cox"
else
  echo "no binary at $TARGET_DIR/$TARGET/dist/cox[.exe]" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp "$BINARY" "$STAGE/$PACKED_NAME"
cp README.md "$STAGE/"
chmod 755 "$STAGE/$PACKED_NAME"

if [ -n "${COX_SIGN_IDENTITY:-}" ]; then
  echo "==> signing as $COX_SIGN_IDENTITY"
  # Hardened runtime and a secure timestamp are what notarisation would demand
  # later; signing that way now means a notarised release changes nothing here.
  codesign --force --options runtime --timestamp \
    --sign "$COX_SIGN_IDENTITY" "$STAGE/$PACKED_NAME"
  codesign --verify --strict --verbose=2 "$STAGE/$PACKED_NAME"
fi

TARBALL="$OUT_DIR/cox-$TARGET.tar.xz"
tar -cJf "$TARBALL" -C "$STAGE" "$PACKED_NAME" README.md

echo "==> $TARBALL"
if command -v shasum >/dev/null 2>&1; then
  shasum -a 256 "$TARBALL"
elif command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$TARBALL"
fi