// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Language-server support for the `diagnostics` tool (P41). Lives in
//! `cox-tools` because talking to a server means a process and its pipes,
//! which the core never touches (D2); split into modules so the wire, the
//! server lifecycle and the tool can each be tested alone.

pub mod client;
pub mod diag;
pub mod server;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use async_trait::async_trait;
use cox_protocol::config::{LspConfig, LspServerConfig};
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::path::confine;
use client::LspError;
use server::{Options, Pipes, Server};

/// Turns a configured server into the argv to run: the sandbox wrap, which
/// lives with the session (`sandboxed_argv`), not here. `Err` is a refusal.
pub type Spawner = Arc<dyn Fn(&LspServerConfig) -> Result<Vec<String>, String> + Send + Sync>;
/// Starts a ready argv in a workspace root: [`server::spawn`], or a test's
/// in-memory fake.
pub type Launcher = Arc<dyn Fn(&[String], &Path) -> Result<Pipes, LspError> + Send + Sync>;

/// Largest file sent to a server. It travels inside one JSON message,
/// which JSON escaping can grow, under the client's 16 MiB cap.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// `[lsp] after_edit` until that key exists. Off, so an edit's text does
/// not change for users; tests opt in with [`crate::edit::LspEdit::reporting`].
pub const AFTER_EDIT: bool = false;
/// `lsp.after_edit_ms` until that key exists.
pub const AFTER_EDIT_MS: u64 = 1500;
/// Lines of new diagnostics an edit may append. Errors sort first.
const MAX_NEW_LINES: usize = 10;

/// Servers `diagnostics` has already started, shared with `edit`.
/// There is no launcher here: [`LspPool::running_for`] cannot start a process.
pub struct LspPool {
    cfg: LspConfig,
    /// Running servers by `[lsp.servers]` name.
    servers: Mutex<HashMap<String, Arc<Server>>>,
    /// Last settled diagnostics per file. `Server` keeps the same map
    /// private, so an edit diffs against this copy from before the write.
    seen: Mutex<HashMap<PathBuf, Vec<diag::Diagnostic>>>,
}

/// `diagnostics` (T41.6): a file's compiler and linter diagnostics from a
/// language server, one per language, started on first use and kept for
/// the session.
pub struct DiagnosticsTool {
    pool: Arc<LspPool>,
    spawner: Spawner,
    launcher: Launcher,
    /// One start at a time, so two parallel calls never start two servers
    /// for one language.
    starting: tokio::sync::Mutex<()>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiagnosticsInput {
    /// File to check, relative to the session `cwd` or absolute within a workspace root.
    path: String,
    /// Longest wait for the server, in milliseconds; capped by `lsp.timeout_s`.
    #[serde(default)]
    wait_ms: Option<u64>,
}

impl LspPool {
    pub fn new(cfg: LspConfig) -> Self {
        Self {
            cfg,
            servers: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// The configured server whose extensions include `path`'s.
    fn server_for(&self, path: &str) -> Option<(&String, &LspServerConfig)> {
        let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        self.cfg
            .servers
            .iter()
            .find(|(_, s)| s.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)))
    }

    /// A server already running for `path`. Never starts one: an edit must
    /// not be the call that launches a language server.
    pub fn running_for(&self, path: &Path) -> Option<Arc<Server>> {
        let name = self.server_for(&path.to_string_lossy())?.0.clone();
        let server = lock(&self.servers).get(&name).cloned()?;
        server.running().then_some(server)
    }

    pub(crate) fn cached(&self, path: &Path) -> Vec<diag::Diagnostic> {
        lock(&self.seen).get(path).cloned().unwrap_or_default()
    }

    fn remember(&self, path: &Path, diags: &[diag::Diagnostic]) {
        lock(&self.seen).insert(path.to_path_buf(), diags.to_vec());
    }

