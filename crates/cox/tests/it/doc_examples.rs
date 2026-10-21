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
const MANIFEST: &str = include_str!("../../../../plugins/cursor/plugin.toml");

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

/// T33.40.11: every `toml` fence in the Jev guide is a `Config` fragment
/// or a plugin manifest, so a renamed key fails here.
#[test]
fn jev_guide_toml_blocks_match_config_or_the_manifest() {
    let text = include_str!("../../../../docs/plugins/jev.md");
    let blocks = toml_fences(text);
    assert!(
        blocks.len() >= 3,
        "the guide should show decide, risk thresholds and the legacy provider"
    );
    let mut saw_decide = false;
    let mut saw_thresholds = false;
    let mut saw_typesafe = false;
    for block in &blocks {
        if let Ok(config) = toml_edit::de::from_str::<cox_protocol::Config>(block) {
            if config.plugins.decide.risk.as_deref() == Some("jev")
                && config.plugins.decide.route.as_deref() == Some("jev")
            {
                saw_decide = true;
            }
            if let Some(jev) = config.plugins.entries.get("jev") {
                assert_eq!(jev["risk"]["irreversible"], 0.7, "{jev}");
                assert_eq!(jev["risk"]["exfiltration"], 0.5, "{jev}");
                saw_thresholds = true;
            }
            if config
                .providers
                .typesafe
                .as_ref()
                .is_some_and(|t| t.api_key_env == "TYPESAFE_API_KEY")
            {
                saw_typesafe = true;
            }
            continue;
        }
        let manifest: cox_plugin_api::PluginManifest = toml_edit::de::from_str(block)
            .unwrap_or_else(|e| panic!("neither Config nor a manifest:\n{block}\n{e}"));
        manifest
            .validate()
            .unwrap_or_else(|e| panic!("manifest: {e}"));
    }
    assert!(saw_decide, "no [plugins.decide] block names jev");
    assert!(saw_thresholds, "no [plugins.jev.risk] block");
    assert!(saw_typesafe, "no [providers.typesafe] block");
}

fn toml_fences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line.trim()) {
            (Some(_), "```") => out.push(current.take().unwrap_or_default()),
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, "```toml") => current = Some(String::new()),
            (None, _) => {}
        }
    }
    out
}
