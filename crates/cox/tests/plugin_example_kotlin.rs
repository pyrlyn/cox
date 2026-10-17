// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.36 Check: the Kotlin reference plugin (`plugins/examples/kotlin`,
//! Kotlin/Wasm `wasmWasi` over cox's Kotlin PDK, `wasi = true` for the
//! stdlib's `random_get`) end to end, through the checks every wasm guest
//! shares (`common/wasm_example.rs`). `#[ignore]`d because it needs a JDK
//! and Gradle — `just plugin-examples kotlin` stages the package in
//! `build/plugin` first and then runs these tests through
//! `--run-ignored only`; the `plugin-examples` CI job does both, and a
//! missing toolchain there fails the job.
//!
//! `plugin_example_kotlin_template` also scaffolds `cox plugin new --lang
//! kotlin` with every wasm capability, builds it against the in-repo PDK
//! with the `gradle` on `PATH`, and starts it in the host, so the template
//! cannot drift from the PDK.

#![cfg(feature = "plugins")]

#[path = "common/wasm_example.rs"]
mod wasm_example;

use std::path::PathBuf;

use serde_json::json;
use wasm_example::{cox, plugin_row, repo_root, run, stdout};

const IGNORE: &str = "needs java and gradle: run `just plugin-examples kotlin`";

fn package_dir() -> PathBuf {
    let dir = repo_root().join("plugins/examples/kotlin/build/plugin");
    assert!(
        dir.join("example_kotlin.wasm").is_file(),
        "build the example first ({IGNORE}): {}/example_kotlin.wasm is missing",
        dir.display()
    );
    dir
}

#[test]
#[ignore = "needs java and gradle: run `just plugin-examples kotlin`"]
fn plugin_example_kotlin() {
    wasm_example::installs_and_counts_failures_across_processes(&package_dir(), "example-kotlin");
}

#[test]
#[ignore = "needs java and gradle: run `just plugin-examples kotlin`"]
fn plugin_example_kotlin_counts_turns_in_its_status_and_resets_them() {
    wasm_example::counts_turns_in_its_status_and_resets_them(&package_dir());
}

#[test]
#[ignore = "needs java and gradle: run `just plugin-examples kotlin`"]
fn plugin_example_kotlin_template() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (home, work) = (home.path(), work.path());
    let pkg = work.join("demo-kt");
    stdout(cox(home, work).args([
        "plugin",
        "new",
        "demo-kt",
        "--lang",
        "kotlin",
        "--with",
        "status,panel,command,key,renderer,hook,event,tool",
        "--dir",
        pkg.to_str().unwrap(),
    ]));

    let sdk = repo_root()
        .join("plugins/sdk-kotlin")
        .canonicalize()
        .unwrap();
    let sdk = format!("-PcoxSdk={}", sdk.display());
    // No daemon: the build must leave no Gradle process behind the test.
    run(
        "gradle",
        &["--no-daemon", "-q", &sdk, "pluginPackage"],
        &pkg,
        IGNORE,
    );
    let staged = pkg.join("build/plugin");

    // Starting it runs `cox_init`, so every scaffolded declaration is read
    // back through the host's own types.
    let live = wasm_example::start(&staged, wasm_example::store(home), home);
    let init = live.plugins()[0].init_out().unwrap().clone();
    assert_eq!(init.subscribe, ["turn_started"]);
    assert_eq!(init.commands[0].name, "hello");
    assert_eq!(init.tools[0]["name"], "hello");

    stdout(cox(home, work).args(["plugin", "install", staged.to_str().unwrap(), "--yes"]));
    let row = plugin_row(home, work, "demo-kt");
    assert_eq!(row["loaded"], true, "{row}");
    assert_eq!(row["declared"]["tools"], json!(["hello"]), "{row}");
    assert_eq!(row["declared"]["ui"]["status"], true, "{row}");
}
