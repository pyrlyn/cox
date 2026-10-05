// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Tools that execute through the ACP client instead of the local machine
//! (T11.1 step 4): `read`/`edit`/`write` through `fs/*` so the editor's
//! buffers stay authoritative, `bash` through `terminal/*`. Same names,
//! subjects and risk classes as the local tools they replace, so prompts,
//! transcripts and permission rules read identically; only the scholar's
//! hand changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CreateTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest, SessionId,
    TerminalOutputRequest, WaitForTerminalExitRequest, WriteTextFileRequest,
};
use agent_client_protocol::{Client, ConnectionTo};
use async_trait::async_trait;
use cox_protocol::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use serde_json::Value;

/// How to reach the client on behalf of one ACP session. Cloned into every
/// proxy tool of that session at `session/new` time.
#[derive(Clone, Debug)]
pub struct ClientLink {
    /// The agent-side connection; requests go to the counterpart (client).
    pub conn: ConnectionTo<Client>,
    /// The ACP session these calls belong to.
    pub session: SessionId,
    /// The session's working directory (relatives resolve against it).
    pub cwd: PathBuf,
}

/// Absolute path for a client RPC: relatives resolve against the session cwd.
fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

fn failed(what: &str, err: impl std::fmt::Display) -> ToolOutput {
    ToolOutput {
        text: format!("{what} failed: {err}"),
        is_error: true,
        diff: None,
        structured: None,
    }
}

/// `read` through `fs/read_text_file`. `mode = "outline"` fetches the text
/// and outlines it with the same keyword fallback cox uses for grammars it
/// does not ship; tree-sitter lives in `cox-tools`, which this crate may
/// not depend on (plan.md dependency direction).
pub struct FsReadTool {
    link: Arc<ClientLink>,
}

/// `edit` through `fs/read_text_file` + `fs/write_text_file`: exact
/// string replacement applied agent-side. No `apply_patch`: the client
/// offers no patch RPC.
pub struct FsEditTool {
    link: Arc<ClientLink>,
}

/// `write` through `fs/write_text_file`.
pub struct FsWriteTool {
    link: Arc<ClientLink>,
}

/// `bash` through `terminal/create` + `terminal/wait_for_exit` +
/// `terminal/output` + `terminal/release`. Foreground only: a detached
/// terminal has nowhere to report back to, so `background: true` is a clear
/// error rather than a silent drop.
pub struct TerminalBashTool {
    link: Arc<ClientLink>,
}

impl FsReadTool {
    /// Serves `read` from the client's buffers.
    pub fn new(link: Arc<ClientLink>) -> Self {
        Self { link }
    }
}

impl FsEditTool {
    /// Serves `edit` through the client's buffers.
    pub fn new(link: Arc<ClientLink>) -> Self {
        Self { link }
    }
}

impl FsWriteTool {
    /// Serves `write` through the client's buffers.
    pub fn new(link: Arc<ClientLink>) -> Self {
        Self { link }
    }
}

impl TerminalBashTool {
    /// Serves `bash` on the client's terminals.
    pub fn new(link: Arc<ClientLink>) -> Self {
        Self { link }
    }
}

fn read_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "lines": {"type": "string"},
            "mode": {"type": "string"}
        },
        "required": ["path"]
    })
}

fn edit_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "old": {"type": "string"},
            "new": {"type": "string"},
            "replace_all": {"type": "boolean"}
        },
        "required": ["path", "old", "new"]
    })
}

fn write_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "content": {"type": "string"}
        },
        "required": ["path", "content"]
    })
}

fn bash_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {"type": "string"},
            "timeout_s": {"type": "number"},
            "background": {"type": "boolean"}
        },
        "required": ["command"]
    })
}

async fn read_via_client(
    link: &ClientLink,
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let req = ReadTextFileRequest::new(link.session.clone(), path.to_path_buf());
    let req = match line {
        Some(first) => req.line(first),
        None => req,
    };
    let req = match limit {
        Some(n) => req.limit(n),
        None => req,
    };
    link.conn
        .send_request(req)
        .block_task()
        .await
        .map(|resp| resp.content)
        .map_err(|e| e.to_string())
}

