// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The cloud agent drivers (T56.6, P56): one per granted `[[cloud_agents]]`
//! entry, each an `ExternalAgent` that `agent(preset: <name>)` dispatches
//! the way P35's external agents are, but always in the background. A turn
//! sends the prompt and the repository's GitHub URL to the backend's hosted
//! agent, records the run in `cloud_runs` (T56.5) so it outlives this
//! session, and folds the run's stream into the task's answer. Here, in
//! session assembly, because it joins the store, the permission engine and
//! the Cursor client, which may not see each other.
//!
//! Guards: code leaving the machine is `Engine`'s `CloudAgent(<owner>/<repo>)`
//! decision (T56.4), asked before any request. Only a GitHub `origin` is
//! accepted, and the URL sent is rebuilt from the owner and the repository
//! alone, so credentials, a port or a path in the configured remote never
//! leave. The sandbox and `path::confine` do not apply: nothing runs here.
//! Every string from the backend passes `cox_sanitize::sanitize`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_core::permission::rules::{CLOUD_AGENT, is_github_repo};
use cox_core::permission::{Engine, Outcome, cloud_agent_approval_text};
use cox_cursor_cloud::client::Client;
use cox_cursor_cloud::wire::{CreateAgentRequest, Repo, RunStatus, StreamEvent};
use cox_plugin_api::CloudAgentDecl;
use cox_protocol::Config;
use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::{CallId, ItemId, SessionId, TurnId};
use cox_protocol::traits::{ExternalAgent, TaskTurn};
use cox_protocol::types::{
    ApprovalPolicy, Event, ItemKind, PermissionMode, Risk, SandboxMode, StopReason, ToolCall, Usage,
};
use cox_store::Store;
use cox_store::cloud_runs::NewCloudRun;
use cox_tools::git::git;
use futures::StreamExt as _;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// One driver per entry whose key resolves, plus one warning per entry left
/// out. `key` is `cox_provider::http::resolve_key` (a test passes its own,
/// never the OS keychain), called with the entry's `key_env` and the
/// backend's name as the keyring section, as `Client::from_env` does.
pub(crate) fn drivers(
    decls: Vec<CloudAgentDecl>,
    config: &Config,
    session: SessionId,
    cwd: &Path,
    store: Arc<Store>,
    key: impl Fn(&str, &str) -> Result<String, ProviderError>,
) -> (Vec<Arc<dyn ExternalAgent>>, Vec<String>) {
    let (mut out, mut left_out): (Vec<Arc<dyn ExternalAgent>>, _) = (Vec::new(), Vec::new());
    if decls.is_empty() {
        return (out, left_out);
    }
    let home = std::env::home_dir();
    let engine = match Engine::compile(&config.permissions, home.as_deref(), cwd) {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            left_out.push(format!("cloud agents left out: permissions: {e}"));
            return (out, left_out);
        }
    };
    for decl in decls {
        let client = key(&decl.key_env, decl.backend.name())
            .map_err(|_| format!("{} is not set", decl.key_env))
            .and_then(|secret| Client::new(secret).map_err(|e| e.to_string()));
        match client {
            Ok(client) => out.push(Arc::new(CloudDriver {
                name: decl.name,
                backend: decl.backend.name(),
                model: decl.model,
                client,
                engine: engine.clone(),
                approval: config.permissions.approval,
                sandbox: config.sandbox.mode,
                session,
                cwd: cwd.to_path_buf(),
                store: store.clone(),
            })),
            Err(e) => left_out.push(format!("cloud agent {} left out: {e}", decl.name)),
        }
    }
    (out, left_out)
}

struct CloudDriver {
    name: String,
    backend: &'static str,
    model: Option<String>,
    client: Client,
    engine: Arc<Engine>,
    approval: ApprovalPolicy,
    sandbox: SandboxMode,
    /// The session whose tasks the runs belong to, for `cloud_runs`.
    session: SessionId,
    cwd: PathBuf,
    store: Arc<Store>,
}

