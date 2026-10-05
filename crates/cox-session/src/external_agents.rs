// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The host drivers behind `ExternalAgent` (T35.13, EA§4–§5): one per
//! granted `[[external_agents]]` entry, each spawning T35.2's sandboxed
//! `Command` for a turn. Here, in session assembly (T37.1; `crates/cox`
//! before), because a driver joins three crates that may not see each
//! other: the stream-json mapper is in `cox-core`, the ACP client in
//! `cox-acp`, the spawn spec in `cox-plugin`.
//!
//! What stays out of a driver: the CLI's own tool calls are not judged per
//! call by `cox_permission::Engine` or `PreToolUse` hooks (EA§2) — the
//! process sandbox the wrap put it under is the guard. Only what the agent
//! asks cox for (ACP `session/request_permission`, `fs/*`) meets the engine
//! and `path::confine`, inside `cox_acp::connect`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::v1::{
    ContentBlock, NewSessionRequest, PromptRequest, SessionUpdate,
};
use cox_acp::{Approver, ClientHost};
use cox_core::external_agent::StreamJsonMapper;
use cox_core::permission::Engine;
use cox_plugin::external_agent::{AgentSource, ExternalAgentCommand};
use cox_plugin_api::AgentMode;
use cox_protocol::Config;
use cox_protocol::config::{CHILD_ENV_ALLOWLIST, ExternalAgentConfig};
use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::{ItemId, TurnId};
use cox_protocol::traits::ExternalAgent;
use cox_protocol::types::{
    ApprovalPolicy, Decision, Event, ItemKind, PermissionMode, SandboxMode, SandboxPolicy,
    ToolCall, Usage, Why,
};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use tokio_util::sync::CancellationToken;

/// Bytes of the CLI's stderr kept to explain a failed run.
const STDERR_TAIL: usize = 2048;

/// One driver per entry whose CLI and key are both there, plus one warning
/// per entry left out (EA§7). `path` is the `PATH` a bare CLI name is
/// looked up in; `key` is `cox_provider::http::resolve_key` (a test passes
/// its own, never the OS keychain), called with the entry's `key_env` and
/// its name as the keyring section.
pub(crate) fn drivers(
    agents: Vec<ExternalAgentCommand>,
    config: &Config,
    cwd: &Path,
    writable: &[PathBuf],
    path: Option<&OsStr>,
    key: impl Fn(&str, &str) -> Result<String, ProviderError>,
) -> (Vec<Arc<dyn ExternalAgent>>, Vec<String>) {
    let mut out: Vec<Arc<dyn ExternalAgent>> = Vec::new();
    let mut left_out = Vec::new();
    if agents.is_empty() {
        return (out, left_out);
    }
    let acp = match acp_host(config, cwd, writable) {
        Ok(host) => Arc::new(host),
        Err(e) => {
            left_out.push(format!("external agents left out: {e}"));
            return (out, left_out);
        }
    };
    for agent in agents {
        let name = agent.name().to_string();
        if let Some(cli) = agent.missing_cli(path) {
            left_out.push(format!(
                "external agent {name} ({}) left out: `{}` is not on PATH",
                agent.origin(),
                cli.display()
            ));
            continue;
        }
        let Ok(secret) = key(agent.key_env(), &name) else {
            left_out.push(format!(
                "external agent {name} ({}) left out: {} is not set",
                agent.origin(),
                agent.key_env()
            ));
            continue;
        };
        let spawn = Spawn {
            name,
            key_env: agent.key_env().to_string(),
            mode: agent.mode(),
            agent,
            key: secret,
            cwd: cwd.to_path_buf(),
        };
        out.push(match spawn.mode {
            AgentMode::StreamJson => Arc::new(StreamJson(spawn)),
            AgentMode::Acp => Arc::new(Acp {
                spawn,
                host: acp.clone(),
            }),
        });
    }
    (out, left_out)
}

