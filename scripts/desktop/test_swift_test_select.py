# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for `swift_test_select`: which Swift packages a change makes `swift_test.sh` run."""

import unittest
from pathlib import Path

import swift_test_select as sel

ROOT = Path(__file__).resolve().parents[2]

# The real dependency graph, as manifests: the drift test below keeps it honest.
MANIFESTS = {
    "CoxModel": "",
    "CoxUI": "",
    "CoxCore": '.package(path: "../CoxModel"),',
    "CoxPlatform": '.package(path: "../CoxModel"),',
    "CoxTranscriptText": '.package(path: "../CoxModel"),',
    "CoxTranscript": (
        '.package(path: "../CoxModel"),\n.package(path: "../CoxTranscriptText"),\n'
        '.package(path: "../CoxUI"),'
    ),
}

TREE = {
    "desktop/macos/Packages/CoxModel/Sources/A.swift": "1",
    "desktop/macos/Packages/CoxUI/Sources/B.swift": "1",
    "desktop/macos/Packages/CoxUI/Tests/__Snapshots__/x.png": "1",
    "desktop/macos/Packages/CoxCore/Sources/C.swift": "1",
    "desktop/macos/Packages/CoxPlatform/Sources/D.swift": "1",
    "desktop/macos/Packages/CoxTranscriptText/Sources/E.swift": "1",
    "desktop/macos/Packages/CoxTranscript/Sources/F.swift": "1",
    "desktop/macos/Fixtures/edit.json": "1",
    "desktop/macos/.swiftlint.yml": "1",
    "crates/cox-tools/src/git.rs": "1",
    "Cargo.lock": "1",
    "AGENTS.md": "1",
    "docs/readme.md": "1",
    "mise.toml": "1",
    "scripts/desktop/swift_test.sh": "1",
    ".github/workflows/ci.yml": "1",
}


def run_plan(tree, markers=(), toolchain="Xcode 27", run_all=False):
    def ls_files(paths):
        return [
            f"100644 {blob} 0\t{path}"
            for path, blob in tree.items()
            if any(path == p or path.startswith(p + "/") for p in paths)
        ]

    return sel.plan(MANIFESTS, ls_files, toolchain, set(markers), run_all)


def passed_markers(tree, **kwargs):
    return {f"{name}-{digest}" for name, digest, _ in run_plan(tree, **kwargs)}


def selected(tree, markers):
    return {name for name, _, run in run_plan(tree, markers) if run}


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.green = passed_markers(TREE)

    def test_no_markers_runs_every_package(self):
        self.assertEqual(selected(TREE, set()), set(MANIFESTS))

    def test_unchanged_inputs_skip_every_package(self):
        self.assertEqual(selected(TREE, self.green), set())

    def test_a_rust_only_change_runs_only_the_package_linking_cox_ffi(self):
        changed = {**TREE, "crates/cox-tools/src/git.rs": "2"}
        self.assertEqual(selected(changed, self.green), {"CoxCore"})

    def test_a_cargo_lock_change_runs_only_cox_core(self):
        self.assertEqual(selected({**TREE, "Cargo.lock": "2"}, self.green), {"CoxCore"})

    def test_a_cox_ui_only_change_runs_cox_ui_and_cox_transcript(self):
        changed = {**TREE, "desktop/macos/Packages/CoxUI/Sources/B.swift": "2"}
        self.assertEqual(selected(changed, self.green), {"CoxUI", "CoxTranscript"})

    def test_a_re_recorded_snapshot_reruns_its_package_and_dependents(self):
        changed = {**TREE, "desktop/macos/Packages/CoxUI/Tests/__Snapshots__/x.png": "2"}
        self.assertEqual(selected(changed, self.green), {"CoxUI", "CoxTranscript"})

    def test_a_cox_model_change_runs_it_and_every_dependent_but_not_cox_ui(self):
        changed = {**TREE, "desktop/macos/Packages/CoxModel/Sources/A.swift": "2"}
        self.assertEqual(
            selected(changed, self.green),
            {"CoxModel", "CoxCore", "CoxPlatform", "CoxTranscriptText", "CoxTranscript"},
        )

    def test_a_cox_transcript_text_change_reaches_only_itself_and_cox_transcript(self):
        changed = {**TREE, "desktop/macos/Packages/CoxTranscriptText/Sources/E.swift": "2"}
        self.assertEqual(selected(changed, self.green), {"CoxTranscriptText", "CoxTranscript"})

    def test_a_fixture_change_runs_the_packages_whose_tests_replay_it(self):
        changed = {**TREE, "desktop/macos/Fixtures/edit.json": "2"}
        self.assertEqual(
            selected(changed, self.green),
            {"CoxModel", "CoxPlatform", "CoxTranscriptText", "CoxTranscript"},
        )

    def test_a_shared_file_change_runs_every_package(self):
        for path in sel.SHARED:
            if path in TREE:
                self.assertEqual(selected({**TREE, path: "2"}, self.green), set(MANIFESTS), path)

    def test_a_new_toolchain_runs_every_package(self):
        markers = passed_markers(TREE, toolchain="Xcode 27.0")
        self.assertEqual(
            {n for n, _, run in run_plan(TREE, markers, toolchain="Xcode 27.1") if run},
            set(MANIFESTS),
        )

    def test_a_change_no_package_reads_runs_nothing(self):
        self.assertEqual(selected({**TREE, "docs/readme.md": "2"}, self.green), set())

    def test_the_repository_root_agents_file_reaches_only_cox_core(self):
        self.assertEqual(selected({**TREE, "AGENTS.md": "2"}, self.green), {"CoxCore"})

    def test_run_all_ignores_markers(self):
        rows = run_plan(TREE, self.green, run_all=True)
        self.assertEqual({n for n, _, run in rows if run}, set(MANIFESTS))

    def test_a_package_without_an_input_list_fails_closed(self):
        with self.assertRaises(sel.UnknownPackage):
            sel.inputs("CoxNew", {"CoxNew": ""})


class DriftTests(unittest.TestCase):
    """The lists must follow the real packages, or a skip could hide a broken one."""

    @classmethod
    def setUpClass(cls):
        cls.manifests = {
            p.parent.name: p.read_text()
            for p in (ROOT / sel.PACKAGES).glob("*/Package.swift")
        }

    def test_every_package_on_disk_has_an_input_list(self):
        self.assertEqual(set(self.manifests), set(sel.EXTRA))

    def test_the_test_graph_matches_the_real_manifests(self):
        for name, text in self.manifests.items():
            self.assertEqual(
                sorted(sel.local_dependencies(text)),
                sorted(sel.local_dependencies(MANIFESTS[name])),
                name,
            )

    def test_every_listed_extra_path_exists(self):
        for name, extra in sel.EXTRA.items():
            for path in extra + sel.SHARED:
                self.assertTrue((ROOT / path).exists(), f"{name}: {path}")

    def test_a_package_reading_fixtures_hashes_them(self):
        for name in self.manifests:
            tests = ROOT / sel.PACKAGES / name / "Tests"
            reads = any(
                '"Fixtures' in p.read_text() or '.appending(path: "Fixtures")' in p.read_text()
                for p in tests.rglob("*.swift")
            )
            if reads:
                self.assertIn(sel.FIXTURES, sel.EXTRA[name], name)


if __name__ == "__main__":
    unittest.main()
