// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! v0.1 DoD §4.5: `cox mcp` serves the real `read`/`grep`/`glob` to an MCP
//! client. `cox-mcp`'s own server test uses stand-in tools; this one spawns
//! the built binary and talks newline-delimited JSON-RPC over its stdio,
//! the way Claude Code does, so the built-in tools and the gate are the
//! shipped ones.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Client {
    fn spawn(work: &Path, home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cox"))
            .current_dir(work)
            .env("COX_HOME", home)
            .env("HOME", home)
            .args(["--cwd", work.to_str().unwrap(), "mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and waits for the response carrying its id; a
    /// server that never answers fails the test instead of hanging it.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("no response to `{method}`"));
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    }

    fn call(&mut self, id: u64, tool: &str, args: Value) -> String {
        let resp = self.request(id, "tools/call", json!({"name": tool, "arguments": args}));
        assert_ne!(resp["result"]["isError"], true, "{tool} failed: {resp}");
        resp["result"]["content"]
            .as_array()
            .unwrap_or_else(|| panic!("{tool}: no content in {resp}"))
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cox_mcp_serves_read_grep_and_glob_from_the_built_binary() {
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("notes.txt"), "alpha\nneedle here\n").unwrap();
    std::fs::create_dir(work.path().join("src")).unwrap();
    std::fs::write(work.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

    let mut client = Client::spawn(work.path(), home.path());
    let init = client.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cox-e2e", "version": "0"}
        }),
    );
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let list = client.request(2, "tools/list", json!({}));
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["glob", "grep", "read"]);

    let read = client.call(3, "read", json!({"path": "notes.txt"}));
    assert!(read.contains("needle here"), "{read}");
    let grep = client.call(4, "grep", json!({"pattern": "needle"}));
    assert!(grep.contains("notes.txt"), "{grep}");
    let glob = client.call(5, "glob", json!({"pattern": "**/*.rs"}));
    assert!(glob.contains("lib.rs"), "{glob}");
}
