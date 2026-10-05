// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One language server's lifecycle (T41.4): start it from a ready argv (the
//! caller has already wrapped it in the sandbox), `initialize` it with the
//! workspace root, keep its view of each file in sync, collect a file's
//! diagnostics by pull or by push with a quiet period, and stop it. The
//! process sits behind [`Process`] and [`Pipes`], so the protocol logic is
//! tested over an in-memory pipe and the tool (T41.6) can be given a fake.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cox_protocol::config::CHILD_ENV_ALLOWLIST;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use super::client::{Client, LspError, Notification};
use super::diag::{Diagnostic, PublishDiagnosticsParams, path_for, uri_for};

/// How much of the server's stderr is kept for error text.
const STDERR_TAIL: usize = 4 * 1024;
/// `shutdown` is a courtesy before the kill, so it gets little time.
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// The running server process, as the [`Server`] needs it.
pub trait Process: Send + Sync {
    /// Whether it is still running.
    fn running(&self) -> bool;
    /// The last bytes it wrote to stderr.
    fn stderr_tail(&self) -> String;
    /// Kills it and everything it started. Idempotent.
    fn kill(&self);
}

/// A started server: its stdout, its stdin and the process behind them.
pub struct Pipes {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// Shared so a caller can still read the stderr tail of a server
    /// whose `initialize` failed.
    pub process: Arc<dyn Process>,
}

/// Starts `argv` in `cwd` in its own process group, with only the child
/// env allowlist (D14) that `bash` and MCP servers get, stdio piped and
/// stderr kept as a bounded tail.
pub fn spawn(argv: &[String], cwd: &Path) -> Result<Pipes, LspError> {
    let Some((program, args)) = argv.split_first() else {
        return Err(LspError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty language server argv",
        )));
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).current_dir(cwd).env_clear();
    for key in CHILD_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows gets a job object instead (T57.6); until then only the
    // leader dies there.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut cmd = tokio::process::Command::from(cmd);
    // The backstop for an exit path that never calls `stop`.
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(LspError::Closed);
    };
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    tokio::spawn(keep_tail(stderr, tail.clone()));
    Ok(Pipes {
        reader: Box::new(stdout),
        writer: Box::new(stdin),
        process: Arc::new(Child {
            pid: child.id(),
            child: Mutex::new(child),
            tail,
            killed: AtomicBool::new(false),
        }),
    })
}

/// Reads stderr to the end, so a chatty server never blocks on a full
/// pipe, keeping the last `STDERR_TAIL` bytes.
async fn keep_tail(mut stderr: impl AsyncRead + Unpin, tail: Arc<Mutex<VecDeque<u8>>>) {
    let mut buf = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let mut tail = lock(&tail);
        tail.extend(&buf[..n]);
        let over = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..over);
    }
}

struct Child {
    #[cfg_attr(
        windows,
        expect(dead_code, reason = "T57.6 kills the tree through a job object")
    )]
    pid: Option<u32>,
    child: Mutex<tokio::process::Child>,
    tail: Arc<Mutex<VecDeque<u8>>>,
    killed: AtomicBool,
}

impl Process for Child {
    fn running(&self) -> bool {
        matches!(lock(&self.child).try_wait(), Ok(None))
    }

    fn stderr_tail(&self) -> String {
        let tail = lock(&self.tail);
        let (a, b) = tail.as_slices();
        String::from_utf8_lossy(&[a, b].concat()).trim().to_string()
    }

    fn kill(&self) {
        if self.killed.swap(true, Ordering::SeqCst) {
            return;
        }
        // The group, not just the leader: a sandbox wrapper or a server
        // that forks (rust-analyzer's `cargo check`) leaves children.
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            crate::bash::kill_group(pid);
        }
        let _ = lock(&self.child).start_kill();
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Timings and the workspace a server is started for.
#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    /// Deadline for `initialize` and for any one `diagnostics` call.
    pub timeout: Duration,
    /// How long after the last pushed diagnostics a result counts as done.
    pub quiet: Duration,
}

/// A file's diagnostics, and a note when the server had not settled.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    /// Set when the deadline passed first: what arrived is returned as is.
    pub note: Option<String>,
}

