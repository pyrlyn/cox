// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T48.1: every `tests/cmd/*.toml` case runs the real `cox` binary with a
//! scratch `COX_HOME` and the scripted provider, and compares its whole
//! output with a reviewed fixture. `run_cli.rs` keeps the cases a fixture
//! cannot express (answering on stdin mid-run, prefix rules).

#[test]
fn cli_cases_match_their_fixtures() {
    // Cargo sets it for every run (`.cargo/config.toml`, A49); without it a
    // case could reach the developer's keychain.
    assert_eq!(std::env::var("COX_KEYRING").as_deref(), Ok("off"));
    let home = tempfile::tempdir().expect("scratch home");
    let home_path = home.path().display().to_string();
    trycmd::TestCases::new()
        .default_bin_name("cox")
        .env("COX_HOME", &home_path)
        .env("HOME", &home_path)
        .env("COX_PROVIDER", "scripted")
        // An error's `Error:` line must not grow a backtrace on a machine
        // that sets RUST_BACKTRACE.
        .env("RUST_LIB_BACKTRACE", "0")
        .insert_var("[HOME]", home_path.clone())
        .expect("[HOME] is a new variable")
        .case("tests/cmd/*.toml");
}
