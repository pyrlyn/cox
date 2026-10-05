// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T35.9 Check: `docs/plugins/cursor.md`'s commands run against the real
//! `cox` binary. The doc's literal `cox plugin install plugins/cursor
//! --yes`, from a cwd shaped like the doc's own example, then `cox
//! doctor`'s row for a missing CLI and key — the exact line the doc's
//! "When something is missing" section quotes. No wasm build (install
//! digests the tree, never loads the module — a stub `cursor.wasm` is
//! enough), no network, no real key, no keychain (`COX_KEYRING=off`).

#![cfg(feature = "plugins")]

use std::fs;
use std::path::Path;

use assert_cmd::Command;

/// The repository's own manifest: the doc installs this exact plugin.
const MANIFEST: &str = include_str!("../../../plugins/cursor/plugin.toml");

fn cox(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(cwd)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env_remove("CURSOR_API_KEY")
        // No `agent` CLI reachable: the doc's documented "missing" row.
        .env("PATH", "")
        .args(["--cwd", cwd.to_str().expect("utf-8 path")]);
    cmd
}

/// Doc Check: `cox plugin install plugins/cursor --yes` grants exactly the
/// capability line the doc quotes, then `cox doctor` reports the row the
/// doc shows for a missing CLI and an unset `CURSOR_API_KEY`.
#[test]
fn cursor_doc_commands_match_the_real_binary() {
    let work = tempfile::tempdir().expect("work dir");
    let pkg = work.path().join("plugins/cursor");
    fs::create_dir_all(&pkg).expect("pkg dir");
    fs::write(pkg.join("plugin.toml"), MANIFEST).expect("manifest");
    fs::write(pkg.join("cursor.wasm"), b"stub").expect("wasm stub");
    let home = tempfile::tempdir().expect("home dir");

    let out = cox(home.path(), work.path())
        .args(["plugin", "install", "plugins/cursor", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).expect("utf-8");
    assert!(
        out.contains("agent:cursor agent acp key=CURSOR_API_KEY"),
        "{out}"
    );
    assert!(out.contains("plugin cursor enabled"), "{out}");

    // Not `.success()`: an empty `PATH` also fails unrelated rows (`git`,
    // the toolchain) — doctor's own exit code is not what this doc checks.
    let out = cox(home.path(), work.path())
        .arg("doctor")
        .assert()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).expect("utf-8");
    let row = out
        .lines()
        .find(|l| l.starts_with("external agent cursor:"))
        .unwrap_or_else(|| panic!("no external agent row in:\n{out}"));
    assert_eq!(
        row,
        "external agent cursor: ⚠ agent not found on PATH; key_env CURSOR_API_KEY not set"
    );
    assert!(
        out.contains("fix: install the `agent` CLI or fix `command`"),
        "{out}"
    );
}
