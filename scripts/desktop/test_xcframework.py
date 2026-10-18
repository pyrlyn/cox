# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for the skip stamp in `xcframework.sh` (T61.3).

The real script runs against a scratch copy of the repo layout with fake `cargo` and
`xcodebuild` on PATH, so no Rust is compiled: each fake appends its name to a log, and the
tests read the log to see whether the generator and the packaging ran.
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "xcframework.sh"

FAKE_CARGO = """#!/bin/bash
echo "cargo $1" >> "$FAKE_LOG"
case "$1" in
  metadata) printf '{"target_directory":"%s"}' "$FAKE_TARGET" ;;
  build) mkdir -p "$FAKE_TARGET/aarch64-apple-darwin/dist"
         printf '%s' "$FAKE_LIB_BYTES" > "$FAKE_TARGET/aarch64-apple-darwin/dist/libcox_ffi.a" ;;
  run) out=""
       while [ $# -gt 0 ]; do [ "$1" = --out-dir ] && out="$2"; shift; done
       mkdir -p "$out"
       for f in cox_ffiFFI.h cox_ffiFFI.modulemap cox_ffi.swift; do echo gen > "$out/$f"; done ;;
esac
"""

FAKE_XCODEBUILD = """#!/bin/bash
echo "xcodebuild" >> "$FAKE_LOG"
[ -z "${FAKE_XCODEBUILD_FAIL:-}" ] || exit 1
while [ $# -gt 0 ]; do [ "$1" = -output ] && out="$2"; shift; done
mkdir -p "$out"
"""


class Stamp(unittest.TestCase):
    def setUp(self):
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        (tmp / "scripts/desktop").mkdir(parents=True)
        shutil.copy(SCRIPT, tmp / "scripts/desktop/xcframework.sh")
        bin_dir = tmp / "bin"
        bin_dir.mkdir()
        for name, body in (("cargo", FAKE_CARGO), ("xcodebuild", FAKE_XCODEBUILD)):
            (bin_dir / name).write_text(body)
            (bin_dir / name).chmod(0o755)
        self.root, self.log = tmp, tmp / "log"
        self.env = {
            **os.environ,
            "PATH": f"{bin_dir}:{os.environ['PATH']}",
            "FAKE_LOG": str(self.log),
            "FAKE_TARGET": str(tmp / "target"),
            "FAKE_LIB_BYTES": "lib-1",
        }
        self.out = tmp / "desktop/macos/build"
        self.stamp = self.out / "CoxFFI.xcframework/.source-sha256"

    def run_script(self, **env):
        self.log.write_text("")
        done = subprocess.run(
            ["bash", str(self.root / "scripts/desktop/xcframework.sh")],
            env={**self.env, **env}, capture_output=True, text=True,
        )
        calls = self.log.read_text().split("\n")
        return done, "cargo run" in calls, "xcodebuild" in calls

    def test_a_second_run_on_the_same_library_prints_unchanged_and_skips_the_rebuild(self):
        first, generated, packaged = self.run_script()
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertTrue(generated and packaged)
        second, generated, packaged = self.run_script()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertIn("unchanged", second.stdout)
        self.assertFalse(generated or packaged)

    def test_a_changed_library_rebuilds_and_restamps(self):
        self.run_script()
        old = self.stamp.read_text()
        done, generated, packaged = self.run_script(FAKE_LIB_BYTES="lib-2")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(generated and packaged)
        self.assertNotEqual(self.stamp.read_text(), old)

    def test_a_missing_or_corrupt_stamp_rebuilds(self):
        self.run_script()
        self.stamp.write_text("not-a-hash\n")
        _, generated, packaged = self.run_script()
        self.assertTrue(generated and packaged)
        self.stamp.unlink()
        _, generated, packaged = self.run_script()
        self.assertTrue(generated and packaged)

    def test_missing_bindings_rebuild_even_with_a_matching_stamp(self):
        self.run_script()
        (self.out / "bindings/cox_ffi.swift").unlink()
        _, generated, _ = self.run_script()
        self.assertTrue(generated)
        self.assertTrue((self.out / "bindings/cox_ffi.swift").exists())

    def test_a_failed_packaging_leaves_no_stamp_so_the_next_run_retries(self):
        self.run_script()
        failed, _, _ = self.run_script(FAKE_LIB_BYTES="lib-2", FAKE_XCODEBUILD_FAIL="1")
        self.assertNotEqual(failed.returncode, 0)
        self.assertFalse(self.stamp.exists())
        _, generated, packaged = self.run_script(FAKE_LIB_BYTES="lib-2")
        self.assertTrue(generated and packaged)


if __name__ == "__main__":
    unittest.main()
