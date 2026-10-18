// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T66.23: what a person sees of `cox serve` and `cox attach --stdio` — the
//! hidden worker flag, and the bridge that never starts a supervisor. The
//! supervisor running end to end is T66.24's test.

use assert_cmd::Command;
use predicates::prelude::*;

fn cox(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.env("COX_HOME", home);
    cmd
}

#[test]
fn serve_help_hides_the_worker_flag() {
    let home = tempfile::tempdir().expect("scratch home");
    cox(home.path())
        .args(["serve", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("worker").not());
}

#[test]
fn attach_needs_stdio() {
    let home = tempfile::tempdir().expect("scratch home");
    cox(home.path()).arg("attach").assert().failure();
}

#[cfg(unix)]
#[test]
fn attach_without_a_supervisor_fails_naming_the_socket() {
    let home = tempfile::tempdir().expect("scratch home");
    let socket = home.path().join("run").join("serve.sock");
    cox(home.path())
        .args(["attach", "--stdio"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains(socket.display().to_string()));
    // Never a supervisor of its own: nothing was created to listen on.
    assert!(!socket.exists());
}

#[cfg(not(unix))]
#[test]
fn both_commands_refuse_where_there_is_no_socket_transport() {
    let home = tempfile::tempdir().expect("scratch home");
    for args in [&["serve"][..], &["attach", "--stdio"][..]] {
        cox(home.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("not supported yet"));
    }
}
