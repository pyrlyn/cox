// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.1: the instruction chain on a fixture tree — order, includes, cycles,
//! symlink dedupe, budget, and byte-stability across runs.

use std::fs;
use std::path::Path;

use cox_ext::instructions::{Roots, load};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Four files: root `AGENTS.md`, root `CLAUDE.md` with an `@docs/style.md`
/// include, `.claude/CLAUDE.md`, and `sub/AGENTS.md`; cwd is `sub`.
fn fixture() -> (tempfile::TempDir, Roots) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root.join("AGENTS.md"), "# Root agents\nUse cargo.\n");
    write(
        &root.join("CLAUDE.md"),
        "Style: see @docs/style.md and mail ops@example.com.\n",
    );
    write(&root.join("docs/style.md"), "Two spaces.\n");
    write(&root.join(".claude/CLAUDE.md"), "Project claude file.\n");
    write(&root.join("sub/AGENTS.md"), "Sub agents.\n");
    let roots = Roots {
        cox_home: None,
        claude_home: None,
        git_root: Some(root.clone()),
        cwd: root.join("sub"),
    };
    (dir, roots)
}

#[test]
fn instructions_fixture_tree_renders_in_documented_order() {
    let (_dir, roots) = fixture();
    let loaded = load(&roots, 8000);
    assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
    assert_eq!(
        loaded.files,
        [
            "AGENTS.md",
            "CLAUDE.md",
            ".claude/CLAUDE.md",
            "sub/AGENTS.md"
        ]
    );
    insta::assert_snapshot!(loaded.block);
}

#[test]
fn instructions_order_is_stable_across_runs() {
    let (_dir, roots) = fixture();
    let first = load(&roots, 8000);
    let second = load(&roots, 8000);
    assert_eq!(first, second);
}

#[test]
fn instructions_cycle_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root.join("AGENTS.md"), "A then @b.md\n");
    write(&root.join("b.md"), "B then @AGENTS.md\n");
    let roots = Roots {
        cox_home: None,
        claude_home: None,
        git_root: Some(root.clone()),
        cwd: root.clone(),
    };
    let loaded = load(&roots, 8000);
    assert_eq!(
        loaded.notices,
        ["instruction include cycle: AGENTS.md → b.md → AGENTS.md"]
    );
    assert!(
        loaded.block.contains("A then B then @AGENTS.md"),
        "{}",
        loaded.block
    );
}

#[test]
fn instructions_budget_drops_later_files_with_a_notice() {
    let (_dir, roots) = fixture();
    // Room for the first two sections only.
    let loaded = load(&roots, 30);
    assert_eq!(loaded.files, ["AGENTS.md", "CLAUDE.md"]);
    assert_eq!(loaded.notices.len(), 2, "{:?}", loaded.notices);
    assert!(loaded.notices[0].starts_with("instruction file .claude/CLAUDE.md dropped"));
}

#[cfg(unix)]
#[test]
fn instructions_symlinked_duplicate_loads_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root.join("AGENTS.md"), "Shared.\n");
    std::os::unix::fs::symlink(root.join("AGENTS.md"), root.join("CLAUDE.md")).unwrap();
    let roots = Roots {
        cox_home: None,
        claude_home: None,
        git_root: Some(root.clone()),
        cwd: root.clone(),
    };
    let loaded = load(&roots, 8000);
    assert_eq!(loaded.files, ["AGENTS.md"]);
}

#[test]
fn instructions_homes_come_first_and_missing_tree_is_empty() {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join("AGENTS.md"), "Global.\n");
    let work = tempfile::tempdir().unwrap();
    let roots = Roots {
        cox_home: Some(home.path().to_path_buf()),
        claude_home: None,
        git_root: None,
        cwd: work.path().to_path_buf(),
    };
    let loaded = load(&roots, 8000);
    assert_eq!(loaded.files.len(), 1);
    assert!(loaded.block.starts_with("# Instructions\n## "));
    let none = load(
        &Roots {
            cox_home: None,
            ..roots
        },
        8000,
    );
    assert_eq!(none, Default::default());
}
