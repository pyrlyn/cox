// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.34 Check: the Go reference plugin (`plugins/examples/go`, TinyGo
//! `wasip1`, `wasi = true`) end to end, the same effects T33.28 asserts for
//! the Rust one in `plugins.rs`. `#[ignore]`d because it needs Go and
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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use cox_plugin::{Lane, LivePlugins, PluginHost};
use cox_plugin_api::{CommandIn, CommandOut, EventBatch, RenderIn, Slot, Widget};
use cox_protocol::config::PluginsConfig;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{PluginStore, Store as _};
use serde_json::{Value, json};

const IGNORE: &str = "needs go and tinygo: run `just plugin-examples go`";

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/read_missing_then_done.toml"
);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example_dir() -> PathBuf {
    let dir = repo_root().join("plugins/examples/go");
    assert!(
        dir.join("example_go.wasm").is_file(),
        "build the example first ({IGNORE}): {}/example_go.wasm is missing",
        dir.display()
    );
    dir
}

fn cox(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(cwd)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", SCENARIO)
        .args(["--cwd", cwd.to_str().unwrap()]);
    cmd
}

fn stdout(cmd: &mut Command) -> Vec<u8> {
    cmd.assert().success().get_output().stdout.clone()
}

/// The notices the plugin's hook left in the session's rollout.
fn hook_notices(home: &Path, session: &str) -> Vec<String> {
    let path = home.join("sessions").join(format!("{session}.jsonl"));
    let text = std::fs::read_to_string(&path).unwrap();
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["event"].clone())
        .filter(|e| e["type"] == "notice")
        .filter_map(|e| e["text"].as_str().map(str::to_owned))
        .filter(|t| t.starts_with("plugin example-go: failed tool calls"))
        .collect()
}

fn plugin_row(home: &Path, cwd: &Path, id: &str) -> Value {
    let list: Value =
        serde_json::from_slice(&stdout(cox(home, cwd).args(["plugin", "list", "--json"]))).unwrap();
    list["plugins"]
        .as_array()
        .and_then(|plugins| plugins.iter().find(|p| p["id"] == id))
        .cloned()
        .unwrap_or_else(|| panic!("{id} not in `cox plugin list --json`: {list}"))
}

#[test]
#[ignore = "needs go and tinygo: run `just plugin-examples go`"]
fn plugin_example_go() {
    let dir = example_dir();
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (home, cwd) = (home.path(), cwd.path());

    stdout(cox(home, cwd).args(["plugin", "install", dir.to_str().unwrap(), "--yes"]));
    stdout(cox(home, cwd).args(["plugin", "enable", "example-go", "--yes"]));

    let first: Value = serde_json::from_slice(&stdout(cox(home, cwd).args([
        "run",
        "-p",
        "read it",
        "--output-format",
        "json",
    ])))
    .unwrap();
    let session = first["session"].as_str().unwrap().to_string();
    stdout(cox(home, cwd).args(["run", "-p", "again", "--resume", &session]));

    // The count lives in the plugin's kv store, so the second process
    // continues where the first stopped.
    assert_eq!(
        hook_notices(home, &session),
        [
            "plugin example-go: failed tool calls: 1",
            "plugin example-go: failed tool calls: 2"
        ]
    );

    let row = plugin_row(home, cwd, "example-go");
    assert_eq!(row["loaded"], true, "{row}");
    let declared = &row["declared"];
    assert_eq!(declared["events"], json!(["turn_started"]), "{row}");
    assert_eq!(
        declared["hooks"],
        json!(["PostToolUse", "PostToolUseFailure"]),
        "{row}"
    );
    assert_eq!(declared["kv"], true, "{row}");
    assert_eq!(declared["ui"]["status"], true, "{row}");
    assert_eq!(declared["ui"]["commands"], true, "{row}");
}

fn call<O: serde::de::DeserializeOwned>(
    host: &PluginHost,
    export: &str,
    input: &impl serde::Serialize,
) -> O {
    host.call(Lane::Control, export, input, Duration::from_secs(30))
        .unwrap()
        .unwrap()
}

fn status_text(host: &PluginHost) -> String {
    let render = RenderIn {
        slot: Slot::StatusRight,
        width: 80,
        height: 1,
    };
    match call(host, "cox_render", &render) {
        Widget::Text(lines) => lines[0][0].text.clone(),
        other => panic!("a text segment, got {other:?}"),
    }
}

