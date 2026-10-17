#!/usr/bin/env python3
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Which Swift packages `swift_test.sh` must test (T63.2).

SwiftPM has no per-test result cache, so the unit that can be skipped soundly is a whole
package whose every input (its sources, tests, reference images, local dependencies, the
files its tests read, the toolchain) is byte-identical to a run that passed. The input
lists live here, not in the shell script, so a test pins the selection rules and a drift
test fails when a package or a local dependency has no list.

`plan` prints one `NAME HASH run|skip` line per package; the shell script tests the `run`
ones and writes `markers/NAME-HASH` for each that passes.
"""

import argparse
import hashlib
import re
import subprocess
import sys
from pathlib import Path

PACKAGES = "desktop/macos/Packages"
FIXTURES = "desktop/macos/Fixtures"

# Besides the package's own directory. Tests locate these through `#filePath`, so a
# Package.swift never names them: Fixtures for every package whose tests replay a fixture,
# the repository root files `CoxCore`'s WelcomeTests summarises, and what builds the
# XCFramework `CoxCore` links.
EXTRA = {
    "CoxModel": [FIXTURES],
    "CoxUI": [],
    "CoxCore": [
        "crates",
        "Cargo.toml",
        "Cargo.lock",
        ".cargo",
        "AGENTS.md",
        "CLAUDE.md",
        "scripts/desktop/xcframework.sh",
    ],
    "CoxPlatform": [FIXTURES],
    "CoxTranscriptText": [FIXTURES],
    "CoxTranscript": [FIXTURES],
}

# Every package depends on these: a lint rule, a pinned tool, this selection logic, the
# job that runs the tests and the toolchain setup it uses.
SHARED = [
    "desktop/macos/.swiftlint.yml",
    "mise.toml",
    "scripts/desktop/swift_test.sh",
    "scripts/desktop/swift_test_select.py",
    ".github/workflows/ci.yml",
    ".github/actions/desktop-macos/action.yml",
]


class UnknownPackage(Exception):
    """A package with no input list: fail closed, never silently skip it."""


def local_dependencies(manifest_text):
    """Package names a manifest pulls in with `.package(path: "../Name")`."""
    return re.findall(r'\.package\(\s*path:\s*"\.\./([^"/]+)"', manifest_text)


def inputs(name, manifests):
    """Tracked paths whose content decides `name`'s test result."""
    if name not in EXTRA:
        raise UnknownPackage(f"no input list for {name}; add one to EXTRA")
    paths = [f"{PACKAGES}/{name}"]
    # Transitive: CoxTranscript reaches CoxModel through CoxTranscriptText as well.
    todo = list(local_dependencies(manifests.get(name, "")))
    seen = {name}
    while todo:
        dep = todo.pop()
        if dep in seen:
            continue
        seen.add(dep)
        paths.append(f"{PACKAGES}/{dep}")
        todo.extend(local_dependencies(manifests.get(dep, "")))
    paths.extend(EXTRA[name])
    return paths + SHARED


def package_hash(name, manifests, ls_files, toolchain):
    """Digest of the git-tracked content of `name`'s inputs plus the toolchain identity.

    `ls_files(paths)` returns `git ls-files -s` lines (mode, blob id, stage, path): the
    blob ids are the content, so a re-recorded snapshot changes the hash.
    """
    listing = "\n".join(sorted(ls_files(inputs(name, manifests))))
    digest = hashlib.sha256()
    digest.update(listing.encode())
    digest.update(b"\0" + toolchain.encode())
    return digest.hexdigest()[:40]


def plan(manifests, ls_files, toolchain, markers, run_all):
    """`[(name, hash, run)]` in package-name order; `markers` is the set of marker names."""
    rows = []
    for name in sorted(manifests):
        digest = package_hash(name, manifests, ls_files, toolchain)
        rows.append((name, digest, run_all or f"{name}-{digest}" not in markers))
    return rows


def _git_ls_files(paths):
    out = subprocess.run(
        ["git", "ls-files", "-s", "--", *paths], check=True, capture_output=True, text=True
    )
    return out.stdout.splitlines()


def _toolchain():
    xcode = subprocess.run(["xcodebuild", "-version"], check=True, capture_output=True, text=True)
    macos = subprocess.run(["sw_vers", "-productVersion"], check=True, capture_output=True, text=True)
    return xcode.stdout + macos.stdout


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--markers", required=True, type=Path)
    parser.add_argument("--all", action="store_true", help="run every package")
    args = parser.parse_args(argv)

    root = Path(subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True
    ).stdout.strip())
    manifests = {
        p.parent.name: p.read_text() for p in sorted((root / PACKAGES).glob("*/Package.swift"))
    }
    markers = {p.name for p in args.markers.iterdir()} if args.markers.is_dir() else set()
    try:
        rows = plan(manifests, _git_ls_files, _toolchain(), markers, args.all)
    except UnknownPackage as error:
        print(f"swift_test_select: {error}", file=sys.stderr)
        return 1
    for name, digest, run in rows:
        print(name, digest, "run" if run else "skip")
    return 0


if __name__ == "__main__":
    sys.exit(main())