/// Every user-config `[external_agents.<name>]` entry (T52.2, DT§3.3.1)
/// resolved through `ExternalAgentCommand`'s one constructor, wrapped by
/// [`crate::sandbox::agent_argv`] with the session's `writable` roots plus
/// the entry's own state directories, plus one warning per entry refused.
/// Each state directory passes `path::confine` against `home` (symlinks
/// resolved) and may not be `home` itself: an entry widens writes to a
/// directory under the user's home, never to the home or outside it. A bad
/// directory, command or wrap refuses the whole entry (fail closed; the
/// rest still resolve). The config layer has already dropped anything a
/// project config tried to set here.
pub fn config_agents(
    config: &Config,
    writable: &[PathBuf],
    home: Option<&Path>,
) -> (Vec<ExternalAgentCommand>, Vec<String>) {
    let (mut agents, mut refused) = (Vec::new(), Vec::new());
    for (name, entry) in &config.external_agents {
        match config_agent(name, entry, config, writable, home) {
            Ok(agent) => agents.push(agent),
            Err(e) => refused.push(format!("external agent {name} (user config) left out: {e}")),
        }
    }
    (agents, refused)
}

fn config_agent(
    name: &str,
    entry: &ExternalAgentConfig,
    config: &Config,
    writable: &[PathBuf],
    home: Option<&Path>,
) -> Result<ExternalAgentCommand, String> {
    if entry.command.trim().is_empty() {
        return Err(String::from("`command` is empty"));
    }
    if entry.key_env.trim().is_empty() {
        return Err(String::from("`key_env` is empty"));
    }
    let state = state_dirs(&entry.writable, home)?;
    let decl = cox_plugin_api::ExternalAgentDecl {
        name: name.to_string(),
        command: entry.command.clone(),
        args: entry.args.clone(),
        mode: AgentMode::Acp,
        key_env: entry.key_env.clone(),
    };
    let roots: Vec<PathBuf> = writable.iter().chain(&state).cloned().collect();
    let wrap =
        |program: &Path, args: &[String]| crate::sandbox::agent_argv(program, args, config, &roots);
    ExternalAgentCommand::resolve(AgentSource::Config { writable: &state }, &decl, wrap)
        .map_err(|e| e.to_string())
}

/// An entry's `writable` list, each confined to a directory strictly under
/// `home` through the one path guard.
fn state_dirs(dirs: &[PathBuf], home: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    let home = home.ok_or_else(|| String::from("`writable` needs a home directory"))?;
    let canon_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let roots = [home.to_path_buf()];
    dirs.iter()
        .map(|dir| {
            let shown = dir.display().to_string();
            let confined = cox_tools::path::confine(&roots, home, &shown)
                .map_err(|_| format!("`writable` {shown} is not under your home"))?;
            if confined == canon_home || confined == home {
                return Err(format!("`writable` {shown} is your whole home"));
            }
            Ok(confined)
        })
        .collect()
}

/// How one entry's CLI is started: for a turn here, for a whole session in
/// `acp_session` (T52.4), through the same env allowlist and process group.
pub(crate) struct Spawn {
    pub(crate) name: String,
    pub(crate) agent: ExternalAgentCommand,
    pub(crate) mode: AgentMode,
    pub(crate) key_env: String,
    pub(crate) key: String,
    pub(crate) cwd: PathBuf,
}

impl Spawn {
    /// The wrapped argv plus `extra`, with the child env allowlist (D14)
    /// and the key from `key_env` only — cox's own provider keys stay
    /// behind. Its own process group, so `Reap` takes everything it started.
    pub(crate) fn spawn(&self, extra: &[&str], stdin: Stdio) -> Result<(Child, Reap), CoreError> {
        use std::os::unix::process::CommandExt as _;

        let mut cmd = self.agent.command();
        cmd.args(extra).env_clear();
        for var in CHILD_ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(var) {
                cmd.env(var, value);
            }
        }
        cmd.env(&self.key_env, &self.key)
            .current_dir(&self.cwd)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut cmd = tokio::process::Command::from(cmd);
        cmd.kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| self.error(format!("cannot start: {e}")))?;
        let reap = Reap(child.id());
        Ok((child, reap))
    }

    pub(crate) fn error(&self, message: String) -> CoreError {
        CoreError::ExternalAgent {
            agent: self.name.clone(),
            message,
        }
    }
}

