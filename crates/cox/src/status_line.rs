// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The user's status command (`[tui.status_line]`, P46, A77): turns the
//! TUI's status JSON into at most one sanitized line by running the
//! configured command under the sandbox, with a timeout, a 300 ms debounce
//! and cancel-on-new-input. Separate from `session.rs` because it is a
//! process runner with its own lifecycle; `session.rs` only wires it.
//!
//! Trust: the command is the user's own (a project config cannot set it,
//! `cox-config`'s guard), but what it prints is untrusted, so the line
//! passes `cox_sanitize::sanitize` (colours and links are stripped). It runs
//! read-only without network unless the user chose `danger-full-access`,
//! and never bare: a host whose sandbox cannot wrap it gets an error the
//! caller turns into one warning and no row.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use cox_protocol::config::{CHILD_ENV_ALLOWLIST, StatusLineConfig};
use cox_protocol::{SandboxMode, SandboxPolicy, SessionId};
use cox_tools::sandbox;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Quiet period after the last input before the command runs (Claude
/// Code's statusline debounce).
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The most stdout read from one run; the row shows only the first line.
const STDOUT_CAP: u64 = 4 * 1024;

/// Why the status line cannot run at all on this host.
#[derive(Debug, thiserror::Error)]
pub enum StatusLineError {
    /// Nothing confines commands here and the user did not choose
    /// `danger-full-access`: the command is never run bare.
    #[error("no sandbox backend on this host, so the status line command does not run")]
    NoSandbox,
    /// The sandbox wrap itself failed (a Landlock ruleset, say).
    #[error("the sandbox cannot wrap the status line command: {0}")]
    Sandbox(#[from] io::Error),
}

/// The policy the command runs under: read-only and offline, unless the
/// session itself is `danger-full-access`, the user's own choice.
fn script_policy(policy: &SandboxPolicy) -> SandboxPolicy {
    if policy.mode == SandboxMode::DangerFullAccess {
        return policy.clone();
    }
    SandboxPolicy {
        mode: SandboxMode::ReadOnly,
        network: false,
        writable: Vec::new(),
        ..policy.clone()
    }
}

/// The sandboxed `/bin/sh -c <command>`, or why it cannot be built.
/// `sandbox::command` falls back to a bare shell where no backend exists,
/// so that case is refused here first.
fn command(
    cfg: &StatusLineConfig,
    policy: &SandboxPolicy,
    roots: &[PathBuf],
) -> Result<std::process::Command, StatusLineError> {
    let policy = script_policy(policy);
    if policy.mode != SandboxMode::DangerFullAccess
        && sandbox::backend(policy.linux_backend).is_none()
    {
        return Err(StatusLineError::NoSandbox);
    }
    Ok(sandbox::command(
        &policy,
        roots,
        &[],
        Path::new("/bin/sh"),
        &cfg.command,
    )?)
}

/// Runs the command once with `input` on stdin and returns its first
/// stdout line, sanitized and trimmed. Empty output, a non-zero exit, a
/// timeout or a spawn failure is `Ok(None)` (the row goes blank, D14); only
/// a sandbox that cannot wrap the command is an `Err`.
pub async fn run_once(
    cfg: &StatusLineConfig,
    policy: &SandboxPolicy,
    roots: &[PathBuf],
    input: &Value,
    columns: u16,
) -> Result<Option<String>, StatusLineError> {
    use std::os::unix::process::CommandExt as _;

    let mut cmd = command(cfg, policy, roots)?;
    cmd.env_clear();
    for key in CHILD_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.env("COLUMNS", columns.to_string());
    if let Some(root) = roots.first() {
        cmd.current_dir(root);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else {
        return Ok(None);
    };
    // Whatever ends this run — exit, timeout, or the caller dropping the
    // future for a newer input — takes the whole group down with it.
    let _reap = Reap(child.id());
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Ok(None);
    };
    let payload = input.to_string();
    let run = async move {
        let write = async move {
            // A script that never reads stdin closes the pipe: not an error.
            let _ = stdin.write_all(payload.as_bytes()).await;
        };
        let mut out = Vec::new();
        let mut stdout = stdout.take(STDOUT_CAP);
        let read = stdout.read_to_end(&mut out);
        let (_, read) = tokio::join!(write, read);
        read.ok()?;
        let status = child.wait().await.ok()?;
        status.success().then_some(out)
    };
    let timeout = Duration::from_millis(u64::from(cfg.timeout_ms));
    let Ok(Some(out)) = tokio::time::timeout(timeout, run).await else {
        return Ok(None);
    };
    let text = cox_sanitize::sanitize(&String::from_utf8_lossy(&out));
    let line = text.lines().next().unwrap_or("").trim();
    Ok((!line.is_empty()).then(|| line.to_string()))
}

/// Kills the process group the command leads, the kill a cancelled `bash`
/// call ends with.
struct Reap(Option<u32>);

impl Drop for Reap {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            cox_tools::bash::kill_group(pid);
        }
    }
}