/// Keyword outline for `mode = "outline"`: the fallback grade cox itself
/// uses where it ships no grammar.
fn keyword_outline(path: &Path, content: &str) -> String {
    let mut out = Vec::new();
    for (n, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("fn ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("func ")
            || trimmed.starts_with("pub ")
            || trimmed.starts_with("export ")
            || trimmed.starts_with('#')
        {
            out.push(format!("{}: {}", n + 1, trimmed));
        }
    }
    if out.is_empty() {
        return format!("{}: (no outline items found)", path.display());
    }
    out.join("\n")
}

fn parse_lines(range: &str) -> Result<(Option<u32>, Option<u32>), String> {
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| format!("bad lines range {range:?}, want \"a-b\""))?;
    let (start, end): (u32, u32) = start
        .trim()
        .parse::<u32>()
        .and_then(|s| end.trim().parse::<u32>().map(|e| (s, e)))
        .map_err(|_| format!("bad lines range {range:?}, want \"a-b\""))?;
    if start == 0 || end < start {
        return Err(format!("bad lines range {range:?}, want \"a-b\""));
    }
    Ok((Some(start), Some(end - start + 1)))
}

#[async_trait]
impl Tool for FsReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description:
                "Reads a file from the editor buffer. Same contract as the local read tool."
                    .to_string(),
            input_schema: read_schema(),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Denied {
                why: "read needs a path".to_string(),
            })?;
        let mode = input.get("mode").and_then(Value::as_str).unwrap_or("text");
        let (line, limit) = match input.get("lines").and_then(Value::as_str) {
            Some(range) => parse_lines(range).map_err(|why| ToolError::Denied { why })?,
            None => (None, None),
        };
        let abs = absolute(&self.link.cwd, path);
        // Outline fetches wide (capped) and outlines locally; ranged and
        // whole reads pass the window through to the client.
        let (line, limit) = if mode == "outline" {
            (Some(1), Some(5000))
        } else {
            (line, limit)
        };
        let content = read_via_client(&self.link, &abs, line, limit)
            .await
            .map_err(|e| ToolOutput {
                text: format!("read {} failed: {e}", abs.display()),
                is_error: true,
                diff: None,
                structured: None,
            })
            .map_err(|o| ToolError::Denied { why: o.text })?;
        if mode == "outline" {
            let mut text = keyword_outline(&abs, &content);
            text.push_str(&format!(
                "\n[outline of {} via the editor buffer]",
                abs.display()
            ));
            return Ok(ToolOutput {
                text,
                is_error: false,
                diff: None,
                structured: None,
            });
        }
        let numbered: String = content
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{}\t{l}\n", line.unwrap_or(1) + i as u32))
            .collect();
        Ok(ToolOutput {
            text: numbered,
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[async_trait]
impl Tool for FsEditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Exact string replacement in the editor buffer.".to_string(),
            input_schema: edit_schema(),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Denied {
                why: "edit needs a path".to_string(),
            })?;
        let old = input.get("old").and_then(Value::as_str).unwrap_or_default();
        let new = input.get("new").and_then(Value::as_str).unwrap_or_default();
        let replace_all = input
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if old.is_empty() {
            return Err(ToolError::Denied {
                why: "edit needs a non-empty old string".to_string(),
            });
        }
        let abs = absolute(&self.link.cwd, path);
        let content = read_via_client(&self.link, &abs, None, None)
            .await
            .map_err(|e| ToolError::Denied {
                why: format!("edit read {} failed: {e}", abs.display()),
            })?;
        let matches = content.matches(old).count();
        if matches == 0 {
            return Ok(ToolOutput {
                text: format!("edit {}: old string not found", abs.display()),
                is_error: true,
                diff: None,
                structured: None,
            });
        }
        if matches > 1 && !replace_all {
            return Ok(ToolOutput {
                text: format!(
                    "edit {}: old string matches {matches} times, pass replace_all",
                    abs.display()
                ),
                is_error: true,
                diff: None,
                structured: None,
            });
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        let req = WriteTextFileRequest::new(self.link.session.clone(), abs.clone(), updated);
        if let Err(e) = self.link.conn.send_request(req).block_task().await {
            return Ok(failed(&format!("edit write {}", abs.display()), e));
        }
        Ok(ToolOutput {
            text: format!(
                "edited {} ({} replacement{})",
                abs.display(),
                if replace_all { matches } else { 1 },
                if matches == 1 && !replace_all {
                    ""
                } else {
                    "s"
                }
            ),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[async_trait]
impl Tool for FsWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_string(),
            description: "Writes a file through the editor buffer.".to_string(),
            input_schema: write_schema(),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Denied {
                why: "write needs a path".to_string(),
            })?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let abs = absolute(&self.link.cwd, path);
        let req =
            WriteTextFileRequest::new(self.link.session.clone(), abs.clone(), content.to_string());
        if let Err(e) = self.link.conn.send_request(req).block_task().await {
            return Ok(failed(&format!("write {}", abs.display()), e));
        }
        Ok(ToolOutput {
            text: format!("wrote {} ({} bytes)", abs.display(), content.len()),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[async_trait]
impl Tool for TerminalBashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Runs a shell command on the client's terminal.".to_string(),
            input_schema: bash_schema(),
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if command.trim().is_empty() {
            return Err(ToolError::Denied {
                why: "bash needs a command".to_string(),
            });
        }
        if input
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(ToolError::Denied {
                why: "background bash is not supported over ACP; run it in the foreground"
                    .to_string(),
            });
        }
        let timeout = Duration::from_secs(
            input
                .get("timeout_s")
                .and_then(Value::as_u64)
                .filter(|s| *s > 0)
                .unwrap_or(120),
        );
        let create = CreateTerminalRequest::new(self.link.session.clone(), "sh".to_string())
            .args(vec!["-c".to_string(), command.to_string()])
            .cwd(self.link.cwd.clone());
        let terminal = match self.link.conn.send_request(create).block_task().await {
            Ok(resp) => resp.terminal_id,
            Err(e) => return Ok(failed("terminal/create", e)),
        };
        let wait = WaitForTerminalExitRequest::new(self.link.session.clone(), terminal.clone());
        let exited =
            tokio::time::timeout(timeout, self.link.conn.send_request(wait).block_task()).await;
        let status = match exited {
            Ok(Ok(resp)) => Some(resp.exit_status),
            Ok(Err(e)) => {
                let _ = self
                    .link
                    .conn
                    .send_request(ReleaseTerminalRequest::new(
                        self.link.session.clone(),
                        terminal,
                    ))
                    .block_task()
                    .await;
                return Ok(failed("terminal/wait_for_exit", e));
            }
            Err(_) => {
                let _ = self
                    .link
                    .conn
                    .send_request(ReleaseTerminalRequest::new(
                        self.link.session.clone(),
                        terminal,
                    ))
                    .block_task()
                    .await;
                return Ok(ToolOutput {
                    text: format!("command timed out after {}s: {command}", timeout.as_secs()),
                    is_error: true,
                    diff: None,
                    structured: None,
                });
            }
        };
        let output = self
            .link
            .conn
            .send_request(TerminalOutputRequest::new(
                self.link.session.clone(),
                terminal.clone(),
            ))
            .block_task()
            .await
            .map(|r| r.output)
            .unwrap_or_default();
        let _ = self
            .link
            .conn
            .send_request(ReleaseTerminalRequest::new(
                self.link.session.clone(),
                terminal,
            ))
            .block_task()
            .await;
        let code = status.and_then(|s| s.exit_code);
        let mut text = output;
        text.push_str(&format!(
            "\n[exit {}]",
            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        ));
        Ok(ToolOutput {
            text,
            is_error: code != Some(0),
            diff: None,
            structured: None,
        })
    }
}