/// Kills the CLI's process group however the turn ends — done, failed,
/// cancelled, or the core dropping the turn — with the kill a cancelled
/// `bash` ends with.
pub(crate) struct Reap(Option<u32>);

impl Drop for Reap {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            cox_tools::bash::kill_group(pid);
        }
    }
}

/// The last `STDERR_TAIL` bytes the CLI wrote to stderr, sanitized. Read to
/// the end so a chatty CLI never blocks on a full pipe.
pub(crate) async fn stderr_tail(mut stderr: impl AsyncRead + Unpin) -> String {
    let (mut tail, mut buf) = (Vec::new(), [0u8; 4096]);
    while let Ok(n) = stderr.read(&mut buf).await {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&buf[..n]);
        let over = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..over);
    }
    cox_sanitize::sanitize(String::from_utf8_lossy(&tail).trim())
}

/// `mode = "stream-json"` (EA§5): the CLI runs once per turn with the prompt
/// as its last argument, and each stdout line goes through the mapper.
struct StreamJson(Spawn);

#[async_trait::async_trait]
impl ExternalAgent for StreamJson {
    fn name(&self) -> &str {
        &self.0.name
    }

    async fn turn(
        &self,
        turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        let (mut child, _reap) = self.0.spawn(&[&prompt], Stdio::null())?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(self.0.error("no stdio pipes".into()));
        };
        let stderr = tokio::spawn(stderr_tail(stderr));
        let mut lines = BufReader::new(stdout).lines();
        let mut mapper = StreamJsonMapper::new(turn, self.0.name.clone(), cox_sanitize::sanitize);
        loop {
            let line = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                line = lines.next_line() => line,
            };
            let line = match line {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => return Err(self.0.error(format!("reading its output: {e}"))),
            };
            for ev in mapper.map_line(&line) {
                // `result` is the run's last line: its `TurnDone` ends the turn.
                let done = matches!(ev, Event::TurnDone { .. });
                if events.send(ev).await.is_err() || done {
                    return Ok(None);
                }
            }
        }
        // EOF with no `result` line: the CLI failed before it finished.
        let status = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(None),
            status = child.wait() => status,
        };
        let tail = stderr.await.unwrap_or_default();
        let status = status.map_or_else(|e| e.to_string(), |s| s.to_string());
        Err(self
            .0
            .error(format!("exited ({status}) with no result: {tail}")))
    }
}

/// Everything an ACP `ClientHost` is built from, shared by every ACP entry.
pub(crate) struct AcpHost {
    roots: Vec<PathBuf>,
    cwd: PathBuf,
    sandbox: Option<SandboxPolicy>,
    engine: Arc<Engine>,
    mode: PermissionMode,
    approval: ApprovalPolicy,
}

/// The host every ACP agent in a session in `cwd` is answered from: the
/// rules the session's own engine compiled from (`Session::new`), and, since
/// T52.2, the agent's own policy (`agent_policy`, network on), so the
/// terminals it asks cox for run as the agent itself does.
pub(crate) fn acp_host(
    config: &Config,
    cwd: &Path,
    writable: &[PathBuf],
) -> Result<AcpHost, String> {
    let home = std::env::home_dir();
    let engine = Engine::compile(&config.permissions, home.as_deref(), cwd)
        .map_err(|e| format!("permissions: {e}"))?;
    let policy = crate::sandbox::agent_policy(config);
    Ok(AcpHost {
        roots: writable.to_vec(),
        cwd: cwd.to_path_buf(),
        sandbox: (policy.mode != SandboxMode::DangerFullAccess).then_some(policy),
        engine: Arc::new(engine),
        mode: config.permissions.mode,
        approval: config.permissions.approval,
    })
}