/// The TUI's status value plus the fields only the runtime knows, under
/// Claude Code's statusline names so an existing script runs unchanged
/// (D4). A non-object `tui` value is replaced.
pub fn input(tui: Value, session: SessionId, cwd: &Path, project: &Path) -> Value {
    let mut value = match tui {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    value.insert("session_id".into(), json!(session.to_string()));
    value.insert("cwd".into(), json!(cwd.display().to_string()));
    value.insert(
        "workspace".into(),
        json!({
            "current_dir": cwd.display().to_string(),
            "project_dir": project.display().to_string(),
        }),
    );
    value.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    Value::Object(value)
}

type Run = Pin<Box<dyn Future<Output = Result<Option<String>, StatusLineError>> + Send>>;

/// Starts the runner: each `(input, columns)` from `rx` runs the command
/// 300 ms after the last one arrived, a newer input drops (and so kills)
/// the run in flight, and with `refresh_s` set the last input re-runs on
/// that period. Every result goes to `out`. Refuses up front, before any
/// task starts, when the sandbox cannot wrap the command; if that changes
/// later the row is blanked and the runner stops.
pub fn spawn(
    cfg: StatusLineConfig,
    policy: SandboxPolicy,
    roots: Vec<PathBuf>,
    mut rx: mpsc::Receiver<(Value, u16)>,
    out: impl Fn(Option<String>) + Send + 'static,
) -> Result<JoinHandle<()>, StatusLineError> {
    command(&cfg, &policy, &roots)?;
    let refresh = (cfg.refresh_s > 0).then(|| Duration::from_secs(u64::from(cfg.refresh_s)));
    Ok(tokio::spawn(async move {
        let start = |input: &(Value, u16)| -> Run {
            let (cfg, policy, roots) = (cfg.clone(), policy.clone(), roots.clone());
            let (value, columns) = input.clone();
            Box::pin(async move { run_once(&cfg, &policy, &roots, &value, columns).await })
        };
        let mut last: Option<(Value, u16)> = None;
        let mut due: Option<Instant> = None;
        let mut next_refresh = refresh.map(|period| Instant::now() + period);
        let mut running: Option<Run> = None;
        loop {
            tokio::select! {
                input = rx.recv() => {
                    let Some(input) = input else { break };
                    last = Some(input);
                    due = Some(Instant::now() + DEBOUNCE);
                    running = None;
                }
                () = until(due) => {
                    due = None;
                    running = last.as_ref().map(&start);
                }
                () = until(next_refresh) => {
                    next_refresh = refresh.map(|period| Instant::now() + period);
                    if running.is_none() && due.is_none() {
                        running = last.as_ref().map(&start);
                    }
                }
                result = finish(&mut running) => {
                    running = None;
                    match result {
                        Ok(line) => out(line),
                        Err(_) => {
                            out(None);
                            break;
                        }
                    }
                }
            }
        }
    }))
}

/// Sleeps until `at`, or forever when there is no deadline.
async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The run in flight, or never when there is none.
async fn finish(running: &mut Option<Run>) -> Result<Option<String>, StatusLineError> {
    match running {
        Some(run) => run.await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant as StdInstant;

    use cox_protocol::LinuxBackend;

    use super::*;

    /// A host with no sandbox never runs the command, so there is nothing
    /// to observe; those tests pass vacuously there.
    fn sandboxed() -> bool {
        sandbox::backend(LinuxBackend::Auto).is_some()
    }

    fn policy() -> SandboxPolicy {
        SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
            readonly_in_workspace: Vec::new(),
            linux_backend: LinuxBackend::Auto,
        }
    }

    fn cfg(command: &str, timeout_ms: u32) -> StatusLineConfig {
        StatusLineConfig {
            command: command.to_string(),
            refresh_s: 0,
            timeout_ms,
        }
    }

    async fn once(command: &str, timeout_ms: u32, input: &Value) -> Option<String> {
        let root = tempfile::tempdir().expect("tempdir");
        run_once(
            &cfg(command, timeout_ms),
            &policy(),
            &[root.path().to_path_buf()],
            input,
            80,
        )
        .await
        .expect("the sandbox wraps the command")
    }

    #[tokio::test]
    async fn status_line_output_is_sanitized_first_line() {
        if !sandboxed() {
            return;
        }
        let line = once(
            r"printf '\033[31mhi\033]0;x\007\nsecond'",
            2_000,
            &json!({}),
        )
        .await;
        assert_eq!(line.as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn status_line_timeout_kills_and_blanks() {
        if !sandboxed() {
            return;
        }
        let started = StdInstant::now();
        let line = once("sleep 5; echo late", 200, &json!({})).await;
        assert_eq!(line, None);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn status_line_nonzero_exit_blanks() {
        if !sandboxed() {
            return;
        }
        assert_eq!(once("echo hi; exit 3", 2_000, &json!({})).await, None);
    }

    #[tokio::test]
    async fn status_line_cannot_write_the_workspace() {
        if !sandboxed() {
            return;
        }
        // The temp dir stays writable under every policy, so the workspace
        // root must live somewhere else (as `sandbox_macos` does).
        let home = std::env::var("HOME").expect("HOME");
        let root = tempfile::Builder::new()
            .prefix(".cox-status-line-ro-")
            .tempdir_in(home)
            .expect("tempdir under HOME");
        let line = run_once(
            &cfg("touch x; echo ran", 2_000),
            &policy(),
            &[root.path().to_path_buf()],
            &json!({}),
            80,
        )
        .await
        .expect("the sandbox wraps the command");
        assert!(
            !root.path().join("x").exists(),
            "a status command wrote the workspace"
        );
        assert_eq!(line.as_deref(), Some("ran"), "the command itself ran");
    }

    #[tokio::test]
    async fn status_line_reads_claude_field_names() {
        if !sandboxed() {
            return;
        }
        let root = tempfile::tempdir().expect("tempdir");
        let value = input(
            json!({"model": {"id": "claude-sonnet-5", "display_name": "Sonnet 5"}}),
            SessionId::new(),
            root.path(),
            root.path(),
        );
        let line = once("cat", 2_000, &value).await.expect("cat echoes stdin");
        let echoed: Value = serde_json::from_str(&line).expect("one JSON line");
        assert_eq!(echoed["model"]["display_name"], "Sonnet 5");
        assert!(echoed["session_id"].is_string(), "{echoed}");
        assert!(echoed["workspace"]["project_dir"].is_string(), "{echoed}");
        assert_eq!(echoed["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn newer_input_cancels_the_running_command() {
        if !sandboxed() {
            return;
        }
        let root = tempfile::tempdir().expect("tempdir");
        let (tx, rx) = mpsc::channel(4);
        let (lines_tx, mut lines) = mpsc::unbounded_channel();
        let _runner = spawn(
            cfg(
                "if grep -q slow; then sleep 5; echo slow; else echo fast; fi",
                10_000,
            ),
            policy(),
            vec![root.path().to_path_buf()],
            rx,
            move |line| {
                let _ = lines_tx.send(line);
            },
        )
        .expect("the sandbox wraps the command");
        tx.send((json!({"k": "slow"}), 80)).await.expect("send");
        // Past the debounce, so the slow run is in flight.
        tokio::time::sleep(Duration::from_millis(600)).await;
        tx.send((json!({"k": "fast"}), 80)).await.expect("send");
        let first = tokio::time::timeout(Duration::from_secs(3), lines.recv())
            .await
            .expect("an answer well before the slow run could end")
            .expect("runner alive");
        assert_eq!(first.as_deref(), Some("fast"));
    }
}
