// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.5: Claude settings lift only what cox understands, accumulate across
//! files in Claude's order, and never fail on a broken file.

use std::path::{Path, PathBuf};

use cox_ext::claude_settings::{load, paths};
use cox_protocol::config::HookConfig;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude")
}

#[test]
fn claude_settings_lift_rules_hooks_and_env_across_files() {
    let s = load(&[
        fixtures().join("settings.json"),
        fixtures().join("missing.json"),
        fixtures().join("settings.local.json"),
    ]);
    assert!(s.notices.is_empty(), "{:?}", s.notices);
    assert_eq!(s.files.len(), 2);
    assert_eq!(
        s.allow,
        ["Bash(npm run test:*)", "Read(~/.zshrc)", "Bash(git status)"]
    );
    assert_eq!(s.deny, ["Bash(rm -rf *)", "Read(./.env)"]);
    assert!(s.ask.is_empty());
    assert_eq!(
        s.hooks["PreToolUse"],
        [HookConfig {
            matcher: Some("Bash".into()),
            command: "rtk hook".into(),
            timeout_s: Some(10),
        }]
    );
    assert_eq!(s.hooks["Stop"][0].matcher, None);
    assert_eq!(s.env["FOO"], "local");
    let layer = s.to_layer();
    assert_eq!(layer["permissions"]["deny"][0], "Bash(rm -rf *)");
    assert_eq!(layer["hooks"]["PreToolUse"][0]["command"], "rtk hook");
    assert_eq!(layer["hooks"]["PreToolUse"][0]["timeout_s"], 10);
    assert!(layer.get("env").is_none());
}

#[test]
fn claude_settings_broken_file_is_a_notice_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("settings.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let s = load(std::slice::from_ref(&bad));
    assert!(s.is_empty());
    assert_eq!(s.notices.len(), 1);
    assert!(
        s.notices[0].contains("settings.json skipped"),
        "{:?}",
        s.notices
    );
}

#[test]
fn claude_settings_paths_follow_claude_precedence() {
    let p = paths(Some(Path::new("/h/.claude")), Some(Path::new("/repo")));
    let p: Vec<String> = p.iter().map(|p| p.display().to_string()).collect();
    assert_eq!(
        p,
        [
            "/h/.claude/settings.json",
            "/repo/.claude/settings.json",
            "/repo/.claude/settings.local.json"
        ]
    );
}

/// T25.5: `keybindings.json` context blocks flatten to `(key, action)`
/// pairs; a `null` unbinding is dropped, a broken file is a notice, and a
/// missing one is nothing at all.
#[test]
fn claude_keybindings_json_flattens_context_blocks() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        cox_ext::claude_settings::keybindings(dir.path()),
        Default::default()
    );
    let json = r#"{ "$schema": "x", "bindings": [
        { "context": "Chat", "bindings": { "ctrl+s": "chat:submit", "ctrl+g": null } },
        { "context": "Global", "bindings": { "ctrl+o": "app:toggleTranscript" } } ] }"#;
    std::fs::write(dir.path().join("keybindings.json"), json).expect("write");
    let k = cox_ext::claude_settings::keybindings(dir.path());
    assert_eq!(
        k.bindings,
        [
            ("ctrl+s".to_string(), "chat:submit".to_string()),
            ("ctrl+o".to_string(), "app:toggleTranscript".to_string()),
        ]
    );
    std::fs::write(dir.path().join("keybindings.json"), "{ nope").expect("write");
    assert_eq!(
        cox_ext::claude_settings::keybindings(dir.path())
            .notices
            .len(),
        1
    );
}
