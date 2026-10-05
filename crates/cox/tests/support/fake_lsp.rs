// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T41.8: a test-only language server, so `tests/lsp.rs` drives the real
//! `diagnostics` tool, its sandbox wrap and its shutdown with no real server
//! installed. It speaks just enough LSP over stdio: `initialize` answers
//! full-text sync, and every `didOpen`/`didSave` publishes one error at 1:1
//! reading `fake: <the file's first line>`. A `[[bin]]` of this crate only
//! because `CARGO_BIN_EXE_*` exposes nothing else to an integration test;
//! `Cargo.toml` keeps it out of the release archives.
//!
//! Invoked as `fake_lsp [--pid-file <path>]`. The pid file is an argument,
//! not an env var, because cox starts a server with the child env allowlist
//! only. It exits on `exit` or at the end of stdin.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fake lsp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [flag, path] if flag == "--pid-file" => {
            std::fs::write(path, std::process::id().to_string())
                .map_err(|e| format!("{path}: {e}"))?;
        }
        _ => return Err("usage: fake_lsp [--pid-file <path>]".into()),
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut out = std::io::stdout().lock();
    // The text of every open document, by URI.
    let mut docs: HashMap<String, String> = HashMap::new();
    while let Some(msg) = read(&mut input)? {
        let method = msg["method"].as_str().unwrap_or_default();
        let doc = &msg["params"]["textDocument"];
        let uri = doc["uri"].as_str().unwrap_or_default().to_string();
        match method {
            "initialize" => {
                let caps = json!({"capabilities": {"textDocumentSync": 1}});
                write(
                    &mut out,
                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": caps}),
                )?;
            }
            "textDocument/didOpen" => {
                docs.insert(uri.clone(), doc["text"].as_str().unwrap_or_default().into());
                publish(&mut out, &uri, &docs)?;
            }
            "textDocument/didChange" => {
                let text = msg["params"]["contentChanges"][0]["text"].as_str();
                docs.insert(uri, text.unwrap_or_default().into());
            }
            "textDocument/didSave" => publish(&mut out, &uri, &docs)?,
            "exit" => return Ok(()),
            _ if msg.get("id").is_some() => {
                // `shutdown`, and any other request, gets a `null` result.
                write(
                    &mut out,
                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}),
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn publish(out: &mut impl Write, uri: &str, docs: &HashMap<String, String>) -> Result<(), String> {
    let text = docs.get(uri).map(String::as_str).unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    let diagnostic = json!({
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
        "severity": 1,
        "message": format!("fake: {first}"),
    });
    let params = json!({"uri": uri, "diagnostics": [diagnostic]});
    let note =
        json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params});
    write(out, &note)
}

/// One `Content-Length` framed message; `None` at the end of input.
fn read(input: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut len = None;
    loop {
        let mut line = String::new();
        let n = input.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(value.trim().parse::<usize>().map_err(|e| e.to_string())?);
        }
    }
    let len = len.ok_or("a message without Content-Length")?;
    let mut body = vec![0; len];
    input.read_exact(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn write(out: &mut impl Write, msg: &Value) -> Result<(), String> {
    let body = msg.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}
