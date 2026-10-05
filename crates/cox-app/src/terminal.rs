// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The desktop terminal pane's process (T51.3, DT§3.2): the user's login
//! shell in a PTY, in the session's cwd, under the session's own sandbox
//! policy. Separate from `live` because the pane is the user's own
//! terminal, not the agent's: its bytes never reach the model, the rollout
//! or the ledger, so nothing here touches the session's event stream.
//! Swift never spawns a process (DT§4.6); it only draws the bytes this
//! hands out (SwiftTerm interprets the escape sequences) and sends keys in.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cox_protocol::SandboxPolicy;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgid};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc;

/// One PTY read; a burst is several of these.
const CHUNK: usize = 16 * 1024;
/// Chunks waiting for the pane. When it falls behind, the reader blocks,
/// the PTY fills and the shell waits — a stalled view costs at most this
/// many chunks, as a real terminal would.
const CHUNKS: usize = 64;
/// What one `next_output` hands out at most, so a flood still arrives in
/// frame-sized batches.
const BATCH: usize = 256 * 1024;
/// How long the shell gets to pass SIGHUP on to its jobs before its group
/// is killed.
const CLOSE_GRACE: Duration = Duration::from_millis(200);
const CLOSE_POLL: Duration = Duration::from_millis(10);

/// What opening or driving a terminal can fail with.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("no allowed login shell is installed")]
    NoShell,
    #[error("{0} is not a directory")]
    Cwd(PathBuf),
    #[error("sandbox: {0}")]
    Sandbox(io::Error),
    #[error("pty: {0}")]
    Pty(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn pty(e: impl std::fmt::Display) -> TerminalError {
    TerminalError::Pty(e.to_string())
}

/// Everything a terminal is started from; the session fills it in.
#[derive(Debug, Clone)]
pub struct TerminalSpec {
    /// An absolute path from [`shell`], never a `PATH` lookup.
    pub shell: PathBuf,
    /// The session's cwd (its worktree when it has one).
    pub cwd: PathBuf,
    /// The session's resolved `[sandbox]`, as `bash` runs under it.
    pub policy: SandboxPolicy,
    pub roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

/// The login shell for `$SHELL` (`env_shell`), from `bash`'s allowlist in
/// its fixed directories (A12).
pub fn shell(env_shell: Option<&str>) -> Result<PathBuf, TerminalError> {
    cox_tools::bash::login_shell(env_shell).ok_or(TerminalError::NoShell)
}

/// `<shell> -l -i` wrapped by the same `sandbox::argv` every command runs
/// under; bare only for `danger-full-access`.
pub fn argv(spec: &TerminalSpec) -> Result<Vec<String>, TerminalError> {
    let program = [
        spec.shell.to_string_lossy().into_owned(),
        "-l".to_string(),
        "-i".to_string(),
    ];
    cox_tools::sandbox::argv(&spec.policy, &spec.roots, &spec.writable_roots, &program)
        .map_err(TerminalError::Sandbox)
}

/// One running terminal. Dropping it closes it.
pub struct TerminalHandle {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    output: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
    closed: AtomicBool,
}

/// Starts `spec`'s shell. The environment is this process's, which
/// `load_login_env` made the login shell's (DT§4.8).
pub fn open(spec: &TerminalSpec) -> Result<TerminalHandle, TerminalError> {
    if !spec.cwd.is_dir() {
        // portable-pty would quietly start in `$HOME` instead.
        return Err(TerminalError::Cwd(spec.cwd.clone()));
    }
    let argv = argv(spec)?;
    let pair = native_pty_system()
        .openpty(size(spec.cols, spec.rows))
        .map_err(pty)?;
    let mut cmd = CommandBuilder::from_argv(argv.into_iter().map(OsString::from).collect());
    cmd.cwd(&spec.cwd);
    // The inherited `PWD` names the app's directory, not this one.
    cmd.env("PWD", &spec.cwd);
    cmd.env_remove("OLDPWD");
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(cmd).map_err(pty)?;
    // Only the child keeps the slave open, so the reader sees the end.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(pty)?;
    let writer = pair.master.take_writer().map_err(pty)?;
    let (tx, rx) = mpsc::channel(CHUNKS);
    std::thread::Builder::new()
        .name("cox-terminal".into())
        .spawn(move || pump(reader, &tx))?;
    Ok(TerminalHandle {
        master: Mutex::new(pair.master),
        writer: Mutex::new(Some(writer)),
        child: Mutex::new(child),
        output: tokio::sync::Mutex::new(rx),
        closed: AtomicBool::new(false),
    })
}

/// Reads the PTY until the shell and everything holding the slave exit.
/// A plain thread: the read blocks, and `blocking_send` is its backpressure.
fn pump(mut reader: Box<dyn Read + Send>, tx: &mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0; CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // EIO once the last slave descriptor closed.
            Err(_) => return,
        }
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TerminalHandle {
    /// Keys and pastes from the pane, as the terminal encodes them.
    pub fn write(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut writer = lock(&self.writer);
        let writer = writer
            .as_mut()
            .ok_or_else(|| TerminalError::Pty("the terminal is closed".into()))?;
        writer.write_all(bytes)?;
        Ok(writer.flush()?)
    }

    /// The pane's new size; the shell gets SIGWINCH.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        lock(&self.master).resize(size(cols, rows)).map_err(pty)
    }

    /// The next bytes the shell wrote, every chunk already waiting joined
    /// up to one batch; `None` once it exited and the PTY drained.
    pub async fn next_output(&self) -> Option<Vec<u8>> {
        let mut rx = self.output.lock().await;
        let mut out = rx.recv().await?;
        while out.len() < BATCH {
            match rx.try_recv() {
                Ok(more) => out.extend(more),
                Err(_) => break,
            }
        }
        Some(out)
    }

    /// The shell's exit code, once it exited.
    pub fn exit_status(&self) -> Option<u32> {
        lock(&self.child)
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.exit_code())
    }

    /// A job other than the shell holds the terminal's foreground, so
    /// closing now would kill it and the pane asks first (T51.6). The
    /// shell leads its own session, so its group id is its pid.
    pub fn is_busy(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        let shell = lock(&self.child)
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok());
        let foreground = lock(&self.master).process_group_leader();
        matches!((shell, foreground), (Some(shell), Some(group)) if shell != group)
    }

    /// Ends the terminal: SIGHUP, then SIGKILL, to the shell's group, the
    /// foreground group and every descendant's group. A background job is
    /// its own group, and the shell forwards SIGHUP only while it is still
    /// scheduled; under load it used to be killed first and the job lived.
    /// Idempotent.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut child = lock(&self.child);
        let shell = child.process_id().and_then(|pid| i32::try_from(pid).ok());
        let foreground = lock(&self.master).process_group_leader();
        // Descendants are reparented once the shell exits, so collect them
        // before either signal.
        let groups = groups_to_signal(shell, foreground);
        signal(&groups, Signal::SIGHUP);
        let deadline = Instant::now() + CLOSE_GRACE;
        while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
            std::thread::sleep(CLOSE_POLL);
        }
        signal(&groups, Signal::SIGKILL);
        // Reaps the shell; a group already gone answers ESRCH above.
        let _ = child.try_wait();
        // EOF to the slave side.
        lock(&self.writer).take();
    }
}

