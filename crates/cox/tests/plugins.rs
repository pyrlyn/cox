// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.28 Check: the Rust reference plugin (`plugins/examples/rust`, built by
//! `cox-plugin-fixtures`) end to end. The real binary installs it into a
//! scratch `COX_HOME`, `enable --yes` grants it, and two `run -p` turns with
//! the scripted provider each fail one tool call: the rollout carries the
//! hook's notice with the count kept in the plugin's kv store across both
//! processes, and `cox plugin list --json` shows what the manifest
//! contributes. The subscription, status segment and `/example:reset` are
//! driven through the host API, since a headless run has no status row.

#![cfg(feature = "plugins")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use cox_plugin::{Lane, LivePlugins, PluginHost};
use cox_plugin_api::{CommandIn, CommandOut, EventBatch, RenderIn, Slot, Widget};
use cox_plugin_fixtures::EXAMPLE_DIR;
use cox_protocol::config::PluginsConfig;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{PluginStore, Store as _};
use serde_json::{Value, json};

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/read_missing_then_done.toml"
);

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
        .filter(|t| t.starts_with("plugin example: failed tool calls"))
        .collect()
}

#[test]
fn example_plugin_counts_failures_across_a_two_turn_headless_run() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (home, cwd) = (home.path(), cwd.path());

    stdout(cox(home, cwd).args(["plugin", "install", EXAMPLE_DIR, "--yes"]));
    stdout(cox(home, cwd).args(["plugin", "enable", "example", "--yes"]));

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
        hook_notices(home, &session),
        [
            "plugin example: failed tool calls: 1",
            "plugin example: failed tool calls: 2"
        ]
    );

    let list: Value =
        serde_json::from_slice(&stdout(cox(home, cwd).args(["plugin", "list", "--json"]))).unwrap();
    let row = &list["plugins"][0];
    assert_eq!(row["id"], "example", "{list}");
    assert_eq!(row["loaded"], true, "{list}");
    let declared = &row["declared"];
    assert_eq!(declared["events"], json!(["turn_started"]), "{list}");
    assert_eq!(
        declared["hooks"],
        json!(["PostToolUse", "PostToolUseFailure"]),
        "{list}"
    );
    assert_eq!(declared["kv"], true, "{list}");
    assert_eq!(declared["ui"]["status"], true, "{list}");
    assert_eq!(declared["ui"]["commands"], true, "{list}");
}

