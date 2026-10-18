// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The checks every wasm guest language's PL§13 example shares (T33.34 Go,
//! T33.36 Kotlin): each language's test file builds nothing itself, finds
//! its built package and hands it here, so the examples are held to one
//! behaviour instead of copies of it that drift. A module of the `it` binary,
//! compiled only with the `plugins` feature (`it/main.rs`).

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

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/read_missing_then_done.toml"
);

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn cox(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(cwd)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", SCENARIO)
        .args(["--cwd", cwd.to_str().unwrap()]);
    cmd
}

pub fn stdout(cmd: &mut Command) -> Vec<u8> {
    cmd.assert().success().get_output().stdout.clone()
}

/// The notices plugin `id`'s hook left in the session's rollout.
fn hook_notices(home: &Path, session: &str, id: &str) -> Vec<String> {
    let path = home.join("sessions").join(format!("{session}.jsonl"));
    let text = std::fs::read_to_string(&path).unwrap();
    let prefix = format!("plugin {id}: failed tool calls");
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["event"].clone())
        .filter(|e| e["type"] == "notice")
        .filter_map(|e| e["text"].as_str().map(str::to_owned))
        .filter(|t| t.starts_with(&prefix))
        .collect()
}

pub fn plugin_row(home: &Path, cwd: &Path, id: &str) -> Value {
    let list: Value =
        serde_json::from_slice(&stdout(cox(home, cwd).args(["plugin", "list", "--json"]))).unwrap();
    list["plugins"]
        .as_array()
        .and_then(|plugins| plugins.iter().find(|p| p["id"] == id))
        .cloned()
        .unwrap_or_else(|| panic!("{id} not in `cox plugin list --json`: {list}"))
}

/// T33.28's two-process check: the example installed from `package` counts
/// failed tool calls in its hook, and the count lives in its kv store, so
/// the second `cox` process continues where the first stopped.
pub fn installs_and_counts_failures_across_processes(package: &Path, id: &str) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (home, cwd) = (home.path(), cwd.path());

    stdout(cox(home, cwd).args(["plugin", "install", package.to_str().unwrap(), "--yes"]));
    stdout(cox(home, cwd).args(["plugin", "enable", id, "--yes"]));

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

    assert_eq!(
        hook_notices(home, &session, id),
        [
            format!("plugin {id}: failed tool calls: 1"),
            format!("plugin {id}: failed tool calls: 2")
        ]
    );

    let row = plugin_row(home, cwd, id);
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

/// A plugin store in `home`, as a session opens it.
pub fn store(home: &Path) -> Arc<dyn PluginStore> {
    Arc::new(cox_store::Store::open(home).unwrap())
}

/// Loads and starts the package in `dir` the way a session does, with a
/// long call cap: the callers are about what the calls do, not their budget.
pub fn start(dir: &Path, store: Arc<dyn PluginStore>, home: &Path) -> LivePlugins {
    let (mut manifest, _) =
        cox_plugin::discover::load_manifest(dir, &dir.join("plugin.toml"), None).unwrap();
    assert!(manifest.wasi, "a wasip1 guest asks for WASI");
    manifest.limits.call_ms = Some(30_000);
    let wasm = std::fs::read(dir.join(manifest.wasm.as_deref().unwrap())).unwrap();
    let mut live = LivePlugins::default();
    live.load(&manifest, &wasm, store).unwrap();
    let warnings = live.start(&PluginsConfig::default(), SessionId::new(), home);
    assert!(warnings.is_empty(), "{warnings:?}");
    live
}

/// The exports a headless run never reaches — the subscription, the status
/// segment and `/<id>:reset` — decoded into the host's own types, so a
/// guest payload that drifts from `cox-plugin-api` fails here.
pub fn counts_turns_in_its_status_and_resets_them(dir: &Path) {
    let home = tempfile::tempdir().unwrap();
    let store = store(home.path());
    let live = start(dir, store.clone(), home.path());

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
    let next = start(dir, store, home.path());
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

/// The `PATH` a guest toolchain runs with: `PLUGIN_TOOLS_PATH` when set (no
/// `COX_` prefix, which cox would read as config).
/// `just plugin-examples <lang>` sets it to the `PATH` it built the example
/// with, because the `mise exec` that starts nextest from the repository
/// root drops the `plugins/` tools and puts other ones first (Homebrew's
/// `go` against mise's `GOROOT` refuses its compiler; TinyGo looks `go` up
/// on `PATH` too). CI sets it the same way: its `cargo` is a mise shim too.
fn tool_path() -> std::ffi::OsString {
    std::env::var_os("PLUGIN_TOOLS_PATH")
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default()
}

/// Runs a guest toolchain program in `cwd` and returns its stdout; fails
/// the test, naming `hint`, if it cannot start or fails.
pub fn run(program: &str, args: &[&str], cwd: &Path, hint: &str) -> String {
    // With `PATH` set on the command, Unix resolves `program` through it.
    let out = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", tool_path())
        .stderr(std::process::Stdio::inherit())
        .output()
        .unwrap_or_else(|e| panic!("{program} ({hint}): {e}"));
    assert!(out.status.success(), "{program} {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
