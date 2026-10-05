// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `bash`: one shell command under the session's sandbox policy on a PTY,
//! with streamed output, an env allowlist and a SIGTERM→SIGKILL timeout
//! (plan.md T3.7, §1.11). Separate from the other tools because it is the
//! only one that spawns a process; `classify` has its own file so the
//! permission engine can rate a command line without running it.

mod classify;
mod shell;

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use async_trait::async_trait;
use cox_protocol::{
    ArchivePut, Concurrency, Risk, SandboxMode, SandboxPolicy, Segments, TaskId, Tool, ToolCx,
    ToolError, ToolOutput, ToolSpec,
};
#[cfg(unix)]
use nix::{
    libc,
    poll::{PollFd, PollFlags, poll},
    pty::{Winsize, openpty},
    sys::signal::{Signal, killpg},
    sys::termios::Termios,
    unistd::{Pid, setsid},
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use classify::{classify, segments};
pub use shell::default_shell;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a process gets between SIGTERM and SIGKILL.
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the PTY to drain after the shell exited before
/// giving up on grandchildren that still hold it open.
#[cfg(unix)]
const REAP_GRACE: Duration = Duration::from_millis(500);
/// How long one `poll` on the master waits before re-checking the phase.
#[cfg(unix)]
const POLL_SLICE_MS: u8 = 50;
/// Reader phases: read while the child runs, read what is left once it
/// exited, stop even if a grandchild keeps writing.
#[cfg(unix)]
const RUNNING: u8 = 0;
#[cfg(unix)]
const DRAINING: u8 = 1;
#[cfg(unix)]
const STOP: u8 = 2;
use cox_protocol::config::CHILD_ENV_ALLOWLIST as ENV_ALLOWLIST;

/// `bash`: runs one command line and returns its stripped output.
pub struct BashTool;

#[derive(Debug, Deserialize, JsonSchema)]
struct BashInput {
    /// The command line, run with `<shell> -c` in the session's working directory.
    command: String,
    /// Which shell interprets the command line (default `sh`).
    #[serde(default)]
    shell: Shell,
    /// Seconds before the command is sent SIGTERM, then SIGKILL (default 120).
    #[serde(default)]
    timeout_s: Option<u64>,
    /// Run as a background task and return its id at once; the exit code and
    /// the archived output arrive as a notice when it finishes.
    #[serde(default)]
    background: bool,
}

/// The shells a command line may be written for. The enum *is* the
/// allowlist: a name the model invents fails to deserialise, so nothing
/// the model says can pick the program that gets spawned.
#[derive(Debug, Default, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Shell {
    #[default]
    Sh,
    Bash,
    Zsh,
    Fish,
    Dash,
    Ksh,
    Tcsh,
    Nu,
    Pwsh,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Shell::Sh => "sh",
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Dash => "dash",
            Shell::Ksh => "ksh",
            Shell::Tcsh => "tcsh",
            Shell::Nu => "nu",
            Shell::Pwsh => "pwsh",
        }
    }

    /// The installed binary for this shell (`shell::resolve`, T57.2);
    /// every one of them, PowerShell included, takes the command line
    /// after `-c`.
    fn path(self) -> Result<PathBuf, ToolError> {
        let host = shell::Host::current();
        shell::resolve(self.name(), host, &shell::System).ok_or_else(|| ToolError::Denied {
            why: format!(
                "shell `{}` is not installed here (looked in {})",
                self.name(),
                shell::searched(host)
            ),
        })
    }
}

/// The login shell a user's own terminal runs (the desktop terminal pane,
/// T51.3): `$SHELL`'s file name when it names a shell of the allowlist
/// above, else the platform default (`zsh` on macOS, `sh` elsewhere), each
/// resolved by `shell::resolve` (on Unix never on `PATH`) — the directory `$SHELL`
/// points into is ignored, so a workspace cannot pick the program. `None`
/// when neither is installed.
pub fn login_shell(env_shell: Option<&str>) -> Option<PathBuf> {
    let named = env_shell
        .and_then(|shell| Path::new(shell).file_name()?.to_str())
        .and_then(|name| serde_json::from_value::<Shell>(Value::String(name.to_string())).ok());
    let default = if cfg!(target_os = "macos") {
        Shell::Zsh
    } else {
        Shell::Sh
    };
    named
        .into_iter()
        .chain([default])
        .find_map(|shell| shell.path().ok())
}