/// A group that already exited answers ESRCH, which is what we wanted.
fn signal(groups: &[Pid], signal: Signal) {
    for group in groups {
        let _ = killpg(*group, signal);
    }
}

/// The shell, the foreground leader and every descendant, as process
/// groups. Never 0 (our own group) or 1 (launchd/init).
fn groups_to_signal(shell: Option<i32>, foreground: Option<i32>) -> Vec<Pid> {
    let mut pids = Vec::new();
    if let Some(shell) = shell {
        pids.push(shell);
        pids.extend(descendant_pids(shell));
    }
    if let Some(foreground) = foreground {
        pids.push(foreground);
    }
    let mut groups = Vec::new();
    for pid in pids {
        let group = getpgid(Some(Pid::from_raw(pid)))
            .map(|g| g.as_raw())
            .unwrap_or(pid);
        if group > 1 && !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups.into_iter().map(Pid::from_raw).collect()
}

fn descendant_pids(root: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut queue = child_pids(root);
    while let Some(pid) = queue.pop() {
        if pid <= 1 || out.contains(&pid) {
            continue;
        }
        out.push(pid);
        queue.extend(child_pids(pid));
    }
    out
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
// why: libproc proc_listchildpids FFI declaration (macOS).
#[allow(unsafe_code)]
unsafe extern "C" {
    fn proc_listchildpids(ppid: i32, buffer: *mut i32, buffersize: i32) -> i32;
}

/// `proc_listchildpids` returns how many pids fit, and a full buffer may
/// have stopped early, so grow until the result is shorter than the buffer.
#[cfg(target_os = "macos")]
// why: calls the libproc proc_listchildpids FFI with a buffer we size.
#[allow(unsafe_code)]
fn child_pids(parent: i32) -> Vec<i32> {
    let mut cap = 32usize;
    loop {
        let mut buf = vec![0i32; cap];
        let bytes = buf.len() * std::mem::size_of::<i32>();
        // SAFETY: `buf` is a uniquely owned i32 buffer of `bytes` bytes.
        // libproc writes at most that many bytes of pid_t values into it.
        let n = unsafe { proc_listchildpids(parent, buf.as_mut_ptr(), bytes as i32) };
        if n <= 0 {
            return Vec::new();
        }
        let n = n as usize;
        if n < cap {
            buf.truncate(n);
            return buf;
        }
        cap = cap.saturating_mul(2);
        if cap > 4096 {
            return buf;
        }
    }
}

/// `/proc/<pid>/task/<pid>/children` is the kernel's child list. A missing
/// file (or a pid that just exited) means there is nothing to signal.
#[cfg(target_os = "linux")]
fn child_pids(parent: i32) -> Vec<i32> {
    let path = format!("/proc/{parent}/task/{parent}/children");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn child_pids(_parent: i32) -> Vec<i32> {
    Vec::new()
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::SandboxMode;
    use std::path::Path;

    const WAIT: Duration = Duration::from_secs(20);

    fn policy(mode: SandboxMode) -> SandboxPolicy {
        SandboxPolicy {
            mode,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        }
    }

    /// `/bin/sh` rather than the user's shell, so no dotfile of the
    /// developer's changes what the test sees.
    fn spec(cwd: &Path, mode: SandboxMode) -> TerminalSpec {
        TerminalSpec {
            shell: PathBuf::from("/bin/sh"),
            cwd: cwd.to_path_buf(),
            policy: policy(mode),
            roots: vec![cwd.to_path_buf()],
            writable_roots: vec![cwd.to_path_buf()],
            cols: 80,
            rows: 24,
        }
    }

    /// Reads output until it contains `needle` (true) or the shell exits
    /// (false).
    async fn read_until(term: &TerminalHandle, needle: &str) -> (bool, String) {
        let mut seen = Vec::new();
        let found = tokio::time::timeout(WAIT, async {
            while let Some(bytes) = term.next_output().await {
                seen.extend(bytes);
                if String::from_utf8_lossy(&seen).contains(needle) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        (found, String::from_utf8_lossy(&seen).into_owned())
    }

    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).expect("canonical")
    }

    #[tokio::test]
    async fn terminal_starts_in_the_session_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = canonical(dir.path()).join("project");
        std::fs::create_dir(&cwd).expect("project");
        let term = open(&spec(&cwd, SandboxMode::DangerFullAccess)).expect("open");
        term.write(b"echo \"CWD=$(pwd -P)=\"\n").expect("write");
        let (found, seen) = read_until(&term, &format!("CWD={}=", cwd.display())).await;
        assert!(found, "{seen}");
    }

    #[test]
    fn terminal_refuses_a_missing_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gone = dir.path().join("gone");
        let err = open(&spec(&gone, SandboxMode::DangerFullAccess))
            .err()
            .expect("a missing cwd is refused");
        assert!(matches!(err, TerminalError::Cwd(_)), "{err}");
    }

    /// The pane's shell runs under the session's Seatbelt profile, so a
    /// write outside the workspace fails as `bash`'s would.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    // why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
    #[allow(unsafe_code)]
    async fn terminal_write_outside_the_workspace_is_denied_under_workspace_write() {
        let base = tempfile::tempdir().expect("tempdir");
        let base_path = canonical(base.path());
        let (ws, outside, scratch) = (
            base_path.join("ws"),
            base_path.join("outside"),
            base_path.join("scratch"),
        );
        for dir in [&ws, &outside, &scratch] {
            std::fs::create_dir(dir).expect("dir");
        }
        // The profile always lets the temp dir be written; move it off
        // `base` so `outside` is really outside (nextest: own process).
        // SAFETY: before this test starts any thread that reads the env.
        unsafe { std::env::set_var("TMPDIR", &scratch) };
        let term = open(&spec(&ws, SandboxMode::WorkspaceWrite)).expect("open");
        let inside = ws.join("in.txt");
        let denied = outside.join("out.txt");
        let line = format!(
            "echo x > '{}'; echo x > '{}'; echo DONE-$((40+2))\n",
            inside.display(),
            denied.display()
        );
        term.write(line.as_bytes()).expect("write");
        let (found, seen) = read_until(&term, "DONE-42").await;
        assert!(found, "{seen}");
        assert!(inside.is_file(), "the workspace stays writable: {seen}");
        assert!(!denied.exists(), "outside the workspace is denied: {seen}");
    }

    #[test]
    fn terminal_shell_comes_from_the_allowlist() {
        // Only the name counts: the directory `$SHELL` points into is not
        // where the program is taken from.
        let shell = shell(Some("/tmp/evil/sh")).expect("sh is installed");
        assert!(!shell.starts_with("/tmp"), "{}", shell.display());
        assert_eq!(shell.file_name().and_then(|n| n.to_str()), Some("sh"));
        // A name off the allowlist falls back to the platform default.
        let fallback = shell_or_default(Some("/tmp/evil/not-a-shell"));
        assert!(!fallback.starts_with("/tmp"), "{}", fallback.display());
        assert_eq!(fallback, shell_or_default(None));
        // And the pane's argv runs exactly that path, with `-l -i`.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut spec = spec(dir.path(), SandboxMode::DangerFullAccess);
        spec.shell = shell.clone();
        let argv = argv(&spec).expect("argv");
        let program = shell.to_string_lossy().into_owned();
        assert_eq!(argv, [program.as_str(), "-l", "-i"]);
    }

    fn shell_or_default(env: Option<&str>) -> PathBuf {
        shell(env).expect("the platform default shell is installed")
    }

    #[tokio::test]
    async fn terminal_close_kills_the_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let term = open(&spec(dir.path(), SandboxMode::DangerFullAccess)).expect("open");
        term.write(b"sleep 300 & echo \"JOB=$!=\"\n")
            .expect("write");
        let (found, seen) = read_until(&term, "=\r").await;
        assert!(found, "{seen}");
        let pid = Pid::from_raw(background_pid(&seen));
        assert!(nix::sys::signal::kill(pid, None).is_ok(), "the job runs");
        term.close();
        let deadline = Instant::now() + Duration::from_secs(5);
        while nix::sys::signal::kill(pid, None).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            nix::sys::signal::kill(pid, None).is_err(),
            "the job outlived close"
        );
        assert!(term.exit_status().is_some(), "the shell was reaped");
        assert!(term.next_output_after_close_ends().await);
    }

    /// The shell ignores SIGHUP, so it cannot forward it. The job is its own
    /// process group and still has to die with the pane.
    #[tokio::test]
    async fn terminal_close_kills_a_background_job_the_shell_does_not_signal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let term = open(&spec(dir.path(), SandboxMode::DangerFullAccess)).expect("open");
        term.write(b"trap '' HUP; sleep 300 & echo \"JOB=$!=\"\n")
            .expect("write");
        let (found, seen) = read_until(&term, "=\r").await;
        assert!(found, "{seen}");
        let pid = Pid::from_raw(background_pid(&seen));
        assert!(nix::sys::signal::kill(pid, None).is_ok(), "the job runs");
        term.close();
        let deadline = Instant::now() + Duration::from_secs(5);
        while nix::sys::signal::kill(pid, None).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            nix::sys::signal::kill(pid, None).is_err(),
            "the job outlived close"
        );
    }

    fn background_pid(seen: &str) -> i32 {
        seen.split("JOB=")
            .filter_map(|rest| rest.split('=').next()?.parse().ok())
            .next()
            .expect("the job's pid")
    }

    #[tokio::test]
    async fn terminal_is_busy_only_while_a_foreground_job_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let term = open(&spec(dir.path(), SandboxMode::DangerFullAccess)).expect("open");
        term.write(b"echo READY-$((40+2))\n").expect("write");
        let (found, seen) = read_until(&term, "READY-42").await;
        assert!(found, "{seen}");
        assert!(!term.is_busy(), "an idle prompt is not busy");
        term.write(b"sleep 300\n").expect("write");
        assert!(term.busy_becomes(true).await, "sleep holds the foreground");
        // ^C ends the job and the prompt takes the foreground back.
        term.write(b"\x03").expect("write");
        assert!(term.busy_becomes(false).await, "the prompt is back");
        term.close();
        assert!(!term.is_busy(), "a closed terminal is not busy");
    }

    impl TerminalHandle {
        /// Polls `is_busy` until it answers `want` or the wait runs out.
        async fn busy_becomes(&self, want: bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.is_busy() != want && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            self.is_busy() == want
        }

        /// Drains what is left; true once the stream ended.
        async fn next_output_after_close_ends(&self) -> bool {
            tokio::time::timeout(WAIT, async { while self.next_output().await.is_some() {} })
                .await
                .is_ok()
        }
    }
}
