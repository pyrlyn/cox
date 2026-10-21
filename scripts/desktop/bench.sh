#!/usr/bin/env bash
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# DT§1's budget suite (T37.33). In process, CoxTranscript's TranscriptBenchmarkTests: a
# 2 000-block transcript's first frame, scrolling and a 200 tok/s stream. From outside the app,
# desktop/macos/Benchmarks' cox-bench: desktop/macos/build/Cox.app launched on a 2 000-block
# fixture, to its first window, then its idle memory. Every MEASURE line lands in
# desktop/macos/build/bench.log and the summary below; a budget missed fails the run after
# every measurement has printed.
#
#   just desktop-bench [runs]
#   bash scripts/desktop/bench.sh [runs]      # with Cox.app already built (`just desktop-app`)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
macos="$root/desktop/macos"
app="$macos/build/Cox.app"
log="$macos/build/bench.log"
runs="${1:-5}"

[ -d "$app" ] || { echo "no $app: run \`just desktop-app\` first" >&2; exit 1; }
: >"$log"
status=0

# `--no-parallel`: the suites run on the main actor, and interleaved they time each other.
# swiftbuild for the same reason as swift_test.sh (SwiftPM #9655).
swift test --no-parallel --build-system swiftbuild --package-path "$macos/Packages/CoxTranscript" \
  --filter TranscriptBenchmarkTests 2>&1 | tee -a "$log" || status=1

swift test --package-path "$macos/Benchmarks" 2>&1 | tee -a "$log" || status=1
swift run -c release --package-path "$macos/Benchmarks" cox-bench \
  "$app" "$macos/Fixtures/edit.json" "$runs" 2>&1 | tee -a "$log" || status=1

echo
echo "DT§1 budgets on $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.memsize | awk '{print $1 / 2^30}') GB, macOS $(sw_vers -productVersion):"
grep -o 'MEASURE.*' "$log" | sed 's/^MEASURE /  /' || { echo "no MEASURE lines" >&2; status=1; }
exit "$status"