/// A command line and the shell that runs it, kept together so every hop
/// down to the sandbox carries both.
#[cfg_attr(
    windows,
    expect(dead_code, reason = "T57.8's Windows spawn path uses it")
)]
#[derive(Clone)]
struct Cmd {
    line: String,
    shell: PathBuf,
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        let input_schema = serde_json::to_value(schema_for!(BashInput)).unwrap_or(Value::Null);
        // Resolved once per process: the tool schema is part of the
        // cache-stable prefix (T57.2 names the Windows default shell here).
        static DEFAULT: OnceLock<String> = OnceLock::new();
        let default =
            DEFAULT.get_or_init(|| shell::default_label(shell::Host::current(), &shell::System));
        ToolSpec {
            name: "bash".to_string(),
            description: format!(
                "Runs a shell command line in the workspace and returns its output \
                (stdout and stderr interleaved, ANSI stripped) followed by `[exit <code> in \
                <ms>]`. Output streams while the command runs; a long-running command is \
                stopped after `timeout_s` seconds (default 120). Prefer the dedicated `read`, \
                `grep`, `glob` and `edit` tools for file work; use `bash` for builds, tests, \
                git and anything that needs a process. `shell` picks the interpreter \
                ({default} by default, or `bash`, `zsh`, `fish`, `dash`, `ksh`, `tcsh`, `nu`, \
                `pwsh` when the command line needs that shell's syntax); it errors if the \
                shell is not installed. Pass `background: true` for a server \
                or watcher you do not want to wait for."
            ),
            input_schema,
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

    fn segments(&self, input: &Value) -> Option<Segments> {
        Some(segments(&self.subject(input)))
    }

    fn risk(&self, input: &Value) -> Risk {
        match input.get("command").and_then(Value::as_str) {
            Some(command) => classify(command),
            None => Risk::Exec,
        }
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: BashInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid bash input: {e}"),
        })?;
        // Before `background`, which would otherwise report a started task.
        if cfg!(windows) {
            return Err(unavailable());
        }
        let timeout = input
            .timeout_s
            .filter(|s| *s > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        let cmd = Cmd {
            line: input.command,
            shell: input.shell.path()?,
        };
        if input.background {
            return Ok(background(cmd, timeout, cx));
        }
        let run = run(
            &cmd,
            &cx.cwd,
            Workspace {
                read: &cx.roots,
                write: &cx.writable_roots,
            },
            &cx.sandbox,
            &cx.cancel,
            &cx.output,
            timeout,
        )
        .await?;
        let text = run.render();
        // Only a command the sandbox actually confined can have been denied
        // by it; the loop turns this into `ApprovalRequired { SandboxDenied }`
        // under `on-failure` (T4.3).
        let confined = cx.sandbox.mode != SandboxMode::DangerFullAccess
            && crate::sandbox::backend(cx.sandbox.linux_backend).is_some();
        let denied = (confined && run.ended.is_none() && run.code != Some(0))
            .then(|| denial(&text))
            .flatten();
        // `exit_code` is what a detached run's `TaskCompleted` reports (T27.1).
        let mut structured = serde_json::json!({ "exit_code": run.code });
        if let Some(line) = denied {
            structured["sandbox_denied"] = Value::String(line);
        }
        Ok(ToolOutput {
            is_error: run.ended.is_some() || run.code != Some(0),
            text,
            diff: None,
            structured: Some(structured),
        })
    }
}

/// Windows has no spawn path for `bash` yet: T57.8 adds ConPTY inside a job
/// object. A `Denied` so the model sees the reason and tries another tool.
fn unavailable() -> ToolError {
    ToolError::Denied {
        why: "`bash` is not available on Windows yet".to_string(),
    }
}

/// What Seatbelt, bwrap and Landlock denials look like from inside the
/// shell. `Permission denied` also covers a plain mode-bit refusal — that
/// false positive costs one question, the miss would cost a silent failure.
const DENIAL_MARKERS: &[&str] = &[
    "Operation not permitted",
    "Read-only file system",
    "Permission denied",
    "Could not resolve host",
    "Network is unreachable",
];

