// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `[external_agents.<name>]` entries with a `target` (T68.7, A158): an
//! `agent(preset: <name>)` that runs `cox run --output-format stream-json` on
//! a target through the launcher, folded by the stream-json pump in
//! `external_agents`. Separate from the local drivers because nothing local
//! runs here: no CLI lookup, no key, no sandbox wrap.
//!
//! Guards: the remote `cox` applies its own sandbox and keeps its own keys, so
//! the local wrap does not apply and `cox_target::command` hands the launcher a
//! scrubbed environment (no provider key, no ssh agent). Only checked tokens
//! reach the remote command line; the prompt goes over stdin. Like
//! `CloudAgent`, every call is the engine's `TargetAgent(<kind:name>)`
//! decision and only an allow rule lifts it, `bypass` and `auto` included,
//! since the work runs on a machine the sandbox does not cover.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use cox_core::permission::{Engine, Outcome};
use cox_protocol::Config;
use cox_protocol::errors::CoreError;
use cox_protocol::ids::{CallId, TurnId};
use cox_protocol::traits::{ExternalAgent, TaskTurn};
use cox_protocol::types::{
    ApprovalPolicy, DecidedBy, Event, PermissionMode, Risk, SandboxMode, ToolCall, Usage,
};
use cox_target::{Resolved, Tty};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::external_agents::{Reap, pump};

/// The permission rule's tool name, as `canonical_tool` lower-cases
/// `TargetAgent`.
const TARGET_AGENT: &str = "targetagent";

/// One driver per entry that names a target and resolves, plus one warning per
/// entry left out. A launcher is looked up once, here, so a missing one is
/// reported when the session opens rather than on the first call.
pub(crate) fn drivers(config: &Config, cwd: &Path) -> (Vec<Arc<dyn ExternalAgent>>, Vec<String>) {
    let (mut out, mut left_out) = (Vec::<Arc<dyn ExternalAgent>>::new(), Vec::new());
    let on_target = config
        .external_agents
        .iter()
        .filter_map(|(name, e)| Some((name, e.target.as_deref()?)))
        .collect::<Vec<_>>();
    let engine = Engine::compile(&config.permissions, std::env::home_dir().as_deref(), cwd);
    let engine = match engine {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            left_out.extend(
                on_target
                    .iter()
                    .map(|(name, _)| format!("external agent {name} left out: permissions: {e}")),
            );
            return (out, left_out);
        }
    };
    for (name, text) in on_target {
        match crate::targets::resolve(config, text) {
            Ok(resolved) => out.push(Arc::new(TargetAgent {
                name: name.clone(),
                resolved,
                engine: engine.clone(),
                approval: config.permissions.approval,
            })),
            Err(e) => left_out.push(format!("external agent {name} left out: {e}")),
        }
    }
    (out, left_out)
}

struct TargetAgent {
    name: String,
    resolved: Resolved,
    engine: Arc<Engine>,
    approval: ApprovalPolicy,
}

/// What the remote runs: kept in one place so the flag that makes `cox run`
/// read its prompt from stdin (T68.6) is switched here and nowhere else. Every
/// token was checked by `cox_target::resolve` or is a literal.
fn remote_argv(cwd: Option<&str>) -> Vec<&str> {
    let mut argv = vec!["cox"];
    if let Some(cwd) = cwd {
        argv.extend(["--cwd", cwd]);
    }
    argv.extend(["run", "--output-format", "stream-json", "--prompt-stdin"]);
    argv
}

impl TargetAgent {
    /// Allowed only by a rule: a refusal names the rule that would allow it,
    /// because the turn has no route to the user's approval prompt yet.
    fn approve(&self, mode: PermissionMode) -> Result<(), String> {
        let subject = self.resolved.target.to_string();
        let call = ToolCall {
            id: CallId::new(),
            name: TARGET_AGENT.into(),
            input: serde_json::json!({"preset": self.name, "target": subject}),
            risk: Risk::Exec,
            subject: subject.clone(),
            segments: None,
        };
        // why: `bypass` and `auto` allow before any rule is read, so they are
        // asked as `default`; `plan` stays `plan`, which denies a run that
        // acts. `DangerFullAccess`: nothing local sandboxes this run, so the
        // engine must not treat the call as contained.
        let mode = if mode == PermissionMode::Plan {
            mode
        } else {
            PermissionMode::Default
        };
        let sandbox = SandboxMode::DangerFullAccess;
        let outcome = self.engine.decide(&call, mode, self.approval, sandbox, &[]);
        match outcome {
            Outcome::Allow {
                by: DecidedBy::Rule,
            } => Ok(()),
            Outcome::Deny { reason, .. } => Err(reason),
            _ => Err(format!(
                "Run the {} preset on {subject}? The prompt and the task's work leave this \
                 machine. A background task cannot ask for approval yet; allow it with \
                 `TargetAgent({subject})` in your own config's `permissions.allow`.",
                self.name
            )),
        }
    }