impl AcpHost {
    /// A `ClientHost` whose `Ask` verdicts go to `approver` and whose
    /// `session/update`s go to `updates`.
    pub(crate) fn client(
        &self,
        approver: Arc<dyn Approver>,
        updates: mpsc::UnboundedSender<SessionUpdate>,
    ) -> ClientHost {
        ClientHost {
            roots: self.roots.clone(),
            cwd: self.cwd.clone(),
            sandbox: self.sandbox.clone(),
            engine: self.engine.clone(),
            mode: self.mode,
            approval: self.approval,
            grants: Vec::new(),
            approver,
            updates: Some(updates),
        }
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn sandboxed(&self) -> bool {
        self.sandbox.is_some()
    }
}

/// `mode = "acp"` (EA§4): the CLI speaks ACP on its stdio for one turn:
/// `initialize`, `session/new`, one `session/prompt`; the agent's message
/// chunks become the turn's answer.
struct Acp {
    spawn: Spawn,
    host: Arc<AcpHost>,
}

/// An `Ask` verdict cannot reach the user yet: `ExternalAgent::turn` has no
/// route to the session's `ApprovalRequired` relay, so the request is
/// refused (fail closed) with the way out named. A rule the engine already
/// allows or denies is decided as usual.
pub(crate) struct RefuseAsk;

#[async_trait::async_trait]
impl Approver for RefuseAsk {
    async fn approve(&self, call: ToolCall, _why: Why) -> Decision {
        Decision::Deny {
            reason: format!(
                "{} needs approval, which an external agent cannot ask for yet; \
                 allow it with a permissions rule",
                call.name
            ),
        }
    }
}

#[async_trait::async_trait]
impl ExternalAgent for Acp {
    fn name(&self) -> &str {
        &self.spawn.name
    }

    async fn turn(
        &self,
        _turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        let (mut child, _reap) = self.spawn.spawn(&[], Stdio::piped())?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(self.spawn.error("no stdio pipes".into()));
        };
        let stderr = tokio::spawn(stderr_tail(stderr));
        let (tx, mut updates) = mpsc::unbounded_channel();
        let host = self.host.client(Arc::new(RefuseAsk), tx);
        let cwd = self.host.cwd().to_path_buf();
        let sandboxed = self.host.sandboxed();
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let run = cox_acp::connect(transport, host, async move |cx| {
            cx.send_request(cox_acp::initialize_request(sandboxed))
                .block_task()
                .await?;
            let session = cx
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let prompt = vec![ContentBlock::from(prompt)];
            cx.send_request(PromptRequest::new(session.session_id, prompt))
                .block_task()
                .await
        });
        tokio::pin!(run);
        let mut answer = String::new();
        let result = loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                Some(update) = updates.recv() => collect(&mut answer, update),
                result = &mut run => break result,
            }
        };
        while let Ok(update) = updates.try_recv() {
            collect(&mut answer, update);
        }
        if let Err(e) = result {
            let tail = stderr.await.unwrap_or_default();
            let e = cox_sanitize::sanitize(&e.to_string());
            return Err(self.spawn.error(format!("{e}: {tail}")));
        }
        if !answer.is_empty() {
            let item = ItemId::new();
            let kind = ItemKind::AssistantMessage { text: answer };
            let _ = events.send(Event::ItemStarted { item, kind }).await;
            let _ = events.send(Event::ItemDone { item }).await;
        }
        // The documented `PromptResponse` carries no token counts (EA§6).
        Ok(None)
    }
}

