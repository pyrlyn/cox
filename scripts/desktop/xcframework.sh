#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Builds desktop/macos/build/CoxFFI.xcframework (T37.15, DT§7): cox-ffi as a
# static library for aarch64-apple-darwin only (A67: no Intel slice, no
# universal binary) plus the UniFFI Swift bindings the CoxCore package wraps.
#
#   just desktop-xcframework
#   mise exec -- bash scripts/desktop/xcframework.sh
#
# Plain `cargo` here, so the caller picks the toolchain: the recipe through
# `mise exec`, CI through .github/actions/rust (the same mise.toml pin).
#
# Output, all gitignored under desktop/macos/build/:
#   CoxFFI.xcframework/   the library, its C header and module.modulemap
#   bindings/cox_ffi.swift the generated Swift over that module
#
# The deployment target is set explicitly: without it the library inherits
# the build machine's macOS version and the app's link warns or fails on an
# older one (research.md 9.3.7). The dist profile is what ships (A15).
set -euo pipefail

TARGET=aarch64-apple-darwin
PROFILE=dist
export MACOSX_DEPLOYMENT_TARGET=26.0

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
out="$root/desktop/macos/build"

# Ask cargo rather than assuming ./target: a shared build.target-dir in
# ~/.cargo/config.toml moves it (same lookup as `just release`).
target_dir="$(cargo metadata --format-version 1 --no-deps | tr ',' '\n' \
  | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)"
lib="$target_dir/$TARGET/$PROFILE/libcox_ffi.a"

# The library the app links: no `bindgen` feature, so clap and the generator
# stay out of it.
cargo build -p cox-ffi --lib --profile "$PROFILE" --target "$TARGET"

# Library mode: the generator reads the exported metadata from the built
# archive. Same profile and target, so only uniffi and cox-ffi rebuild.
gen="$(mktemp -d)"
trap 'rm -rf "$gen"' EXIT
cargo run -q -p cox-ffi --features bindgen --bin uniffi-bindgen \
  --profile "$PROFILE" --target "$TARGET" -- \
  generate --library "$lib" --language swift --out-dir "$gen/swift"

# -create-xcframework wants a headers directory holding only the header and
# a `module.modulemap`; it refuses an existing output.
mkdir -p "$gen/headers" "$out/bindings"
cp "$gen/swift/cox_ffiFFI.h" "$gen/headers/"
cp "$gen/swift/cox_ffiFFI.modulemap" "$gen/headers/module.modulemap"
rm -rf "$out/CoxFFI.xcframework"
xcodebuild -create-xcframework \
  -library "$lib" -headers "$gen/headers" \
  -output "$out/CoxFFI.xcframework"
cp "$gen/swift/cox_ffi.swift" "$out/bindings/"

echo "$out/CoxFFI.xcframework"