/// One initialized language server.
pub struct Server {
    client: Client,
    process: Arc<dyn Process>,
    /// One call at a time: document versions and the notification stream
    /// belong to the server, not to a call.
    state: tokio::sync::Mutex<State>,
    /// The server advertised `diagnosticProvider`, so it is asked (pull).
    pull: bool,
    opts: Options,
    stopped: AtomicBool,
}

struct State {
    notes: UnboundedReceiver<Notification>,
    versions: HashMap<PathBuf, i32>,
    /// The latest diagnostics each file got, pushed or pulled.
    latest: HashMap<PathBuf, Vec<Diagnostic>>,
    /// `$/progress` tokens begun and not yet ended: the server is working.
    busy: HashSet<String>,
}

impl Server {
    /// Connects to started pipes and completes `initialize`/`initialized`.
    pub async fn start(pipes: Pipes, opts: Options) -> Result<Self, LspError> {
        let (client, notes) = Client::start(pipes.reader, pipes.writer);
        let root_uri = uri_for(&opts.root);
        let name = opts
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let params = json!({
            // `null`, not cox's pid: a sandboxed server may not see cox's
            // pid and would take its absence for a dead parent and exit.
            "processId": null,
            "clientInfo": {"name": "cox", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                // Without this a server may never send `$/progress`, so
                // there is nothing to wait on for a freshly started one
                // (research.md §4.8).
                "window": {"workDoneProgress": true},
                "workspace": {"workspaceFolders": true, "configuration": true},
                "textDocument": {
                    "synchronization": {"didSave": true},
                    "publishDiagnostics": {"versionSupport": true},
                    "diagnostic": {"dynamicRegistration": false},
                },
            },
        });
        let result = client.request("initialize", params, opts.timeout).await;
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                pipes.process.kill();
                return Err(e);
            }
        };
        let pull = result
            .pointer("/capabilities/diagnosticProvider")
            .is_some_and(|p| !p.is_null() && *p != Value::Bool(false));
        client.notify("initialized", json!({})).await?;
        Ok(Self {
            client,
            process: pipes.process,
            state: tokio::sync::Mutex::new(State {
                notes,
                versions: HashMap::new(),
                latest: HashMap::new(),
                busy: HashSet::new(),
            }),
            pull,
            opts,
            stopped: AtomicBool::new(false),
        })
    }

    /// Syncs `text` as the content of `path` (absolute) and returns its
    /// diagnostics, waiting at most `wait` and at most the timeout. A
    /// deadline is not an error: what arrived comes back with a note.
    pub async fn diagnostics(
        &self,
        path: &Path,
        text: &str,
        wait: Duration,
    ) -> Result<Report, LspError> {
        let uri = uri_for(path).ok_or_else(|| {
            LspError::Parse(format!("{} is not an absolute path", path.display()))
        })?;
        let budget = wait.min(self.opts.timeout);
        let deadline = Instant::now() + budget;
        let mut st = self.state.lock().await;
        // Pushes that arrived since the last call are older than this one.
        while let Ok(note) = st.notes.try_recv() {
            st.absorb(note, path, 0);
        }
        let version = match st.versions.get(path).copied() {
            None => {
                let open = json!({"textDocument": {
                    "uri": uri, "languageId": language_id(path), "version": 1, "text": text,
                }});
                self.client.notify("textDocument/didOpen", open).await?;
                1
            }
            Some(prev) => {
                let change = json!({
                    "textDocument": {"uri": uri, "version": prev + 1},
                    "contentChanges": [{"text": text}],
                });
                self.client.notify("textDocument/didChange", change).await?;
                prev + 1
            }
        };
        st.versions.insert(path.to_path_buf(), version);
        // rust-analyzer runs its check on save, not on change.
        let save = json!({"textDocument": {"uri": uri}});
        self.client.notify("textDocument/didSave", save).await?;

        if self.pull {
            loop {
                match self.pull_once(&mut st, &uri, path, deadline).await {
                    Ok(()) => {}
                    Err(LspError::Timeout { .. }) => return Ok(partial(&st, path, budget)),
                    // A server may cancel a pull it now considers stale
                    // (rust-analyzer does this while still loading, LSP's
                    // reserved `RequestCancelled`/`ContentModified` range):
                    // that is not a failure, just an empty answer to retry.
                    Err(LspError::Server { code, .. }) if (-32802..=-32800).contains(&code) => {}
                    Err(e) => return Err(e),
                }
                // A freshly opened file's empty pull may predate the
                // server finishing its load (research.md §4.8): its
                // indexing can end (or the server can ask for a re-pull)
                // in more than one wave before the slower check behind the
                // diagnostics actually finishes, so keep re-pulling on
                // each such sign, bounded by the same deadline as
                // everything else here. A server that reports neither
                // still answers empty, unchanged, once the deadline
                // passes.
                let worth_a_retry = version == 1 && st.latest.get(path).is_none_or(Vec::is_empty);
                if !worth_a_retry || !wait_for_pull_trigger(&mut st, path, version, deadline).await
                {
                    break;
                }
            }
            return Ok(Report {
                diagnostics: st.latest.get(path).cloned().unwrap_or_default(),
                note: None,
            });
        }

        let mut got = false;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(partial(&st, path, budget));
            }
            // Before the first push, wait for it; after it, only for the
            // quiet period, unless the server reports work in progress.
            let settle = got && st.busy.is_empty();
            let span = if settle {
                left.min(self.opts.quiet)
            } else {
                left
            };
            match tokio::time::timeout(span, st.notes.recv()).await {
                Ok(Some(note)) => got |= st.absorb(note, path, version),
                Ok(None) => return Err(LspError::Closed),
                Err(_) if settle && span < left => {
                    return Ok(Report {
                        diagnostics: st.latest.get(path).cloned().unwrap_or_default(),
                        note: None,
                    });
                }
                Err(_) => {}
            }
        }
    }

    /// One `textDocument/diagnostic` pull; stores a non-`unchanged` result.
    async fn pull_once(
        &self,
        st: &mut State,
        uri: &str,
        path: &Path,
        deadline: Instant,
    ) -> Result<(), LspError> {
        let left = deadline.saturating_duration_since(Instant::now());
        let params = json!({"textDocument": {"uri": uri}});
        match self
            .client
            .request("textDocument/diagnostic", params, left)
            .await
        {
            // `unchanged` means the last report still holds.
            Ok(report) if report["kind"] == "unchanged" => Ok(()),
            Ok(report) => {
                let items = report.get("items").cloned().unwrap_or(Value::Null);
                let diags: Vec<Diagnostic> = serde_json::from_value(items)
                    .map_err(|e| LspError::Parse(format!("diagnostic report: {e}")))?;
                st.latest.insert(path.to_path_buf(), diags);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Whether the process runs and `stop`/`kill` has not been called.
    pub fn running(&self) -> bool {
        !self.stopped.load(Ordering::SeqCst) && self.process.running()
    }

    /// The server's last stderr bytes, for error text.
    pub fn stderr_tail(&self) -> String {
        self.process.stderr_tail()
    }

    /// `shutdown`, `exit`, then a group kill. Idempotent.
    pub async fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self
            .client
            .request("shutdown", Value::Null, STOP_TIMEOUT)
            .await;
        let _ = self.client.notify("exit", Value::Null).await;
        self.process.kill();
    }

    /// The group kill alone, for a caller that cannot await (`Tool::shutdown`).
    pub fn kill(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.process.kill();
    }
}

impl State {
    /// Records one notification; true when it is a push for `path` at
    /// `version` or newer (a server without version support counts too).
    fn absorb(&mut self, note: Notification, path: &Path, version: i32) -> bool {
        match note.method.as_str() {
            "textDocument/publishDiagnostics" => {
                let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(note.params) else {
                    return false;
                };
                let Some(file) = path_for(&p.uri) else {
                    return false;
                };
                // A push for an older version than the one just synced is
                // about text the server no longer has.
                let fresh = p.version.is_none_or(|v| v >= version) && same_file(&file, path);
                if fresh {
                    self.latest.insert(path.to_path_buf(), p.diagnostics);
                }
                fresh
            }
            "$/progress" => {
                let token = match note.params.get("token") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => return false,
                };
                match note.params.pointer("/value/kind").and_then(Value::as_str) {
                    Some("begin") => {
                        self.busy.insert(token);
                    }
                    Some("end") => {
                        self.busy.remove(&token);
                    }
                    _ => {}
                }
                false
            }
            _ => false,
        }
    }
}

