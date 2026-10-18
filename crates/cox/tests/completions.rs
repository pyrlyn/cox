// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later

//! Completion must be usable before a user has configured a provider or session.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn every_shell_generates_without_loading_config_or_creating_state() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "invalid = [").unwrap();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish", "clink"] {
        Command::new(env!("CARGO_BIN_EXE_cox"))
            .current_dir(scratch.path())
            .env("COX_HOME", &home)
            .env("COX_KEYRING", "off")
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("cox"))
            .stdout(predicate::str::contains("--model"))
            .stderr(predicate::str::is_empty());
    }
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 1);
}

#[test]
fn shell_is_required_and_unknown_shells_are_rejected() {
    for args in [vec!["completions"], vec!["completions", "unknown"]] {
        Command::new(env!("CARGO_BIN_EXE_cox"))
            .env("COX_KEYRING", "off")
            .args(args)
            .assert()
            .code(2)
            .stdout(predicate::str::is_empty());
    }
}