/// The first output line that reads like a sandbox denial.
fn denial(text: &str) -> Option<String> {
    text.lines()
        .find(|line| DENIAL_MARKERS.iter().any(|m| line.contains(m)))
        .map(|line| line.trim().chars().take(200).collect())
}

/// Spawns the command detached, for callers with no task registry (`cox
/// mcp`): it outlives cancellation and its full output lands in the archive
/// under this call. Inside a session the core strips `background` and runs
/// the call as a task instead (T27.1), so this path never sees a session.
fn background(cmd: Cmd, timeout: Duration, cx: &ToolCx) -> ToolOutput {
    let task = TaskId::new();
    let (cwd, roots, writable_roots, sandbox) = (
        cx.cwd.clone(),
        cx.roots.clone(),
        cx.writable_roots.clone(),
        cx.sandbox.clone(),
    );
    let archive = cx.archive.clone();
    let (session, call) = (cx.session, cx.call);
    let subject = cmd.line.clone();
    tokio::spawn(async move {
        // The turn's output channel closes when this call returns, so the
        // background run streams into a sink nobody reads.
        let (sink, _) = mpsc::channel(1);
        if let Ok(run) = run(
            &cmd,
            &cwd,
            Workspace {
                read: &roots,
                write: &writable_roots,
            },
            &sandbox,
            &CancellationToken::new(),
            &sink,
            timeout,
        )
        .await
        {
            let _ = archive
                .put(ArchivePut {
                    session,
                    call,
                    tool: "bash".into(),
                    subject: Some(cmd.line),
                    bytes: run.render().into_bytes(),
                })
                .await;
        }
    });
    ToolOutput {
        text: format!(
            "background task {task} started: {subject}\n\
             its output is archived under call {call} when it finishes"
        ),
        is_error: false,
        diff: None,
        structured: None,
    }
}

struct Run {
    raw: Vec<u8>,
    code: Option<u32>,
    /// Why the command was stopped early, if it was.
    ended: Option<&'static str>,
    elapsed: Duration,
}

impl Run {
    fn render(&self) -> String {
        let ms = self.elapsed.as_millis();
        let tail = match (self.ended, self.code) {
            (Some(why), _) => format!("[{why} after {ms}ms; killed]"),
            (None, Some(code)) => format!("[exit {code} in {ms}ms]"),
            (None, None) => format!("[exit unknown in {ms}ms]"),
        };
        let body = strip_ansi(&self.raw);
        if body.is_empty() {
            tail
        } else {
            format!("{}\n{tail}", body.trim_end_matches('\n'))
        }
    }
}

/// Builds the child: the sandbox decides the program (`sandbox-exec`,
/// `bwrap` or the bare shell) and any pre-exec hook, this only adds cwd and
/// the environment.
#[cfg_attr(
    windows,
    expect(dead_code, reason = "T57.8's Windows spawn path uses it")
)]
fn command_for(
    cmd: &Cmd,
    cwd: &Path,
    roots: &[PathBuf],
    writable_roots: &[PathBuf],
    sandbox: &SandboxPolicy,
) -> Result<Command, ToolError> {
    let mut cmd = crate::sandbox::command(sandbox, roots, writable_roots, &cmd.shell, &cmd.line)
        .map_err(|_| ToolError::Io)?;
    cmd.current_dir(cwd);
    cmd.env_clear();
    for key in ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    // Fewer escape sequences to strip, and no pager waiting on a TTY.
    cmd.env("NO_COLOR", "1");
    cmd.env("PAGER", "cat");
    cmd.env("GIT_PAGER", "cat");
    Ok(cmd)
}