/// Waits, bounded by `deadline`, for a sign that a first empty pull for
/// `path` was premature: the server's `$/progress` ending after it began,
/// or it asking to pull again (`workspace/diagnostic/refresh`). Mirrors how
/// push mode already waits the full deadline for its first push (research
/// shows rust-analyzer's indexing runs several seconds, well past a short
/// grace period); a server that reports neither leaves the deadline to
/// pass and the original, empty pull stands (research.md §4.8).
async fn wait_for_pull_trigger(
    st: &mut State,
    path: &Path,
    version: i32,
    deadline: Instant,
) -> bool {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let Ok(Some(note)) = tokio::time::timeout(left, st.notes.recv()).await else {
            return false;
        };
        let refresh = note.method == "workspace/diagnostic/refresh";
        let was_busy = !st.busy.is_empty();
        st.absorb(note, path, version);
        if refresh || (was_busy && st.busy.is_empty()) {
            return true;
        }
    }
}

/// What arrived for `path` before the deadline, with the note that says so.
fn partial(st: &State, path: &Path, waited: Duration) -> Report {
    Report {
        diagnostics: st.latest.get(path).cloned().unwrap_or_default(),
        note: Some(format!(
            "the language server was still working after {} ms; these diagnostics \
             may be incomplete (call again with a larger wait_ms)",
            waited.as_millis()
        )),
    }
}

