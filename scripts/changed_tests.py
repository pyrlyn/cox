#!/usr/bin/env python3
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""`just test`: run the nextest tests of the workspace crates a change can break (A99, T50.7).

Why a script and not a tool: nextest's `rdeps()` filterset already adds every
crate that depends on a changed one, so all that is left is mapping changed
files to the crates that own them. The maintained tools that do that
(cargo-delta, cargo-rail, cargo-affected) guess file ownership from the module
tree, need coverage builds or bring a config file; owning a file by its crate
directory is simpler and never misses a snapshot or fixture inside a crate.

Usage: `just test [--changed-since REF] [--dry-run] [NEXTEST_ARGS...]`.
REF defaults to the merge-base with `origin/main`; committed, staged,
unstaged and untracked changes all count. `just check-all` runs everything.
Tests: `python3 -m unittest discover -s scripts -p 'test_changed_tests.py'`.
"""

import json
import os
import shlex
import subprocess
import sys

# Files outside every crate that can change how all of them build or test.
WHOLE_WORKSPACE = ("Cargo.toml", "Cargo.lock", "mise.toml", "justfile")
WHOLE_WORKSPACE_PREFIXES = (".cargo/", "rust-toolchain")


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def owner(path, crate_dirs):
    """The package whose directory holds `path`, or None."""
    for rel, name in crate_dirs:
        if path.startswith(rel + "/"):
            return name
    return None


def needles(path):
    """Strings a crate uses to name `path` or a directory above it.

    Tests read some files outside their crate (`docs/config.jsonschema`,
    `fixtures/…`) through a literal such as `"../../docs/x.md"`,
    `join("docs/x.md")` or `root.join("plugins")`, so a crate naming the file
    or a directory above it that way is selected too. A bare `"docs"` is not
    a needle: it matches every unrelated string that happens to say "docs".
    """
    out = [f"/{path}\"", f"\"{path}\""]
    parts = path.split("/")[:-1]
    for i in range(1, len(parts) + 1):
        d = "/".join(parts[:i])
        out += [f"/{d}\"", f"join(\"{d}\")"]
    return out


def select(changed, crate_dirs, grep):
    """`None` for the whole workspace, else the set of package names to seed `rdeps()`."""
    picked, outside = set(), []
    for path in changed:
        if path in WHOLE_WORKSPACE or path.startswith(WHOLE_WORKSPACE_PREFIXES):
            return None
        name = owner(path, crate_dirs)
        if name:
            picked.add(name)
        else:
            outside.append(path)
    if outside:
        hits = grep([n for p in outside for n in needles(p)])
        picked |= {n for n in (owner(h, crate_dirs) for h in hits) if n}
    return picked


def main(argv):
    ref, dry, rest = None, False, []
    it = iter(argv)
    for arg in it:
        if arg == "--changed-since":
            ref = next(it, None)
            if not ref:
                sys.exit("test: --changed-since needs a git ref")
        elif arg.startswith("--changed-since="):
            ref = arg.split("=", 1)[1]
        elif arg == "--dry-run":
            dry = True
        else:
            rest.append(arg)

    root = run("git", "rev-parse", "--show-toplevel").strip()
    os.chdir(root)
    if ref is None:
        ref = run("git", "merge-base", "origin/main", "HEAD").strip()
    changed = sorted(set(
        run("git", "diff", "-z", "--name-only", "--no-renames", ref, "--").split("\0")
        + run("git", "ls-files", "-z", "--others", "--exclude-standard").split("\0")
    ) - {""})

    meta = json.loads(run("mise", "exec", "--", "cargo", "metadata", "--no-deps", "--format-version", "1"))
    crate_dirs = sorted(
        ((os.path.relpath(os.path.dirname(p["manifest_path"]), root), p["name"]) for p in meta["packages"]),
        key=lambda d: -len(d[0]),
    )

    def grep(patterns):
        args = ["git", "grep", "-z", "-l", "-F"] + [a for p in patterns for a in ("-e", p)] + ["--", "crates"]
        res = subprocess.run(args, capture_output=True, text=True)
        if res.returncode > 1:  # 1 is "no match"
            raise subprocess.CalledProcessError(res.returncode, args, res.stdout, res.stderr)
        return [h for h in res.stdout.split("\0") if h]

    picked = select(changed, crate_dirs, grep)
    cmd = ["mise", "exec", "--", "cargo", "nextest", "run", "--workspace"]
    if picked is None:
        print(f"test: a workspace-wide file changed since {ref}; running every crate", file=sys.stderr)
    elif not picked:
        print(f"test: no workspace crate changed since {ref}; nothing to run", file=sys.stderr)
        return 0
    else:
        cmd += ["-E", " | ".join(f"rdeps(={n})" for n in sorted(picked))]
    cmd += rest
    if dry:
        print(shlex.join(cmd))
        return 0
    return subprocess.run(cmd).returncode


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except subprocess.CalledProcessError as e:
        sys.exit(f"test: {shlex.join(e.cmd)} failed: {e.stderr.strip()}")
