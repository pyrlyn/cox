// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Tool summaries (DT§4.3 "Summaries"): a call's one-line description, its
//! icon key and whether it explores the tree. Written in Rust from the call's
//! input and the result's data (`ToolResult.structured`, the diff) so no UI
//! parses tool output, and every surface says the same thing. Separate from
//! the fold because it is a pure function of one call.

use cox_protocol::types::{TodoState, ToolCall, ToolResult};
use cox_render::diffstat;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a UI draws in front of the summary; each maps to one symbol there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Icon {
    Read,
    Edit,
    Shell,
    Search,
    Web,
    Todo,
    Ask,
    Agent,
    Mcp,
    Tool,
}

pub fn icon(tool: &str) -> Icon {
    match tool {
        "read" | "outline" => Icon::Read,
        "edit" | "write" | "apply_patch" => Icon::Edit,
        "bash" => Icon::Shell,
        "grep" | "glob" => Icon::Search,
        "web_fetch" => Icon::Web,
        "todo" => Icon::Todo,
        "ask_user" => Icon::Ask,
        "agent" => Icon::Agent,
        t if t.starts_with("mcp__") => Icon::Mcp,
        _ => Icon::Tool,
    }
}

/// What an exploring call looked at, for a `ToolGroup`'s count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Explore {
    File(String),
    Search,
}

/// `Some` for the calls that fold into a `ToolGroup` (read, grep, glob,
/// outline).
pub fn explore(call: &ToolCall) -> Option<Explore> {
    match call.name.as_str() {
        "read" | "outline" => Some(Explore::File(field(call, "path"))),
        "grep" | "glob" => Some(Explore::Search),
        _ => None,
    }
}

/// "Explored 2 files, 1 search".
pub fn explored(files: usize, searches: usize) -> String {
    let parts: Vec<String> = [(files, "file", "files"), (searches, "search", "searches")]
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| plural(n as u64, one, many))
        .collect();
    format!("Explored {}", parts.join(", "))
}

/// The call's line: present tense while `result` is `None`, past tense
/// with what the result reports once it is done.
pub fn summary(call: &ToolCall, result: Option<&ToolResult>) -> String {
    let done = result.is_some();
    let tense = |running: &str, past: &str| if done { past } else { running }.to_owned();
    let data = |key: &str| result.and_then(|r| r.structured.as_deref()?.get(key)?.as_u64());
    let target = field(call, "path");
    let mut line = match call.name.as_str() {
        "bash" => {
            let mut line = format!("{} `{}`", tense("Running", "Ran"), field(call, "command"));
            if let Some(r) = result {
                match data("exit_code") {
                    Some(code) => line += &format!(" — exit {code}"),
                    None if !r.ok => line += " — stopped",
                    None => {}
                }
                line += &format!(" · {}", seconds(r.duration_ms));
            }
            return line;
        }
        "read" | "outline" => {
            let outline = call.name == "outline" || field_opt(call, "mode") == Some("outline");
            let verb = match outline {
                true => tense("Outlining", "Outlined"),
                false => tense("Reading", "Read"),
            };
            let mut line = format!("{verb} `{target}`");
            match (field_opt(call, "lines"), data("lines")) {
                (Some(range), _) => line += &format!(" · lines {range}"),
                (None, Some(n)) => line += &format!(" · {}", plural(n, "line", "lines")),
                (None, None) => {}
            }
            line
        }
        "grep" | "glob" => {
            let mut line = format!(
                "{} `{}`",
                tense("Searching", "Searched"),
                field(call, "pattern")
            );
            if let Some(n) = data("matches") {
                line += &format!(" · {}", plural(n, "match", "matches"));
            } else if let Some(n) = data("files") {
                line += &format!(" · {}", plural(n, "file", "files"));
            }
            line
        }
        "edit" | "apply_patch" => {
            let mut line = format!(
                "{} `{}`",
                tense("Editing", "Edited"),
                one_line(&call.subject)
            );
            if let Some(diff) = result.and_then(|r| r.diff.as_ref()) {
                let (added, removed) = diffstat::counts(&diff.unified);
                line += &format!(" +{added} −{removed}");
            }
            line
        }
        "write" => {
            let lines = field_opt(call, "content").map_or(0, |c| c.lines().count());
            let lines = plural(lines as u64, "line", "lines");
            format!("{} `{target}` · {lines}", tense("Writing", "Wrote"))
        }
        "web_fetch" => {
            let mut line = format!("{} `{}`", tense("Fetching", "Fetched"), field(call, "url"));
            if let Some(r) = result.filter(|r| r.ok) {
                line += &format!(" · {}", size(r.bytes));
            }
            line
        }
        "todo" => {
            let mut line = tense("Updating todos", "Updated todos");
            if let Some(items) = result.and_then(ToolResult::todo_list) {
                let finished = items.iter().filter(|i| i.state == TodoState::Done).count();
                line += &format!(" · {finished}/{} done", items.len());
            }
            line
        }
        "ask_user" => format!("{}: {}", tense("Asking", "Asked"), field(call, "question")),
        "agent" => format!(
            "{} to `{}`: {}",
            tense("Delegating", "Delegated"),
            one_line(&call.subject),
            field(call, "task")
        ),
        name => match name.strip_prefix("mcp__").and_then(|s| s.split_once("__")) {
            Some((server, tool)) => format!("{} `{server}:{tool}`", tense("Calling", "Called")),
            None if call.subject.is_empty() || call.subject == name => name.to_owned(),
            None => format!("{name} `{}`", one_line(&call.subject)),
        },
    };
    if result.is_some_and(|r| !r.ok) {
        line += " — failed";
    }
    line
}

