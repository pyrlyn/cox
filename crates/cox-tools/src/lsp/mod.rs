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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::config::{LspConfig, LspServerConfig};
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::path::confine;
use client::LspError;
use diag::Diagnostic;
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

/// The session's language servers, one per language. Shared because
/// `diagnostics` starts them (T41.6) while `edit`/`write` only ask one that
/// already runs (T59.3): an edit must never run a project's build scripts.
pub struct LspPool {
    cfg: LspConfig,
    spawner: Spawner,
    launcher: Launcher,
    /// Running servers by `[lsp.servers]` name.
    pool: Mutex<HashMap<String, Arc<Server>>>,
    /// One start at a time, so two parallel calls never start two servers
    /// for one language.
    starting: tokio::sync::Mutex<()>,
}

/// `diagnostics` (T41.6): a file's compiler and linter diagnostics from a
/// language server, one per language, started on first use and kept for
/// the session.
pub struct DiagnosticsTool {
    pool: Arc<LspPool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiagnosticsInput {
    /// File to check, relative to the session `cwd` or absolute within a workspace root.
    path: String,
    /// Longest wait for the server, in milliseconds; capped by `lsp.timeout_s`.
    #[serde(default)]
    wait_ms: Option<u64>,
}

impl DiagnosticsTool {
    pub fn new(cfg: LspConfig, spawner: Spawner) -> Self {
        Self::from_pool(Arc::new(LspPool::new(cfg, spawner)))
    }

    pub fn with_launcher(cfg: LspConfig, spawner: Spawner, launcher: Launcher) -> Self {
        Self::from_pool(Arc::new(LspPool::with_launcher(cfg, spawner, launcher)))
    }

    pub fn from_pool(pool: Arc<LspPool>) -> Self {
        Self { pool }
    }
}

impl LspPool {
    pub fn new(cfg: LspConfig, spawner: Spawner) -> Self {
        Self::with_launcher(cfg, spawner, Arc::new(server::spawn))
    }

    pub fn with_launcher(cfg: LspConfig, spawner: Spawner, launcher: Launcher) -> Self {
        Self {
            cfg,
            spawner,
            launcher,
            pool: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
        }
    }

    /// The running server for `path`'s language. Never starts one.
    pub fn running_for(&self, path: &Path) -> Option<Arc<Server>> {
        let (name, _) = self.server_for(&path.to_string_lossy())?;
        let pooled = lock(&self.pool).get(name).cloned();
        pooled.filter(|s| s.running())
    }

