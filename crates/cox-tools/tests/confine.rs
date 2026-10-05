// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Exhaustive escape-attempt table for `cox_tools::path::confine`, the one
//! function every path from the model must pass through (AGENTS.md trust
//! boundaries). Lives in its own integration test (not `path.rs`'s inline
//! `#[cfg(test)]` module) so it reads as the spec: one function per named
//! escape/allow case, plus the grep guard (`confine_is_the_only_path_constructor`)
//! that keeps every other tool routed through `confine` instead of building
//! its own `Path`/`PathBuf` from a model-supplied string.

use std::fs;
use std::path::{Path, PathBuf};

use cox_protocol::ToolError;
use cox_tools::path::confine;

/// A fresh, self-contained tempdir tree per test (tests run concurrently,
/// so nothing here is shared). `root`/`root2`/`outside` are canonicalized
/// once at setup, and every expectation below is built from those
/// canonical values — not the raw tempdir path — because on macOS the
/// tempdir base itself sits behind a symlink (`/tmp` -> `/private/tmp`),
/// which `confine`'s own symlink-resolution step would otherwise make a
/// correct result look like a mismatch.
struct Workspace {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    root2: PathBuf,
    outside: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");

        let root = tmp.path().join("root");
        let root2 = tmp.path().join("root2");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(root.join("sub")).expect("mkdir root/sub");
        fs::write(root.join("sub/file.txt"), b"hi").expect("write root/sub/file.txt");
        fs::create_dir_all(&root2).expect("mkdir root2");
        fs::write(root2.join("other.txt"), b"hi").expect("write root2/other.txt");
        fs::create_dir_all(outside.join("target")).expect("mkdir outside/target");
        fs::create_dir_all(outside.join("target2")).expect("mkdir outside/target2");
        fs::write(outside.join("secret.txt"), b"nope").expect("write outside/secret.txt");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("target"), root.join("link_out"))
                .expect("symlink link_out");
            std::os::unix::fs::symlink(outside.join("target2"), root.join("linkdir"))
                .expect("symlink linkdir");
        }

        // Canonicalize after every fixture file exists, so both the roots
        // handed to `confine` and the paths this test compares against are
        // already resolved.
        let root = fs::canonicalize(&root).expect("canonicalize root");
        let root2 = fs::canonicalize(&root2).expect("canonicalize root2");
        let outside = fs::canonicalize(&outside).expect("canonicalize outside");

        Self {
            _tmp: tmp,
            root,
            root2,
            outside,
        }
    }
}

fn assert_confined(result: Result<PathBuf, ToolError>) {
    match result {
        Err(ToolError::Confined { .. }) => {}
        other => panic!("expected ToolError::Confined, got {other:?}"),
    }
}

// --- allowed cases -------------------------------------------------------

#[test]
fn confine_plain_relative_file_in_root_is_ok() {
    let ws = Workspace::new();
    let got = confine(std::slice::from_ref(&ws.root), &ws.root, "sub/file.txt").expect("confine");
    assert_eq!(got, ws.root.join("sub/file.txt"));
}

#[test]
fn confine_root_itself_is_ok() {
    let ws = Workspace::new();
    let got = confine(std::slice::from_ref(&ws.root), &ws.root, ".").expect("confine");
    assert_eq!(got, ws.root);
}

#[test]
fn confine_nonexistent_leaf_in_existing_dir_is_ok() {
    let ws = Workspace::new();
    let got =
        confine(std::slice::from_ref(&ws.root), &ws.root, "sub/new-file.txt").expect("confine");
    assert_eq!(got, ws.root.join("sub/new-file.txt"));
}

#[test]
fn confine_deeply_nonexistent_path_under_root_is_ok() {
    let ws = Workspace::new();
    let got = confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "brand/new/nested/file.txt",
    )
    .expect("confine");
    assert_eq!(got, ws.root.join("brand/new/nested/file.txt"));
}

#[test]
fn confine_dot_slash_dotdot_collapses_within_root_is_ok() {
    let ws = Workspace::new();
    let got = confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "./a/../sub/file.txt",
    )
    .expect("confine");
    assert_eq!(got, ws.root.join("sub/file.txt"));
}

