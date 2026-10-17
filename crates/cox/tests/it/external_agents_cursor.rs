// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T35.7 Check: Cursor's `agent` CLI end to end, with the test-only
//! `fake_agent` binary (`tests/support/fake_agent.rs`) standing in for it
//! and replaying the documented shapes `cox-vendor cursor-fixtures` wrote to
//! `tests/fixtures/cursor/`. Everything else is the real binary: `cox
//! plugin install --yes` grants a plugin shaped like `plugins/cursor`
//! (`command = "agent"` found on `PATH`, `key_env = "CURSOR_API_KEY"`), the
//! session wraps and spawns it under the sandbox, and the stream-json
//! mapper (`cox-core`) or the ACP client (`cox-acp`) reads it. Only the
//! parent model is scripted, to issue `agent(preset: "cursor")`. No network;
//! the key is a fake value.

#![cfg(all(feature = "plugins", unix))]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/external_agent_cursor.toml"
);

/// A module whose `cox_init` answers `{}` (the default `InitOut`): the
/// plugin's own code plays no part in driving the CLI (EA§2).
const WAT: &str = r#"(module
  (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
  (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
  (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
  (func (export "cox_init") (result i32) (local $p i64)
    (local.set $p (call $alloc (i64.const 2)))
    (call $store (local.get $p) (i32.const 123))
    (call $store (i64.add (local.get $p) (i64.const 1)) (i32.const 125))
    (call $output_set (local.get $p) (i64.const 2))
    (i32.const 0)))"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cursor")
        .join(name)
}

fn read_fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(fixture(name)).expect("fixture");
    serde_json::from_str(&text).expect("fixture json")
}

fn quoted(items: &[&str]) -> String {
    let items: Vec<String> = items.iter().map(|a| format!("{a:?}")).collect();
    items.join(", ")
}

/// A scratch `COX_HOME` with the plugin granted, a workspace, and a `PATH`
/// dir whose `agent` is the fake.
struct Rig {
    home: TempDir,
    work: TempDir,
    bin: TempDir,
}

impl Rig {
    /// `args` are the manifest's (the mode's own invocation); `allow` the
    /// user config's `[permissions] allow` rules.
    fn new(mode: &str, fixture_name: &str, args: &[&str], allow: &[&str]) -> Rig {
        let dir = || tempfile::tempdir().expect("tempdir");
        // The `PATH` dir lives outside `/tmp`: Linux bwrap gives the wrapped
        // CLI a private `/tmp`, so an `agent` there would not exist for it.
        // A real CLI is never installed under `/tmp`.
        let bin = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
        let (home, work, pkg) = (dir(), dir(), dir());
        let rig = Rig { home, work, bin };
        let agent = rig.bin.path().join("agent");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake_agent"), agent).expect("agent");
        let fx = fixture(fixture_name).display().to_string();
        let args = quoted(&[&["--fixture", fx.as_str()], args].concat());
        let manifest = format!(
            "api = 1\nid = \"cursor\"\nversion = \"0.1.0\"\nname = \"Cursor\"\n\
             wasm = \"plugin.wasm\"\n\n[[external_agents]]\nname = \"cursor\"\n\
             command = \"agent\"\nargs = [{args}]\nmode = \"{mode}\"\n\
             key_env = \"CURSOR_API_KEY\"\n"
        );
        std::fs::write(pkg.path().join("plugin.toml"), manifest).expect("manifest");
        std::fs::write(pkg.path().join("plugin.wasm"), WAT).expect("wasm");
        let config = format!("[permissions]\nallow = [{}]\n", quoted(allow));
        std::fs::write(rig.home.path().join("config.toml"), config).expect("config");
        let pkg = pkg.path().to_str().expect("utf-8 path");
        rig.cox(&["plugin", "install", pkg, "--yes"])
            .assert()
            .success();
        rig
    }

    fn cox(&self, args: &[&str]) -> Command {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let path =
            std::iter::once(self.bin.path().to_path_buf()).chain(std::env::split_paths(&path));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(self.work.path())
            .env("COX_HOME", self.home.path())
            .env("HOME", self.home.path())
            .env("PATH", std::env::join_paths(path).expect("PATH"))
            .env("COX_PROVIDER", "scripted")
            .env("COX_SCENARIO", SCENARIO)
            .env("CURSOR_API_KEY", "not-a-real-key")
            .args(["--cwd", self.work.path().to_str().expect("utf-8 path")])
            .args(args);
        cmd
    }