fn call<O: serde::de::DeserializeOwned>(
    host: &PluginHost,
    export: &str,
    input: &impl serde::Serialize,
) -> O {
    host.call(Lane::Control, export, input, Duration::from_secs(5))
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

#[test]
fn example_plugin_counts_turns_in_its_status_and_resets_them() {
    let home = tempfile::tempdir().unwrap();
    let dir = Path::new(EXAMPLE_DIR);
    let (mut manifest, _) =
        cox_plugin::discover::load_manifest(dir, &dir.join("plugin.toml"), None).unwrap();
    // A long call cap: the default 200 ms covers the whole call, host functions included, which
    // a loaded CI runner can spend; this test is about what the calls do, not their budget.
    manifest.limits.call_ms = Some(30_000);
    let wasm = std::fs::read(dir.join(manifest.wasm.as_deref().unwrap())).unwrap();
    let store: Arc<dyn PluginStore> = Arc::new(cox_store::Store::open(home.path()).unwrap());
    let mut live = LivePlugins::default();
    live.load(&manifest, &wasm, store).unwrap();
    let warnings = live.start(&PluginsConfig::default(), SessionId::new(), home.path());
    assert!(warnings.is_empty(), "{warnings:?}");

    let plugin = &live.plugins()[0];
    let init = plugin.init_out().unwrap();
    assert_eq!(init.subscribe, ["turn_started"]);
    assert_eq!(plugin.granted_status(), [Slot::StatusRight]);
    assert_eq!(plugin.granted_commands()[0].name, "reset");

    let host = plugin.host();
    let batch = EventBatch {
        first_seq: 1,
        dropped: 0,
        events: vec![
            json!({ "type": "turn_started", "seq": 1 }),
            json!({ "type": "turn_started", "seq": 2 }),
        ],
    };
    let effects: cox_plugin_api::Effects = call(host, "cox_on_event", &batch);
    assert!(effects.redraw);
    assert_eq!(status_text(host), "turns 2 · failed tools 0");

    let reset = CommandIn {
        name: "reset".into(),
        args: String::new(),
    };
    let out: CommandOut = call(host, "cox_command", &reset);
    assert!(matches!(out, CommandOut::Notice(n) if n.text == "counters reset"));
    assert_eq!(status_text(host), "turns 0 · failed tools 0");
}

/// T45.4: the parent dispatches `reviewer`; the child's own turn, pinned by
/// the marker on the task's second line, answers with its tool names —
/// `grep` under the plugin's definition, `glob` under the local one. The
/// marker never reaches the parent's own text, so its `done` stays FIFO.
const AGENT_SCENARIO: &str = "[[turn]]\n\
    tool_calls = [{ name = \"agent\", input = { task = \"review\\nREVIEWER-CHILD\", preset = \"reviewer\" } }]\n\n\
    [[turn]]\nwhen_contains = \"REVIEWER-CHILD\"\necho_tools = true\n\n\
    [[turn]]\ntext = \"done\"\n";

/// An agents-only package (no wasm, PL§13) whose `reviewer` keeps `grep`,
/// and the scenario above beside it.
fn agent_package(root: &Path) -> (String, String) {
    let pkg = root.join("review-kit");
    std::fs::create_dir_all(pkg.join("agents")).unwrap();
    std::fs::write(
        pkg.join("plugin.toml"),
        "api = 1\nid = \"review-kit\"\nversion = \"0.1.0\"\nname = \"Review kit\"\n\n\
         [[agents]]\nname = \"reviewer\"\nfile = \"agents/reviewer.md\"\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: reviews from the plugin\ntools: [grep]\n---\nReview it.\n",
    )
    .unwrap();
    let scenario = root.join("plugin_agent.toml");
    std::fs::write(&scenario, AGENT_SCENARIO).unwrap();
    let path = |p: &Path| p.to_str().unwrap().to_string();
    (path(&pkg), path(&scenario))
}

/// The headless run's events, its one `agent` call's result and its
/// stderr, where session warnings go. The exit code is not asserted: a
/// denied call ends a headless run with 2.
fn agent_run(home: &Path, cwd: &Path, scenario: &str) -> (Vec<Value>, Value, String) {
    let out = cox(home, cwd)
        .env("COX_SCENARIO", scenario)
        .args(["run", "-p", "go", "--output-format", "stream-json"])
        .output()
        .unwrap();
    let events: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let result = events
        .iter()
        .find(|e| e["type"] == "tool_call_done")
        .map(|e| e["result"].clone())
        .expect("the agent call finished");
    (events, result, String::from_utf8(out.stderr).unwrap())
}

#[test]
fn granted_plugin_agent_is_dispatchable() {
    let (home, cwd, root) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (home, cwd) = (home.path(), cwd.path());
    let (pkg, scenario) = agent_package(root.path());
    stdout(cox(home, cwd).args(["plugin", "install", &pkg, "--yes"]));

    let (_, result, _) = agent_run(home, cwd, &scenario);
    assert_eq!(result["ok"], true, "{result}");
    let tools = result["visible"].as_str().unwrap();
    assert!(tools.contains("grep") && !tools.contains("glob"), "{tools}");
}

#[test]
fn ungranted_plugin_agent_is_not_loaded() {
    let (home, cwd, root) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (home, cwd) = (home.path(), cwd.path());
    let (pkg, scenario) = agent_package(root.path());
    // Installed, but the approval is answered "no": no grant row.
    cox(home, cwd)
        .args(["plugin", "install", &pkg])
        .write_stdin("n\n")
        .assert()
        .success();

    let (events, result, _) = agent_run(home, cwd, &scenario);
    assert_eq!(result["ok"], false, "{result}");
    assert!(
        events.iter().any(|e| e["type"] == "notice"
            && e["text"]
                .as_str()
                .is_some_and(|t| t.contains("plugin review-kit is not loaded"))),
        "{events:#?}"
    );
}

#[test]
fn local_agent_wins_over_plugin_agent() {
    let (home, cwd, root) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (home, cwd) = (home.path(), cwd.path());
    let (pkg, scenario) = agent_package(root.path());
    stdout(cox(home, cwd).args(["plugin", "install", &pkg, "--yes"]));
    let local = cwd.join(".cox/agents");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(
        local.join("reviewer.md"),
        "---\nname: reviewer\ndescription: reviews locally\ntools: [glob]\n---\nReview it.\n",
    )
    .unwrap();

    let (_, result, stderr) = agent_run(home, cwd, &scenario);
    let tools = result["visible"].as_str().unwrap();
    assert!(tools.contains("glob") && !tools.contains("grep"), "{tools}");
    // The skip is a session warning (`Warning::Agent`), like every other
    // agent-definition caveat, so the headless run prints it on stderr.
    assert!(
        stderr.contains("cox: warning: plugin review-kit: agent reviewer skipped"),
        "{stderr}"
    );
}
