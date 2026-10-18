# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""The desktop-macos action's caches are workflow config, so these tests pin them (T61.4): a
dropped condition would rebuild the XCFramework on every run, a dropped key part would hand a
stale framework to the app, and nothing else would fail."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ACTION = ROOT / ".github" / "actions" / "desktop-macos" / "action.yml"
MISS = "steps.ffi.outputs.cache-hit != 'true'"


def steps():
    # No YAML parser in the stdlib: the action's steps are the blocks that start at a
    # four-space dash under `runs.steps`.
    text = ACTION.read_text().split("  steps:\n", 1)[1]
    return [b for b in re.split(r"(?m)^(?=    - )", text) if b.strip().startswith("- ")]


def step(marker):
    found = [b for b in steps() if marker in b]
    assert len(found) == 1, (marker, len(found))
    return found[0]


class FfiCache(unittest.TestCase):
    def test_the_cache_key_names_os_profile_and_every_rust_input(self):
        block = step("id: ffi")
        self.assertIn("actions/cache@", block)
        for part in ["runner.os", "COX_FFI_PROFILE"]:
            self.assertIn(part, block)
        for path in ["crates/**", "Cargo.toml", "Cargo.lock", "mise.toml", "scripts/desktop/xcframework.sh"]:
            self.assertIn(f"'{path}'", block)
        for path in ["CoxFFI.xcframework", "desktop/macos/build/bindings"]:
            self.assertIn(path, block)

    def test_a_near_miss_never_restores_a_framework(self):
        self.assertNotIn("restore-keys", step("id: ffi"))

    def test_the_rust_steps_run_only_on_a_miss(self):
        for marker in [
            "pyrlyn/ci/.github/actions/setup-rust@",
            "Swatinem/rust-cache@",
            "pyrlyn/ci/.github/actions/sccache@",
            "scripts/desktop/xcframework.sh\n",
        ]:
            self.assertIn(f"if: {MISS}", step(marker), marker)

    def test_a_hit_still_installs_mise_for_the_callers(self):
        block = step("mise without Rust")
        self.assertIn("steps.ffi.outputs.cache-hit == 'true'", block)
        self.assertIn("aqua:yonaskolb/XcodeGen", block)
        self.assertNotIn("rust", block.split("install_args", 1)[1].lower())

    def test_the_arm64_check_always_runs(self):
        self.assertNotIn("if:", step("name: arm64 only"))


class SwiftCache(unittest.TestCase):
    def test_swiftpm_clones_and_package_builds_are_cached_on_the_resolved_pins(self):
        block = step("name: Swift build cache")
        self.assertIn("actions/cache@", block)
        for path in ["~/Library/Caches/org.swift.swiftpm", "desktop/macos/Packages/*/.build"]:
            self.assertIn(path, block)
        self.assertIn("hashFiles('desktop/macos/Packages/*/Package.resolved'", block)
        # The restore key without the hash seeds an incremental build when the pins moved.
        restore = block.split("restore-keys:", 1)[1]
        self.assertTrue(any("hashFiles" not in line for line in restore.strip().splitlines()))


class Pins(unittest.TestCase):
    def test_every_third_party_action_is_pinned_by_full_sha(self):
        # pyrlyn/ci's own actions ride `@main` by design (the first-party CI repository).
        uses = re.findall(r"(?m)^\s*(?:- )?uses: (\S+)", ACTION.read_text())
        self.assertTrue(uses)
        for ref in uses:
            if ref.startswith("pyrlyn/ci/") and ref.endswith("@main"):
                continue
            self.assertRegex(ref, r"@[0-9a-f]{40}$", ref)


if __name__ == "__main__":
    unittest.main()