/// The agent's own message text; plans, thoughts and its tool calls are the
/// agent's business, run inside its own sandboxed process (EA§2).
fn collect(answer: &mut String, update: SessionUpdate) {
    if let SessionUpdate::AgentMessageChunk(chunk) = update
        && let ContentBlock::Text(text) = chunk.content
    {
        answer.push_str(&text.text);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use cox_protocol::types::Submission;

    /// A package whose `bin/agent` is `script`, resolved and wrapped by the
    /// session's real sandbox wrap; `None` where this host has no argv
    /// sandbox backend (the wrap refuses, T35.2's own test covers that).
    /// The package sits inside the workspace: Linux bwrap gives the wrapped
    /// CLI a private `/tmp`, where a package in its own temp dir would not
    /// exist for it.
    fn fake_cli(
        ws: &Path,
        mode: AgentMode,
        script: &str,
    ) -> Option<(ExternalAgentCommand, Config)> {
        let pkg = &ws.join("pkg");
        std::fs::create_dir_all(pkg.join("bin")).expect("mkdir");
        std::fs::write(pkg.join("bin/agent"), script).expect("agent");
        let exec = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(pkg.join("bin/agent"), exec).expect("chmod");
        let decl = cox_plugin_api::ExternalAgentDecl {
            name: "cursor".into(),
            command: "bin/agent".into(),
            args: vec![],
            mode,
            key_env: "CURSOR_API_KEY".into(),
        };
        let mut config = Config::default();
        config.core.workspace_roots = vec![ws.to_path_buf()];
        let roots = config.core.workspace_roots.clone();
        let wrap = |p: &Path, a: &[String]| crate::sandbox::sandboxed_argv(p, a, &config, &roots);
        let source = AgentSource::Plugin {
            id: "cur",
            dir: pkg,
        };
        let agent = ExternalAgentCommand::resolve(source, &decl, wrap).ok()?;
        Some((agent, config))
    }

    fn fake_key(env: &str, _section: &str) -> Result<String, ProviderError> {
        match env {
            "CURSOR_API_KEY" => Ok("test-key".into()),
            _ => Err(ProviderError::Auth),
        }
    }

    fn one_driver(
        agent: ExternalAgentCommand,
        config: &Config,
        ws: &Path,
    ) -> Arc<dyn ExternalAgent> {
        let roots = [ws.to_path_buf()];
        let (mut drivers, left_out) = drivers(vec![agent], config, ws, &roots, None, fake_key);
        assert!(left_out.is_empty(), "{left_out:?}");
        drivers.pop().expect("one driver")
    }

    /// Prints the documented stream-json lines, answering with the prompt
    /// (its last argument) and whether the key reached it.
    const STREAM_JSON: &str = "#!/bin/sh\n\
        for last; do :; done\n\
        echo '{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"m\",\"permissionMode\":\"default\"}'\n\
        printf '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"did %s with %s\"}]}}\\n' \"$last\" \"$CURSOR_API_KEY\"\n\
        echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\"}'\n";

    const DISPATCH: &str = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "work", preset = "cursor" } }]
