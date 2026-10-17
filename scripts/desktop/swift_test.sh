#!/usr/bin/env bash
# The Swift package tests for CI and `just desktop-test` (T63.2). A package is skipped only
# when COX_SWIFT_TEST_ALL=0 and a pass marker exists for the exact hash of its inputs
# (swift_test_select.py owns the input lists and the hash). Anything else runs every package:
# unset, `1`, a local run. ci.yml sets 0 on pull requests only and restores and saves
# $COX_SWIFT_TEST_MARKERS around this script; a failed package never gets a marker.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

pkgs=desktop/macos/Packages
markers=${COX_SWIFT_TEST_MARKERS:-desktop/macos/build/swift-test-pass}
mkdir -p "$markers"

select_args=(--markers "$markers")
if [ "${COX_SWIFT_TEST_ALL:-1}" != 0 ]; then select_args+=(--all); fi
plan=$(python3 scripts/desktop/swift_test_select.py "${select_args[@]}")

failed=()
while read -r name hash action; do
  marker="$markers/$name-$hash"
  if [ "$action" = skip ]; then
    echo "$name: inputs unchanged since a passing run ($hash), skipped"
    touch "$marker" # still in use: keep it past the pruning below
    continue
  fi
  # swiftbuild emits ColorResource symbols for Colors.xcassets. The native build system
  # copies the catalog and leaves those symbols missing, so `Color(.surfaceWindow)` does
  # not type-check (SwiftPM #9655).
  if swift test --no-parallel --build-system swiftbuild --package-path "$pkgs/$name"; then
    touch "$marker"
  else
    failed+=("$name")
  fi
done <<<"$plan"

# A hash older than two weeks will not match again soon; keeps the cache small.
find "$markers" -type f -mtime +14 -delete
if [ ${#failed[@]} -gt 0 ]; then
  echo "swift test failed in: ${failed[*]}" >&2
  exit 1
fi