    /// Diagnostics in the post-edit result that `before` did not have.
    /// `None` when no server is running, it has died, or it misses the
    /// deadline: the edit already landed, and a language server must not
    /// turn that into an error or a retry.
    pub(crate) async fn introduced(
        &self,
        path: &Path,
        text: &str,
        before: &[diag::Diagnostic],
        roots: &[PathBuf],
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let server = self.running_for(path)?;
        let report = tokio::select! {
            r = server.diagnostics(path, text, Duration::from_millis(AFTER_EDIT_MS)) => r.ok(),
            () = cancel.cancelled() => return None,
        }?;
        // A note means the deadline won. A partial list is not the edit's
        // delta, and it must not replace the snapshot.
        if report.note.is_some() {
            return None;
        }
        self.remember(path, &report.diagnostics);
        let fresh = only_new(&report.diagnostics, before);
        if fresh.is_empty() {
            return None;
        }
        let formatted = diag::format(&root_for(path, roots, cwd), path, &fresh);
        let lines: Vec<&str> = formatted.lines().take(MAX_NEW_LINES).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

fn only_new(after: &[diag::Diagnostic], before: &[diag::Diagnostic]) -> Vec<diag::Diagnostic> {
    let seen: HashSet<(u32, String, String)> = before.iter().map(diag_key).collect();
    let mut fresh: Vec<diag::Diagnostic> = after
        .iter()
        .filter(|d| !seen.contains(&diag_key(d)))
        .cloned()
        .collect();
    fresh.sort_by_key(|d| (d.severity(), d.range.start));
    fresh.truncate(MAX_NEW_LINES);
    fresh
}

/// Set difference for an edit: start line, code and message. Column and
/// severity can move without the diagnostic being a new one.
fn diag_key(d: &diag::Diagnostic) -> (u32, String, String) {
    let code = match &d.code {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    (d.range.start.line, code, d.message.clone())
}

impl DiagnosticsTool {
    pub fn new(cfg: LspConfig, spawner: Spawner) -> Self {
        Self::with_launcher(cfg, spawner, Arc::new(server::spawn))
    }

    pub fn with_launcher(cfg: LspConfig, spawner: Spawner, launcher: Launcher) -> Self {
        Self::from_pool(Arc::new(LspPool::new(cfg)), spawner, launcher)
    }

    fn from_pool(pool: Arc<LspPool>, spawner: Spawner, launcher: Launcher) -> Self {
        Self {
            pool,
            spawner,
            launcher,
            starting: tokio::sync::Mutex::new(()),
        }
    }

    /// The session's `diagnostics` tool, on the pool `edit` also holds.
    pub fn on_pool(pool: Arc<LspPool>, spawner: Spawner) -> Self {
        Self::from_pool(pool, spawner, Arc::new(server::spawn))
    }

    #[cfg(test)]
    fn pool(&self) -> Arc<LspPool> {
        self.pool.clone()
    }

    fn running(&self, name: &str) -> bool {
        let pooled = lock(&self.pool.servers).get(name).cloned();
        pooled.is_some_and(|s| s.running())
    }

    /// The pooled server, or a new one when it is missing or has died.
    /// `Err` is the text the model gets.
    async fn server(
        &self,
        name: &str,
        cfg: &LspServerConfig,
        root: &Path,
    ) -> Result<Arc<Server>, String> {
        let _one = self.starting.lock().await;
        let pooled = lock(&self.pool.servers).get(name).cloned();
        if let Some(s) = pooled {
            if s.running() {
                return Ok(s);
            }
            self.evict(name, &s);
        }
        let argv = (self.spawner)(cfg).map_err(|why| {
            format!(
                "cannot start the `{name}` language server here: {why}. {}",
                use_bash(name)
            )
        })?;
        let pipes = (self.launcher)(&argv, root)
            .map_err(|e| format!("cannot start `{}`: {e}. {}", cfg.command, use_bash(name)))?;
        let process = pipes.process.clone();
        let opts = Options {
            root: root.to_path_buf(),
            timeout: Duration::from_secs(u64::from(self.pool.cfg.timeout_s)),
            quiet: Duration::from_millis(u64::from(self.pool.cfg.quiet_ms)),
        };
        match Server::start(pipes, opts).await {
            Ok(s) => {
                let s = Arc::new(s);
                lock(&self.pool.servers).insert(name.to_string(), s.clone());
                Ok(s)
            }
            Err(e) => Err(with_tail(
                format!(
                    "the `{name}` language server did not start: {e}. {}",
                    use_bash(name)
                ),
                &process.stderr_tail(),
            )),
        }
    }

    /// Drops `server` from the pool if it is still the pooled one, and kills it.
    fn evict(&self, name: &str, server: &Arc<Server>) {
        let mut pool = lock(&self.pool.servers);
        if pool.get(name).is_some_and(|s| Arc::ptr_eq(s, server)) {
            pool.remove(name);
        }
        drop(pool);
        server.kill();
    }
}

#[async_trait]
impl Tool for DiagnosticsTool {
    fn spec(&self) -> ToolSpec {
        let input_schema =
            serde_json::to_value(schema_for!(DiagnosticsInput)).unwrap_or(Value::Null);
        let languages: Vec<String> = self
            .pool
            .cfg
            .servers
            .iter()
            .map(|(name, s)| format!("{name} (.{})", s.extensions.join(" .")))
            .collect();
        ToolSpec {
            name: "diagnostics".to_string(),
            description: format!(
                "Returns one file's compiler and linter diagnostics from a language server, \
                 as `path:line:col: severity: message [source code]` lines, most severe \
                 first, then a summary. Pass the file after editing it to see what broke. \
                 The first call for a language starts its server, which runs the project's \
                 build scripts; later calls reuse it. `wait_ms` bounds the wait for a slow \
                 server; a partial result says so. Languages: {}. For any other file, run \
                 the project's own checker with `bash`.",
                languages.join(", ")
            ),
            input_schema,
            deferred: true,
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

    /// Starting a server runs the project's build scripts and proc macros,
    /// so the call that would start one is `Exec` and the `Engine` asks;
    /// once it runs, asking it is `ReadOnly`.
    fn risk(&self, input: &Value) -> Risk {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match self.pool.server_for(path) {
            Some((name, _)) if !self.running(name) => Risk::Exec,
            _ => Risk::ReadOnly,
        }
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: DiagnosticsInput =
            serde_json::from_value(input).map_err(|e| ToolError::Denied {
                why: format!("invalid input: {e}"),
            })?;
        let path = confine(&cx.roots, &cx.cwd, &input.path)?;
        let Some((name, cfg)) = self.pool.server_for(&path.to_string_lossy()) else {
            return Ok(failure(format!(
                "no language server is configured for {}. {}",
                input.path,
                use_bash("")
            )));
        };
        if on_path(&cfg.command).is_none() {
            return Ok(failure(format!(
                "the `{name}` language server `{}` is not on PATH. {}",
                cfg.command,
                use_bash(name)
            )));
        }
        let meta = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::NotFound,
            _ => ToolError::Io,
        })?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(ToolError::TooLarge {
                bytes: meta.len(),
                cap: MAX_FILE_BYTES,
            });
        }
        let bytes = std::fs::read(&path).map_err(|_| ToolError::Io)?;
        let text = String::from_utf8(bytes).map_err(|_| ToolError::Binary)?;
        let root = root_for(&path, &cx.roots, &cx.cwd);
        let timeout = Duration::from_secs(u64::from(self.pool.cfg.timeout_s));
        let wait = input.wait_ms.map_or(timeout, Duration::from_millis);

        let mut restarted = false;
        loop {
            let server = match self.server(name, cfg, &root).await {
                Ok(s) => s,
                Err(text) => return Ok(failure(text)),
            };
            let result = tokio::select! {
                r = server.diagnostics(&path, &text, wait) => r,
                () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            };
            match result {
                Ok(report) => {
                    self.pool.remember(&path, &report.diagnostics);
                    let mut out = diag::format(&root, &path, &report.diagnostics);
                    if let Some(note) = report.note {
                        out.push_str("\n(");
                        out.push_str(&note);
                        out.push(')');
                    }
                    return Ok(ToolOutput {
                        text: out,
                        is_error: false,
                        diff: None,
                        structured: None,
                    });
                }
                Err(e) => {
                    let dead = matches!(e, LspError::Closed) || !server.running();
                    let tail = server.stderr_tail();
                    if dead {
                        self.evict(name, &server);
                    }
                    if dead && !restarted {
                        restarted = true;
                        continue;
                    }
                    return Ok(failure(with_tail(
                        format!(
                            "the `{name}` language server failed: {e}. {}",
                            use_bash(name)
                        ),
                        &tail,
                    )));
                }
            }
        }
    }

    /// Kills every server; the next call starts a fresh one.
    fn shutdown(&self) {
        let servers: Vec<Arc<Server>> = lock(&self.pool.servers).drain().map(|(_, s)| s).collect();
        for server in servers {
            server.kill();
        }
    }
}

/// `program` as the host would find it: a path with a directory as given
/// when it is a file, a bare name on `PATH`. `pub` so `doctor` reports a
/// server as found by the same rule the tool starts it by.
pub fn on_path(program: &str) -> Option<PathBuf> {
    let bare = Path::new(program);
    if bare.components().count() > 1 {
        return bare.is_file().then(|| bare.to_path_buf());
    }
    if program.is_empty() {
        return None;
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The fallback the model gets whenever no server can answer (falsifier 1).
fn use_bash(name: &str) -> String {
    let checker = match name {
        "rust" => "`cargo check`",
        "typescript" => "`tsc --noEmit`",
        "python" => "`pyright` or `mypy`",
        "go" => "`go vet ./...`",
        _ => "its own build or lint command",
    };
    format!("Run the project's checker with `bash` instead, for example {checker}.")
}

fn with_tail(text: String, tail: &str) -> String {
    if tail.is_empty() {
        text
    } else {
        format!("{text}\nserver stderr (last lines):\n{tail}")
    }
}

fn failure(text: String) -> ToolOutput {
    ToolOutput {
        text,
        is_error: true,
        diff: None,
        structured: None,
    }
}

/// The workspace root `path` (already confined) lies under, as given or
/// canonical, so the printed path is relative to it.
fn root_for(path: &Path, roots: &[PathBuf], cwd: &Path) -> PathBuf {
    roots
        .iter()
        .flat_map(|r| [Some(r.clone()), std::fs::canonicalize(r).ok()])
        .flatten()
        .find(|r| path.starts_with(r))
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cox_protocol::{
        Archive, ArchiveId, ArchivePut, CallId, SandboxMode, SandboxPolicy, SessionId, StoreError,
    };
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::lsp::client::read_message;
    use crate::lsp::server::tests::{
        FakeRead, FakeWrite, diag, fake_pipes, handshake, push, reply,
    };

    struct NoopArchive;

    #[async_trait]
    impl Archive for NoopArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn cx(root: &Path) -> ToolCx {
        let (tx, _rx) = mpsc::channel(16);
        ToolCx {
            roots: vec![root.to_path_buf()],
            writable_roots: vec![root.to_path_buf()],
            cwd: root.to_path_buf(),
            sandbox: SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            archive: Arc::new(NoopArchive),
            cancel: CancellationToken::new(),
            output: tx,
            session: SessionId::new(),
            call: CallId::new(),
            agent: None,
            preset: None,
            relay: None,
        }
    }

    /// Answers a save with one error per `ERR ` line of the text just sent,
    /// or `fake: first line` when the file has none (the older tests).
    async fn fake_server(mut r: FakeRead, mut w: FakeWrite) {
        handshake(&mut r, &mut w, json!({"textDocumentSync": 1})).await;
        let mut uri = Value::Null;
        let mut text = String::new();
        while let Ok(Some(msg)) = read_message(&mut r).await {
            match msg["method"].as_str() {
                Some("textDocument/didOpen") => {
                    uri = msg["params"]["textDocument"]["uri"].clone();
                    text = msg["params"]["textDocument"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
                Some("textDocument/didChange") => {
                    text = msg["params"]["contentChanges"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
                Some("textDocument/didSave") => {
                    let mut diagnostics: Vec<Value> = text
                        .lines()
                        .filter_map(|line| line.strip_prefix("ERR ").map(diag))
                        .collect();
                    if diagnostics.is_empty() {
                        diagnostics.push(diag("fake: first line"));
                    }
                    push(
                        &mut w,
                        "textDocument/publishDiagnostics",
                        json!({"uri": uri, "diagnostics": diagnostics}),
                    )
                    .await;
                }
                Some("shutdown") => reply(&mut w, &msg, Value::Null).await,
                _ => {}
            }
        }
    }

    /// Kill flags of every fake the launcher started.
    type Started = Arc<Mutex<Vec<Arc<AtomicBool>>>>;

    /// A tool whose servers are `servers` (name, command, extension) and
    /// whose launcher starts in-memory fakes.
    fn tool(servers: &[(&str, &str, &str)]) -> (DiagnosticsTool, Started) {
        let cfg = LspConfig {
            servers: servers
                .iter()
                .map(|(name, command, ext)| {
                    let server = LspServerConfig {
                        command: command.to_string(),
                        args: vec![],
                        extensions: vec![ext.to_string()],
                    };
                    (name.to_string(), server)
                })
                .collect(),
            ..LspConfig::default()
        };
        let started: Started = Arc::default();
        let log = started.clone();
        let spawner: Spawner = Arc::new(|s: &LspServerConfig| Ok(vec![s.command.clone()]));
        let launcher: Launcher = Arc::new(move |_argv: &[String], _root: &Path| {
            let (pipes, r, w, killed) = fake_pipes();
            lock(&log).push(killed);
            tokio::spawn(fake_server(r, w));
            Ok(pipes)
        });
        (
            DiagnosticsTool::with_launcher(cfg, spawner, launcher),
            started,
        )
    }

    fn workspace(files: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "fn main() {}\n").unwrap();
        }
        (dir, root)
    }

    #[tokio::test(start_paused = true)]
    async fn no_server_for_extension_names_bash() {
        let (_dir, root) = workspace(&["notes.zzz", "src/a.rs"]);
        let (tool, started) = tool(&[("rust", "cox-no-such-language-server", "rs")]);

        let none = tool
            .call(json!({"path": "notes.zzz"}), &cx(&root))
            .await
            .unwrap();
        assert!(none.is_error);
        assert!(none.text.contains("`bash`"), "{}", none.text);

        let missing = tool
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        assert!(missing.is_error);
        assert!(missing.text.contains("not on PATH"), "{}", missing.text);
        assert!(missing.text.contains("`bash`"), "{}", missing.text);
        assert!(missing.text.contains("cargo check"), "{}", missing.text);
        assert!(lock(&started).is_empty(), "nothing was started");
    }

    #[tokio::test(start_paused = true)]
    async fn path_outside_workspace_is_confined() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (tool, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let out = tool
            .call(json!({"path": "../../elsewhere/b.rs"}), &cx(&root))
            .await;
        assert!(matches!(out, Err(ToolError::Confined { .. })), "{out:?}");
        assert!(lock(&started).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn risk_is_exec_until_server_runs() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (tool, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let input = json!({"path": "src/a.rs"});
        assert_eq!(tool.risk(&input), Risk::Exec);
        assert_eq!(tool.risk(&json!({"path": "notes.zzz"})), Risk::ReadOnly);
        assert_eq!(tool.spec().risk, Risk::ReadOnly);
        assert!(tool.spec().deferred);

        let out = tool.call(input.clone(), &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "src/a.rs:1:1: error: fake: first line\n1 error");
        assert_eq!(tool.risk(&input), Risk::ReadOnly);

        // A second call reuses the running server.
        tool.call(input.clone(), &cx(&root)).await.unwrap();
        assert_eq!(lock(&started).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_every_server() {
        let (_dir, root) = workspace(&["src/a.rs", "b.py"]);
        let (tool, started) = tool(&[("rust", "/bin/sh", "rs"), ("python", "/bin/sh", "py")]);
        for path in ["src/a.rs", "b.py"] {
            let out = tool.call(json!({"path": path}), &cx(&root)).await.unwrap();
            assert!(!out.is_error, "{}", out.text);
        }
        let flags: Vec<Arc<AtomicBool>> = lock(&started).clone();
        assert_eq!(flags.len(), 2);
        assert!(flags.iter().all(|k| !k.load(Ordering::SeqCst)));

        tool.shutdown();
        assert!(flags.iter().all(|k| k.load(Ordering::SeqCst)));
        // The next call starts a fresh server, so it asks again.
        assert_eq!(tool.risk(&json!({"path": "src/a.rs"})), Risk::Exec);
    }

    fn seeded(body: &str) -> (tempfile::TempDir, PathBuf) {
        let (dir, root) = workspace(&["src/a.rs"]);
        std::fs::write(root.join("src/a.rs"), body).unwrap();
        (dir, root)
    }

    /// Diagnostics first, so the server is running and the pre-edit errors
    /// are in the pool, then one opted-in or quiet edit.
    async fn edit_after_check(report: bool, new: &str) -> String {
        let (_dir, root) = seeded("fn main() {}\nERR existed\n");
        let (diag_tool, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let saw = diag_tool
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        assert!(saw.text.contains("existed"), "{}", saw.text);
        let edit = if report {
            crate::edit::LspEdit::reporting(diag_tool.pool())
        } else {
            crate::edit::LspEdit::sharing(diag_tool.pool())
        };
        let out = edit
            .call(
                json!({"path": "src/a.rs", "old": "fn main() {}", "new": new}),
                &cx(&root),
            )
            .await
            .unwrap();
        assert_eq!(lock(&started).len(), 1, "edit must not start a server");
        out.text
    }

    #[tokio::test(start_paused = true)]
    async fn edit_appends_a_diagnostic_the_change_introduced() {
        let text = edit_after_check(true, "fn main() {}\nERR introduced").await;
        assert!(text.contains("error: introduced"), "{text}");
        assert!(!text.contains("existed"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn edit_omits_diagnostics_that_existed_before_the_change() {
        let text = edit_after_check(true, "fn ready() {}").await;
        assert!(!text.contains("existed"), "{text}");
        assert!(!text.contains("error:"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn edit_adds_nothing_when_no_language_server_is_running() {
        let (_dir, root) = seeded("fn main() {}\n");
        let (diag_tool, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let edit = crate::edit::LspEdit::reporting(diag_tool.pool());
        let out = edit
            .call(
                json!({"path": "src/a.rs", "old": "fn main() {}", "new": "fn ready() {}"}),
                &cx(&root),
            )
            .await
            .unwrap();
        assert_eq!(
            out.text,
            format!("edited {}", root.join("src/a.rs").display())
        );
        assert!(lock(&started).is_empty(), "edit must not spawn");
    }

    #[tokio::test(start_paused = true)]
    async fn edit_sharing_the_pool_does_not_append_diagnostics() {
        let text = edit_after_check(false, "fn main() {}\nERR introduced").await;
        assert!(!text.contains("introduced"), "{text}");
        assert!(!text.contains("error:"), "{text}");
    }

    #[test]
    fn on_path_finds_a_bare_name_and_a_path() {
        assert!(on_path("sh").is_some());
        assert_eq!(on_path("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert_eq!(on_path("cox-no-such-language-server"), None);
        assert_eq!(on_path(""), None);
    }
}
