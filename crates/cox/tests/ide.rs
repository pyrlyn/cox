// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T11.2: the Zed `agent_servers` snippet in `docs/ide.md` is valid JSON
//! and points at `cox acp`. T37.2: `cox acp` opens its sessions through
//! `cox-session`, so an IDE gets the same tools as `cox run -p`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

use serde_json::{Value, json};

/// First ```json fenced block in the doc.
fn snippet() -> String {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ide.md"),
    )
    .expect("docs/ide.md");
    let start = doc.find("```json").expect("json fence") + "```json".len();
    let end = doc[start..].find("```").expect("fence end") + start;
    doc[start..end].to_string()
}

#[test]
fn ide_zed_snippet_is_valid_json_for_cox_acp() {
    let value: serde_json::Value = serde_json::from_str(&snippet()).expect("valid json");
    let cox = &value["agent_servers"]["cox"];
    assert_eq!(cox["type"], "custom");
    assert_eq!(cox["command"], "cox");
    assert_eq!(cox["args"], serde_json::json!(["acp"]));
}

/// A scenario whose one turn answers with the offered tool names.
fn echo_tools_scenario(dir: &Path) -> String {
    let path = dir.join("echo_tools.toml");
    std::fs::write(&path, "[[turn]]\necho_tools = true\n").unwrap();
    path.to_str().unwrap().to_string()
}

fn cox(work: &Path, home: &Path, scenario: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(work)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", scenario);
    cmd
}

/// Drives `cox acp` over newline-delimited JSON-RPC on its stdio through
/// `initialize`, `session/new` and one `session/prompt` of `prompt`'s
/// content blocks, and returns the `initialize` result and the agent's
/// streamed text.
fn acp_prompt(work: &Path, home: &Path, scenario: &str, prompt: Value) -> (Value, String) {
    let mut child = cox(work, home, scenario)
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut text = String::new();
    let mut request = |id: u64, method: &str, params: Value, text: &mut String| -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        loop {
            let line = lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("no response to `{method}`"));
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                assert!(msg["error"].is_null(), "{method} failed: {msg}");
                return msg["result"].clone();
            }
            let update = &msg["params"]["update"];
            if update["sessionUpdate"] == "agent_message_chunk" {
                text.push_str(update["content"]["text"].as_str().unwrap_or_default());
            }
        }
    };
    let init = request(
        1,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
        &mut text,
    );
    let session = request(
        2,
        "session/new",
        json!({"cwd": work, "mcpServers": []}),
        &mut text,
    );
    let id = session["sessionId"].clone();
    request(
        3,
        "session/prompt",
        json!({"sessionId": id, "prompt": prompt}),
        &mut text,
    );
    let _ = child.kill();
    let _ = child.wait();
    (init, text)
}

/// Gives `home` one MCP server — cox's own `cox mcp` — whose tools are
/// not deferred, so they show in the request's tool list.
fn with_mcp_server(home: &Path) {
    let config = format!(
        "[mcp]\ndeferred = false\n\n[mcp.servers.self]\ncommand = {:?}\nargs = [\"mcp\"]\nsandbox = false\n",
        env!("CARGO_BIN_EXE_cox"),
    );
    std::fs::write(home.join("config.toml"), config).unwrap();
}

#[test]
fn acp_session_offers_the_same_tools_as_run_p() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let scenario = echo_tools_scenario(home.path());
    with_mcp_server(home.path());
    let out = cox(work.path(), home.path(), &scenario)
        .args(["--cwd", work.path().to_str().unwrap(), "run", "-p", "hi"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let run_p = String::from_utf8(out.stdout).unwrap();
    let run_p: Vec<&str> = run_p.lines().collect();
    // An MCP server's tools: an ACP session built on its own lacked them.
    for name in ["read", "bash", "mcp__self__read"] {
        assert!(run_p.contains(&name), "{name} missing from {run_p:?}");
    }
    let hi = json!([{"type": "text", "text": "hi"}]);
    let (_, acp) = acp_prompt(work.path(), home.path(), &scenario, hi);
    assert_eq!(acp.lines().collect::<Vec<_>>(), run_p);
}

/// Every event of every rollout under `home` (`$COX_HOME/sessions/<id>.jsonl`,
/// one `{"seq", "ts", "event"}` line each), unwrapped.
fn rollout_events(home: &Path) -> Vec<Value> {
    let mut events = Vec::new();
    for entry in std::fs::read_dir(home.join("sessions")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let text = std::fs::read_to_string(&path).unwrap();
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let line: Value = serde_json::from_str(line).unwrap();
                events.push(line["event"].clone());
            }
        }
    }
    events
}

/// T40.8: `initialize` advertises image prompts, and an image block in
/// `session/prompt` reaches the scripted provider as an image: the rollout
/// records it on the user message (only what was sent is recorded, T40.2),
/// and the call's usage, the provider's own estimate of the request it
/// received, prices it at `IMAGE_TOKEN_ESTIMATE` (T40.3).
#[test]
fn acp_image_block_reaches_the_provider_as_an_image() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let scenario = home.path().join("seen.toml");
    std::fs::write(&scenario, "[[turn]]\ntext = \"seen\"\n").unwrap();
    // `\x89PNG\r\n\x1a\n` in base64.
    let png = "iVBORw0KGgo=";
    let prompt = json!([
        {"type": "text", "text": "look"},
        {"type": "image", "data": png, "mimeType": "image/png"},
    ]);
    let (init, text) = acp_prompt(work.path(), home.path(), scenario.to_str().unwrap(), prompt);
    assert_eq!(
        init["agentCapabilities"]["promptCapabilities"]["image"],
        true
    );
    assert_eq!(text, "seen");
    let events = rollout_events(home.path());
    let user = events
        .iter()
        .find(|e| e["type"] == "item_started" && e["kind"]["type"] == "user_message")
        .expect("the user message");
    assert_eq!(user["kind"]["attachments"][0]["media_type"], "image/png");
    assert_eq!(user["kind"]["attachments"][0]["data_b64"], png);
    let usage = events
        .iter()
        .find(|e| e["type"] == "usage")
        .expect("a usage event");
    let input = usage["usage"]["input_tokens"].as_u64().unwrap();
    assert!(
        input >= cox_protocol::image::IMAGE_TOKEN_ESTIMATE,
        "{usage}"
    );
}
