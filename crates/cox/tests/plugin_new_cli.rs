// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.29 Check: `cox plugin new demo --lang rust --with status,hook` in a
//! scratch `COX_HOME` writes the PL§13 file tree, and the manifest it
//! writes parses as `PluginManifest` and passes `validate()` the same way
//! `cox plugin install` would — the two together are what "validates
//! against `docs/plugin.schema.json`" means here, since that schema is
//! generated from this same type by `cox-plugin-api`'s own drift test
//! (`crates/cox-plugin-api/src/lib.rs`), so a value this type accepts is a
//! value the schema accepts. `new_refuses_existing_dir` and
//! `new_rejects_invalid_name` cover the two refusals PL§13 names.
//!
//! `new_scaffold_builds_offline_and_its_smoke_test_passes` then builds the
//! scaffold for real: the template's `cox-plugin-sdk` dependency is a `git`
//! source (not on crates.io yet, `plan.md` §14 decision 1), which cargo
//! cannot resolve `--offline` even patched (verified by hand: a `[patch]`
//! on a fresh git source still needs one checkout to identify what it
//! patches). So this test rewrites that one line to a `path` dependency —
//! the "patchable to the in-repo path" the card asks for — before running
//! an `--offline` build with its own `--target-dir` (disk rule: cleaned up
//! with the tempdir) and the smoke test the template wrote.

#![cfg(feature = "plugins")]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;

fn cox_plugin(home: &Path, cwd: &Path, args: &[&str]) -> AssertCommand {
    let mut cmd = AssertCommand::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(cwd)
        .env("COX_HOME", home)
        .env("HOME", home)
        .args(["--cwd", cwd.to_str().unwrap(), "plugin"])
        .args(args);
    cmd
}

/// The cox repository root, from this test binary's own manifest dir
/// (`crates/cox`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn relative_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn new_rejects_invalid_name() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    cox_plugin(home.path(), cwd.path(), &["new", "Bad-Name"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("must match"));
    assert!(!cwd.path().join("Bad-Name").exists());
}

#[test]
fn new_refuses_existing_dir() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir(cwd.path().join("demo")).unwrap();
    std::fs::write(cwd.path().join("demo/keep.txt"), "mine").unwrap();

    cox_plugin(home.path(), cwd.path(), &["new", "demo"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));

    assert_eq!(relative_files(&cwd.path().join("demo")), ["keep.txt"]);
}

#[test]
fn new_writes_the_pl13_tree_and_a_manifest_that_validates() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    cox_plugin(
        home.path(),
        cwd.path(),
        &["new", "demo", "--lang", "rust", "--with", "status,hook"],
    )
    .assert()
    .success();

    let dir = cwd.path().join("demo");
    assert_eq!(
        relative_files(&dir),
        [
            ".gitignore",
            "Cargo.toml",
            "README.md",
            "justfile",
            "plugin.toml",
            "src/lib.rs",
            "tests/smoke.rs",
        ]
    );

    let manifest_path = dir.join("plugin.toml");
    let (manifest, _digest) = cox_plugin::discover::load_manifest(&dir, &manifest_path, None)
        .expect("plugin.toml parses as PluginManifest and passes validate()");
    assert_eq!(manifest.id, "demo");
    assert!(
        manifest.capabilities.ui.status,
        "{:?}",
        manifest.capabilities
    );
    assert_eq!(manifest.capabilities.hooks, ["PostToolUse"]);
    assert!(!manifest.capabilities.ui.panel);
    assert!(!manifest.capabilities.ui.commands);
    assert!(manifest.capabilities.events.is_empty());
    assert!(manifest.provider.is_empty());
    assert!(manifest.mcp.is_empty());

    let lib_rs = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(lib_rs.contains("fn hook"));
    assert!(lib_rs.contains("fn render("));
    assert!(!lib_rs.contains("fn command"));
    assert!(!lib_rs.contains("cox:with"), "an unrendered marker leaked");
}

/// Builds the scaffold `--offline` for real (SDK dependency rewritten to
/// the in-repo path) and runs the smoke test it wrote. Slower than the
/// rest of this file's tests (a wasm32 guest build plus a host build), but
/// not `#[ignore]`: the `wasm32-unknown-unknown` target is already this
/// repository's own requirement (`mise.toml`, `cox-plugin-fixtures`), not
/// an extra toolchain the way Go/Kotlin/Dart are for their own cards.
#[test]
fn new_scaffold_builds_offline_and_its_smoke_test_passes() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    cox_plugin(home.path(), cwd.path(), &["new", "demo", "--lang", "rust"])
        .assert()
        .success();
    let dir = cwd.path().join("demo");

    let cargo_toml_path = dir.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).unwrap();
    let sdk_dir = repo_root().join("plugins/sdk");
    let patched = cargo_toml.replace(
        r#"cox-plugin-sdk = { git = "https://github.com/pyrlyn/cox" }"#,
        &format!(r#"cox-plugin-sdk = {{ path = {:?} }}"#, sdk_dir),
    );
    assert_ne!(
        cargo_toml, patched,
        "the git dependency line moved; update this test"
    );
    std::fs::write(&cargo_toml_path, patched).unwrap();

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target_dir = dir.join("target");
    let run = |args: &[&str]| {
        Command::new(&cargo)
            .current_dir(&dir)
            .args(args)
            .arg("--target-dir")
            .arg(&target_dir)
            .env("CARGO_INCREMENTAL", "0")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CLIPPY_ARGS")
            .status()
            .unwrap_or_else(|e| panic!("run {cargo} {args:?}: {e}"))
    };

    let build = run(&[
        "build",
        "--offline",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "-j",
        "4",
    ]);
    assert!(build.success(), "cargo build --offline failed");

    // The smoke test shells out to whatever `cox` is on PATH; point PATH at
    // this test binary's own freshly built cox.
    let cox_dir = Path::new(env!("CARGO_BIN_EXE_cox"))
        .parent()
        .unwrap()
        .to_path_buf();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![cox_dir];
    paths.extend(std::env::split_paths(&path));
    let new_path = std::env::join_paths(paths).unwrap();

    let test = Command::new(&cargo)
        .current_dir(&dir)
        .args(["test", "--offline", "--release", "-j", "4"])
        .arg("--target-dir")
        .arg(&target_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("PATH", new_path)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CLIPPY_ARGS")
        .status()
        .unwrap_or_else(|e| panic!("run {cargo} test: {e}"));
    assert!(test.success(), "the scaffold's own smoke test failed");

    // Disk rule: this test's target dir is its own, under the tempdir, so
    // dropping `dir`'s tempdir at the end of the test cleans it up; the
    // tempdir crate does this on drop even for a multi-GB build directory.
}
