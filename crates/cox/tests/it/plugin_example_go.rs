// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.34 Check: the Go reference plugin (`plugins/examples/go`, TinyGo
//! `wasip1`, `wasi = true`) end to end, the same effects T33.28 asserts for
//! the Rust one in `plugins.rs`, through the checks every wasm guest shares
//! (`wasm_example.rs`). `#[ignore]`d because it needs Go and
//! TinyGo — `just plugin-examples go` builds `example_go.wasm` first and
//! then runs these tests through `--run-ignored only`; the
//! `plugin-examples` CI job does both, and a missing toolchain there fails
//! the job.
//!
//! `plugin_example_go_template` also scaffolds `cox plugin new --lang go`
//! with the wasm capabilities, builds it against the in-repo SDK with the
//! `go` and `tinygo` on `PATH`, and checks the host loads it, so the
//! template cannot drift from the SDK.

#![cfg(feature = "plugins")]

use std::path::PathBuf;

use crate::wasm_example::{self, cox, plugin_row, repo_root, run, stdout};
use serde_json::json;

const IGNORE: &str = "needs go and tinygo: run `just plugin-examples go`";

fn example_dir() -> PathBuf {
    let dir = repo_root().join("plugins/examples/go");
    assert!(
        dir.join("example_go.wasm").is_file(),
        "build the example first ({IGNORE}): {}/example_go.wasm is missing",
        dir.display()
    );
    dir
}

#[test]
#[ignore = "needs go and tinygo: run `just plugin-examples go`"]
fn plugin_example_go() {
    wasm_example::installs_and_counts_failures_across_processes(&example_dir(), "example-go");
}

#[test]
#[ignore = "needs go and tinygo: run `just plugin-examples go`"]
fn plugin_example_go_counts_turns_in_its_status_and_resets_them() {
    wasm_example::counts_turns_in_its_status_and_resets_them(&example_dir());
}

#[test]
#[ignore = "needs go and tinygo: run `just plugin-examples go`"]
fn plugin_example_go_template() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (home, work) = (home.path(), work.path());
    let pkg = work.join("demo-go");
    stdout(cox(home, work).args([
        "plugin",
        "new",
        "demo-go",
        "--lang",
        "go",
        "--with",
        "status,panel,command,key,renderer,hook,event,tool",
        "--dir",
        pkg.to_str().unwrap(),
    ]));

    let sdk = repo_root().join("plugins/sdk-go").canonicalize().unwrap();
    let module = "github.com/pyrlyn/cox/plugins/sdk-go";
    let replace = format!("-replace={module}={}", sdk.display());
    let require = format!("-require={module}@v0.0.0");
    run("go", &["mod", "edit", &require, &replace], &pkg, IGNORE);
    run("go", &["mod", "tidy"], &pkg, IGNORE);
    run("go", &["vet", "."], &pkg, IGNORE);
    // Markers are stripped line by line; the result must still be gofmt's.
    assert_eq!(run("gofmt", &["-l", "."], &pkg, IGNORE), "", "gofmt -l");
    run(
        "tinygo",
        &[
            "build",
            "-o",
            "demo_go.wasm",
            "-target",
            "wasip1",
            "-buildmode=c-shared",
            ".",
        ],
        &pkg,
        IGNORE,
    );

    stdout(cox(home, work).args(["plugin", "install", pkg.to_str().unwrap(), "--yes"]));
    let row = plugin_row(home, work, "demo-go");
    assert_eq!(row["loaded"], true, "{row}");
    assert_eq!(row["declared"]["tools"], json!(["hello"]), "{row}");
    assert_eq!(row["declared"]["ui"]["status"], true, "{row}");
}
