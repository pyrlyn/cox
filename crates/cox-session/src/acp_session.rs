// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A top-level session driven by an external ACP agent (T52.4, DT§3.3.1):
//! one agent process for the session's life, not one per turn as the
//! subagent driver in `external_agents` runs it. Here, beside that driver,
//! because it reuses the driver's spawn (env allowlist, process group,
//! reap), its `ClientHost` and its sandbox wrap, so there is still one way
//! an agent process starts.
//!
//! The surface gets a plain `Event` stream, folded by `cox_acp::UpdateFold`
//! from what the agent reports, with the `TurnStarted` and user item the
//! agent does not echo put in front of each prompt. ACP allows one prompt
//! in flight, so a prompt sent while one runs waits here in order.
//!
//! What the agent asks cox for with `session/request_permission` and the
//! engine escalates becomes an `ApprovalRequired` in the same stream, with
//! the agent as its source, so it lands in the inbox like any other ask
//! (T52.5); the user's answer comes back through [`AcpSession::approve`].
//!
//! A stored session reopens with `session/load` when the agent advertises
//! it (T52.6). What the agent replays then is dropped: cox's own rollout
//! already holds the session as the user saw it.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, LoadSessionRequest, NewSessionRequest, PromptRequest,
    PromptResponse, SessionId as AcpId, SessionUpdate,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo, ConnectionTo};
use cox_acp::{Approver, ClientHost, UpdateFold};
use cox_plugin::external_agent::ExternalAgentCommand;
use cox_plugin_api::AgentMode;
use cox_protocol::Config;
use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::{CallId, ItemId, SessionId, TurnId};
use cox_protocol::traits::Store as _;
use cox_protocol::types::{
    DecidedBy, Decision, Event, ItemKind, Job, ModelId, Source, Tier, ToolCall, Why,
};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use crate::external_agents::{Reap, Spawn, acp_host, stderr_tail};

/// The core's own event bound (DT§4.5).
const EVENTS: usize = 256;
/// How long a failed prompt waits for the agent's stderr to close, so the
/// error can name what the agent said before it died.
const TAIL_WAIT: Duration = Duration::from_millis(500);
/// How long one of the agent's asks waits in the inbox before it is denied
/// (DT§3.3.1: "a timeout answers deny"). The agent's own turn is blocked on
/// the answer meanwhile, so the ask cannot wait forever; long enough for
/// someone who stepped away for a coffee.
const ASK_WAIT: Duration = Duration::from_secs(15 * 60);

/// Why an agent session did not open.
#[derive(Debug, thiserror::Error)]
pub enum AcpOpenError {
    /// EA§7: its program is on no `PATH` directory, its key is not set, or
    /// its host could not be built. The one warning the surface shows.
    #[error("{0}")]
    Unavailable(String),
    /// The process started but `initialize` or `session/new` failed.
    #[error(transparent)]
    Agent(#[from] CoreError),
    /// A stored session was asked for, and the agent does not advertise
    /// `loadSession`: the session can only be read (T52.6).
    #[error("{0} cannot reopen a session: it does not support session/load")]
    NoLoad(String),
}

/// A stored session to reopen with `session/load` (T52.6).
#[derive(Debug, Clone)]
pub struct AcpResume {
    /// The agent's ACP `sessionId`, as `sessions.agent_session` keeps it.
    pub session: String,
    /// The main turns the rollout already holds, so numbering goes on.
    pub turns: u32,
}

enum Input {
    Prompt(String),
    Cancel,
}

/// The running agent. Ending it, or dropping it, kills its process group.
pub struct AcpSession {
    id: SessionId,
    agent: String,
    input: mpsc::UnboundedSender<Input>,
    asks: Arc<Asks>,
    process: Mutex<Option<(Child, Reap)>>,
}

/// A session just opened, and the events it will emit.
pub struct OpenedAcp {
    pub session: AcpSession,
    pub events: mpsc::Receiver<Event>,
    /// The agent's ACP `sessionId`, for `sessions.agent_session`.
    pub agent_session: String,
}

impl AcpSession {
    /// cox's id for this session: the `Source` its asks carry, and its row.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// The agent's name, as its entry declares it.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The user's answer to the agent's ask `call`. False when nothing waits
    /// under that id: answered already, timed out, or the session closed.
    pub fn approve(&self, call: CallId, decision: Decision) -> bool {
        self.asks.answer(call, decision, DecidedBy::User)
    }