#[test]
fn confine_trailing_slash_is_ok() {
    let ws = Workspace::new();
    let got = confine(std::slice::from_ref(&ws.root), &ws.root, "sub/").expect("confine");
    assert_eq!(got, ws.root.join("sub"));
}

#[test]
fn confine_tilde_expands_to_home() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set in the test env"));
    let got = confine(std::slice::from_ref(&home), &home, "~").expect("confine");
    let want = fs::canonicalize(&home).unwrap_or(home);
    assert_eq!(got, want);
}

#[test]
fn confine_joins_relative_to_cwd_not_root() {
    let ws = Workspace::new();
    let cwd = ws.root.join("sub");
    let got = confine(std::slice::from_ref(&ws.root), &cwd, "file.txt").expect("confine");
    assert_eq!(got, ws.root.join("sub/file.txt"));
}

#[test]
fn confine_second_root_is_reachable() {
    let ws = Workspace::new();
    let got =
        confine(&[ws.root.clone(), ws.root2.clone()], &ws.root2, "other.txt").expect("confine");
    assert_eq!(got, ws.root2.join("other.txt"));
}

// --- rejected cases --------------------------------------------------------

#[test]
fn confine_dotdot_escape_above_root_is_confined() {
    let ws = Workspace::new();
    // One level up from `root` lands in the tempdir itself, which is not
    // under `root` — no symlink involved, a plain lexical escape.
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "../outside/secret.txt",
    ));
}

#[test]
fn confine_absolute_path_outside_roots_is_confined() {
    let ws = Workspace::new();
    let outside_file = ws.outside.join("secret.txt");
    let input = outside_file.to_str().expect("utf8 path");
    assert_confined(confine(std::slice::from_ref(&ws.root), &ws.root, input));
}

#[test]
fn confine_symlink_to_outside_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "link_out",
    ));
}

#[test]
fn confine_dotdot_through_symlink_is_confined() {
    let ws = Workspace::new();
    // `linkdir` resolves outside root; `..` from there escapes further
    // still — a lexical check alone (which would cancel `linkdir/..` back
    // to nothing) misses this, which is exactly what the symlink-resolution
    // step exists to catch.
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "linkdir/../secret.txt",
    ));
}

#[test]
fn confine_nul_byte_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "sub/fi\0le.txt",
    ));
}

#[test]
fn confine_windows_drive_syntax_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(std::slice::from_ref(&ws.root), &ws.root, r"C:\x"));
}

#[test]
fn confine_windows_device_path_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        r"\\?\C:\x",
    ));
}

#[test]
fn confine_alternate_data_stream_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        "file.txt:stream",
    ));
}

#[test]
fn confine_unc_prefix_without_colon_is_confined() {
    let ws = Workspace::new();
    assert_confined(confine(
        std::slice::from_ref(&ws.root),
        &ws.root,
        r"\\server\share",
    ));
}

#[test]
fn confine_empty_roots_always_confines() {
    let ws = Workspace::new();
    let roots: [PathBuf; 0] = [];
    assert_confined(confine(&roots, &ws.root, "sub/file.txt"));
}

// --- the "no other path constructor" guard ---------------------------------

/// Done-when grep test (plan.md T3.1): no file under `crates/cox-tools/src`
/// other than `path.rs` builds a `Path`/`PathBuf` straight from a
/// model-supplied `input` string. Trivially true today (no other tool
/// exists yet); it exists to fail loudly the day a later tool bypasses
/// `confine`.
#[test]
fn confine_is_the_only_path_constructor() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_for_raw_path_construction(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "path built from raw `input` outside path.rs: {violations:?}"
    );
}

fn scan_for_raw_path_construction(dir: &Path, violations: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            scan_for_raw_path_construction(&path, violations);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("path.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read source file");
        if text.contains("Path::new(input") || text.contains("PathBuf::from(input") {
            violations.push(path.display().to_string());
        }
    }
}