/// The exports a headless run never reaches — the subscription, the status
/// segment and `/example-go:reset` — decoded into the host's own types, so
/// a Go payload that drifts from `cox-plugin-api` fails here.
#[test]
#[ignore = "needs go and tinygo: run `just plugin-examples go`"]
fn plugin_example_go_counts_turns_in_its_status_and_resets_them() {
    let dir = example_dir();
    let home = tempfile::tempdir().unwrap();
    let (mut manifest, _) =
        cox_plugin::discover::load_manifest(&dir, &dir.join("plugin.toml"), None).unwrap();
    assert!(manifest.wasi);
    // A long call cap: this test is about what the calls do, not their budget.
    manifest.limits.call_ms = Some(30_000);
    let wasm = std::fs::read(dir.join(manifest.wasm.as_deref().unwrap())).unwrap();
    let store: Arc<dyn PluginStore> = Arc::new(cox_store::Store::open(home.path()).unwrap());
    let mut live = LivePlugins::default();
    live.load(&manifest, &wasm, store.clone()).unwrap();
    let warnings = live.start(&PluginsConfig::default(), SessionId::new(), home.path());
    assert!(warnings.is_empty(), "{warnings:?}");

    let plugin = &live.plugins()[0];
    assert_eq!(plugin.init_out().unwrap().subscribe, ["turn_started"]);
    assert_eq!(plugin.granted_status(), [Slot::StatusRight]);
    assert_eq!(plugin.granted_commands()[0].name, "reset");

    let host = plugin.host();
    let batch = EventBatch {
        first_seq: 1,
        dropped: 0,
        events: vec![
            json!({ "type": "turn_started", "seq": 1 }),
            json!({ "type": "notice", "text": "x" }),
            json!({ "type": "turn_started", "seq": 2 }),
        ],
    };
    let effects: cox_plugin_api::Effects = call(host, "cox_on_event", &batch);
    assert!(effects.redraw);
    assert_eq!(status_text(host), "turns 2 · failed tools 0");

    // A second instance (the next `cox` process) reads the count back from kv.
    let mut next = LivePlugins::default();
    next.load(&manifest, &wasm, store).unwrap();
    assert!(
        next.start(&PluginsConfig::default(), SessionId::new(), home.path())
            .is_empty()
    );
    assert_eq!(
        status_text(next.plugins()[0].host()),
        "turns 2 · failed tools 0"
    );

    let reset = CommandIn {
        name: "reset".into(),
        args: String::new(),
    };
    let out: CommandOut = call(host, "cox_command", &reset);
    assert!(matches!(out, CommandOut::Notice(n) if n.text == "counters reset"));
    assert_eq!(status_text(host), "turns 0 · failed tools 0");
}

/// The `PATH` the Go toolchain runs with: `PLUGIN_TOOLS_PATH` when set (no
/// `COX_` prefix, which cox would read as config).
/// `just plugin-examples go` sets it to the `PATH` it built the example
/// with, because the `mise exec` that starts nextest from the repository
/// root drops the `plugins/` tools and puts other ones first (Homebrew's
/// `go` against mise's `GOROOT` refuses its compiler; TinyGo looks `go` up
/// on `PATH` too). CI sets it the same way: its `cargo` is a mise shim too.
fn tool_path() -> std::ffi::OsString {
    std::env::var_os("PLUGIN_TOOLS_PATH")
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default()
}

/// Runs a Go tool in `cwd` and returns its stdout; fails the test if it fails.
fn run(program: &str, args: &[&str], cwd: &Path) -> String {
    // With `PATH` set on the command, Unix resolves `program` through it.
    let out = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", tool_path())
        .stderr(std::process::Stdio::inherit())
        .output()
        .unwrap_or_else(|e| panic!("{program} ({IGNORE}): {e}"));
    assert!(out.status.success(), "{program} {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
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
    run("go", &["mod", "edit", &require, &replace], &pkg);
    run("go", &["mod", "tidy"], &pkg);
    run("go", &["vet", "."], &pkg);
    // Markers are stripped line by line; the result must still be gofmt's.
    assert_eq!(run("gofmt", &["-l", "."], &pkg), "", "gofmt -l");
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
    );

    stdout(cox(home, work).args(["plugin", "install", pkg.to_str().unwrap(), "--yes"]));
    let row = plugin_row(home, work, "demo-go");
    assert_eq!(row["loaded"], true, "{row}");
    assert_eq!(row["declared"]["tools"], json!(["hello"]), "{row}");
    assert_eq!(row["declared"]["ui"]["status"], true, "{row}");
}