/// The repository as GitHub names it, and the only URL cox sends for it.
#[derive(Debug, PartialEq, Eq)]
struct Remote {
    repo: String,
    url: String,
}

/// `owner/repo` from a GitHub remote in any form git accepts (https, ssh,
/// scp-like); `None` for any other host or shape. Only the owner and the
/// repository survive, so userinfo, a port, a query or a fragment cannot.
fn parse_github(raw: &str) -> Option<Remote> {
    let (authority, path) = match raw.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(scheme.as_str(), "https" | "http" | "ssh" | "git") {
                return None;
            }
            rest.split_once('/')?
        }
        None => raw.split_once(':')?,
    };
    // The last `@` ends any userinfo, which may itself hold an `@`.
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = path.split(['?', '#']).next()?.trim_matches('/');
    let repo = path.strip_suffix(".git").unwrap_or(path);
    is_github_repo(repo).then(|| Remote {
        repo: repo.to_string(),
        url: format!("https://github.com/{repo}"),
    })
}

/// Where the run starts: the current branch (or the commit when detached),
/// and whether a remote-tracking ref of `origin` already has `HEAD`. Read
/// from the last fetch, so it makes no network call.
struct Head {
    git_ref: String,
    commit: String,
    on_remote: bool,
}

impl Head {
    async fn read(cwd: &Path) -> Result<Self, String> {
        let commit = git(cwd, &["rev-parse", "--verify", "HEAD"])
            .await
            .map(|c| c.trim().to_string())
            .ok_or("the repository has no commit to start from")?;
        let branch = git(cwd, &["symbolic-ref", "--short", "-q", "HEAD"]).await;
        let git_ref = branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| commit.clone());
        let contains = ["for-each-ref", "--contains", "HEAD", "refs/remotes/origin"];
        let on_remote = git(cwd, &contains)
            .await
            .is_some_and(|refs| !refs.trim().is_empty());
        Ok(Self {
            git_ref,
            commit,
            on_remote,
        })
    }

    /// Said wherever the user or the model reads about the run, since the
    /// backend clones from GitHub and never sees unpushed commits.
    fn warning(&self) -> Option<String> {
        let short: String = self.commit.chars().take(12).collect();
        (!self.on_remote).then(|| {
            format!(
                "Warning: local HEAD {short} is not on origin; the run starts from {} as GitHub has it, without unpushed commits.",
                self.git_ref
            )
        })
    }
}

#[async_trait::async_trait]
impl ExternalAgent for CloudDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn background_only(&self) -> bool {
        true
    }

    /// Outside a task there is no row to record the run against, so this is
    /// refused before any request.
    async fn turn(
        &self,
        turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        let cx = TaskTurn {
            task: None,
            mode: PermissionMode::Default,
        };
        self.task_turn(cx, turn, prompt, events, cancel).await
    }

    async fn task_turn(
        &self,
        cx: TaskTurn,
        turn: TurnId,
        prompt: String,
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, CoreError> {
        // T56.7 turns the run's usage into the ledger row.
        self.run(cx, turn, prompt, &events, &cancel)
            .await
            .map(|()| None)
            .map_err(|message| CoreError::ExternalAgent {
                agent: self.name.clone(),
                message: cox_sanitize::sanitize(&message),
            })
    }
}