    async fn run(
        &self,
        mode: PermissionMode,
        turn: TurnId,
        prompt: String,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        let fail = |message: String| CoreError::ExternalAgent {
            agent: self.name.clone(),
            message: cox_sanitize::sanitize(&message),
        };
        if let Err(message) = self.approve(mode) {
            return Err(fail(message));
        }
        let Resolved {
            target,
            launcher,
            cwd,
        } = &self.resolved;
        let argv = remote_argv(cwd.as_deref());
        let command = cox_target::command(target, launcher, &argv, Tty::No)
            .map_err(|e| fail(e.to_string()))?;
        let mut command = tokio::process::Command::from(command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| fail(format!("cannot start {}: {e}", launcher.display())))?;
        let _reap = Reap::new(child.id());
        if let Some(mut stdin) = child.stdin.take() {
            // Written aside so a remote that answers before it has read it all
            // never blocks the pump; a write that fails is the remote having
            // exited, which the pump reports.
            // `cox run --prompt-stdin` reads its first line as a JSON string
            // (T68.6), so a newline in the prompt cannot end it early.
            let first = format!("{}\n", serde_json::Value::String(prompt));
            tokio::spawn(async move {
                let _ = stdin.write_all(first.as_bytes()).await;
            });
        }
        pump(&self.name, turn, child, true, events, cancel).await
    }
}

#[async_trait::async_trait]
impl ExternalAgent for TargetAgent {
    fn name(&self) -> &str {
        &self.name
    }

    async fn turn(
        &self,
        turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        let mode = PermissionMode::Default;
        self.run(mode, turn, prompt, &events, &cancel).await
    }

    async fn task_turn(
        &self,
        cx: TaskTurn,
        turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        self.run(cx.mode, turn, prompt, &events, &cancel).await
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use cox_protocol::config::ExternalAgentConfig;
    use cox_protocol::types::Submission;

    use super::*;

    /// A fake `ssh` that records its argv and the stdin it was given, writes
    /// a file beside itself (outside the workspace, where a sandboxed child could not), then answers as `cox run` does: a raw
    /// event line, the alias line and the summary with `usage`.
    const REMOTE: &str = "#!/bin/sh\n\
        printf '%s\\n' \"$@\" > \"$0.argv\"\n\
        read -r first\n\
        printf '%s\\n' \"$first\" > \"$0.stdin\"\n\
        prompt=$(printf '%s' \"$first\" | sed 's/^\"//;s/\"$//')\n\
        touch \"$0.outside\"\n\
        echo '{\"type\":\"item_started\",\"item\":\"x\"}'\n\
        printf '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"remote saw %s\"}]}}\\n' \"$prompt\"\n\
        echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"ok\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"cache_read_tokens\":2,\"cache_write_tokens\":1}}'\n";

    const DISPATCH: &str = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "work", preset = "remote" } }]