/// Makes the PTY slave the child's stdio and controlling terminal.
#[cfg(unix)]
// why: pre_exec + ioctl(TIOCSCTTY) after fork to attach the PTY are unsafe.
#[allow(unsafe_code)]
fn attach_pty(cmd: &mut Command, slave: &OwnedFd) -> Result<(), ToolError> {
    let stdio = |fd: &OwnedFd| fd.try_clone().map(Stdio::from).map_err(|_| ToolError::Io);
    cmd.stdin(stdio(slave)?)
        .stdout(stdio(slave)?)
        .stderr(stdio(slave)?);
    // SAFETY: between fork and exec only async-signal-safe calls run here:
    // `setsid` and one `ioctl`, nothing that allocates or locks.
    unsafe {
        cmd.pre_exec(|| {
            setsid()?;
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(unix)]
fn signal(pid: u32, sig: Signal) {
    // The child is its own session leader (`attach_pty` calls setsid), so
    // its pid is the process group of everything it started.
    let _ = killpg(Pid::from_raw(pid as i32), sig);
}

/// SIGKILLs the process group led by `pid`, the kill a cancelled `bash`
/// call ends with. `pub` so another host-spawned process that leads its own
/// group (an external agent's CLI, T35.13) is reaped the same way, not by a
/// second implementation. Unix-only: Windows kills a tree through a job
/// object (T57.5, T57.6).
#[cfg(unix)]
pub fn kill_group(pid: u32) {
    signal(pid, Signal::SIGKILL);
}

/// How a command run through `run_line` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit code, or 128 plus the signal that ended it.
    pub code: Option<u32>,
    /// Why it was stopped early (`cancelled`, `timed out`), if it was.
    pub stopped: Option<&'static str>,
}

/// Runs one `sh -c` command line exactly as `bash` does — same sandbox wrap,
/// env allowlist, PTY and process-group kill — with `roots` as both the
/// readable and the writable workspace. `pub` so a host that runs a command
/// for someone else (an external agent's ACP terminal, T35.11) has no second
/// spawn path. Output streams to `output` with ANSI stripped; `cancel` ends
/// the run with SIGTERM, then SIGKILL, of the whole group.
pub async fn run_line(
    line: &str,
    cwd: &Path,
    roots: &[PathBuf],
    sandbox: &SandboxPolicy,
    cancel: &CancellationToken,
    output: &mpsc::Sender<String>,
    timeout: Duration,
) -> Result<Exit, ToolError> {
    if cfg!(windows) {
        return Err(unavailable());
    }
    let cmd = Cmd {
        line: line.to_owned(),
        shell: Shell::Sh.path()?,
    };
    let workspace = Workspace {
        read: roots,
        write: roots,
    };
    let run = run(&cmd, cwd, workspace, sandbox, cancel, output, timeout).await?;
    Ok(Exit {
        code: run.code,
        stopped: run.ended,
    })
}

/// No spawn path on Windows until T57.8; `call` and `run_line` already
/// refuse, this only keeps the shared callers compiling.
#[cfg(windows)]
async fn run(
    _cmd: &Cmd,
    _cwd: &Path,
    _workspace: Workspace<'_>,
    _sandbox: &SandboxPolicy,
    _cancel: &CancellationToken,
    _output: &mpsc::Sender<String>,
    _timeout: Duration,
) -> Result<Run, ToolError> {
    Err(unavailable())
}

#[cfg(unix)]
async fn run(
    cmd: &Cmd,
    cwd: &Path,
    workspace: Workspace<'_>,
    sandbox: &SandboxPolicy,
    cancel: &CancellationToken,
    output: &mpsc::Sender<String>,
    timeout: Duration,
) -> Result<Run, ToolError> {
    let start = Instant::now();
    let size = Winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(&size, None::<&Termios>).map_err(|_| ToolError::Io)?;
    let mut child = command_for(cmd, cwd, workspace.read, workspace.write, sandbox)?;
    attach_pty(&mut child, &pty.slave)?;
    let mut child = child.spawn().map_err(|_| ToolError::Io)?;
    let pid = child.id();
    let mut reader = File::from(pty.master.try_clone().map_err(|_| ToolError::Io)?);
    let fd = pty.master.as_raw_fd();
    let master = pty.master;
    // Our slave stays open until the reader is done: macOS throws away
    // whatever the master has not read yet when the last slave closes, so
    // a quick `echo ok` would otherwise exit before its output arrived.
    // The reader therefore stops on the exit status plus a drain, not EOF.
    let slave = pty.slave;

    let phase = Arc::new(AtomicU8::new(RUNNING));
    // A run dropped mid-way (its task aborted, the runtime shutting down)
    // never reaches the end below: without this the reader would poll
    // forever and the group would keep running, and either holds the
    // runtime's shutdown open.
    let mut abandoned = Abandoned {
        phase: phase.clone(),
        group: Some(pid),
    };
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    let reader_phase = phase.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            let phase = reader_phase.load(Ordering::Relaxed);
            if phase == STOP {
                break;
            }
            if readable(fd, POLL_SLICE_MS) {
                match reader.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            } else if phase == DRAINING {
                break;
            }
        }
        drop(slave);
        drop(master);
    });
    let mut wait = tokio::task::spawn_blocking(move || child.wait());

    let timer = tokio::time::sleep(timeout);
    tokio::pin!(timer);
    let mut run = Run {
        raw: Vec::new(),
        code: None,
        ended: None,
        elapsed: Duration::ZERO,
    };
    let mut exited = false;
    let mut drained = false;
    let mut killed = false;
    loop {
        tokio::select! {
            chunk = rx.recv(), if !drained => match chunk {
                Some(bytes) => {
                    let _ = output.send(strip_ansi(&bytes)).await;
                    run.raw.extend_from_slice(&bytes);
                }
                None => {
                    drained = true;
                    if exited {
                        break;
                    }
                }
            },
            status = &mut wait, if !exited => {
                exited = true;
                run.code = status
                    .ok()
                    .and_then(Result::ok)
                    .and_then(|s| s.code().or_else(|| s.signal().map(|n| 128 + n)))
                    .map(|c| c as u32);
                phase.store(DRAINING, Ordering::Relaxed);
                if drained {
                    break;
                }
                timer.as_mut().reset(tokio::time::Instant::now() + REAP_GRACE);
            }
            _ = cancel.cancelled(), if run.ended.is_none() => {
                run.ended = Some("cancelled");
                signal(pid, Signal::SIGTERM);
                timer.as_mut().reset(tokio::time::Instant::now() + TERM_GRACE);
            }
            _ = &mut timer => {
                if exited {
                    // The shell is gone but something it started still writes.
                    signal(pid, Signal::SIGKILL);
                    break;
                }
                if run.ended.is_none() {
                    run.ended = Some("timed out");
                    signal(pid, Signal::SIGTERM);
                } else if !killed {
                    killed = true;
                    signal(pid, Signal::SIGKILL);
                } else {
                    break;
                }
                timer.as_mut().reset(tokio::time::Instant::now() + TERM_GRACE);
            }
        }
    }
    phase.store(STOP, Ordering::Relaxed);
    // A stopped run ends with the group SIGKILLed even after its shell went
    // down on SIGTERM: a child that ignores SIGTERM must not outlive it.
    if !exited || run.ended.is_some() {
        signal(pid, Signal::SIGKILL);
    }
    // Finished: the group may be reaped, and its id reused, from here on.
    abandoned.group = None;
    run.elapsed = start.elapsed();
    Ok(run)
}

