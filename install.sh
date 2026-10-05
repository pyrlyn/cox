#!/bin/sh
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# install.sh — installs cox from GitHub releases (T12.2).
# Usage: ./install.sh [vX.Y.Z]   (default: latest release)
# Do not pipe this blindly: it downloads the release archive, verifies its
# SHA-256 checksum first, and only then extracts the binary.
set -eu

REPO="pyrlyn/cox"
VERSION="${1:-latest}"
PREFIX="${PREFIX:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "${os}-${arch}" in
  Darwin-arm64) TARGET="aarch64-apple-darwin" ;;
  Linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) echo "error: no cox release for ${os}/${arch}" >&2; exit 1 ;;
esac

if [ "$VERSION" = "latest" ]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | cut -d'"' -f4)"
  [ -n "$TAG" ] || { echo "error: could not resolve latest release" >&2; exit 1; }
else
  TAG="$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
base="https://github.com/${REPO}/releases/download/${TAG}/cox-${TARGET}.tar.xz"
echo "downloading cox ${TAG} (${TARGET})..."
curl -fsSL "$base" -o "$tmp/cox.tar.xz"
curl -fsSL "${base}.sha256" -o "$tmp/cox.tar.xz.sha256"

want="$(cut -d' ' -f1 < "$tmp/cox.tar.xz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum < "$tmp/cox.tar.xz" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
  got="$(shasum -a 256 < "$tmp/cox.tar.xz" | cut -d' ' -f1)"
else
  echo "error: need sha256sum or shasum to verify the download" >&2
  exit 1
fi
[ "$got" = "$want" ] || { echo "error: checksum mismatch, refusing to install" >&2; exit 1; }

mkdir -p "$PREFIX"
tar -xJf "$tmp/cox.tar.xz" -C "$tmp" cox
mv "$tmp/cox" "$PREFIX/cox"
chmod 755 "$PREFIX/cox"
echo "installed cox ${TAG} to ${PREFIX}/cox"
case ":$PATH:" in
  *":${PREFIX}:"*) ;;
  *) echo "note: ${PREFIX} is not on PATH; add it to use cox directly" ;;
esac