impl CloudDriver {
    async fn run(
        &self,
        cx: TaskTurn,
        turn: TurnId,
        prompt: String,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let task = cx
            .task
            .ok_or("a cloud agent runs only as a background task")?;
        let origin = git(&self.cwd, &["remote", "get-url", "origin"])
            .await
            .ok_or("the repository has no `origin` remote")?;
        let remote = parse_github(origin.trim())
            .ok_or("a cloud agent needs a GitHub `origin` remote; other hosts are refused")?;
        let head = Head::read(&self.cwd).await?;
        self.approve(&remote, &head, cx.mode)?;
        let repo = Repo {
            url: remote.url.clone(),
            starting_ref: Some(head.git_ref.clone()),
        };
        let mut request = CreateAgentRequest::new(prompt, repo);
        if let Some(model) = &self.model {
            request = request.with_model(model);
        }
        let created = self.client.create_agent(&request).await;
        let created = created.map_err(|e| e.to_string())?;
        let (agent, run) = (created.agent.id, created.run.id);
        let row = NewCloudRun {
            backend: self.backend.to_string(),
            run_id: run.clone(),
            session_id: self.session,
            task_id: task,
            agent_id: agent.clone(),
            repository: remote.repo,
            model: self.model.clone(),
            status: created.run.status.as_str().to_string(),
            terminal: created.run.status.is_terminal(),
        };
        if let Err(e) = self.store.cloud_run_insert(&row) {
            // A billed run with no row would outlive every trace of it.
            let _ = self.client.cancel_run(&agent, &run).await;
            return Err(format!(
                "could not record run {run}, so it was cancelled: {e}"
            ));
        }
        let Some(answer) = self.follow(&agent, &run, cancel).await? else {
            return Ok(());
        };
        let text = match head.warning() {
            Some(warning) => format!("{warning}\n\n{answer}"),
            None => answer,
        };
        let item = ItemId::new();
        let kind = ItemKind::AssistantMessage {
            text: cox_sanitize::sanitize(&text),
        };
        let _ = events.send(Event::ItemStarted { item, kind }).await;
        let _ = events.send(Event::ItemDone { item }).await;
        let stop = StopReason::EndTurn;
        let _ = events.send(Event::TurnDone { turn, stop }).await;
        Ok(())
    }

    /// `Engine`'s verdict on sending this repository away. A background task
    /// has no route to the user's approval prompt yet (as `RefuseAsk` for
    /// ACP agents), so an `Ask` is refused with the approval text and the
    /// rule that would allow it.
    fn approve(&self, remote: &Remote, head: &Head, mode: PermissionMode) -> Result<(), String> {
        let call = ToolCall {
            id: CallId::new(),
            name: CLOUD_AGENT.into(),
            input: json!({"remote": remote.url, "ref": head.git_ref}),
            risk: Risk::Exec,
            subject: remote.repo.clone(),
            segments: None,
        };
        match self
            .engine
            .decide(&call, mode, self.approval, self.sandbox, &[])
        {
            Outcome::Allow { .. } => Ok(()),
            Outcome::Deny { reason, .. } => Err(reason),
            Outcome::Ask(_) => {
                let warning = head
                    .warning()
                    .map_or_else(String::new, |w| format!("\n{w}"));
                Err(format!(
                    "{}{warning}\nA background task cannot ask for approval yet; allow it with \
                     `CloudAgent({})` in your own config's `permissions.allow`.",
                    cloud_agent_approval_text(&call),
                    remote.repo
                ))
            }
        }
    }

