// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T35.7: a test-only stand-in for Cursor's `agent` CLI, so
//! `tests/external_agents_cursor.rs` drives the real plugin, sandbox and
//! driver path with no network and no Cursor account. It replays a fixture
//! `cox-vendor cursor-fixtures` wrote from Cursor's and ACP's documented
//! examples (`tests/fixtures/cursor/*.json`) over stdio. A `[[bin]]` of
//! this crate only because `CARGO_BIN_EXE_*` exposes nothing else to an
//! integration test; `Cargo.toml` keeps it out of the release archives.
//!
//! Invoked as `agent --fixture <file> <the mode's own args>`: the fixture's
//! mode must match those args, so a host that invokes the CLI the wrong way
//! fails here rather than passing by accident.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fake agent: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [flag, path, rest @ ..] = args.as_slice() else {
        return Err("usage: agent --fixture <file> <args...>".into());
    };
    if flag != "--fixture" {
        return Err(format!("expected --fixture, got {flag}"));
    }
    // The real CLI refuses to start unauthenticated; the host must hand the
    // key over through `key_env` even though nothing here checks its value.
    if std::env::var_os("CURSOR_API_KEY").is_none() {
        return Err("CURSOR_API_KEY is not set".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let fixture: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let args: Vec<&str> = rest.iter().map(String::as_str).collect();
    match (fixture["mode"].as_str(), args.as_slice()) {
        // The host appends the prompt as the last argument (EA§5).
        (Some("stream-json"), ["-p", "--output-format", "stream-json", _prompt]) => {
            let mut out = std::io::stdout().lock();
            for line in fixture["lines"].as_array().into_iter().flatten() {
                let line = line.as_str().ok_or("a stream-json line is not a string")?;
                writeln!(out, "{line}").map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        (Some("acp"), ["acp"]) => acp(&fixture),
        (mode, args) => Err(format!(
            "fixture mode {mode:?} does not match args {args:?}"
        )),
    }
}

/// Answers each client request with the fixture's documented result. While
/// `session/prompt` runs it first sends `prompt_turn` in order; a request
/// there waits for the client's answer, which is then reported back as one
/// more message chunk (built from the documented chunk) so the test sees
/// what cox decided.
fn acp(fixture: &Value) -> Result<(), String> {
    let mut lines = std::io::stdin().lock().lines();
    let mut out = std::io::stdout().lock();
    let turn = fixture["prompt_turn"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let chunk = turn
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk");
    while let Some(msg) = next(&mut lines)? {
        let (Some(method), Some(id)) = (msg["method"].as_str(), msg.get("id")) else {
            continue; // a notification such as `session/cancel`
        };
        if method == "session/prompt" {
            for m in &turn {
                send(&mut out, m)?;
                if m.get("id").is_none() {
                    continue;
                }
                let answer = loop {
                    let reply = next(&mut lines)?.ok_or("stdin closed before the answer")?;
                    if reply.get("method").is_none() && reply["id"] == m["id"] {
                        break reply;
                    }
                };
                if let Some(chunk) = chunk {
                    let outcome = &answer["result"]["outcome"];
                    let picked = match outcome["outcome"].as_str() {
                        Some("selected") => &outcome["optionId"],
                        _ => &outcome["outcome"],
                    };
                    let mut report = chunk.clone();
                    let text = format!(" [{}: {}]", m["method"].as_str().unwrap_or(""), picked);
                    report["params"]["update"]["content"]["text"] = Value::String(text);
                    send(&mut out, &report)?;
                }
            }
        }
        let reply = match fixture["responses"].get(method) {
            Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            None => json!({ "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("{method} is not in the fixture") } }),
        };
        send(&mut out, &reply)?;
    }
    Ok(())
}

fn next(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) -> Result<Option<Value>, String> {
    match lines.next() {
        None => Ok(None),
        Some(line) => {
            let line = line.map_err(|e| e.to_string())?;
            serde_json::from_str(&line)
                .map(Some)
                .map_err(|e| format!("{line}: {e}"))
        }
    }
}

fn send(out: &mut impl Write, msg: &Value) -> Result<(), String> {
    writeln!(out, "{msg}")
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}
