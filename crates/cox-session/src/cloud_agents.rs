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
//!
//! T56.7: when a run ends its tokens become one `$0` external-usage row
//! (EA§6), named after the requested model; `cloud_runs`' usage claim makes
//! it exactly once, whichever process sees the end. A resumed session
//! finishes what its runs left: [`resume_runs`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_core::Session;
use cox_core::permission::rules::{CLOUD_AGENT, is_github_repo};
use cox_core::permission::{Engine, Outcome, cloud_agent_approval_text};
use cox_cursor_cloud::client::Client;
use cox_cursor_cloud::wire::{CreateAgentRequest, Repo, RunStatus, StreamEvent, UsageResponse};
use cox_plugin_api::CloudAgentDecl;
use cox_protocol::Config;
use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::{CallId, ItemId, SessionId, TaskId, TurnId};
use cox_protocol::traits::{ExternalAgent, TaskTurn};
use cox_protocol::types::{
    ApprovalPolicy, Event, ItemKind, Level, PermissionMode, Risk, SandboxMode, StopReason,
    ToolCall, Usage,
};
use cox_store::Store;
use cox_store::cloud_runs::{CloudRun, NewCloudRun};
use cox_tools::git::git;
use futures::StreamExt as _;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Names a loopback address that replaces `api.cursor.com`, for the offline
/// end-to-end test (T56.8) only. It lives in the environment, never in a
/// manifest or the config, so a plugin or a repository cannot set it, and the
/// client takes loopback alone, so even a stray value cannot move the key.
const TEST_URL_ENV: &str = "COX_CURSOR_CLOUD_URL";