    /// Sends `text` as the next prompt, after the one in flight if any.
    /// False once the agent is gone.
    pub fn prompt(&self, text: String) -> bool {
        self.input.send(Input::Prompt(text)).is_ok()
    }

    /// `session/cancel` for the prompt in flight; prompts waiting behind it
    /// are dropped. The turn ends when the agent answers `cancelled`.
    pub fn cancel(&self) -> bool {
        // ACP: after `session/cancel` the client answers every pending
        // permission request; a deny is the closest `Decision` to that.
        self.asks
            .settle(|| String::from("the turn was interrupted"), false);
        self.input.send(Input::Cancel).is_ok()
    }

    /// Kills the agent's process group now (closing the session, or quit).
    /// Its asks still in the inbox are denied first, and no new one opens.
    pub fn end(&self) {
        self.asks
            .settle(|| String::from("the session closed"), true);
        let process = self
            .process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(process);
    }
}

/// Every agent a top-level session may be driven by: the user config's
/// `[external_agents.<name>]` entries and the granted plugins'
/// `[[external_agents]]` entries, each already wrapped, plus one warning per
/// entry refused or plugin skipped.
pub fn agents(
    config: &Config,
    home: &Path,
    cwd: &Path,
    writable: &[PathBuf],
) -> (Vec<ExternalAgentCommand>, Vec<String>) {
    let (mut agents, mut warnings) =
        crate::external_agents::config_agents(config, writable, std::env::home_dir().as_deref());
    match cox_store::Store::open(home) {
        Ok(store) => {
            let plugins = crate::load_plugins(config, home, cwd, Arc::new(store), Some(writable));
            agents.extend(plugins.external_agents);
            warnings.extend(plugins.notices);
        }
        Err(e) => warnings.push(format!("plugin agents left out: {e}")),
    }
    (agents, warnings)
}

/// Starts `agent` in `cwd` under its wrap and opens an ACP session with it.
/// `path` is where a bare program name is looked up; `key` resolves the
/// entry's `key_env` (a test passes its own, never the OS keychain). The
/// engine's `Ask` verdicts go to the user as inbox items. `id` is cox's id
/// for the session; `resume` reopens a stored one.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    agent: ExternalAgentCommand,
    config: &Config,
    cwd: &Path,
    writable: &[PathBuf],
    path: Option<&OsStr>,
    key: impl Fn(&str, &str) -> Result<String, ProviderError>,
    id: SessionId,
    resume: Option<AcpResume>,
) -> Result<OpenedAcp, AcpOpenError> {
    let name = agent.name().to_string();
    let unavailable = |why: String| {
        AcpOpenError::Unavailable(format!("{name} ({}) cannot start: {why}", agent.origin()))
    };
    if agent.mode() != AgentMode::Acp {
        return Err(unavailable(String::from("it speaks stream-json, not ACP")));
    }
    if let Some(cli) = agent.missing_cli(path) {
        return Err(unavailable(format!("`{}` is not on PATH", cli.display())));
    }
    let Ok(secret) = key(agent.key_env(), &name) else {
        return Err(unavailable(format!("{} is not set", agent.key_env())));
    };
    let host = acp_host(config, cwd, writable).map_err(unavailable)?;
    let spawn = Spawn {
        name: name.clone(),
        key_env: agent.key_env().to_string(),
        mode: agent.mode(),
        agent,
        key: secret,
        cwd: cwd.to_path_buf(),
    };
    let (mut child, reap) = spawn.spawn(&[], Stdio::piped())?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(spawn.error("no stdio pipes".into()).into());
    };
    let tail = tokio::spawn(stderr_tail(stderr));
    let client = move |asks, updates| host.client(asks, updates);
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let mut opened = connect(name, id, transport, client, Some(tail), resume).await?;
    opened.session.process = Mutex::new(Some((child, reap)));
    Ok(opened)
}