    /// The parent's run as stream-json events.
    fn run(&self) -> Vec<Value> {
        let out = self
            .cox(&["run", "-p", "go", "--output-format", "stream-json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let events = json_lines(&String::from_utf8(out).expect("utf-8"));
        let warned = events
            .iter()
            .filter(|e| e["type"] == "notice" && e["level"] == "warn");
        assert_eq!(warned.count(), 0, "{events:#?}");
        events
    }

    /// The external agent's own session: its rollout is the cox event
    /// stream the driver fed, beside the parent's (`parent` is its
    /// `session_started`).
    fn child_rollout(&self, parent: &Value) -> Vec<Value> {
        let parent = parent["session"].as_str().expect("parent session id");
        let dir = self.home.path().join("sessions");
        let mut children = std::fs::read_dir(dir)
            .expect("sessions dir")
            .map(|e| e.expect("entry").path())
            .filter(|p| p.file_stem().and_then(|s| s.to_str()) != Some(parent));
        let child = children.next().expect("the external agent's rollout");
        assert!(children.next().is_none(), "one child session");
        let text = std::fs::read_to_string(child).expect("rollout");
        json_lines(&text)
            .into_iter()
            .map(|r| r["event"].clone())
            .collect()
    }
}

fn json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// The parent's `agent` call result: what the external agent answered.
fn agent_result(events: &[Value]) -> &str {
    let done = events.iter().find(|e| e["type"] == "tool_call_done");
    done.and_then(|e| e["result"]["visible"].as_str())
        .expect("the agent call finished")
}

/// T35.7 Check: every documented stream-json line reaches the external
/// agent's cox event stream with its content intact — each assistant text,
/// each tool call's name and args, each tool result, the model and mode of
/// `init` — in order, ending the turn the way `result` says; and the last
/// assistant text is what the parent's `agent` call returns.
#[test]
fn fake_agent_stream_json_reaches_a_cox_event_stream_unchanged() {
    let args = ["-p", "--output-format", "stream-json"];
    let rig = Rig::new("stream-json", "stream_json.json", &args, &["Agent"]);
    let events = rig.run();
    let fx = read_fixture("stream_json.json");
    let lines: Vec<Value> = fx["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| serde_json::from_str(l.as_str().expect("line")).expect("documented json"))
        .collect();

    // (what, content) per documented line, and per event cox recorded for it.
    let text = |v: &Value| ("text".to_string(), v.clone());
    let want: Vec<(String, Value)> = lines
        .iter()
        .filter_map(|l| {
            let tool = l["tool_call"].as_object().and_then(|o| o.iter().next());
            match (l["type"].as_str()?, l["subtype"].as_str(), tool) {
                ("assistant", _, _) => Some(text(&l["message"]["content"][0]["text"])),
                ("tool_call", Some("started"), Some((key, call))) => {
                    let name = key.trim_end_matches("ToolCall");
                    Some((format!("call {name}"), call["args"].clone()))
                }
                ("tool_call", Some("completed"), Some((_, call))) => {
                    Some(("result".into(), call["result"].clone()))
                }
                ("result", _, _) => Some(("turn".into(), "end_turn".into())),
                _ => None,
            }
        })
        .collect();
    assert_eq!(want.len(), lines.len() - 2, "all but `init` and `user`");
    let child = rig.child_rollout(&events[0]);
    let got: Vec<(String, Value)> = child
        .iter()
        .filter_map(
            |e| match (e["type"].as_str()?, e["kind"]["type"].as_str()) {
                ("item_started", Some("assistant_message")) => Some(text(&e["kind"]["text"])),
                ("item_started", Some("tool_call")) => {
                    let call = &e["kind"]["call"];
                    Some((
                        format!("call {}", call["name"].as_str()?),
                        call["input"].clone(),
                    ))
                }
                // cox carries a documented result object as its JSON text.
                ("tool_call_done", _) => {
                    let visible = e["result"]["visible"].as_str()?;
                    Some(("result".into(), serde_json::from_str(visible).ok()?))
                }
                ("turn_done", _) => Some(("turn".into(), e["stop"]["type"].clone())),
                _ => None,
            },
        )
        .collect();
    assert_eq!(got, want);

    let init = &lines[0];
    let notice = child
        .iter()
        .find(|e| e["type"] == "notice")
        .expect("init notice");
    let text = notice["text"].as_str().expect("notice text");
    assert!(
        text.contains(init["model"].as_str().expect("model")),
        "{text}"
    );
    assert!(
        text.contains(init["permissionMode"].as_str().expect("mode")),
        "{text}"
    );

    let last = lines.iter().rev().find(|l| l["type"] == "assistant");
    let last = last.and_then(|l| l["message"]["content"][0]["text"].as_str());
    assert_eq!(Some(agent_result(&events)), last);
}

/// T35.7 Check: the documented `session/request_permission` (a tool call
/// with no kind, so judged as `bash`) is answered by the session's engine,
/// compiled from `[permissions]`: an `Ask` fails closed to the documented
/// reject option, an `allow` rule picks the allow-once option. The fake
/// reports the option it got after the documented message chunk.
#[test]
fn fake_agent_acp_permission_request_is_decided_by_the_engine() {
    let fx = read_fixture("acp.json");
    let said = fx["prompt_turn"][0]["params"]["update"]["content"]["text"]
        .as_str()
        .expect("documented chunk");

    for (allow, picked) in [
        (&["Agent"][..], "reject-once"),
        (&["Agent", "Bash"][..], "allow-once"),
    ] {
        let rig = Rig::new("acp", "acp.json", &["acp"], allow);
        let events = rig.run();
        let want = format!("{said} [session/request_permission: \"{picked}\"]");
        assert_eq!(agent_result(&events), want, "allow = {allow:?}");
    }
}
