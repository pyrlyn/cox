# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for `changed_tests.select`: which crates `just test` seeds `rdeps()` with (T50.7)."""

import unittest

from changed_tests import needles, select

CRATES = [("crates/cox-sanitize", "cox-sanitize"), ("crates/cox", "cox")]


def no_grep(_patterns):
    return []


class Select(unittest.TestCase):
    def test_a_file_inside_a_crate_selects_that_crate_only(self):
        picked = select(["crates/cox-sanitize/src/lib.rs"], CRATES, no_grep)
        self.assertEqual(picked, {"cox-sanitize"})

    def test_a_crate_whose_name_prefixes_another_is_not_confused(self):
        self.assertEqual(select(["crates/cox/src/main.rs"], CRATES, no_grep), {"cox"})

    def test_a_lockfile_or_toolchain_change_selects_the_whole_workspace(self):
        for path in ["Cargo.lock", "Cargo.toml", ".cargo/config.toml", "mise.toml", "justfile", "rust-toolchain.toml"]:
            self.assertIsNone(select(["crates/cox/src/main.rs", path], CRATES, no_grep), path)

    def test_a_change_outside_every_crate_nobody_names_selects_nothing(self):
        self.assertEqual(select(["plan.md", "desktop/macos/App.swift"], CRATES, no_grep), set())

    def test_a_file_a_crate_names_selects_that_crate(self):
        grep = lambda patterns: ["crates/cox-sanitize/tests/schema.rs"] if '/docs/config.jsonschema"' in patterns else []
        self.assertEqual(select(["docs/config.jsonschema"], CRATES, grep), {"cox-sanitize"})

    def test_needles_cover_the_file_and_every_directory_above_it(self):
        self.assertEqual(
            needles("fixtures/v4a/a.patch"),
            [
                '/fixtures/v4a/a.patch"',
                '"fixtures/v4a/a.patch"',
                '/fixtures"',
                'join("fixtures")',
                '/fixtures/v4a"',
                'join("fixtures/v4a")',
            ],
        )


if __name__ == "__main__":
    unittest.main()