/// [`open`] over any transport: `initialize`, then `session/new` in the
/// host's cwd, or `session/load` for `resume`, then the session loop. A
/// test passes one end of a duplex channel. `host` builds the client from
/// the approver that turns the engine's asks into inbox items and the
/// sender its `session/update`s go to.
pub async fn connect(
    agent: String,
    id: SessionId,
    transport: impl ConnectTo<Client> + 'static,
    host: impl FnOnce(Arc<dyn Approver>, mpsc::UnboundedSender<SessionUpdate>) -> ClientHost,
    tail: Option<JoinHandle<String>>,
    resume: Option<AcpResume>,
) -> Result<OpenedAcp, AcpOpenError> {
    let (input_tx, input) = mpsc::unbounded_channel();
    let (events_tx, events) = mpsc::channel(EVENTS);
    let (ready_tx, ready) = oneshot::channel::<Result<AcpId, Setup>>();
    let (updates_tx, updates) = mpsc::unbounded_channel();
    let (notes_tx, notes) = mpsc::unbounded_channel();
    let asks = Arc::new(Asks::new(agent.clone(), id, notes_tx));
    let host = host(Arc::clone(&asks) as Arc<dyn Approver>, updates_tx);
    let (cwd, sandboxed) = (host.cwd.clone(), host.sandbox.is_some());
    let driver = Driver {
        fold: UpdateFold::new(agent.clone(), TurnId::new()),
        model: ModelId(format!("{agent} · ACP")),
        events: events_tx,
        tail,
        seq: resume.as_ref().map_or(0, |r| r.turns),
        busy: false,
        waiting: VecDeque::new(),
        started: Instant::now(),
    };
    let mut updates = updates;
    let run = cox_acp::connect(transport, host, async move |cx| {
        let setup = async {
            let init = cx
                .send_request(cox_acp::initialize_request(sandboxed))
                .block_task()
                .await?;
            match resume {
                Some(stored) if init.agent_capabilities.load_session => {
                    let session = AcpId::from(stored.session);
                    let load = LoadSessionRequest::new(session.clone(), cwd);
                    cx.send_request(load).block_task().await?;
                    // The replay is what cox's rollout already holds.
                    while updates.try_recv().is_ok() {}
                    Ok::<_, agent_client_protocol::Error>(Some(session))
                }
                Some(_) => Ok(None),
                None => {
                    let new = cx.send_request(NewSessionRequest::new(cwd));
                    Ok(Some(new.block_task().await?.session_id))
                }
            }
        };
        let session = match setup.await {
            Ok(Some(session)) => session,
            Ok(None) => {
                let _ = ready_tx.send(Err(Setup::NoLoad));
                return Ok(());
            }
            Err(e) => {
                let _ = ready_tx.send(Err(Setup::Failed(e.to_string())));
                return Err(e);
            }
        };
        let _ = ready_tx.send(Ok(session.clone()));
        driver.run(cx, session, input, updates, notes).await;
        Ok(())
    });
    // The driver reports a failed prompt itself; a connection that fails
    // after it has no turn left to end.
    tokio::spawn(async move {
        let _ = run.await;
    });
    let failed = |message: String| CoreError::ExternalAgent {
        agent: agent.clone(),
        message: cox_sanitize::sanitize(&message),
    };
    match ready.await {
        Ok(Ok(session)) => Ok(OpenedAcp {
            session: AcpSession {
                id,
                agent: agent.clone(),
                input: input_tx,
                asks,
                process: Mutex::new(None),
            },
            events,
            agent_session: session.to_string(),
        }),
        Ok(Err(Setup::NoLoad)) => Err(AcpOpenError::NoLoad(agent.clone())),
        Ok(Err(Setup::Failed(e))) => Err(failed(e).into()),
        Err(_) => Err(failed(String::from("closed before it answered")).into()),
    }
}

/// Why `connect`'s setup did not reach the session loop.
enum Setup {
    Failed(String),
    NoLoad,
}

/// The session loop: owns the fold, so every event leaves in the order the
/// agent reported it.
struct Driver {
    fold: UpdateFold,
    model: ModelId,
    events: mpsc::Sender<Event>,
    tail: Option<JoinHandle<String>>,
    seq: u32,
    busy: bool,
    waiting: VecDeque<String>,
    started: Instant,
}

type Stopped = Result<PromptResponse, String>;

impl Driver {
    async fn run(
        mut self,
        cx: ConnectionTo<Agent>,
        session: AcpId,
        mut input: mpsc::UnboundedReceiver<Input>,
        mut updates: mpsc::UnboundedReceiver<SessionUpdate>,
        mut notes: mpsc::UnboundedReceiver<Event>,
    ) {
        let (stop_tx, mut stops) = mpsc::unbounded_channel::<Stopped>();
        loop {
            // Updates first: a prompt's response follows its last update on
            // the wire, so it must not overtake them here; nor may an ask
            // overtake the `tool_call` update it is about.
            let alive = tokio::select! {
                biased;
                Some(update) = updates.recv() => {
                    let events = self.fold.update(update, self.now());
                    self.emit(events).await
                }
                Some(note) = notes.recv() => self.emit(vec![note]).await,
                Some(stopped) = stops.recv() => {
                    let mut alive = self.stopped(&mut updates, stopped).await;
                    if alive && let Some(next) = self.waiting.pop_front() {
                        alive = self.start(&cx, &session, next, &stop_tx).await;
                    }
                    alive
                }
                cmd = input.recv() => match cmd {
                    None => false,
                    Some(Input::Prompt(text)) if self.busy => {
                        self.waiting.push_back(text);
                        true
                    }
                    Some(Input::Prompt(text)) => self.start(&cx, &session, text, &stop_tx).await,
                    Some(Input::Cancel) => {
                        self.waiting.clear();
                        if self.busy {
                            // An agent that is gone fails the prompt instead.
                            let _ = cx.send_notification(CancelNotification::new(session.clone()));
                        }
                        true
                    }
                },
            };
            if !alive {
                break;
            }
        }
    }