[[turn]]
text = "done"
"#;

    /// A config whose `remote` preset runs on `ssh:devbox` through a fake
    /// launcher written into `bin`, apart from the workspace `dir`; `allow` is
    /// the user's permission rules.
    fn config(dir: &Path, bin: &Path, allow: &[&str]) -> (Config, PathBuf) {
        let ssh = bin.join("ssh");
        std::fs::write(&ssh, REMOTE).expect("launcher");
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut config = Config::default();
        config.core.workspace_roots = vec![dir.to_path_buf()];
        config.launchers.ssh = Some(ssh.clone());
        config.permissions.allow = allow.iter().map(|r| (*r).to_string()).collect();
        let preset = ExternalAgentConfig {
            target: Some("ssh:devbox".into()),
            ..ExternalAgentConfig::default()
        };
        config.external_agents.insert("remote".into(), preset);
        (config, ssh)
    }

    fn driver(config: &Config, dir: &Path) -> Arc<dyn ExternalAgent> {
        let (mut drivers, left_out) = drivers(config, dir);
        assert!(left_out.is_empty(), "{left_out:?}");
        drivers.pop().expect("one driver")
    }

    fn side_file(ssh: &Path, ext: &str) -> PathBuf {
        PathBuf::from(format!("{}.{ext}", ssh.display()))
    }

    /// The prompt goes over stdin, never onto the launcher's argv, and the
    /// remote run's `usage` becomes one local `usage` row under the preset's
    /// name, so the ledger stays complete.
    #[tokio::test]
    async fn agent_on_target_records_remote_usage() {
        let (ws, bin) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("bin"),
        );
        let (config, ssh) = config(ws.path(), bin.path(), &["TargetAgent(ssh:devbox)"]);
        let driver = driver(&config, ws.path());
        let provider = cox_provider::scripted::Scripted::from_toml(DISPATCH, "").expect("scenario");
        let store = Arc::new(cox_core::MemoryStore::new());
        let session = cox_core::Session::new(
            config,
            Arc::new(provider),
            vec![],
            store.clone(),
            store.clone(),
            ws.path().to_path_buf(),
        )
        .expect("session");
        session.set_external_agents(vec![driver]);
        let mut rx = session.events().expect("events");
        // The `agent` call itself is the parent's; the preset's own guard is
        // the rule, which `bypass` does not waive.
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
        assert_eq!(results.len(), 1, "{results:?}");
        assert!(results[0].contains("remote saw"), "{results:?}");
        assert!(results[0].contains("work"), "{results:?}");
        let rows = store.usage_rows();
        let row = rows
            .iter()
            .find(|r| r.model.0 == "remote")
            .expect("a usage row for the preset");
        let u = &row.usage;
        let tokens = [u.input_tokens, u.output_tokens, u.cache_read_tokens];
        assert_eq!((tokens, u.cache_write_tokens), ([7, 3, 2], 1));
        let argv = std::fs::read_to_string(side_file(&ssh, "argv")).expect("argv");
        assert!(
            argv.contains("stream-json") && argv.contains("devbox"),
            "{argv}"
        );
        assert!(
            !argv.contains("work"),
            "the prompt reached the command line: {argv}"
        );
        let stdin = std::fs::read_to_string(side_file(&ssh, "stdin")).expect("stdin");
        let prompt: String = serde_json::from_str(stdin.trim_end_matches('\n'))
            .expect("the first stdin line is a JSON string");
        assert!(prompt.contains("work"), "{prompt}");
        assert_eq!(stdin.matches('\n').count(), 1, "{stdin}");
    }

    /// The run is not wrapped in the local sandbox (the remote has its own),
    /// so without a `TargetAgent` allow rule it is refused before anything
    /// starts, in `bypass` too; with one the launcher runs as a plain child.
    #[tokio::test]
    async fn agent_on_target_is_not_locally_sandboxed_but_asks() {
        let (ws, bin) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("bin"),
        );
        let (asking, ssh) = config(ws.path(), bin.path(), &[]);
        let dir = ws.path().to_path_buf();
        let run = |config: Config| {
            let dir = dir.clone();
            async move {
                let (tx, mut rx) = mpsc::channel(16);
                let cx = TaskTurn {
                    task: None,
                    mode: PermissionMode::Bypass,
                };
                let cancel = CancellationToken::new();
                let result = driver(&config, &dir)
                    .task_turn(cx, TurnId::new(), "go".into(), tx, cancel)
                    .await;
                rx.close();
                result
            }
        };
        let err = run(asking).await.expect_err("refused without a rule");
        assert!(err.to_string().contains("TargetAgent(ssh:devbox)"), "{err}");
        assert!(!side_file(&ssh, "argv").exists(), "the launcher ran");
        let (allowed, _) = config(ws.path(), bin.path(), &["TargetAgent(ssh:devbox)"]);
        run(allowed).await.expect("allowed by the rule");
        assert!(side_file(&ssh, "outside").exists(), "the launcher ran");
    }

    #[test]
    fn a_target_without_a_launcher_leaves_the_preset_out() {
        let (ws, bin) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("bin"),
        );
        let (mut config, _) = config(ws.path(), bin.path(), &[]);
        config.launchers.ssh = Some(PathBuf::from("ssh"));
        let (drivers, left_out) = drivers(&config, ws.path());
        assert!(drivers.is_empty());
        assert_eq!(left_out.len(), 1, "{left_out:?}");
        assert!(left_out[0].contains("remote"), "{left_out:?}");
    }
}