[[turn]]
text = "done"
"#;

    /// T35.13 Check: a parent's `agent(preset: "cursor")` runs the fake
    /// CLI under the sandbox wrap through the stream-json driver, and the
    /// CLI's answer — with the key from `key_env` — is the task's result.
    #[tokio::test]
    async fn stream_json_driver_answers_a_child_task() {
        let ws = tempfile::tempdir().expect("ws");
        let Some((agent, config)) = fake_cli(ws.path(), AgentMode::StreamJson, STREAM_JSON) else {
            return;
        };
        let driver = one_driver(agent, &config, ws.path());
        let provider = cox_provider::scripted::Scripted::from_toml(DISPATCH, "").expect("scenario");
        let store = Arc::new(cox_core::MemoryStore::new());
        let session = cox_core::Session::new(
            config,
            Arc::new(provider),
            vec![],
            store.clone(),
            store,
            ws.path().to_path_buf(),
        )
        .expect("session");
        session.set_external_agents(vec![driver]);
        let mut rx = session.events().expect("events");
        let mode = PermissionMode::Bypass;
        session
            .submit(Submission::SetPermissionMode { mode })
            .await
            .expect("mode");
        let runner = session.clone();
        let turn = tokio::spawn(async move {
            let sub = Submission::UserTurn {
                text: "go".into(),
                attachments: vec![],
                confirm_think: false,
            };
            runner.submit(sub).await
        });
        let mut results = Vec::new();
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::ToolCallDone { result, .. } => results.push(result.visible),
                Event::TurnDone { .. } => break,
                _ => {}
            }
        }
        turn.await.expect("join").expect("turn");
        assert_eq!(results, ["did work with test-key"]);
    }

    /// T35.13 Check (EA§7): a PATH CLI that is not there, or a key that is
    /// not set, leaves that entry out with exactly one warning each; the
    /// entry whose CLI and key are there still gets its driver.
    #[test]
    fn missing_cli_leaves_the_preset_out_with_one_warning() {
        let pkg = tempfile::tempdir().expect("pkg");
        let bin = tempfile::tempdir().expect("bin");
        let decl = |name: &str, command: &str, key_env: &str| cox_plugin_api::ExternalAgentDecl {
            name: name.into(),
            command: command.into(),
            args: vec![],
            mode: AgentMode::StreamJson,
            key_env: key_env.into(),
        };
        let wrap = |p: &Path, a: &[String]| {
            let argv = std::iter::once(p.display().to_string()).chain(a.iter().cloned());
            Ok::<_, String>(argv.collect())
        };
        let source = AgentSource::Plugin {
            id: "cur",
            dir: pkg.path(),
        };
        let resolve = |d| ExternalAgentCommand::resolve(source, &d, wrap).expect("resolves");
        let exec = std::fs::Permissions::from_mode(0o755);
        std::fs::write(bin.path().join("present"), "#!/bin/sh\n").expect("cli");
        std::fs::set_permissions(bin.path().join("present"), exec).expect("chmod");
        let path = std::env::join_paths([bin.path()]).expect("PATH");
        let agents = vec![
            resolve(decl("cursor", "cox-no-such-agent-cli", "CURSOR_API_KEY")),
            resolve(decl("nokey", "present", "COX_TEST_UNSET_KEY")),
            resolve(decl("ok", "present", "CURSOR_API_KEY")),
        ];
        let ws = pkg.path();
        let (drivers, left_out) = drivers(
            agents,
            &Config::default(),
            ws,
            &[ws.to_path_buf()],
            Some(&path),
            fake_key,
        );
        let names: Vec<_> = drivers.iter().map(|d| d.name().to_string()).collect();
        assert_eq!(names, ["ok"]);
        assert_eq!(left_out.len(), 2, "{left_out:?}");
        assert!(left_out[0].contains("cursor") && left_out[0].contains("not on PATH"));
        assert!(left_out[1].contains("nokey") && left_out[1].contains("COX_TEST_UNSET_KEY"));
    }

    /// T35.13 Check: cancelling a turn kills the CLI's whole process group —
    /// here a background `sleep` the fake agent started, which a kill of the
    /// leader alone would orphan — and the turn ends at once. The `sleep`
    /// holds a FIFO's write end, so its end shows as EOF on the read end:
    /// a pid would not do, since under Linux bwrap the agent sees only its
    /// own pid namespace.
    #[tokio::test]
    async fn cancel_kills_the_external_agent_process() {
        use std::io::Read as _;
        use std::time::Duration;

        let ws = tempfile::tempdir().expect("ws");
        let fifo = ws.path().join("alive");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.is_ok_and(|s| s.success()), "mkfifo");
        let script = format!(
            "#!/bin/sh
sleep 60 > '{}' &
wait
",
            fifo.display()
        );
        let Some((agent, config)) = fake_cli(ws.path(), AgentMode::StreamJson, &script) else {
            return;
        };
        let driver = one_driver(agent, &config, ws.path());
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let run = {
            let (driver, cancel) = (driver.clone(), cancel.clone());
            tokio::spawn(async move { driver.turn(TurnId::new(), "work".into(), tx, cancel).await })
        };
        // A plain thread, not `spawn_blocking`: if the `sleep` never opens
        // the FIFO this thread blocks for good, and must not hold the
        // runtime's shutdown too.
        let (opened, is_open) = tokio::sync::oneshot::channel();
        let (closed, is_closed) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let Ok(mut reader) = std::fs::File::open(&fifo) else {
                return;
            };
            let _ = opened.send(());
            // EOF once no writer is left.
            let _ = reader.read_to_end(&mut Vec::new());
            let _ = closed.send(());
        });
        let started = tokio::time::timeout(Duration::from_secs(10), is_open).await;
        assert!(
            matches!(started, Ok(Ok(()))),
            "the fake agent's child never started"
        );
        cancel.cancel();
        let ended = tokio::time::timeout(Duration::from_secs(5), run).await;
        let ended = ended.expect("the turn ended on cancel").expect("join");
        assert!(matches!(ended, Ok(None)), "{ended:?}");
        let gone = tokio::time::timeout(Duration::from_secs(2), is_closed).await;
        assert!(
            matches!(gone, Ok(Ok(()))),
            "the agent's background process outlived the cancel"
        );
    }

    /// T52.2: an entry's `writable` directories stay strictly under the
    /// user's home, through `path::confine`; the home itself, a path
    /// outside it and a symlink out of it are each refused.
    #[test]
    fn config_agent_state_dirs_stay_under_home() {
        let home = tempfile::tempdir().expect("home");
        let other = tempfile::tempdir().expect("other");
        let h = home.path();
        let ok = state_dirs(&[h.join(".claude")], Some(h)).expect("under home");
        assert_eq!(ok.len(), 1);
        assert!(ok[0].ends_with(".claude"), "{ok:?}");
        assert!(state_dirs(&[h.to_path_buf()], Some(h)).is_err());
        assert!(state_dirs(&[other.path().to_path_buf()], Some(h)).is_err());
        std::os::unix::fs::symlink(other.path(), h.join("out")).expect("symlink");
        assert!(state_dirs(&[h.join("out/state")], Some(h)).is_err());
        assert!(state_dirs(&[h.join(".codex")], None).is_err());
        assert_eq!(state_dirs(&[], None), Ok(Vec::new()));
    }

    /// T52.2: a user-config entry with no `command` or no `key_env` is
    /// refused with one warning naming it, and never reaches the wrap.
    #[test]
    fn config_agent_without_command_or_key_is_refused() {
        let mut config = Config::default();
        config.external_agents.insert(
            "codex".into(),
            ExternalAgentConfig {
                command: String::new(),
                key_env: "CODEX_API_KEY".into(),
                ..ExternalAgentConfig::default()
            },
        );
        config.external_agents.insert(
            "gemini".into(),
            ExternalAgentConfig {
                command: "gemini".into(),
                args: vec!["--acp".into()],
                ..ExternalAgentConfig::default()
            },
        );
        let (agents, refused) = config_agents(&config, &[], None);
        assert!(agents.is_empty());
        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(refused[0].contains("codex") && refused[0].contains("command"));
        assert!(refused[1].contains("gemini") && refused[1].contains("key_env"));
    }

    #[test]
    fn acp_answer_is_the_agent_message_text_only() {
        use agent_client_protocol::schema::v1::ContentChunk;

        let mut answer = String::new();
        let chunk = |t: &str| ContentChunk::new(t.into());
        collect(
            &mut answer,
            SessionUpdate::AgentMessageChunk(chunk("Hello ")),
        );
        collect(
            &mut answer,
            SessionUpdate::AgentThoughtChunk(chunk("secret plan")),
        );
        collect(
            &mut answer,
            SessionUpdate::AgentMessageChunk(chunk("there")),
        );
        assert_eq!(answer, "Hello there");
    }
}