/// What `run` undoes if it is dropped before it finishes: the PTY reader
/// goes to `STOP` and the process group, while `Some`, is SIGKILLed.
#[cfg(unix)]
struct Abandoned {
    phase: Arc<AtomicU8>,
    group: Option<u32>,
}

#[cfg(unix)]
impl Drop for Abandoned {
    fn drop(&mut self) {
        self.phase.store(STOP, Ordering::Relaxed);
        if let Some(pid) = self.group {
            signal(pid, Signal::SIGKILL);
        }
    }
}

#[cfg_attr(
    windows,
    expect(dead_code, reason = "T57.8's Windows spawn path uses it")
)]
#[derive(Clone, Copy)]
struct Workspace<'a> {
    read: &'a [PathBuf],
    write: &'a [PathBuf],
}

/// Whether the master has bytes to read within `timeout`, so the reader
/// can notice `DRAINING`/`STOP` instead of blocking forever on a PTY that
/// a grandchild still holds open.
#[cfg(unix)]
// why: BorrowedFd::borrow_raw on an fd we own for the duration of the poll.
#[allow(unsafe_code)]
fn readable(fd: RawFd, timeout_ms: u8) -> bool {
    // SAFETY: `fd` is the master's descriptor and the master is owned by the
    // reader thread that calls this, so it stays open for the whole call.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut fds = [PollFd::new(fd, PollFlags::POLLIN)];
    matches!(poll(&mut fds, timeout_ms), Ok(n) if n > 0)
}