    /// Kills every server; the next `diagnostics` call starts a fresh one.
    pub fn shutdown(&self) {
        let servers: Vec<Arc<Server>> = lock(&self.pool).drain().map(|(_, s)| s).collect();
        for server in servers {
            server.kill();
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

    fn running(&self, name: &str) -> bool {
        let pooled = lock(&self.pool).get(name).cloned();
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
        let pooled = lock(&self.pool).get(name).cloned();
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
            timeout: Duration::from_secs(u64::from(self.cfg.timeout_s)),
            quiet: Duration::from_millis(u64::from(self.cfg.quiet_ms)),
        };
        match Server::start(pipes, opts).await {
            Ok(s) => {
                let s = Arc::new(s);
                lock(&self.pool).insert(name.to_string(), s.clone());
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
        let mut pool = lock(&self.pool);
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
            Some((name, _)) if !self.pool.running(name) => Risk::Exec,
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
            let server = match self.pool.server(name, cfg, &root).await {
                Ok(s) => s,
                Err(text) => return Ok(failure(text)),
            };
            let result = tokio::select! {
                r = server.diagnostics(&path, &text, wait) => r,
                () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            };
            match result {
                Ok(report) => {
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
                        self.pool.evict(name, &server);
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
        self.pool.shutdown();
    }
}

/// Most new diagnostics one edit result lists, so a change that breaks a
/// whole file does not flood the context.
const MAX_INTRODUCED: usize = 10;

/// Wraps `edit` or `write` (T59.3): when the file's language server already
/// runs, the result ends with the diagnostics the change introduced, so the
/// model sees its own error without a `bash` check. A wrapper rather than
/// code in each tool, so both share one hook. Whatever the server does
/// wrong — dead, slow, cancelled — adds nothing: never an error, no retry.
pub struct AfterEdit {
    inner: Arc<dyn Tool>,
    pool: Arc<LspPool>,
    wait: Duration,
    /// The terminal-text guard (`cox_sanitize::sanitize`), handed in because
    /// `cox-tools` may not depend on it: a server's text is untrusted.
    sanitize: fn(&str) -> String,
}

impl AfterEdit {
    pub fn new(
        inner: Arc<dyn Tool>,
        pool: Arc<LspPool>,
        wait: Duration,
        sanitize: fn(&str) -> String,
    ) -> Self {
        Self {
            inner,
            pool,
            wait,
            sanitize,
        }
    }

    /// The file, its running server and what it reported before the change.
    async fn before(
        &self,
        input: &Value,
        cx: &ToolCx,
    ) -> Option<(PathBuf, Arc<Server>, Vec<Diagnostic>)> {
        let arg = input.get("path")?.as_str()?;
        let path = confine(&cx.writable_roots, &cx.cwd, arg).ok()?;
        let server = self.pool.running_for(&path)?;
        let baseline = match server.last(&path).await {
            Some(known) => known,
            // A file that does not exist yet has nothing to compare against.
            None if !path.exists() => Vec::new(),
            None => self.settled(&server, &path, &read_text(&path)?).await?,
        };
        Some((path, server, baseline))
    }

    /// The server's report for `text`, only when it settled in time. A
    /// deadline report holds whatever arrived — maybe nothing, maybe the
    /// previous text's list — so diffing it would invent or hide errors.
    async fn settled(&self, server: &Server, path: &Path, text: &str) -> Option<Vec<Diagnostic>> {
        let report = server.diagnostics(path, text, self.wait).await.ok()?;
        report.note.is_none().then_some(report.diagnostics)
    }

    /// The lines the result gains: diagnostics absent from `baseline`, keyed
    /// by (start line, code, message), errors first, at most ten.
    async fn introduced(
        &self,
        path: &Path,
        server: &Server,
        baseline: &[Diagnostic],
        root: &Path,
    ) -> Option<String> {
        let text = read_text(path)?;
        let after = self.settled(server, path, &text).await?;
        let key = |d: &Diagnostic| (d.range.start.line, d.code.clone(), d.message.clone());
        let known: Vec<_> = baseline.iter().map(key).collect();
        let new: Vec<Diagnostic> = after
            .into_iter()
            .filter(|d| !known.contains(&key(d)))
            .collect();
        if new.is_empty() {
            return None;
        }
        let listed = diag::format(root, path, &new);
        let mut lines: Vec<&str> = listed.lines().collect();
        let summary = lines.pop()?;
        let mut out = format!("\n\nnew diagnostics ({summary}):");
        for line in lines.iter().take(MAX_INTRODUCED) {
            out.push('\n');
            out.push_str(line);
        }
        if lines.len() > MAX_INTRODUCED {
            out.push_str("\n… more: call `diagnostics` for the full list");
        }
        Some((self.sanitize)(&out))
    }
}

#[async_trait]
impl Tool for AfterEdit {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn subject(&self, input: &Value) -> String {
        self.inner.subject(input)
    }

    fn risk(&self, input: &Value) -> Risk {
        self.inner.risk(input)
    }

    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        self.inner.touches(input)
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        // Room for one sync plus the queue behind a `diagnostics` call
        // holding the server; past it the edit result goes out as is.
        let grace = self.wait * 2;
        let before = tokio::select! {
            b = tokio::time::timeout(grace, self.before(&input, cx)) => b.ok().flatten(),
            () = cx.cancel.cancelled() => None,
        };
        let mut out = self.inner.call(input, cx).await?;
        let Some((path, server, baseline)) = before.filter(|_| !out.is_error) else {
            return Ok(out);
        };
        let root = root_for(&path, &cx.roots, &cx.cwd);
        let added = tokio::select! {
            a = tokio::time::timeout(grace, self.introduced(&path, &server, &baseline, &root)) => a.ok().flatten(),
            () = cx.cancel.cancelled() => None,
        };
        if let Some(added) = added {
            out.text.push_str(&added);
        }
        Ok(out)
    }

    fn shutdown(&self) {
        self.inner.shutdown();
    }
}

/// `path` as UTF-8 text under the size cap a server is sent.
fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
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

    /// Tracks each file's text and answers every save with `fake: first
    /// line`, plus an `E1` error (behind an escape sequence, as untrusted
    /// text) on each line holding `BAD`. A file holding `HANGUP` makes it
    /// hang up, the way a crashed server does while its process lingers;
    /// one holding `SLOW` gets no answer, so the caller's deadline wins.
    async fn fake_server(mut r: FakeRead, mut w: FakeWrite) {
        handshake(&mut r, &mut w, json!({"textDocumentSync": 1})).await;
        let mut docs: HashMap<String, String> = HashMap::new();
        while let Ok(Some(msg)) = read_message(&mut r).await {
            let params = &msg["params"];
            let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
            match msg["method"].as_str() {
                Some("textDocument/didOpen") => {
                    let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                    docs.insert(uri.to_string(), text.to_string());
                }
                Some("textDocument/didChange") => {
                    let text = params["contentChanges"][0]["text"].as_str();
                    docs.insert(uri.to_string(), text.unwrap_or_default().to_string());
                }
                Some("textDocument/didSave") => {
                    let text = docs.get(uri).cloned().unwrap_or_default();
                    if text.contains("HANGUP") {
                        return;
                    }
                    if text.contains("SLOW") {
                        continue;
                    }
                    let mut diags = vec![diag("fake: first line")];
                    for (n, _) in text.lines().enumerate().filter(|(_, l)| l.contains("BAD")) {
                        diags.push(json!({
                            "range": {"start": {"line": n, "character": 0}, "end": {"line": n, "character": 3}},
                            "severity": 1,
                            "code": "E1",
                            "message": "\u{1b}[31mbad token",
                        }));
                    }
                    let params = json!({"uri": uri, "diagnostics": diags});
                    push(&mut w, "textDocument/publishDiagnostics", params).await;
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

    /// Stands in for `cox_sanitize::sanitize`, which `cox-tools` may not
    /// depend on; the session hands in the real one.
    fn strip_escapes(s: &str) -> String {
        s.replace('\u{1b}', "")
    }

    fn after_edit(diags: &DiagnosticsTool, inner: Arc<dyn Tool>) -> AfterEdit {
        let wait = Duration::from_millis(1500);
        AfterEdit::new(inner, diags.pool.clone(), wait, strip_escapes)
    }

    fn edit(old: &str, new: &str) -> Value {
        json!({"path": "src/a.rs", "old": old, "new": new})
    }

    #[tokio::test(start_paused = true)]
    async fn edit_reports_the_error_it_introduced() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        let tool = after_edit(&diags, Arc::new(crate::edit::EditTool));

        let out = tool
            .call(edit("fn main() {}", "fn main() {}\nBAD"), &cx(&root))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("edited "), "{}", out.text);
        assert!(
            out.text.ends_with(
                "\n\nnew diagnostics (1 error):\nsrc/a.rs:2:1: error: [31mbad token [E1]"
            ),
            "{}",
            out.text
        );
    }

    #[tokio::test(start_paused = true)]
    async fn edit_adds_nothing_when_no_server_runs() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (diags, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let tool = after_edit(&diags, Arc::new(crate::edit::EditTool));

        let out = tool
            .call(edit("fn main() {}", "BAD"), &cx(&root))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("diagnostics"), "{}", out.text);
        assert!(lock(&started).is_empty(), "an edit never starts a server");
    }

    #[tokio::test(start_paused = true)]
    async fn errors_that_existed_before_are_not_reported() {
        let (_dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        std::fs::write(root.join("src/b.rs"), "BAD\n").unwrap();
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();

        // `a.rs` was synced, so its last report is the baseline.
        let edit_tool = after_edit(&diags, Arc::new(crate::edit::EditTool));
        let out = edit_tool
            .call(edit("fn main() {}", "fn main() {}\n"), &cx(&root))
            .await
            .unwrap();
        assert!(!out.text.contains("diagnostics"), "{}", out.text);

        // `b.rs` never was, so its old text is synced first.
        let write_tool = after_edit(&diags, Arc::new(crate::write::WriteTool));
        let input = json!({"path": "src/b.rs", "content": "BAD\nfn c() {}\n"});
        let out = write_tool.call(input, &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("diagnostics"), "{}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn a_baseline_cut_off_by_the_deadline_adds_nothing() {
        let (_dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        std::fs::write(root.join("src/b.rs"), "BAD SLOW\n").unwrap();
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();

        // The old text never settles, so there is no baseline to diff: the
        // error that was already there must not come back as new.
        let write_tool = after_edit(&diags, Arc::new(crate::write::WriteTool));
        let input = json!({"path": "src/b.rs", "content": "BAD\nfn c() {}\n"});
        let out = write_tool.call(input, &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("diagnostics"), "{}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn introduced_lists_at_most_ten() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        let tool = after_edit(&diags, Arc::new(crate::edit::EditTool));

        let out = tool
            .call(edit("fn main() {}", &"BAD\n".repeat(12)), &cx(&root))
            .await
            .unwrap();
        let listed = out.text.lines().filter(|l| l.starts_with("src/a.rs:"));
        assert_eq!(listed.count(), 10, "{}", out.text);
        assert!(out.text.contains("(12 errors)"), "{}", out.text);
        assert!(out.text.ends_with("call `diagnostics` for the full list"));
    }

    #[tokio::test(start_paused = true)]
    async fn dead_server_adds_nothing() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        let tool = after_edit(&diags, Arc::new(crate::edit::EditTool));

        let out = tool
            .call(edit("fn main() {}", "HANGUP BAD"), &cx(&root))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("diagnostics"), "{}", out.text);
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.rs")).unwrap(),
            "HANGUP BAD\n"
        );
    }

    #[test]
    fn on_path_finds_a_bare_name_and_a_path() {
        assert!(on_path("sh").is_some());
        assert_eq!(on_path("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert_eq!(on_path("cox-no-such-language-server"), None);
        assert_eq!(on_path(""), None);
    }
}