fn client_for(secret: String) -> Result<Client, cox_cursor_cloud::client::CloudError> {
    let client = Client::new(secret)?;
    match std::env::var(TEST_URL_ENV) {
        Ok(base) if !base.is_empty() => client.with_loopback_base_url(&base),
        _ => Ok(client),
    }
}

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
) -> (Vec<Arc<CloudDriver>>, Vec<String>) {
    let (mut out, mut left_out) = (Vec::new(), Vec::new());
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
            .and_then(|secret| client_for(secret).map_err(|e| e.to_string()));
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

pub(crate) struct CloudDriver {
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

/// The ledger's name for what a run used: the model the preset asked for,
/// or `<backend>:auto` when the backend picked (it reports no model).
fn ledger_model(backend: &str, model: Option<&str>) -> String {
    model.map_or_else(|| format!("{backend}:auto"), str::to_string)
}

/// One run's tokens out of `GET /v1/agents/{id}/usage`: its own entry, or
/// the agent's total, which is the same thing for the one run cox starts
/// per agent. Saturates rather than wraps.
fn run_usage(report: &UsageResponse, run: &str) -> Usage {
    let tokens = report
        .runs
        .iter()
        .find(|entry| entry.id == run)
        .map_or(report.total_usage, |entry| entry.usage);
    let count = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    Usage {
        input_tokens: count(tokens.input_tokens),
        output_tokens: count(tokens.output_tokens),
        cache_read_tokens: count(tokens.cache_read_tokens),
        cache_write_tokens: count(tokens.cache_write_tokens),
        estimated: false,
        cost_usd: 0.0,
        latency_ms: 0,
    }
}

/// T56.7: what a resumed session owes its cloud runs, which `open` runs in
/// the background. A run that ended while cox was closed gets its result
/// and ledger row now; one still going is followed again until it ends; a
/// run whose usage read failed gets its row. A run no granted driver serves
/// is left for a resume that has one.
pub(crate) async fn resume_runs(session: Session, drivers: Vec<Arc<CloudDriver>>) {
    let Some(first) = drivers.first() else {
        return;
    };
    let runs = match first.store.cloud_runs_unsettled(&session.id()) {
        Ok(runs) => runs,
        Err(e) => {
            let text = format!("cloud runs not resumed: {e}");
            let _ = session.notice(Level::Warn, text).await;
            return;
        }
    };
    let pending = runs.into_iter().filter_map(|row| {
        let serves = |d: &&Arc<CloudDriver>| d.backend == row.backend;
        let driver = drivers
            .iter()
            .filter(serves)
            .find(|d| d.model == row.model)
            .or_else(|| drivers.iter().find(serves))?
            .clone();
        let session = session.clone();
        Some(async move {
            if let Err(e) = driver.finalize(&session, &row).await {
                let text = format!("cloud run {} of task {}: {e}", row.run_id, row.task_id);
                let _ = session
                    .notice(Level::Warn, cox_sanitize::sanitize(&text))
                    .await;
            }
        })
    });
    futures::future::join_all(pending).await;
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

    fn ledger_model(&self) -> String {
        ledger_model(self.backend, self.model.as_deref())
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
        let (agent, ran) = (
            self.name.clone(),
            self.run(cx, turn, prompt, &events, &cancel),
        );
        ran.await.map_err(|failure| match failure {
            Failure::Refused(message) => CoreError::ExternalAgentRefused {
                agent,
                message: cox_sanitize::sanitize(&message),
            },
            Failure::Ran(message) => CoreError::ExternalAgent {
                agent,
                message: cox_sanitize::sanitize(&message),
            },
        })
    }
}

/// Why a run ended in an error: refused before any request was made, which
/// the ledger does not see (T56.11), or failed after one went out.
enum Failure {
    Refused(String),
    Ran(String),
}

impl CloudDriver {
    async fn run(
        &self,
        cx: TaskTurn,
        turn: TurnId,
        prompt: String,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<Option<Usage>, Failure> {
        let (task, remote, head) = self.admit(cx).await.map_err(Failure::Refused)?;
        self.start_and_follow(task, remote, head, turn, prompt, events, cancel)
            .await
            .map_err(Failure::Ran)
    }

    /// Every check that runs before a request is made; an `Err` here costs
    /// nothing, so it leaves no ledger row (T56.11).
    async fn admit(&self, cx: TaskTurn) -> Result<(TaskId, Remote, Head), String> {
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
        Ok((task, remote, head))
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_and_follow(
        &self,
        task: TaskId,
        remote: Remote,
        head: Head,
        turn: TurnId,
        prompt: String,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<Option<Usage>, String> {
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
        // A stream that failed leaves the usage unclaimed for a resume.
        let answer = self.follow(&agent, &run, cancel).await?;
        // The core writes the row from what the turn returns (EA§6).
        let usage = self.settle(&agent, &run).await;
        let Some(answer) = answer else {
            return Ok(usage);
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
        Ok(usage)
    }

    /// The run's tokens, for the one caller that wins `cloud_runs`' usage
    /// claim. `None` while the row is not terminal (a later resume follows
    /// the run to its end) or when the read fails (a later resume retries),
    /// and for every caller after the first.
    async fn settle(&self, agent: &str, run: &str) -> Option<Usage> {
        let row = self.store.cloud_run(self.backend, run).ok().flatten()?;
        if !row.terminal || row.usage_recorded {
            return None;
        }
        let report = self.client.usage(agent).await.ok()?;
        let won = self.store.cloud_run_mark_usage_recorded(self.backend, run);
        won.ok()?.then(|| run_usage(&report, run))
    }

    /// One unsettled run of a resumed session. A run still going is never
    /// cancelled from here: closing cox again leaves it running for the
    /// next resume.
    async fn finalize(&self, session: &Session, row: &CloudRun) -> Result<(), String> {
        let (agent, run) = (row.agent_id.as_str(), row.run_id.as_str());
        if !row.terminal {
            let now = self.client.get_run(agent, run).await;
            let now = now.map_err(|e| e.to_string())?;
            let answer = if now.status.is_terminal() {
                self.record(run, &now.status);
                let text = now.result.filter(|t| !t.trim().is_empty());
                text.unwrap_or_else(|| format!("run {run} ended with no text"))
            } else {
                let never = CancellationToken::new();
                self.follow(agent, run, &never).await?.unwrap_or_default()
            };
            let answer = cox_sanitize::sanitize(&answer);
            let done = session.complete_resumed_task(row.task_id, &self.name, &answer);
            done.await.map_err(|e| e.to_string())?;
        }
        if let Some(usage) = self.settle(agent, run).await {
            let model = ledger_model(self.backend, row.model.as_deref());
            let recorded = session.record_resumed_task_usage(&model, usage);
            recorded.map_err(|e| e.to_string())?;
        }
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

    /// A session that dispatches `driver` as a background task, run until
    /// the task's result notice; the ledger it wrote and what was seen
    /// (`created`, the tool result, `completed`, the notice text).
    async fn dispatch(
        ws: &Path,
        driver: Arc<CloudDriver>,
    ) -> (Arc<cox_core::MemoryStore>, Vec<String>) {
        let provider = cox_provider::scripted::Scripted::from_toml(DISPATCH, "").expect("scenario");
        let memory = Arc::new(cox_core::MemoryStore::new());
        let mut config = Config::default();
        config.core.workspace_roots = vec![ws.to_path_buf()];
        let session = cox_core::Session::new(
            config,
            Arc::new(provider),
            vec![],
            memory.clone(),
            memory.clone(),
            ws.to_path_buf(),
        )
        .expect("session");
        session.set_external_agents(vec![driver]);
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
        (memory, seen)
    }

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
        let (_, seen) = dispatch(ws.path(), driver.clone()).await;
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

    /// Cursor's report: the run's own entry differs from the agent total,
    /// so a test sees which one the row took.
    const USAGE: &str = r#"{"totalUsage":{"inputTokens":900,"outputTokens":900},
        "runs":[{"id":"run-1","usage":{"inputTokens":11,"outputTokens":7,
        "cacheReadTokens":5,"cacheWriteTokens":3,"totalTokens":26}}]}"#;

    async fn mount_usage(server: &MockServer, times: u64) {
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(USAGE, "application/json"))
            .expect(times)
            .mount(server)
            .await;
    }

    fn assert_cursor_row(row: &cox_protocol::UsageRow, model: &str) {
        assert_eq!(row.provider, cox_protocol::types::ProviderId::External);
        assert_eq!(row.model.0, model);
        assert_eq!(row.job, cox_protocol::types::Job::Agent);
        let u = &row.usage;
        let tokens = (u.input_tokens, u.output_tokens, u.cache_read_tokens);
        assert_eq!((tokens, u.cache_write_tokens), ((11, 7, 5), 3));
        assert_eq!(u.cost_usd, 0.0, "billed on the user's Cursor plan");
        assert!(!u.estimated);
    }

    /// T56.7 Check: a finished run's ledger row goes through the external
    /// agent's path: provider external, $0, Cursor's four token counts for
    /// this run, the requested model (`cursor:auto` when none was named) and
    /// the task's job tag; the run is marked recorded.
    #[tokio::test]
    async fn cloud_agent_usage_row_is_billed_externally_with_cursor_tokens() {
        assert_eq!(ledger_model("cursor", Some("composer-2")), "composer-2");
        let Some(ws) = repo("git@github.com:acme/widgets.git") else {
            return;
        };
        let (home, server) = (
            tempfile::tempdir().expect("home"),
            MockServer::start().await,
        );
        let sse = ResponseTemplate::new(200).set_body_raw(STREAM, "text/event-stream");
        mount_run(&server, sse).await;
        mount_usage(&server, 1).await;
        let allow = ["CloudAgent(acme/widgets)"];
        let driver = Arc::new(driver(ws.path(), home.path(), &server, &allow));
        let (memory, seen) = dispatch(ws.path(), driver.clone()).await;
        assert!(seen.contains(&"completed".to_string()), "{seen:?}");
        let rows = memory.usage_rows();
        let external: Vec<_> = rows
            .iter()
            .filter(|r| r.provider == cox_protocol::types::ProviderId::External)
            .collect();
        let [row] = external.as_slice() else {
            panic!("one external row: {rows:?}");
        };
        assert_cursor_row(row, "cursor:auto");
        let run = driver.store.cloud_run("cursor", "run-1").expect("read");
        assert!(run.expect("row").usage_recorded);
        server.verify().await;
    }

    /// A resumed session over `home`, and the driver serving its runs,
    /// sharing its store; plus one `RUNNING` row of the session's task.
    fn resumed(ws: &Path, home: &Path) -> (Session, Arc<Store>, TaskId) {
        let (session, store) = crate::testing::scripted_session(home, ws, "");
        let task = TaskId::new();
        let row = NewCloudRun {
            backend: "cursor".into(),
            run_id: "run-1".into(),
            session_id: session.id(),
            task_id: task,
            agent_id: "bc-1".into(),
            repository: "acme/widgets".into(),
            model: None,
            status: "RUNNING".into(),
            terminal: false,
        };
        store.cloud_run_insert(&row).expect("insert");
        (session, store, task)
    }

    fn serving(
        ws: &Path,
        home: &Path,
        server: &MockServer,
        s: &Session,
        store: &Arc<Store>,
    ) -> Arc<CloudDriver> {
        Arc::new(CloudDriver {
            store: store.clone(),
            session: s.id(),
            ..driver(ws, home, server, &[])
        })
    }

    /// The resumed task's `TaskCompleted` and result notice, and the
    /// session's external ledger rows.
    async fn finished(
        session: &Session,
        store: &Store,
        task: TaskId,
    ) -> (String, Vec<cox_protocol::UsageRow>) {
        let mut rx = session.events().expect("events");
        let (mut completed, mut notice) = (false, String::new());
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::TaskCompleted { task: t, .. } => completed |= t == task,
                Event::Notice { text, .. } => notice = text,
                _ => {}
            }
        }
        assert!(completed, "TaskCompleted for {task}: {notice}");
        let rows = store.usage_for_session(&session.id()).expect("usage");
        (notice, rows)
    }

    /// T56.7 Check: whichever caller sees the run end first records its
    /// tokens; a second settle, and a resume after the live turn wrote the
    /// row, read nothing and write nothing.
    #[tokio::test]
    async fn cloud_agent_usage_row_is_written_once() {
        let (ws, home) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("home"),
        );
        let server = MockServer::start().await;
        mount_usage(&server, 1).await;
        let (session, store, _) = resumed(ws.path(), home.path());
        let driver = serving(ws.path(), home.path(), &server, &session, &store);
        assert_eq!(
            driver.settle("bc-1", "run-1").await,
            None,
            "not terminal yet"
        );
        driver.record("run-1", &RunStatus::Finished);
        let first = driver.settle("bc-1", "run-1").await.expect("first");
        assert_eq!((first.input_tokens, first.output_tokens), (11, 7));
        assert_eq!(driver.settle("bc-1", "run-1").await, None, "claimed once");
        resume_runs(session.clone(), vec![driver]).await;
        let rows = store.usage_for_session(&session.id()).expect("usage");
        assert!(rows.is_empty(), "the resume writes no second row: {rows:?}");
        server.verify().await;
    }

    /// T56.7 Check: a run still going when the session reopens is followed
    /// again; its end is the task's result in the resumed session, with the
    /// ledger row and a terminal, recorded `cloud_runs` row.
    #[tokio::test]
    async fn resume_reattaches_to_a_running_cloud_run() {
        let (ws, home) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("home"),
        );
        let server = MockServer::start().await;
        let running = r#"{"id":"run-1","status":"RUNNING"}"#;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(running, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(STREAM, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        mount_usage(&server, 1).await;
        let (session, store, task) = resumed(ws.path(), home.path());
        let driver = serving(ws.path(), home.path(), &server, &session, &store);
        let resume = resume_runs(session.clone(), vec![driver]);
        tokio::time::timeout(Duration::from_secs(20), resume)
            .await
            .expect("the reattached run ended");
        let (notice, rows) = finished(&session, &store, task).await;
        assert!(
            notice.contains("Hello world") && !notice.contains('\u{1b}'),
            "{notice}"
        );
        let [row] = rows.as_slice() else {
            panic!("one row: {rows:?}");
        };
        assert_cursor_row(row, "cursor:auto");
        let run = store
            .cloud_run("cursor", "run-1")
            .expect("read")
            .expect("row");
        assert_eq!(
            (run.status.as_str(), run.terminal, run.usage_recorded),
            ("FINISHED", true, true)
        );
        server.verify().await;
    }

    /// T56.7 Check: a run that ended while cox was closed is finalized at
    /// resume from `GET` alone (no stream): its result, sanitized, is the
    /// task's result item, and its tokens are the ledger row.
    #[tokio::test]
    async fn resume_finalizes_a_run_that_finished_while_closed() {
        let (ws, home) = (
            tempfile::tempdir().expect("ws"),
            tempfile::tempdir().expect("home"),
        );
        let server = MockServer::start().await;
        let ended = r#"{"id":"run-1","status":"FINISHED","result":"fixed while away\u001b[2J"}"#;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(ended, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        mount_usage(&server, 1).await;
        let (session, store, task) = resumed(ws.path(), home.path());
        let driver = serving(ws.path(), home.path(), &server, &session, &store);
        resume_runs(session.clone(), vec![driver]).await;
        let (notice, rows) = finished(&session, &store, task).await;
        assert!(
            notice.contains("fixed while away") && !notice.contains('\u{1b}'),
            "{notice}"
        );
        let [row] = rows.as_slice() else {
            panic!("one row: {rows:?}");
        };
        assert_cursor_row(row, "cursor:auto");
        let run = store
            .cloud_run("cursor", "run-1")
            .expect("read")
            .expect("row");
        assert_eq!(
            (run.status.as_str(), run.terminal, run.usage_recorded),
            ("FINISHED", true, true)
        );
        server.verify().await;
    }
}
