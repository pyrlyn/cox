# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""The sccache wiring is config only, so these tests pin it (T61.7): a dropped variable would
silently turn the compiler cache off and nothing else would fail."""

import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LAUNCHERS = ["RUSTC_WRAPPER", "CMAKE_C_COMPILER_LAUNCHER", "CMAKE_CXX_COMPILER_LAUNCHER"]


def mise():
    return tomllib.loads((ROOT / "mise.toml").read_text())


class MiseEnv(unittest.TestCase):
    def test_sccache_is_pinned_for_linux_and_macos_only(self):
        pin = mise()["tools"]["sccache"]
        self.assertRegex(pin["version"], r"^\d+\.\d+\.\d+$")
        self.assertEqual(sorted(pin["os"]), ["linux", "macos"])

    def test_every_launcher_is_empty_where_sccache_is_not_installed(self):
        env = mise()["env"]
        for name in LAUNCHERS:
            self.assertIn("MISE_SCCACHE_PROBE", env[name], name)
            self.assertIn("os() != 'windows'", env[name], name)

    def test_the_cache_is_bounded(self):
        self.assertRegex(mise()["env"]["SCCACHE_CACHE_SIZE"], r"^\d+[KMG]$")

    def test_the_wrapper_is_never_set_in_cargo_config(self):
        # Not there: a machine without sccache would then fail every build.
        text = (ROOT / ".cargo" / "config.toml").read_text()
        self.assertNotIn("rustc-wrapper", text)


class Workflows(unittest.TestCase):
    def test_the_desktop_action_runs_the_shared_sccache_action_before_cargo(self):
        text = (ROOT / ".github/actions/desktop-macos/action.yml").read_text()
        shared = "uses: pyrlyn/ci/.github/actions/sccache@"
        self.assertIn(shared, text)
        self.assertLess(text.index(shared), text.index("scripts/desktop/xcframework.sh"))

    def test_the_pr_desktop_job_passes_the_backend_read_only_on_pull_requests(self):
        text = (ROOT / ".github/workflows/ci.yml").read_text()
        job = text[text.index("  desktop-macos:") :]
        job = job[: job.index("      - name: swift test")]
        for needle in ["secrets.R2_ACCESS_KEY_ID", "vars.SCCACHE_BUCKET", "github.event_name == 'pull_request'"]:
            self.assertIn(needle, job, needle)


if __name__ == "__main__":
    unittest.main()