    /// `TurnStarted` and the user item, then the prompt, whose response
    /// comes back through `stops`.
    async fn start(
        &mut self,
        cx: &ConnectionTo<Agent>,
        session: &AcpId,
        text: String,
        stops: &mpsc::UnboundedSender<Stopped>,
    ) -> bool {
        let turn = TurnId::new();
        self.fold.start_turn(turn);
        self.seq += 1;
        self.busy = true;
        let item = ItemId::new();
        let kind = ItemKind::UserMessage {
            text: text.clone(),
            attachments: Vec::new(),
        };
        let started = vec![
            Event::TurnStarted {
                turn,
                seq: self.seq,
                job: Job::Main,
                tier: Tier::Code,
                model: self.model.clone(),
            },
            Event::ItemStarted { item, kind },
            Event::ItemDone { item },
        ];
        // Sent here, not in the task: a cancel after it must reach the wire
        // after it, or the agent would cancel nothing and then run it.
        let request = PromptRequest::new(session.clone(), vec![ContentBlock::from(text)]);
        let sent = cx.send_request(request);
        let stops = stops.clone();
        tokio::spawn(async move {
            let result = sent.block_task().await;
            let _ = stops.send(result.map_err(|e| e.to_string()));
        });
        self.emit(started).await
    }

    /// The prompt came back: the updates still queued, then its stop
    /// reason, or the failure with what the agent last wrote to stderr.
    async fn stopped(
        &mut self,
        updates: &mut mpsc::UnboundedReceiver<SessionUpdate>,
        stopped: Stopped,
    ) -> bool {
        let mut out = Vec::new();
        while let Ok(update) = updates.try_recv() {
            out.extend(self.fold.update(update, self.now()));
        }
        self.busy = false;
        match stopped {
            Ok(response) => out.extend(self.fold.stop(response.stop_reason, self.now())),
            Err(e) => {
                let tail = self.tail().await;
                let message = if tail.is_empty() {
                    e
                } else {
                    format!("{e}: {tail}")
                };
                out.extend(self.fold.failed(&message, self.now()));
            }
        }
        self.emit(out).await
    }

    async fn tail(&mut self) -> String {
        let Some(tail) = self.tail.take() else {
            return String::new();
        };
        match tokio::time::timeout(TAIL_WAIT, tail).await {
            Ok(Ok(text)) => text,
            _ => String::new(),
        }
    }