    /// The run's assistant text, or `None` when the task was cancelled (the
    /// run is cancelled too). Ends at `done`, the `result` event, a terminal
    /// status or the end of the stream.
    async fn follow(
        &self,
        agent: &str,
        run: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, String> {
        let stream = self.client.stream_run(agent, run);
        let mut stream = std::pin::pin!(stream.map_err(|e| e.to_string())?);
        let (mut answer, mut result, mut failed) = (String::new(), None, None);
        loop {
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    self.client.cancel_run(agent, run).await.map_err(|e| e.to_string())?;
                    self.record(run, &RunStatus::Cancelled);
                    return Ok(None);
                }
                next = stream.next() => next,
            };
            match next.transpose().map_err(|e| e.to_string())? {
                None | Some(StreamEvent::Done) => break,
                Some(StreamEvent::Assistant { text }) => answer.push_str(&text),
                Some(StreamEvent::Status(status)) => {
                    self.record(run, &status);
                    if status.is_terminal() {
                        break;
                    }
                }
                Some(StreamEvent::Result(end)) => {
                    self.record(run, &end.status);
                    result = end.text;
                    break;
                }
                Some(StreamEvent::Error { message, .. }) => failed = Some(message),
                Some(_) => {}
            }
        }
        if answer.trim().is_empty() {
            answer = result.unwrap_or_default();
        }
        match failed {
            Some(message) if answer.trim().is_empty() => {
                Err(format!("run {run} failed: {message}"))
            }
            _ if answer.trim().is_empty() => Ok(Some(format!("run {run} ended with no text"))),
            _ => Ok(Some(answer)),
        }
    }

    /// A status write that fails leaves the row behind, not the run: the
    /// task's answer does not depend on it, so it is not an error here.
    fn record(&self, run: &str, status: &RunStatus) {
        let terminal = status.is_terminal();
        let _ = self
            .store
            .cloud_run_set_status(self.backend, run, status.as_str(), terminal);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cox_protocol::ids::TaskId;
    use cox_protocol::traits::Store as _;
    use cox_protocol::types::Submission;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const CREATED: &str = r#"{"agent":{"id":"bc-1"},"run":{"id":"run-1","status":"CREATING"}}"#;
    const STREAM: &str = "event: status\ndata: {\"status\":\"RUNNING\"}\n\n\
        event: assistant\ndata: {\"text\":\"Hello \"}\n\n\
        event: thinking\ndata: {\"text\":\"secret plan\"}\n\n\
        event: assistant\ndata: {\"text\":\"world\\u001b[31m\"}\n\n\
        event: result\ndata: {\"runId\":\"run-1\",\"status\":\"FINISHED\"}\n\n\
        event: done\ndata: {}\n\n";

    /// A repository whose `origin` is `remote`, with one commit that no
    /// remote-tracking ref has; `None` where git is missing.
    fn repo(remote: &str) -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().ok()?;
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(dir.path())
                .output()
                .is_ok_and(|o| o.status.success())
        };
        let made = run(&["init", "-q", "-b", "main"])
            && run(&["remote", "add", "origin", remote])
            && run(&["commit", "-q", "--allow-empty", "-m", "first"]);
        made.then_some(dir)
    }

    fn driver(ws: &Path, home: &Path, server: &MockServer, allow: &[&str]) -> CloudDriver {
        let mut config = Config::default();
        config.permissions.allow = allow.iter().map(|r| r.to_string()).collect();
        config.permissions.deny = vec!["CloudAgent(acme/secret)".into()];
        let engine = Engine::compile(&config.permissions, None, ws).expect("engine");
        let client = Client::new("test-key").and_then(|c| c.with_base_url(&server.uri()));
        CloudDriver {
            name: "cursor-cloud".into(),
            backend: "cursor",
            model: None,
            client: client.expect("client"),
            engine: Arc::new(engine),
            approval: config.permissions.approval,
            sandbox: config.sandbox.mode,
            session: SessionId::new(),
            cwd: ws.to_path_buf(),
            store: Arc::new(Store::open(home).expect("store")),
        }
    }

    async fn turn_on(driver: &CloudDriver, cancel: CancellationToken) -> Result<(), CoreError> {
        let cx = TaskTurn {
            task: Some(TaskId::new()),
            mode: PermissionMode::Auto,
        };
        let (tx, _rx) = mpsc::channel(16);
        let out = driver.task_turn(cx, TurnId::new(), "work".into(), tx, cancel);
        out.await.map(|_| ())
    }

    async fn mount_run(server: &MockServer, stream: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v1/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CREATED, "application/json"))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .respond_with(stream)
            .mount(server)
            .await;
    }

    /// T56.6 Check: another host, or a look-alike of GitHub, is refused
    /// before any request.
    #[tokio::test]
    async fn cloud_agent_refuses_a_non_github_remote() {
        for raw in [
            "https://gitlab.com/acme/widgets.git",
            "https://github.com.evil.test/acme/widgets",
            "https://evil.test/github.com/acme/widgets",
            "git@bitbucket.org:acme/widgets.git",
            "file:///srv/acme/widgets",
            "/srv/acme/widgets",
            "https://github.com/acme/widgets/tree/main",
        ] {
            assert_eq!(parse_github(raw), None, "{raw}");
        }
        let Some(ws) = repo("https://gitlab.com/acme/widgets.git") else {
            return;
        };
        let (home, server) = (
            tempfile::tempdir().expect("home"),
            MockServer::start().await,
        );
        let driver = driver(
            ws.path(),
            home.path(),
            &server,
            &["CloudAgent(acme/widgets)"],
        );
        let refused = turn_on(&driver, CancellationToken::new()).await;
        assert!(format!("{refused:?}").contains("GitHub"), "{refused:?}");
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
    }

    /// T56.6 Check: only the owner and the repository leave, whatever the
    /// configured remote carries.
    #[test]
    fn cloud_agent_strips_credentials_from_the_remote() {
        for raw in [
            "https://user:ghp_secret@github.com/acme/widgets.git",
            "https://x-access-token:p@ss@GitHub.com:443/acme/widgets/?token=1#frag",
            "ssh://git@github.com/acme/widgets.git",
            "git@github.com:acme/widgets.git",
            "http://github.com/acme/widgets",
        ] {
            let remote = parse_github(raw).expect(raw);
            assert_eq!(remote.repo, "acme/widgets", "{raw}");
            assert_eq!(remote.url, "https://github.com/acme/widgets", "{raw}");
        }
    }

    /// T56.6 Check: a deny rule, and an `Ask` no one can answer, both stop
    /// the run before the first request; the refusal of the ask carries the
    /// approval text and the unpushed-HEAD warning.
    #[tokio::test]
    async fn cloud_agent_denied_by_engine_makes_no_request() {
        let (home, server) = (
            tempfile::tempdir().expect("home"),
            MockServer::start().await,
        );
        let Some(ws) = repo("https://github.com/acme/secret.git") else {
            return;
        };
        let secret = driver(ws.path(), home.path(), &server, &[]);
        let denied = format!("{:?}", turn_on(&secret, CancellationToken::new()).await);
        assert!(
            denied.contains("denied by rule CloudAgent(acme/secret)"),
            "{denied}"
        );
        let Some(ws) = repo("https://github.com/acme/widgets.git") else {
            return;
        };
        let widgets = driver(ws.path(), home.path(), &server, &[]);
        let asked = format!("{:?}", turn_on(&widgets, CancellationToken::new()).await);
        assert!(
            asked.contains("Run a cloud agent on acme/widgets?"),
            "{asked}"
        );
        assert!(asked.contains("Starting ref: main"), "{asked}");
        assert!(asked.contains("is not on origin"), "{asked}");
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
    }

    const DISPATCH: &str = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "work", preset = "cursor-cloud" } }]