/// A multi-line value (a heredoc command, a whole echoed text) shows its
/// first line; the full input is in the call.
pub fn one_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn field_opt<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    call.input.get(key).and_then(Value::as_str)
}

/// The input field's first line, or the subject's when the input lacks it.
fn field(call: &ToolCall, key: &str) -> String {
    one_line(field_opt(call, key).unwrap_or(&call.subject))
}

pub(crate) fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "840 ms", "4.2 s".
pub(crate) fn seconds(ms: u64) -> String {
    match ms {
        0..1000 => format!("{ms} ms"),
        _ => format!("{:.1} s", ms as f64 / 1000.0),
    }
}

/// "512 B", "12 KB", "3.4 MB".
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::CallId;
    use cox_protocol::types::{Diff, Risk};
    use serde_json::json;

    use super::*;

    fn call(name: &str, input: Value, subject: &str) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: name.into(),
            input,
            risk: Risk::ReadOnly,
            subject: subject.into(),
            segments: None,
        }
    }

    fn result(ok: bool, structured: Option<Value>) -> ToolResult {
        ToolResult {
            ok,
            visible: String::new(),
            archive: None,
            bytes: 12_800,
            duration_ms: 4_210,
            diff: None,
            structured: structured.map(Box::new),
        }
    }

    #[test]
    fn done_summaries_read_the_result_data_not_its_text() {
        let bash = call("bash", json!({"command": "cargo test\nmore"}), "cargo test");
        let read = call("read", json!({"path": "a.rs"}), "a.rs");
        let range = call("read", json!({"path": "a.rs", "lines": "3-5"}), "a.rs");
        let grep = call("grep", json!({"pattern": "fn main"}), ".");
        let glob = call("glob", json!({"pattern": "**/*.rs"}), "**/*.rs");
        let write = call(
            "write",
            json!({"path": "b.rs", "content": "a\nb\n"}),
            "b.rs",
        );
        let web = call(
            "web_fetch",
            json!({"url": "https://x.dev"}),
            "https://x.dev",
        );
        let mcp = call("mcp__github__search", json!({}), "mcp__github__search");
        let agent = call("agent", json!({"task": "find x"}), "explore");
        let lines = result(true, Some(json!({"lines": 120})));
        let cases = [
            (
                &bash,
                result(true, Some(json!({"exit_code": 0}))),
                "Ran `cargo test` — exit 0 · 4.2 s",
            ),
            (
                &bash,
                result(false, Some(json!({"exit_code": null}))),
                "Ran `cargo test` — stopped · 4.2 s",
            ),
            (&read, lines.clone(), "Read `a.rs` · 120 lines"),
            (&range, lines, "Read `a.rs` · lines 3-5"),
            (
                &grep,
                result(true, Some(json!({"matches": 1}))),
                "Searched `fn main` · 1 match",
            ),
            (
                &glob,
                result(true, Some(json!({"files": 7}))),
                "Searched `**/*.rs` · 7 files",
            ),
            (&write, result(true, None), "Wrote `b.rs` · 2 lines"),
            (&web, result(true, None), "Fetched `https://x.dev` · 12 KB"),
            (&mcp, result(false, None), "Called `github:search` — failed"),
            (&agent, result(true, None), "Delegated to `explore`: find x"),
        ];
        for (call, result, want) in cases {
            assert_eq!(summary(call, Some(&result)), want);
        }
        assert_eq!(summary(&read, None), "Reading `a.rs`");
    }

    #[test]
    fn edit_summary_counts_the_diff_lines() {
        let edit = call("edit", json!({"path": "crates/x.rs"}), "crates/x.rs");
        let mut done = result(true, None);
        done.diff = Some(Diff {
            path: "crates/x.rs".into(),
            unified: "--- a\n+++ b\n@@ -1 +1,2 @@\n-old\n+new\n+more\n".into(),
        });
        assert_eq!(summary(&edit, Some(&done)), "Edited `crates/x.rs` +2 −1");
        assert_eq!(icon(&edit.name), Icon::Edit);
    }

    #[test]
    fn explored_names_files_and_searches() {
        assert_eq!(explored(2, 1), "Explored 2 files, 1 search");
        assert_eq!(explored(1, 0), "Explored 1 file");
    }
}