    /// False once the surface stopped listening.
    async fn emit(&self, events: Vec<Event>) -> bool {
        for event in events {
            if self.events.send(event).await.is_err() {
                return false;
            }
        }
        true
    }

    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A `Decision` and who made it, as one ask waits for it.
type Answer = (Decision, DecidedBy);

/// The approver behind a top-level agent session (T52.5, DT§3.3.1): each
/// `Ask` the engine returns for the agent's `session/request_permission`
/// waits here under its call id while its `ApprovalRequired` sits in the
/// inbox. The events go through the driver's `notes`, not straight to the
/// surface, so an ask never overtakes the update it follows.
struct Asks {
    agent: String,
    session: SessionId,
    notes: mpsc::UnboundedSender<Event>,
    /// `None` once the session closed: a new ask is denied at once.
    pending: Mutex<Option<HashMap<CallId, oneshot::Sender<Answer>>>>,
    wait: Duration,
}

impl Asks {
    fn new(agent: String, session: SessionId, notes: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            agent,
            session,
            notes,
            pending: Mutex::new(Some(HashMap::new())),
            wait: ASK_WAIT,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<HashMap<CallId, oneshot::Sender<Answer>>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hands `decision` to the ask waiting under `call`; false if none does.
    fn answer(&self, call: CallId, decision: Decision, by: DecidedBy) -> bool {
        let waiting = self
            .lock()
            .as_mut()
            .and_then(|pending| pending.remove(&call));
        waiting.is_some_and(|tx| tx.send((decision, by)).is_ok())
    }

    /// Denies every ask still waiting with `reason`; `close` also refuses
    /// every later one.
    fn settle(&self, reason: impl Fn() -> String, close: bool) {
        let waiting: Vec<_> = {
            let mut pending = self.lock();
            let waiting = pending
                .as_mut()
                .map(|p| p.drain().map(|(_, tx)| tx).collect())
                .unwrap_or_default();
            if close {
                *pending = None;
            }
            waiting
        };
        for tx in waiting {
            let _ = tx.send((Decision::Deny { reason: reason() }, DecidedBy::Policy));
        }
    }

    fn forget(&self, call: CallId) {
        if let Some(pending) = self.lock().as_mut() {
            pending.remove(&call);
        }
    }

    fn deny(reason: &str) -> Decision {
        Decision::Deny {
            reason: reason.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Approver for Asks {
    async fn approve(&self, call: ToolCall, why: Why) -> Decision {
        let id = call.id;
        let (tx, rx) = oneshot::channel();
        match self.lock().as_mut() {
            Some(pending) => {
                pending.insert(id, tx);
            }
            None => return Self::deny("the session closed"),
        }
        let source = Source {
            session: self.session,
            agent: Some(self.agent.clone()),
            preset: None,
        };
        let asked = Event::ApprovalRequired {
            call,
            why,
            source: Some(source),
        };
        if self.notes.send(asked).is_err() {
            // Nobody would ever see the ask, so nobody could answer it.
            self.forget(id);
            return Self::deny("the session closed");
        }
        let (decision, by) = match tokio::time::timeout(self.wait, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => (Self::deny("the session closed"), DecidedBy::Policy),
            Err(_) => {
                self.forget(id);
                let minutes = self.wait.as_secs() / 60;
                let reason = format!("no answer within {minutes} minutes");
                (Self::deny(&reason), DecidedBy::Policy)
            }
        };
        let _ = self.notes.send(Event::ApprovalDecided {
            call_id: id,
            decision: decision.clone(),
            by,
        });
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::types::Risk;
    use serde_json::Value;

    fn call() -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: Value::Null,
            risk: Risk::Exec,
            subject: "make deploy".into(),
            segments: None,
        }
    }

    fn asks() -> (Arc<Asks>, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Arc::new(Asks::new("fake".into(), SessionId::new(), tx)), rx)
    }

    fn why() -> Why {
        Why::Risk { risk: Risk::Exec }
    }

    #[tokio::test]
    async fn external_closed_session_denies() {
        let (asks, mut notes) = asks();
        let waiting = tokio::spawn({
            let asks = Arc::clone(&asks);
            async move { asks.approve(call(), why()).await }
        });
        let Some(Event::ApprovalRequired { source, .. }) = notes.recv().await else {
            panic!("the ask reaches the stream first");
        };
        assert_eq!(source.and_then(|s| s.agent).as_deref(), Some("fake"));
        asks.settle(|| String::from("the session closed"), true);
        let decision = waiting.await.ok();
        assert!(
            matches!(decision, Some(Decision::Deny { .. })),
            "{decision:?}"
        );
        let Some(Event::ApprovalDecided { by, .. }) = notes.recv().await else {
            panic!("the deny is recorded");
        };
        assert_eq!(by, DecidedBy::Policy);
        let late = asks.approve(call(), why()).await;
        assert!(
            matches!(late, Decision::Deny { .. }),
            "a closed session asks no one"
        );
        assert!(notes.try_recv().is_err(), "and shows nothing");
    }

    #[tokio::test]
    async fn external_ask_answered_by_the_user_is_recorded_as_theirs() {
        let (asks, mut notes) = asks();
        let waiting = tokio::spawn({
            let asks = Arc::clone(&asks);
            async move { asks.approve(call(), why()).await }
        });
        let Some(Event::ApprovalRequired { call, .. }) = notes.recv().await else {
            panic!("the ask reaches the stream first");
        };
        assert!(asks.answer(call.id, Decision::Allow, DecidedBy::User));
        assert!(
            !asks.answer(call.id, Decision::Allow, DecidedBy::User),
            "answered once"
        );
        assert_eq!(waiting.await.ok(), Some(Decision::Allow));
        let Some(Event::ApprovalDecided { by, .. }) = notes.recv().await else {
            panic!("the answer is recorded");
        };
        assert_eq!(by, DecidedBy::User);
    }
}