[[turn]]
text = "done"
"#;

    /// T56.6 Check: `agent(preset)` without `background` still runs in the
    /// background; the run's assistant text (sanitized, thinking left out)
    /// is the task's answer between `TaskCreated` and `TaskCompleted`, the
    /// request carries the clean GitHub URL and the row ends `FINISHED`.
    #[tokio::test]
    async fn cloud_agent_stream_maps_to_task_events() {
        let Some(ws) = repo("https://user:ghp_secret@github.com/acme/widgets.git") else {
            return;
        };
        let (home, server) = (
            tempfile::tempdir().expect("home"),
            MockServer::start().await,
        );
        let sse = ResponseTemplate::new(200).set_body_raw(STREAM, "text/event-stream");
        mount_run(&server, sse).await;
        let driver = Arc::new(driver(
            ws.path(),
            home.path(),
            &server,
            &["CloudAgent(acme/widgets)"],
        ));
        let provider = cox_provider::scripted::Scripted::from_toml(DISPATCH, "").expect("scenario");
        let memory = Arc::new(cox_core::MemoryStore::new());
        let mut config = Config::default();
        config.core.workspace_roots = vec![ws.path().to_path_buf()];
        let session = cox_core::Session::new(
            config,
            Arc::new(provider),
            vec![],
            memory.clone(),
            memory,
            ws.path().to_path_buf(),
        )
        .expect("session");
        session.set_external_agents(vec![driver.clone()]);
        let mut rx = session.events().expect("events");
        let mode = PermissionMode::Bypass;
        let sub = Submission::SetPermissionMode { mode };
        session.submit(sub).await.expect("mode");
        let runner = session.clone();
        tokio::spawn(async move {
            let text = "go".into();
            let sub = Submission::UserTurn {
                text,
                attachments: vec![],
                confirm_think: false,
            };
            runner.submit(sub).await
        });
        let mut seen = Vec::new();
        let collect = async {
            while let Some(ev) = rx.recv().await {
                match &ev {
                    Event::TaskCreated { .. } => seen.push("created".to_string()),
                    Event::ToolCallDone { result, .. } => seen.push(result.visible.clone()),
                    Event::TaskCompleted { .. } => seen.push("completed".to_string()),
                    Event::Notice { text, .. } if text.contains("Hello") => {
                        seen.push(text.clone());
                        break;
                    }
                    _ => {}
                }
            }
        };
        let collected = tokio::time::timeout(Duration::from_secs(20), collect).await;
        assert!(collected.is_ok(), "{seen:?}");
        assert_eq!(seen[0], "created", "{seen:?}");
        let started = seen.iter().any(|s| s.starts_with("background task"));
        assert!(started, "{seen:?}");
        assert!(seen.contains(&"completed".to_string()), "{seen:?}");
        let answer = seen.last().expect("answer");
        assert!(answer.contains("Hello world"), "{answer}");
        assert!(answer.contains("is not on origin"), "{answer}");
        assert!(
            !answer.contains('\u{1b}') && !answer.contains("secret plan"),
            "{answer}"
        );
        let requests = server.received_requests().await.expect("recorded");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json");
        assert_eq!(body["repos"][0]["url"], "https://github.com/acme/widgets");
        assert_eq!(body["repos"][0]["startingRef"], "main");
        assert!(!String::from_utf8_lossy(&requests[0].body).contains("ghp_secret"));
        let row = driver.store.cloud_run("cursor", "run-1").expect("read");
        let row = row.expect("row");
        assert_eq!((row.status.as_str(), row.terminal), ("FINISHED", true));
        assert_eq!(row.repository, "acme/widgets");
    }

    /// T56.6 Check: cancelling the task while the run streams cancels the
    /// run on the backend once, ends the turn and marks the row cancelled.
    #[tokio::test]
    async fn cloud_agent_task_cancel_cancels_the_run() {
        let Some(ws) = repo("git@github.com:acme/widgets.git") else {
            return;
        };
        let (home, server) = (
            tempfile::tempdir().expect("home"),
            MockServer::start().await,
        );
        let stalled = ResponseTemplate::new(200).set_delay(Duration::from_secs(60));
        mount_run(&server, stalled).await;
        Mock::given(method("POST"))
            .and(path("/v1/agents/bc-1/runs/run-1/cancel"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(r#"{"id":"run-1"}"#, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let driver = driver(
            ws.path(),
            home.path(),
            &server,
            &["CloudAgent(acme/widgets)"],
        );
        let cancel = CancellationToken::new();
        let turn = turn_on(&driver, cancel.clone());
        let stop = async {
            while driver
                .store
                .cloud_run("cursor", "run-1")
                .ok()
                .flatten()
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            cancel.cancel();
        };
        let (ended, ()) =
            tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(turn, stop) })
                .await
                .expect("the turn ended on cancel");
        assert!(ended.is_ok(), "{ended:?}");
        let row = driver.store.cloud_run("cursor", "run-1").expect("read");
        let row = row.expect("row");
        assert_eq!((row.status.as_str(), row.terminal), ("CANCELLED", true));
        server.verify().await;
    }
}