/// Drops CSI/OSC escape sequences, carriage returns and other control
/// bytes so the model sees plain text; `cox-tui` sanitises again for
/// display, this is only about not paying tokens for colour codes.
fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    // Parameters and intermediates end at the first final byte.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC ends with BEL or ESC \.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_drops_colour_osc_and_carriage_returns() {
        let raw = b"\x1b[1;32mok\x1b[0m\r\n\x1b]0;title\x07done\r\n";
        assert_eq!(strip_ansi(raw), "ok\ndone\n");
    }

    #[test]
    fn render_reports_exit_code_or_why_it_was_killed() {
        let done = Run {
            raw: b"hi\r\n".to_vec(),
            code: Some(0),
            ended: None,
            elapsed: Duration::from_millis(5),
        };
        assert_eq!(done.render(), "hi\n[exit 0 in 5ms]");
        let killed = Run {
            raw: Vec::new(),
            code: None,
            ended: Some("timed out"),
            elapsed: Duration::from_millis(1000),
        };
        assert_eq!(killed.render(), "[timed out after 1000ms; killed]");
    }

    #[test]
    fn bash_risk_comes_from_the_command_line() {
        let tool = BashTool;
        assert_eq!(
            tool.risk(&serde_json::json!({"command": "ls"})),
            Risk::ReadOnly
        );
        assert_eq!(
            tool.risk(&serde_json::json!({"command": "rm -rf x"})),
            Risk::Destructive
        );
        assert_eq!(tool.risk(&serde_json::json!({})), Risk::Exec);
        assert_eq!(tool.subject(&serde_json::json!({"command": "ls"})), "ls");
    }

    /// Regression: a run dropped mid-way left its PTY reader polling forever
    /// and its command running, so the runtime that ran it never finished
    /// shutting down (the ACP `terminal/release` test hung on Linux this way).
    #[cfg(unix)]
    #[test]
    fn dropped_run_lets_the_runtime_shut_down() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = SandboxPolicy {
            mode: SandboxMode::DangerFullAccess,
            network: true,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        };
        rt.block_on(async {
            let (tx, _rx) = mpsc::channel(64);
            let cancel = CancellationToken::new();
            let long = Duration::from_secs(60);
            let run = run_line("sleep 30", dir.path(), &[], &policy, &cancel, &tx, long);
            let _ = tokio::time::timeout(Duration::from_millis(300), run).await;
        });
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(rt);
            let _ = done.send(());
        });
        assert!(
            ended.recv_timeout(Duration::from_secs(10)).is_ok(),
            "the reader or the command outlived its dropped run"
        );
    }

    /// Until T57.8, a Windows `bash` is refused with a reason, never spawned.
    #[cfg(windows)]
    #[tokio::test]
    async fn bash_is_not_available_on_windows_yet() {
        let policy = SandboxPolicy {
            mode: SandboxMode::DangerFullAccess,
            network: true,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        };
        let (tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let got = run_line(
            "echo",
            Path::new("."),
            &[],
            &policy,
            &cancel,
            &tx,
            DEFAULT_TIMEOUT,
        );
        let why = match got.await {
            Err(ToolError::Denied { why }) => why,
            other => panic!("expected Denied, got {other:?}"),
        };
        assert!(why.contains("not available on Windows"), "{why}");
    }

    #[test]
    fn bash_shell_defaults_to_sh_and_only_accepts_known_names() {
        let parse = |v| serde_json::from_value::<BashInput>(v);
        let input = parse(serde_json::json!({"command": "ls"})).expect("default");
        assert_eq!(input.shell.name(), "sh");
        let input = parse(serde_json::json!({"command": "ls", "shell": "fish"})).expect("fish");
        assert_eq!(input.shell.name(), "fish");
        // Anything outside the enum never reaches a spawn.
        assert!(parse(serde_json::json!({"command": "ls", "shell": "/tmp/evil"})).is_err());
        assert!(parse(serde_json::json!({"command": "ls", "shell": "bash -lc x"})).is_err());
    }

    #[test]
    fn bash_shell_resolves_to_an_absolute_path_or_says_it_is_missing() {
        let sh = Shell::Sh.path().expect("sh exists everywhere");
        assert!(sh.is_absolute() && sh.ends_with("sh"));
        // A shell that is installed nowhere we look is a Denied, not a spawn.
        if Shell::Nu.path().is_err() {
            let why = match Shell::Nu.path() {
                Err(ToolError::Denied { why }) => why,
                other => panic!("expected Denied, got {other:?}"),
            };
            assert!(why.contains("`nu` is not installed"), "{why}");
        }
    }
}
