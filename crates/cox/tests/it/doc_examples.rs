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
//!
//! T33.40.11 Check: every `toml` block in `docs/plugins/jev.md` parses as
//! cox's config or as a `plugin.toml`, and the install and doctor output the
//! page quotes is the real binary's.

#![cfg(feature = "plugins")]

use std::fs;
use std::path::Path;

use assert_cmd::Command;

/// The repository's own manifest: the doc installs this exact plugin.
const MANIFEST: &str = include_str!("../../../../plugins/cursor/plugin.toml");

/// The Jev guide and the manifest it installs.
const JEV_DOC: &str = include_str!("../../../../docs/plugins/jev.md");
const JEV_MANIFEST: &str = include_str!("../../../../plugins/jev/plugin.toml");

fn cox(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(cwd)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env_remove("CURSOR_API_KEY")
        .env_remove("TYPESAFE_API_KEY")
        // No `agent` CLI reachable: the doc's documented "missing" row.
        .env("PATH", "")
        .args(["--cwd", cwd.to_str().expect("utf-8 path")]);
    cmd
}

/// The bodies of a page's fenced blocks tagged `lang` (`""` for untagged),
/// in order. Every fence is tracked, so a tagged block's closing fence is
/// never read as an untagged opening.
fn fenced(doc: &str, lang: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<(bool, String)> = None;
    for line in doc.lines() {
        let line = line.trim_end();
        match current.as_mut() {
            None => {
                if let Some(tag) = line.strip_prefix("```") {
                    current = Some((tag == lang, String::new()));
                }
            }
            Some(_) if line == "```" => {
                if let Some((true, body)) = current.take() {
                    blocks.push(body);
                }
            }
            Some((_, body)) => {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    blocks
}

/// Doc Check (T33.40.11): a `toml` block is either a user config the loader
/// accepts over the defaults (`deny_unknown_fields` catches a renamed key)
/// or a `plugin.toml` the host would load, so the page cannot drift from
/// either schema.
#[test]
fn jev_doc_toml_blocks_parse_as_config_or_manifest() {
    let blocks = fenced(JEV_DOC, "toml");
    assert!(blocks.len() >= 3, "the guide's toml blocks: {blocks:?}");
    for block in &blocks {
        let Err(config) = cox_config::load::check_user_toml(block) else {
            continue;
        };
        let pkg = tempfile::tempdir().expect("pkg dir");
        let path = pkg.path().join("plugin.toml");
        fs::write(&path, block).expect("manifest");
        if let Err(manifest) = cox_plugin::discover::load_manifest(pkg.path(), &path, None) {
            panic!("neither config ({config}) nor plugin.toml ({manifest}):\n{block}");
        }
    }
}

/// Doc Check (T33.40.11): the grant lines the guide quotes are what `cox
/// plugin install` prints for `plugins/jev`, and its doctor rows (the loaded
/// plugin, a missing key, a `[providers.typesafe]` table with no plugin) are
/// the real ones.
#[test]
fn jev_doc_commands_match_the_real_binary() {
    let fences = fenced(JEV_DOC, "");
    let quoted = |head: &str| {
        fences
            .iter()
            .find(|b| b.starts_with(head))
            .unwrap_or_else(|| panic!("no block starting {head:?} in the guide"))
            .clone()
    };

    let work = tempfile::tempdir().expect("work dir");
    let home = tempfile::tempdir().expect("home dir");
    // The leftover table first, before the plugin is installed.
    fs::write(
        home.path().join("config.toml"),
        "[providers.typesafe]\napi_key_env = \"TYPESAFE_API_KEY\"\n",
    )
    .expect("config");
    let doctor = |home: &Path| {
        let out = cox(home, work.path())
            .arg("doctor")
            .assert()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).expect("utf-8")
    };
    let out = doctor(home.path());
    let leftover = quoted("providers.typesafe:");
    assert!(out.contains(leftover.trim_end()), "{out}");

    let pkg = work.path().join("target/plugins/jev");
    fs::create_dir_all(&pkg).expect("pkg dir");
    fs::write(pkg.join("plugin.toml"), JEV_MANIFEST).expect("manifest");
    fs::write(pkg.join("jev.wasm"), b"stub").expect("wasm stub");
    let out = cox(home.path(), work.path())
        .args(["plugin", "install", "target/plugins/jev", "--yes"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).expect("utf-8");
    assert!(out.contains(&quoted("Jev (")), "{out}");

    let out = doctor(home.path());
    for head in ["plugin jev:", "provider typesafe: ⚠"] {
        assert!(out.contains(quoted(head).trim_end()), "{head}: {out}");
    }
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