/// A server may echo a path through a symlink (`/var` vs `/private/var`).
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// The LSP `languageId` for a file, from its extension.
fn language_id(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "py" => "python",
        "go" => "go",
        other => other,
    }
    .to_string()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::lsp::client::{read_message, write_message};
    use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf, duplex, split};

    pub(crate) type FakeRead = BufReader<ReadHalf<DuplexStream>>;
    pub(crate) type FakeWrite = WriteHalf<DuplexStream>;

    /// A process that only records whether it was killed.
    pub(crate) struct FakeProcess(pub(crate) Arc<AtomicBool>);

    impl Process for FakeProcess {
        fn running(&self) -> bool {
            !self.0.load(Ordering::SeqCst)
        }
        fn stderr_tail(&self) -> String {
            "fake stderr".to_string()
        }
        fn kill(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Pipes whose other end is the fake server's, and the kill flag.
    pub(crate) fn fake_pipes() -> (Pipes, FakeRead, FakeWrite, Arc<AtomicBool>) {
        let (ours, theirs) = duplex(256 * 1024);
        let (r, w) = split(ours);
        let (sr, sw) = split(theirs);
        let killed = Arc::new(AtomicBool::new(false));
        let pipes = Pipes {
            reader: Box::new(r),
            writer: Box::new(w),
            process: Arc::new(FakeProcess(killed.clone())),
        };
        (pipes, BufReader::new(sr), sw, killed)
    }

    /// Reads messages until one with `method`, returning it whole.
    pub(crate) async fn expect(r: &mut FakeRead, method: &str) -> Value {
        loop {
            let msg = read_message(r).await.unwrap().expect("client hung up");
            if msg["method"] == method {
                return msg;
            }
        }
    }

    pub(crate) async fn reply(w: &mut FakeWrite, req: &Value, result: Value) {
        let msg = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
        write_message(w, &msg).await.unwrap();
    }

    pub(crate) async fn push(w: &mut FakeWrite, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        write_message(w, &msg).await.unwrap();
    }

    pub(crate) fn diag(message: &str) -> Value {
        json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "severity": 1,
            "message": message,
        })
    }

    /// Answers `initialize` with `capabilities`, then waits for `initialized`.
    pub(crate) async fn handshake(r: &mut FakeRead, w: &mut FakeWrite, capabilities: Value) {
        let init = expect(r, "initialize").await;
        assert_eq!(init["params"]["processId"], Value::Null);
        reply(w, &init, json!({"capabilities": capabilities})).await;
        expect(r, "initialized").await;
    }

    fn opts(quiet_ms: u64) -> Options {
        Options {
            root: PathBuf::from("/work"),
            timeout: Duration::from_secs(30),
            quiet: Duration::from_millis(quiet_ms),
        }
    }

    const FILE: &str = "/work/src/a.rs";
    const URI: &str = "file:///work/src/a.rs";

    #[tokio::test(start_paused = true)]
    async fn push_diagnostics_collected_after_quiet_period() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            handshake(&mut r, &mut w, json!({"textDocumentSync": 1})).await;
            let open = expect(&mut r, "textDocument/didOpen").await;
            assert_eq!(open["params"]["textDocument"]["languageId"], "rust");
            expect(&mut r, "textDocument/didSave").await;
            let uri = URI;
            push(
                &mut w,
                "textDocument/publishDiagnostics",
                json!({"uri": uri, "diagnostics": [diag("first")]}),
            )
            .await;
            // Inside the quiet period: this one replaces the first.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let both =
                json!({"uri": uri, "version": 1, "diagnostics": [diag("first"), diag("second")]});
            push(&mut w, "textDocument/publishDiagnostics", both).await;
            (r, w)
        });
        let server = Server::start(pipes, opts(500)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "fn main() {}\n", Duration::from_secs(10))
            .await
            .unwrap();
        let messages: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(messages, ["first", "second"]);
        assert_eq!(report.note, None);
        drop(fake);
    }

    #[tokio::test(start_paused = true)]
    async fn pull_used_when_advertised() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            let caps = json!({"diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}});
            handshake(&mut r, &mut w, caps).await;
            expect(&mut r, "textDocument/didOpen").await;
            expect(&mut r, "textDocument/didSave").await;
            let req = expect(&mut r, "textDocument/diagnostic").await;
            assert_eq!(req["params"]["textDocument"]["uri"], URI);
            reply(
                &mut w,
                &req,
                json!({"kind": "full", "items": [diag("pulled")]}),
            )
            .await;
            (r, w)
        });
        let server = Server::start(pipes, opts(500)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].message, "pulled");
        assert_eq!(report.note, None);
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn pull_retries_after_indexing_ends_when_the_first_result_is_empty() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            let init = expect(&mut r, "initialize").await;
            assert_eq!(
                init["params"]["capabilities"]["window"]["workDoneProgress"],
                true
            );
            let caps = json!({"diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}});
            reply(&mut w, &init, json!({"capabilities": caps})).await;
            expect(&mut r, "initialized").await;
            expect(&mut r, "textDocument/didOpen").await;
            expect(&mut r, "textDocument/didSave").await;
            // Too early: the server has not loaded the crate yet.
            let first = expect(&mut r, "textDocument/diagnostic").await;
            reply(&mut w, &first, json!({"kind": "full", "items": []})).await;
            let begin = json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "begin"}});
            push(&mut w, "$/progress", begin).await;
            let end = json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "end"}});
            push(&mut w, "$/progress", end).await;
            // The re-pull now finds the real error.
            let second = expect(&mut r, "textDocument/diagnostic").await;
            reply(
                &mut w,
                &second,
                json!({"kind": "full", "items": [diag("E0308")]}),
            )
            .await;
            (r, w)
        });
        let server = Server::start(pipes, opts(50)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].message, "E0308");
        assert_eq!(report.note, None);
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn pull_retries_after_a_refresh_request_when_the_first_result_is_empty() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            let caps = json!({"diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}});
            handshake(&mut r, &mut w, caps).await;
            expect(&mut r, "textDocument/didOpen").await;
            expect(&mut r, "textDocument/didSave").await;
            let first = expect(&mut r, "textDocument/diagnostic").await;
            reply(&mut w, &first, json!({"kind": "full", "items": []})).await;
            // The server asks the client to pull again instead of pushing.
            write_message(
                &mut w,
                &json!({"jsonrpc": "2.0", "id": 9000, "method": "workspace/diagnostic/refresh"}),
            )
            .await
            .unwrap();
            let second = expect(&mut r, "textDocument/diagnostic").await;
            reply(
                &mut w,
                &second,
                json!({"kind": "full", "items": [diag("E0308")]}),
            )
            .await;
            (r, w)
        });
        let server = Server::start(pipes, opts(50)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(report.diagnostics[0].message, "E0308");
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn pull_retries_after_the_server_cancels_a_stale_pull() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            let caps = json!({"diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}});
            handshake(&mut r, &mut w, caps).await;
            expect(&mut r, "textDocument/didOpen").await;
            expect(&mut r, "textDocument/didSave").await;
            // rust-analyzer cancels a pull it now considers stale instead
            // of answering it, while it is still loading.
            let first = expect(&mut r, "textDocument/diagnostic").await;
            write_message(
                &mut w,
                &json!({"jsonrpc": "2.0", "id": first["id"], "error": {
                    "code": -32801, "message": "content modified",
                }}),
            )
            .await
            .unwrap();
            let begin = json!({"token": "t", "value": {"kind": "begin"}});
            push(&mut w, "$/progress", begin).await;
            let end = json!({"token": "t", "value": {"kind": "end"}});
            push(&mut w, "$/progress", end).await;
            let second = expect(&mut r, "textDocument/diagnostic").await;
            reply(
                &mut w,
                &second,
                json!({"kind": "full", "items": [diag("E0308")]}),
            )
            .await;
            (r, w)
        });
        let server = Server::start(pipes, opts(50)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(report.diagnostics[0].message, "E0308");
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn pull_with_no_progress_or_refresh_stands_once_the_deadline_passes() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            let caps = json!({"diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}});
            handshake(&mut r, &mut w, caps).await;
            expect(&mut r, "textDocument/didOpen").await;
            expect(&mut r, "textDocument/didSave").await;
            let req = expect(&mut r, "textDocument/diagnostic").await;
            reply(&mut w, &req, json!({"kind": "full", "items": []})).await;
            (r, w)
        });
        let server = Server::start(pipes, opts(50)).await.unwrap();
        let start = Instant::now();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(10))
            .await
            .unwrap();
        // No progress and no refresh ever arrived: the wait (bounded by
        // the same 10 s deadline as everything else) elapses and the
        // original, empty pull result stands, exactly as it does today.
        assert!(report.diagnostics.is_empty());
        assert_eq!(report.note, None);
        let elapsed = Instant::now() - start;
        assert!(elapsed >= Duration::from_secs(9), "{elapsed:?}");
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn second_call_sends_did_change_with_next_version() {
        let (pipes, mut r, mut w, killed) = fake_pipes();
        let fake = tokio::spawn(async move {
            handshake(&mut r, &mut w, json!({"textDocumentSync": 1})).await;
            let open = expect(&mut r, "textDocument/didOpen").await;
            assert_eq!(open["params"]["textDocument"]["version"], 1);
            assert_eq!(open["params"]["textDocument"]["text"], "one");
            expect(&mut r, "textDocument/didSave").await;
            let first = json!({"uri": URI, "version": 1, "diagnostics": [diag("v1")]});
            push(&mut w, "textDocument/publishDiagnostics", first).await;
            let change = expect(&mut r, "textDocument/didChange").await;
            assert_eq!(change["params"]["textDocument"]["version"], 2);
            assert_eq!(change["params"]["contentChanges"][0]["text"], "two");
            expect(&mut r, "textDocument/didSave").await;
            // A late push for version 1 does not end the wait for version 2.
            let stale = json!({"uri": URI, "version": 1, "diagnostics": [diag("stale")]});
            push(&mut w, "textDocument/publishDiagnostics", stale).await;
            let second = json!({"uri": URI, "version": 2, "diagnostics": [diag("v2")]});
            push(&mut w, "textDocument/publishDiagnostics", second).await;
            let shutdown = expect(&mut r, "shutdown").await;
            reply(&mut w, &shutdown, Value::Null).await;
            expect(&mut r, "exit").await;
            (r, w)
        });
        let server = Server::start(pipes, opts(200)).await.unwrap();
        let wait = Duration::from_secs(10);
        let one = server
            .diagnostics(Path::new(FILE), "one", wait)
            .await
            .unwrap();
        assert_eq!(one.diagnostics[0].message, "v1");
        let two = server
            .diagnostics(Path::new(FILE), "two", wait)
            .await
            .unwrap();
        assert_eq!(two.diagnostics[0].message, "v2");
        server.stop().await;
        server.stop().await;
        assert!(killed.load(Ordering::SeqCst));
        assert!(!server.running());
        fake.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_returns_partial_with_note() {
        let (pipes, mut r, mut w, _) = fake_pipes();
        let fake = tokio::spawn(async move {
            handshake(&mut r, &mut w, json!({"textDocumentSync": 1})).await;
            expect(&mut r, "textDocument/didSave").await;
            // Work begins and never ends, so the quiet period never counts.
            let begin = json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "begin", "title": "Indexing"}});
            push(&mut w, "$/progress", begin).await;
            let early = json!({"uri": URI, "diagnostics": [diag("early")]});
            push(&mut w, "textDocument/publishDiagnostics", early).await;
            (r, w)
        });
        let server = Server::start(pipes, opts(50)).await.unwrap();
        let report = server
            .diagnostics(Path::new(FILE), "x", Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].message, "early");
        let note = report.note.expect("a deadline leaves a note");
        assert!(note.contains("still working"), "{note}");
        drop(fake);
    }
}
