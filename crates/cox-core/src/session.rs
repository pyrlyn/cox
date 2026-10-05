// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Session`: Submission in, Event out. The only type `cox-tui` / `cox run`
//! / ACP should talk to; they never call a provider or tool themselves.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use cox_protocol::agent::AgentDef;
use cox_protocol::errors::{CoreError, ProviderError, StoreError, ToolError};
use cox_protocol::ids::{ArchiveId, CallId, ItemId, SessionId, TaskId, TurnId};
use cox_protocol::traits::{
    Advisor, Archive, ArchivePut, Checkpointer, EventTap, ExternalAgent, Hook, HunkReverter,
    Provider, RepoMapper, Store, Tool, Worktrees,
};
use cox_protocol::types::{
    ArchiveRef, Attachment, Content, ContextBreakdown, Decision, Event, HookEvent, HookOutcome,
    ItemKind, Job, Level, Message, Mode, ModelId, PermissionMode, ProviderId, Request, Role,
    SandboxMode, StopReason, Submission, Tier, ToolCall, ToolResult,
};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::budget;
use crate::cache_diag::CacheTracker;
use crate::compact::{self, TurnMark};
use crate::context::{Stable, assemble_with_skills};
use crate::dedup::Dedup;
use crate::hooks;
use crate::permission::{Engine, Outcome};
use crate::rollout::History;
use crate::router::{Overrides, Route, RouteError, Router};
use crate::turn::{consume_provider, results_message, run_signed_tools, run_tools};

/// Loop states from plan.md §1.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No turn in flight.
    Idle,
    /// Building the next provider request.
    Assembling,
    /// A provider stream is open.
    Streaming,
    /// Tools from the last assistant message are running.
    RunningTools,
    /// Waiting on `Submission::Approve`.
    AwaitingApproval,
    /// Compaction is running (wired in T8.1).
    #[allow(dead_code)]
    Compacting,
    /// Emitting `TurnDone` and flushing.
    Finishing,
    /// `Submission::Interrupt` is draining work.
    Interrupted,
}

enum Step {
    Continue,
    Done,
}

pub(crate) struct Inner {
    pub(crate) state: State,
    pub(crate) history: Vec<Message>,
    provider_calls: u32,
    spent_usd: f64,
    budget_warned: bool,
    pub(crate) permission_mode: PermissionMode,
    /// `AllowForSession` grants as `(tool, subject prefix)`.
    pub(crate) grants: Vec<(String, String)>,
    /// Calls parked in `AwaitingApproval`, answered by `Submission::Approve`.
    pending: HashMap<CallId, oneshot::Sender<Decision>>,
    /// `ask_user` calls waiting in `QuestionAsked`, resumed by
    /// `Submission::Answer` (DT G4).
    questions: HashMap<CallId, oneshot::Sender<Option<String>>>,
    /// Provider rounds so far in this session; the dedup window counts these.
    round: u32,
    dedup: Dedup,
    /// Deferred tools found through `tool_search`, in discovery order.
    discovered: Vec<String>,
    /// Where each turn starts in `history` (T8.1 compaction cuts on these).
    pub(crate) turn_marks: Vec<TurnMark>,
    /// `call_id` → archived payload for microcompaction (T8.2): the request
    /// replaces old results with `Pointer`s, the stored history keeps them.
    pub(crate) archives: HashMap<CallId, ArchiveRef>,
    /// This round's tool images (T40.5), already archived, waiting to join
    /// the results message after every `ToolResult`.
    tool_images: HashMap<CallId, Content>,
    /// Last request's prefix hashes + whether it hit the cache (T8.3).
    pub(crate) cache: CacheTracker,
    /// Last call's cache share, for the status line (T8.3 step 1).
    pub(crate) cache_ratio: f64,
    /// Session routing overrides from `/model` (T9.1).
    pub(crate) overrides: Overrides,
    /// The mode in force (P42): `core.mode` at build, then `/mode`.
    mode: Mode,
    /// The main-tier override a mode replaced, while the mode's own tier
    /// stands; `/mode editor` restores it, and `/model` clears it so the
    /// user's own pick survives leaving the mode.
    mode_tier: Option<Option<Tier>>,
    /// The tier the running user turn was moved to: by `route` advice
    /// (T33.20) or by `confirm_think` onto think (T37.24.11); `None` outside
    /// a turn and whenever the static pick stands.
    pub(crate) routed: Option<Tier>,
    /// Running background tasks: label, tier and kind by id (T9.2, T27.1).
    pub(crate) tasks: HashMap<TaskId, (String, Tier, crate::tasks::TaskKind)>,
    /// Subagents a follow-up can reach, running or finished (T34.5, SM§2).
    pub(crate) children: HashMap<TaskId, crate::tasks::Child>,
    /// Received-message counter per task (T34.6, SM§5):
    /// `MAX_MESSAGES_PER_TASK` denies the 17th delivery instead of letting
    /// a flood spin the addressee.
    pub(crate) message_counts: HashMap<TaskId, u32>,
    /// Running calls `Submission::Background` may detach (T27.1).
    pub(crate) detach: HashMap<CallId, CancellationToken>,
    /// Facts `extract_memory` saved, awaiting surface drain (T10.2).
    pub(crate) extracted: Vec<crate::memory_extract::Fact>,
    /// Monotonic turn counter for the FTS index (T10.3) and the
    /// `checkpoints` rows (T26.1).
    pub(crate) turn_seq: u32,
    /// The turn a `/redo` wrote its pre-images under (T26.4), so a second
    /// `/redo` does not undo the first.
    pub(crate) redone: Option<u32>,
    /// The user titled the session (A113): no generated title follows.
    pub(crate) renamed: bool,
    /// Context size of the last main call, for the §1.10 auto trigger.
    pub(crate) last_context_tokens: u32,
    /// The last main call's usage: `ContextBreakdown.cached` (A98).
    last_usage: Option<cox_protocol::types::Usage>,
    /// The model catalog `ContextBreakdown.window` is read from (A98),
    /// loaded from this session's config on the first request.
    catalog: Option<cox_models::Catalog>,
    /// Whether this turn already compacted after a context-length error.
    retried_after_too_long: bool,
    /// `SessionStart` source awaiting its one dispatch (T22.3): armed by
    /// `build`, flushed on the first `submit` — by then the surface has
    /// installed the hook runner, and only once per session.
    startup: Option<&'static str>,
    /// `additional_context` from the `SessionStart` hook (T22.3), appended
    /// to `system[3]`, the one block after the last cache breakpoint.
    startup_context: String,
    /// The repo map last in `system[2]` (P43): set once at session start or
    /// on resume, then only by `/repomap refresh` or compaction.
    pub(crate) repomap: Option<String>,
    /// Where that map's text is archived; on resume, set from the rollout
    /// before the text is read back on the first submit.
    pub(crate) repomap_archive: Option<ArchiveId>,
}

/// The mode and effort as they stand after a change, for every surface to
/// read instead of echoing its own request (DT G5).
fn state_changed(inner: &Inner) -> Event {
    Event::StateChanged {
        mode: inner.permission_mode,
        effort: inner.overrides.effort,
    }
}

/// The mode as it stands after a change, with the permission mode it
/// left in force (P42).
fn mode_changed(inner: &Inner) -> Event {
    Event::ModeChanged {
        mode: inner.mode,
        permission_mode: inner.permission_mode,
    }
}

/// One conversation: a provider, tools, a store, and an event stream.
#[derive(Clone)]
pub struct Session {
    pub(crate) id: SessionId,
    pub(crate) config: cox_protocol::Config,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) tools: Vec<Arc<dyn Tool>>,
    pub(crate) store: Arc<dyn Store>,
    pub(crate) archive: Arc<dyn Archive>,
    pub(crate) engine: Arc<Engine>,
    pub(crate) cwd: PathBuf,
    /// What this session's turns are for; `Main` unless it is a subagent.
    pub(crate) job: Job,
    /// The tier every provider call in this session is routed to.
    pub(crate) tier: Tier,
    /// The subagent name this session runs as (`explore-2`, T27.2), set by
    /// `spawn_child`; `None` for the session the user is talking to. Read
    /// by `turn::run_one` to label every `ToolCx` this session hands to a
    /// tool call (T34.3), the same way `relay_approval` labels a relayed
    /// approval.
    pub(crate) agent: Option<String>,
    /// The dispatched preset/def name (`explore`), alongside `agent`.
    pub(crate) preset: Option<String>,
    /// Root of every turn/provider/tool span emitted by this session.
    pub(crate) telemetry_span: tracing::Span,
    /// The current turn's token; each turn replaces it (`renew_cancel`)
    /// with a fresh child of `ended`.
    pub(crate) cancel: Arc<StdMutex<CancellationToken>>,
    /// Session-scoped (T38.2): the parent of every turn token this session
    /// hands out, so `end` reaches a shell detached in an older turn, whose
    /// `ToolCx::cancel` `interrupt` no longer reaches.
    pub(crate) ended: CancellationToken,
    /// The hook runner, installed once by the surface and shared with
    /// children so a subagent's calls run the same hooks.
    hook: Arc<OnceLock<Arc<dyn Hook>>>,
    /// Where pre-images come from (T26.1); installed by the surface like
    /// the hook, shared with children. Absent in tests and in `cox mcp`.
    checkpointer: Arc<OnceLock<Arc<dyn Checkpointer>>>,
    /// What puts one of Review's hunks back (T51.19); installed by the
    /// surface like the checkpointer, since it is cox-render's.
    hunks: Arc<OnceLock<Arc<dyn HunkReverter>>>,
    /// Roots mutation may target. Empty until a worktree-isolated surface
    /// narrows it; ordinary sessions write anywhere they can read.
    writable_roots: Arc<OnceLock<Vec<PathBuf>>>,
    /// Where `agent(isolation: "worktree")` gets its worktree (T27.3);
    /// installed by the surface, shared with children. Absent in tests.
    worktrees: Arc<OnceLock<Arc<dyn Worktrees>>>,
    /// Builds the repo map (P43); installed by the surface like `worktrees`.
    /// Not copied to children: only the session the user talks to has a map.
    pub(crate) repo_mapper: Arc<OnceLock<Arc<dyn RepoMapper>>>,
    /// Custom subagent definitions the surface discovered on disk (T34.1:
    /// `cox_ext::agents::discover`, which this crate never calls itself);
    /// installed like `worktrees`, empty until then. Not copied to
    /// children — only `new`/`resume` push the `agent` tool at all.
    agent_defs: Arc<OnceLock<Vec<AgentDef>>>,
    /// The `AGENTS.md`/`CLAUDE.md` block (T50.1) the surface read once
    /// with `cox_ext::instructions::load` — this crate reads no files.
    /// Shared with children: a subagent follows the same project rules.
    instructions: Arc<OnceLock<String>>,
    /// The `system[2]` skills index (T22.2, T50.1), installed with
    /// `instructions`. Not copied to children: a child's tool list may
    /// have no `skill` tool for the index to point at.
    skills_index: Arc<OnceLock<String>>,
    /// The task id `send_message`'s `Relay` impl stamps a child's own
    /// message with (T34.6, SM§4), set once by `subagent::spawn` right
    /// after the child session exists; unset for the session the user is
    /// talking to.
    self_task: Arc<OnceLock<TaskId>>,
    /// Granted `[[external_agents]]` drivers the surface built (T35.5,
    /// EA§3), offered by the `agent` tool like `agent_defs`; not copied to
    /// children, for the same reason.
    external_agents: Arc<OnceLock<Vec<Arc<dyn ExternalAgent>>>>,
    /// The plugin host's event rings (T33.10, PL§5), installed once by the
    /// surface. Not copied to children: a child writes its own rollout, so
    /// its sequence numbers would interleave with the parent's.
    event_tap: Arc<OnceLock<Arc<dyn EventTap>>>,
    /// Decision-point sources (T33.20, PL§4), installed once by the surface
    /// like the hook. Not copied to children: `route` is a main-turn point.
    advisors: Arc<OnceLock<Vec<Arc<dyn Advisor>>>>,
    /// The driver this child's turns run on instead of the model, set once
    /// by `subagent::spawn` for an external-agent preset; unset otherwise.
    external: Arc<OnceLock<Arc<dyn ExternalAgent>>>,
    /// Exact registry name (`explore-2`) → task id (T34.6, SM§4), one map
    /// per subagent tree: the parent owns it, `spawn_child` hands every
    /// child the same `Arc` (T34.9) so a child's own `Relay::send_message`
    /// resolves a sibling's name itself instead of only "parent" or a
    /// literal `TaskId` it has no way to learn in a scripted scenario. A
    /// child never writes it — only the parent's `name_task` does — but
    /// nothing below the type system enforces that; see `resolve_name_or_id`.
    pub(crate) task_names: Arc<Mutex<HashMap<String, TaskId>>>,
    /// The one "checkpoints off" warning per session has been emitted.
    pub(crate) checkpoint_warned: Arc<AtomicBool>,
    /// Bumped by `complete_task` whenever `inner.tasks` empties out
    /// (T34.9): `wait_idle` below awaits it instead of a caller-guessed
    /// delay. Fresh per session — only a session built by `new`/`resume`
    /// ever calls `register_task` on itself (a child never gets the
    /// `agent` tool, so it never spawns a background task under its own
    /// id), so a child's own unused copy is harmless.
    pub(crate) tasks_idle: Arc<Notify>,
    /// T34.2's `core.max_concurrent_subagents` cap: how many `agent` slots
    /// this session has reserved right now. A plain atomic, not the
    /// `inner.tasks` map: a burst of parallel `agent` calls (the core's own
    /// `turn.rs` dispatches `Concurrency::Parallel` tools concurrently) must
    /// check-and-reserve in one indivisible step, which an `await`-ing
    /// `Mutex` round trip cannot give without a `Drop` guard that itself
    /// needs to run async cleanup; a compare-exchange loop needs neither.
    pub(crate) agent_slots: Arc<AtomicU32>,
    tx: mpsc::Sender<Event>,
    rx: Arc<StdMutex<Option<mpsc::Receiver<Event>>>>,
    pub(crate) inner: Arc<Mutex<Inner>>,
}

impl Session {
    /// Constructs a session and emits `SessionStarted`. The `agent` tool is
    /// added here rather than by the caller because it needs a handle to
    /// this very session; children (`spawn_child`) do not get one, so a
    /// subagent cannot spawn subagents.
    pub fn new(
        config: cox_protocol::Config,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        cwd: PathBuf,
    ) -> Result<Self, CoreError> {
        Self::new_with_id(
            SessionId::new(),
            config,
            provider,
            tools,
            store,
            archive,
            cwd,
        )
    }

    /// [`Session::new`] under an id the caller picked: a surface whose
    /// tools come from something that must see the id first (a plugin's
    /// `cox_init`, T33.12) chooses it before the session exists, so the
    /// tool list is complete at construction and the prefix never changes.
    pub fn new_with_id(
        id: SessionId,
        config: cox_protocol::Config,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        cwd: PathBuf,
    ) -> Result<Self, CoreError> {
        let mut session = Self::build(
            config,
            provider,
            tools,
            store,
            archive,
            cwd,
            None,
            id,
            None,
            Job::Main,
            Tier::Code,
            None,
            None,
        )?;
        let parent = session.clone();
        session
            .tools
            .push(Arc::new(crate::subagent::AgentTool::new(parent)));
        Ok(session)
    }

    /// Restores a session from a reconstructed [`History`] (T17.1). Reuses
    /// `id`, skips store creation and the persisted `SessionStarted`, and
    /// wires the `agent` tool like [`new`].
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        config: cox_protocol::Config,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        cwd: PathBuf,
        id: SessionId,
        history: History,
    ) -> Result<Self, CoreError> {
        let mut session = Self::build(
            config,
            provider,
            tools,
            store,
            archive,
            cwd,
            Some((id, history)),
            id,
            None,
            Job::Main,
            Tier::Code,
            None,
            None,
        )?;
        let parent = session.clone();
        session
            .tools
            .push(Arc::new(crate::subagent::AgentTool::new(parent)));
        Ok(session)
    }

    /// A child session sharing this one's provider, store and archive
    /// (plan.md T3.9): its own rollout and budget, `parent_id` set. `cwd`
    /// is the parent's unless the child runs in a worktree (T27.3).
    /// `resume` restores a finished child (T34.5, SM§2) with the same
    /// job, tier and parent — the child-side twin of [`Session::resume`],
    /// which stays top-level only. `agent` and `preset` (T34.3) are the
    /// same `name`/preset name `relay_approval` already labels a relayed
    /// approval with, so every `ToolCx` this child hands its tools
    /// (`ask_user` included) carries the same label.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_child(
        &self,
        config: cox_protocol::Config,
        tools: Vec<Arc<dyn Tool>>,
        job: Job,
        tier: Tier,
        cwd: Option<PathBuf>,
        resume: Option<(SessionId, History)>,
        agent: String,
        preset: String,
    ) -> Result<Self, CoreError> {
        let mut child = Self::build(
            config,
            self.provider.clone(),
            tools,
            self.store.clone(),
            self.archive.clone(),
            cwd.unwrap_or_else(|| self.cwd.clone()),
            resume,
            SessionId::new(),
            Some(self.id),
            job,
            tier,
            Some(agent),
            Some(preset),
        )?;
        child.hook = self.hook.clone();
        child.checkpointer = self.checkpointer.clone();
        child.hunks = self.hunks.clone();
        child.worktrees = self.worktrees.clone();
        child.instructions = self.instructions.clone();
        child.checkpoint_warned = self.checkpoint_warned.clone();
        // T34.9: share this session's name→TaskId registry so the child can
        // resolve a sibling by name itself (`resolve_name_or_id`).
        child.task_names = self.task_names.clone();
        // T38.2: ending this session also ends the child's detached shells.
        child.ended = self.ended.child_token();
        child.renew_cancel();
        Ok(child)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: cox_protocol::Config,
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        cwd: PathBuf,
        resume: Option<(SessionId, History)>,
        // The id of a session that is not resumed.
        fresh: SessionId,
        parent_id: Option<SessionId>,
        job: Job,
        tier: Tier,
        agent: Option<String>,
        preset: Option<String>,
    ) -> Result<Self, CoreError> {
        let is_resume = resume.is_some();
        // Subagents announce themselves with `SubagentStart`, not `SessionStart`.
        let is_child = parent_id.is_some();
        let (
            id,
            history_messages,
            permission_mode,
            grants,
            turn_marks,
            truncated_notice,
            turns,
            repomap_archive,
        ) = match resume {
            Some((id, history)) => {
                let truncated_notice = history.truncated_notice();
                let turn_marks = history
                    .turn_marks
                    .iter()
                    .map(|mark| TurnMark {
                        item: mark.item,
                        start: mark.message_index,
                        seq: mark.seq,
                    })
                    .collect();
                (
                    id,
                    history.messages,
                    // T50.4: a rollout with no mode record (written
                    // before T50.2/T50.4) resumes in the configured mode.
                    history.permission_mode.unwrap_or(config.permissions.mode),
                    history.grants,
                    turn_marks,
                    truncated_notice,
                    history.turns,
                    history.repomap,
                )
            }
            None => (
                fresh,
                Vec::new(),
                config.permissions.mode,
                Vec::new(),
                Vec::new(),
                None,
                0,
                None,
            ),
        };
        // P42: a top-level session opens in `core.mode`, which only narrows
        // the mode above. A child runs under its parent's live mode (T45.1),
        // so config never re-applies one to it.
        let mode = if is_child {
            Mode::Editor
        } else {
            config.core.mode
        };
        let mode_preset = crate::mode::preset(mode);
        let permission_mode = crate::mode::apply(mode_preset, permission_mode);
        let (tx, rx) = mpsc::channel(256);
        let home = std::env::home_dir();
        let engine = Engine::compile(&config.permissions, home.as_deref(), &cwd)?;
        let dedup = Dedup::new(config.context.dedup_window_turns);
        let parent = parent_id
            .map(|parent| parent.to_string())
            .unwrap_or_default();
        let telemetry_span = tracing::info_span!(
            "invoke_agent cox",
            gen_ai.operation.name = "invoke_agent",
            gen_ai.agent.name = "cox",
            gen_ai.conversation.id = %id,
            cox.session.id = %id,
            cox.session.parent_id = %parent,
            cox.job = ?job,
            cox.tier = ?tier,
            cox.cwd = %cwd.display(),
        );
        let ended = CancellationToken::new();
        let session = Self {
            id,
            config,
            provider,
            tools,
            store,
            archive,
            engine: Arc::new(engine),
            cwd: cwd.clone(),
            job,
            tier,
            agent,
            preset,
            telemetry_span,
            cancel: Arc::new(StdMutex::new(ended.child_token())),
            ended,
            hook: Arc::new(OnceLock::new()),
            checkpointer: Arc::new(OnceLock::new()),
            hunks: Arc::new(OnceLock::new()),
            writable_roots: Arc::new(OnceLock::new()),
            worktrees: Arc::new(OnceLock::new()),
            repo_mapper: Arc::new(OnceLock::new()),
            agent_defs: Arc::new(OnceLock::new()),
            instructions: Arc::new(OnceLock::new()),
            skills_index: Arc::new(OnceLock::new()),
            self_task: Arc::new(OnceLock::new()),
            external_agents: Arc::new(OnceLock::new()),
            event_tap: Arc::new(OnceLock::new()),
            advisors: Arc::new(OnceLock::new()),
            external: Arc::new(OnceLock::new()),
            task_names: Arc::new(Mutex::new(HashMap::new())),
            checkpoint_warned: Arc::new(AtomicBool::new(false)),
            tasks_idle: Arc::new(Notify::new()),
            agent_slots: Arc::new(AtomicU32::new(0)),
            tx,
            rx: Arc::new(StdMutex::new(Some(rx))),
            inner: Arc::new(Mutex::new(Inner {
                state: State::Idle,
                history: history_messages,
                provider_calls: 0,
                spent_usd: 0.0,
                budget_warned: false,
                permission_mode,
                grants,
                pending: HashMap::new(),
                questions: HashMap::new(),
                round: 0,
                dedup,
                discovered: Vec::new(),
                turn_marks,
                archives: HashMap::new(),
                tool_images: HashMap::new(),
                turn_seq: turns,
                redone: None,
                renamed: false,
                cache: CacheTracker::new(),
                cache_ratio: 0.0,
                overrides: Overrides {
                    main_tier: mode_preset.main_tier,
                    ..Overrides::default()
                },
                mode,
                mode_tier: mode_preset.main_tier.map(|_| None),
                routed: None,
                tasks: HashMap::new(),
                children: HashMap::new(),
                message_counts: HashMap::new(),
                detach: HashMap::new(),
                extracted: Vec::new(),
                last_context_tokens: 0,
                last_usage: None,
                catalog: None,
                retried_after_too_long: false,
                startup: (!is_child).then_some(if is_resume { "resume" } else { "startup" }),
                startup_context: String::new(),
                repomap: None,
                repomap_archive,
            })),
        };
        let started = Event::SessionStarted {
            session: id,
            config_digest: String::new(),
            cwd: session.cwd.clone(),
        };
        if !is_resume {
            session
                .store
                .session_create(&cox_protocol::SessionRow {
                    id,
                    created_at: String::new(),
                    cwd: session.cwd.clone(),
                    project_slug: String::new(),
                    title: None,
                    parent_id,
                    rollout_path: PathBuf::new(),
                })
                .map_err(|error| CoreError::Store { error })?;
            session.store.rollout_append(&id, &started).ok();
        }
        // T50.4: a top-level session records the mode it opens in, fresh or
        // resumed, so resume never falls back to a wider mode and a flag
        // that overrides the record on resume is itself recorded. Rollout
        // only, like the persisted `SessionStarted`: every surface already
        // has the opening mode from the config it built the session with.
        // A child's mode is its parent's (T45.1, T50.2 `restart`). Effort is
        // `None` here: a session opens with no override.
        if !is_child {
            session
                .store
                .rollout_append(
                    &id,
                    &Event::StateChanged {
                        mode: permission_mode,
                        effort: None,
                    },
                )
                .map_err(|error| CoreError::Store { error })?;
        }
        let _ = session.tx.try_send(started);
        // P42: a surface learns a non-default opening mode from the core,
        // like any later `/mode`, instead of re-reading config.
        if mode != Mode::Editor {
            let _ = session.tx.try_send(Event::ModeChanged {
                mode,
                permission_mode,
            });
        }
        if let Some(notice) = truncated_notice {
            let _ = session.tx.try_send(notice);
        }
        tracing::info!(
            parent: &session.telemetry_span,
            event.name = "cox.session.started",
            "agent session started"
        );
        // T4.3: the sandbox being off is loud on every surface — one line
        // after `SessionStarted` in stream-json, a pinned banner in the TUI.
        if session.config.sandbox.mode == SandboxMode::DangerFullAccess {
            let _ = session.tx.try_send(Event::Notice {
                level: Level::Security,
                text: crate::permission::policy::DANGER_FULL_ACCESS.into(),
            });
        }
        Ok(session)
    }

    /// The id a surface needs to name this session to the outside (the
    /// presence record, `cox --resume`).
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Takes the event receiver once.
    pub fn events(&self) -> Option<mpsc::Receiver<Event>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// The transcript the next `assemble` will send (loop tests).
    pub async fn history(&self) -> Vec<Message> {
        self.inner.lock().await.history.clone()
    }

    /// Emits a `Notice` found outside the loop before the first turn — a
    /// surface's session-start check (T30.16: an LM Studio model not
    /// trained for tool use). Goes through `emit`, so it is recorded in the
    /// rollout like every other notice.
    pub async fn notice(&self, level: Level, text: String) -> Result<(), CoreError> {
        self.emit(Event::Notice { level, text }).await
    }

    pub(crate) fn clone_handle(&self) -> Self {
        self.clone()
    }

    pub(crate) async fn emit(&self, ev: Event) -> Result<(), CoreError> {
        // T22.3: `Notification` is observe-only and fires before the event
        // it announces is recorded, so a broken hook's warning still lands
        // before `TurnDone` (§1.15 rule 7).
        if let Some(extra) = hooks::notification_payload(&ev) {
            let _ = hooks::fire_configured(self, HookEvent::Notification, extra).await;
        }
        // T28.4: the rollout is what leaves the session, so the copy written
        // here is scrubbed; `ev` — the in-memory history and every surface —
        // keeps the original (redacting model input is out of scope).
        let scrubbed = crate::redact::scrub_event(&ev);
        let redacted = scrubbed.as_ref() != &ev && matches!(&ev, Event::ToolCallDone { .. });
        let seq = self
            .store
            .rollout_append(&self.id, scrubbed.as_ref())
            .map_err(|error| CoreError::Store { error })?;
        self.tap(seq, scrubbed.as_ref());
        let _ = self.tx.send(ev).await;
        // T28.4: a tool result the scrub changed raises the notice right
        // behind it — PostToolUse's per-call signal, emitted where the
        // change is detected so streamed output cannot dodge it either.
        if redacted {
            let notice = Event::Notice {
                level: Level::Security,
                text: "tool output contained a secret-shaped string; redacted in the rollout"
                    .into(),
            };
            let seq = self
                .store
                .rollout_append(&self.id, &notice)
                .map_err(|error| CoreError::Store { error })?;
            self.tap(seq, &notice);
            let _ = self.tx.send(notice).await;
        }
        Ok(())
    }

    /// Cancels the provider stream and running tools.
    pub fn interrupt(&self) {
        self.cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
    }

    /// Ends the session's work for good (T38.2): cancels every turn token
    /// it ever handed out, so a `bash` detached in any turn is killed, not
    /// orphaned. For a surface leaving the session (quit, `/clear`, fork,
    /// handoff, headless exit); `interrupt` stays turn-scoped.
    ///
    /// The session the user talks to also calls `Tool::shutdown` on its
    /// tools (T41.5), once however often `end` is called; a child
    /// (`spawn_child`, the only constructor that sets `agent`) shares
    /// tools with its parent and leaves them running.
    pub fn end(&self) {
        let first = !self.ended.is_cancelled();
        self.ended.cancel();
        if first && self.agent.is_none() {
            for tool in &self.tools {
                tool.shutdown();
            }
        }
    }

    /// A fresh turn token under `ended`: a previous `Esc` may have left
    /// the old one cancelled.
    pub(crate) fn renew_cancel(&self) {
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = self.ended.child_token();
    }

    pub(crate) fn cancel_token(&self) -> CancellationToken {
        self.cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Installs the hook runner (T7.4); a second call is ignored so the
    /// runner stays byte-stable for the session and its children.
    pub fn set_hook(&self, hook: Arc<dyn Hook>) {
        let _ = self.hook.set(hook);
    }

    pub(crate) fn hook(&self) -> Option<Arc<dyn Hook>> {
        self.hook.get().cloned()
    }

    /// Installs the decision-point sources (T33.20); a second call is
    /// ignored, like `set_hook`.
    pub fn set_advisors(&self, advisors: Vec<Arc<dyn Advisor>>) {
        let _ = self.advisors.set(advisors);
    }

    /// The advisor `[plugins.decide]` names by plugin id, if it is live.
    pub(crate) fn advisor(&self, id: &str) -> Option<Arc<dyn Advisor>> {
        self.advisors.get()?.iter().find(|a| a.id() == id).cloned()
    }

    /// Installs the pre-image source for `/rewind` (T26.1); a second call
    /// is ignored so a session and its children share one.
    pub fn set_checkpointer(&self, checkpointer: Arc<dyn Checkpointer>) {
        let _ = self.checkpointer.set(checkpointer);
    }

    pub(crate) fn checkpointer(&self) -> Option<Arc<dyn Checkpointer>> {
        self.checkpointer.get().cloned()
    }

    /// Installs what `Submission::RevertHunk` computes the new bytes with
    /// (T51.19); a second call is ignored, as for the checkpointer.
    pub fn set_hunk_reverter(&self, hunks: Arc<dyn HunkReverter>) {
        let _ = self.hunks.set(hunks);
    }

    pub(crate) fn hunk_reverter(&self) -> Option<Arc<dyn HunkReverter>> {
        self.hunks.get().cloned()
    }

    /// Narrows mutations without hiding read-only workspace roots.
    pub fn set_writable_roots(&self, roots: Vec<PathBuf>) {
        let _ = self.writable_roots.set(roots);
    }

    pub(crate) fn writable_roots(&self) -> &[PathBuf] {
        self.writable_roots
            .get()
            .map(Vec::as_slice)
            .unwrap_or(&self.config.core.workspace_roots)
    }

    /// Installs the worktree provider (T27.3); a second call is ignored
    /// like `set_checkpointer`.
    pub fn set_worktrees(&self, worktrees: Arc<dyn Worktrees>) {
        let _ = self.worktrees.set(worktrees);
    }

    pub(crate) fn worktrees(&self) -> Option<Arc<dyn Worktrees>> {
        self.worktrees.get().cloned()
    }

    /// Installs the repo-map builder (P43); a second call is ignored like
    /// `set_worktrees`. The map itself is built on the first submit.
    pub fn set_repo_mapper(&self, mapper: Arc<dyn RepoMapper>) {
        let _ = self.repo_mapper.set(mapper);
    }

    /// Installs the custom subagent definitions the surface discovered
    /// (T34.1); a second call is ignored like `set_worktrees`. Discovery
    /// happens once at session build, so the `agent` tool's schema stays
    /// byte-stable for the rest of the session (D6e).
    pub fn set_agent_defs(&self, defs: Vec<AgentDef>) {
        let _ = self.agent_defs.set(defs);
    }

    pub(crate) fn agent_defs(&self) -> &[AgentDef] {
        self.agent_defs.get().map(Vec::as_slice).unwrap_or(&[])
    }

    /// Installs the instruction-file block and the skills index `system[2]`
    /// carries (T50.1), read once by the surface at session build. A second
    /// call is ignored, so the cached prefix cannot change mid-session (§1.9).
    pub fn set_instructions(&self, block: String, skills_index: String) {
        let _ = self.instructions.set(block);
        let _ = self.skills_index.set(skills_index);
    }

    /// Installs the granted external-agent drivers (T35.5); the surface
    /// leaves out an entry whose CLI or key is missing (EA§7). A second
    /// call is ignored like `set_agent_defs`, keeping `agent`'s schema
    /// byte-stable (D6e).
    pub fn set_external_agents(&self, agents: Vec<Arc<dyn ExternalAgent>>) {
        let _ = self.external_agents.set(agents);
    }

    pub(crate) fn external_agents(&self) -> &[Arc<dyn ExternalAgent>] {
        self.external_agents.get().map(Vec::as_slice).unwrap_or(&[])
    }

    /// Installs the plugin host's event tap (T33.10); a second call is
    /// ignored like `set_external_agents`, so plugins see one ordered
    /// stream for the whole session.
    pub fn set_event_tap(&self, tap: Arc<dyn EventTap>) {
        let _ = self.event_tap.set(tap);
    }

    /// Hands a recorded event to the tap, which queues it and returns
    /// (PL§5 "never blocking").
    fn tap(&self, seq: u64, ev: &Event) {
        if let Some(tap) = self.event_tap.get() {
            tap.offer(seq, ev);
        }
    }

    /// `subagent::spawn` calls this once for an external-agent child.
    pub(crate) fn set_external(&self, agent: Arc<dyn ExternalAgent>) {
        let _ = self.external.set(agent);
    }

    /// `subagent::spawn` calls this once, right after the child exists
    /// (T34.6, SM§4); a second call is ignored like `set_worktrees`.
    pub(crate) fn set_self_task(&self, task: TaskId) {
        let _ = self.self_task.set(task);
    }

    /// `Some` only for a subagent's own session.
    pub(crate) fn self_task(&self) -> Option<TaskId> {
        self.self_task.get().copied()
    }

    /// Feeds one submission into the state machine.
    pub async fn submit(&self, sub: Submission) -> Result<(), CoreError> {
        // T22.3: `SessionStart` runs once per session, as late as the first
        // submission — `build` cannot await and the surface installs the
        // hook runner (`set_hook`) only after `Session::new` returns. Its
        // `additionalContext` rides in `system[3]` from here on.
        // A `let` first: an `if let` scrutinee's guard would live through the
        // block and deadlock `start_repomap`'s own lock.
        let startup = self.inner.lock().await.startup.take();
        if let Some(source) = startup {
            let outcome = hooks::fire_configured(
                self,
                HookEvent::SessionStart,
                serde_json::json!({ "source": source }),
            )
            .await;
            if let HookOutcome::Modify { input } = outcome {
                let (_, context) = hooks::prompt_rewrite(String::new(), input);
                self.inner.lock().await.startup_context = context.unwrap_or_default();
            }
            // P43: the same once-per-session slot, before the first request.
            self.start_repomap().await?;
        }
        match sub {
            Submission::UserTurn {
                text,
                confirm_think,
                attachments,
            } => self.run_turn(text, attachments, confirm_think).await,
            Submission::Interrupt => {
                self.interrupt();
                Ok(())
            }
            Submission::Approve { call_id, decision } => {
                let waiter = self.inner.lock().await.pending.remove(&call_id);
                match waiter {
                    Some(tx) => {
                        let _ = tx.send(decision);
                        Ok(())
                    }
                    None => {
                        self.emit(Event::Notice {
                            level: Level::Warn,
                            text: format!("no approval pending for call {call_id}"),
                        })
                        .await
                    }
                }
            }
            Submission::Answer { call_id, text } => {
                let waiter = self.inner.lock().await.questions.remove(&call_id);
                match waiter {
                    Some(tx) => {
                        let _ = tx.send(text);
                        Ok(())
                    }
                    None => {
                        self.emit(Event::Notice {
                            level: Level::Warn,
                            text: format!("no question pending for call {call_id}"),
                        })
                        .await
                    }
                }
            }
            Submission::SetEffort { effort } => {
                let changed = {
                    let mut inner = self.inner.lock().await;
                    inner.overrides.effort = effort;
                    state_changed(&inner)
                };
                self.emit(changed).await
            }
            Submission::SetPermissionMode { mode } => {
                let changed = {
                    let mut inner = self.inner.lock().await;
                    inner.permission_mode = mode;
                    state_changed(&inner)
                };
                self.emit(changed).await
            }
            Submission::RevokeGrant { tool, subject } => self.revoke(tool, subject).await,
            Submission::Compact { focus } => self
                .compact(compact::Trigger::Manual, focus)
                .await
                .map(|_| ()),
            Submission::SwitchModel { tier, model } => self.switch_model(tier, model).await,
            Submission::Rewind {
                to_turn,
                code,
                conversation,
            } => self.rewind(to_turn, code, conversation).await,
            Submission::Redo => self.redo().await,
            Submission::Rename { title } => self.rename(&title).await,
            Submission::RevertFile { path, to_turn } => self.revert_file(&path, to_turn).await,
            Submission::RevertHunk {
                path,
                to_turn,
                hunk,
                now_digest,
            } => self.revert_hunk(&path, to_turn, hunk, &now_digest).await,
            Submission::Background { call_id } => self.background(call_id).await,
            Submission::UserShell { command, share } => self.user_shell(command, share).await,
            Submission::UserAgent { name, task } => self.user_agent(name, task).await,
            Submission::Command { command } if command.name == "compact" => {
                let focus = (!command.args.is_empty()).then(|| command.args.join(" "));
                self.compact(compact::Trigger::Manual, focus)
                    .await
                    .map(|_| ())
            }
            // P43: `/repomap` shows the map; `/repomap refresh` is one of
            // the two ways it changes mid-session.
            Submission::Command { command } if command.name == "repomap" => {
                self.repomap_command(&command.args).await
            }
            // T25.6: `/init [--force]` scaffolds AGENTS.md; the write asks
            // first, like any other model-initiated write.
            Submission::Command { command } if command.name == "mode" => {
                self.switch_mode(command.args.first().map(String::as_str))
                    .await
            }
            Submission::Command { command } if command.name == "init" => {
                let force = command.args.iter().any(|a| a == "--force" || a == "force");
                self.run_init(force).await
            }
            Submission::Shutdown => {
                // T10.2: optional cheap extraction first; a failure warns but
                // never fails the shutdown. `SessionEnd` fires after, either way.
                if self.config.memory.extract
                    && let Err(error) = self.extract_memory().await
                {
                    self.emit(Event::Notice {
                        level: Level::Warn,
                        text: format!("memory extraction failed: {error}"),
                    })
                    .await?;
                }
                let _ = hooks::fire(self, HookEvent::SessionEnd, serde_json::json!({})).await;
                Ok(())
            }
            // T34.5: `hop` is the parent's own count (SM§5) — a surface's
            // message starts at 0, a relayed sibling message arrives with
            // the hop `subagent::relay` computed, never the child's.
            Submission::TaskMessage {
                task,
                from,
                hop,
                text,
            } => self.deliver(task, from, hop, text).await,
            _ => Ok(()),
        }
    }

    /// The engine's verdict for `call` under the session's current mode
    /// and grants (plan.md §1.8).
    pub(crate) async fn decide(&self, call: &ToolCall) -> Outcome {
        let inner = self.inner.lock().await;
        self.engine.decide(
            call,
            inner.permission_mode,
            self.config.permissions.approval,
            self.config.sandbox.mode,
            &inner.grants,
        )
    }

    /// The route for `job` under the session's overrides (T9.1). Pure
    /// except for the lock that reads the overrides.
    pub(crate) async fn route_for(
        &self,
        job: Job,
        confirm_think: bool,
    ) -> Result<Route, RouteError> {
        let (mut overrides, routed) = {
            let inner = self.inner.lock().await;
            (inner.overrides.clone(), inner.routed)
        };
        // T33.20: inside a user turn, the tier `route` advice chose stands in
        // for the static main tier on every call of that turn (it is never
        // above it). `step` strips earlier turns' thinking from such a turn's
        // own `Request` (T33.40.8); history is never rewritten and no
        // `ModelSwitched` fires.
        if matches!(job, Job::Main) && routed.is_some() {
            overrides.main_tier = routed;
        }
        Router::pick(&self.config, job, self.tier, &overrides, confirm_think)
    }

    /// `/model <tier> [model]` (T9.1 step 3): main turns run on `tier` with
    /// `model`, or the tier default when `None`; thinking blocks are
    /// stripped because their signatures bind to the previous model.
    pub(crate) async fn switch_model(
        &self,
        tier: Tier,
        model: Option<ModelId>,
    ) -> Result<(), CoreError> {
        let route_err = |e: RouteError| CoreError::Config {
            key: "tiers".into(),
            message: e.notice(),
        };
        let from = self
            .route_for(Job::Main, true)
            .await
            .map_err(route_err)?
            .model;
        {
            let mut inner = self.inner.lock().await;
            inner.overrides.main_tier = Some(tier);
            // P42: the user's own pick outlives the mode's.
            inner.mode_tier = None;
            match model {
                Some(m) => {
                    inner.overrides.models.insert(tier, m);
                }
                None => {
                    inner.overrides.models.remove(&tier);
                }
            }
            inner.history = crate::router::strip_thinking(&inner.history);
        }
        let to = self
            .route_for(Job::Main, true)
            .await
            .map_err(route_err)?
            .model;
        self.emit(Event::ModelSwitched { tier, from, to }).await
    }

    /// Best-effort FTS index of one model-visible text (T10.3): empty
    /// texts are skipped by the store, and failures never fail turns.
    pub(crate) async fn index_text(&self, text: &str) {
        let seq = self.inner.lock().await.turn_seq;
        let _ = self.store.rollout_index(&self.id, seq, text);
    }

    /// Parks `call_id` until `Submission::Approve` answers it.
    pub(crate) async fn await_decision(&self, call_id: CallId) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.inner.lock().await;
        inner.state = State::AwaitingApproval;
        inner.pending.insert(call_id, tx);
        rx
    }

    /// Parks a subagent's call (T27.2) until this session's surface answers
    /// it. Unlike `await_decision` the state stays as it is: this session
    /// is still running the `agent` call that owns the child.
    /// `Relay::ask` for this session: parks the call, raises
    /// `QuestionAsked` and waits for `Submission::Answer`. A session that
    /// closes first dismisses it.
    pub(crate) async fn raise_question(
        &self,
        call_id: CallId,
        question: &str,
        options: &[String],
        source: Option<cox_protocol::types::Source>,
    ) -> Result<Option<String>, ToolError> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().await.questions.insert(call_id, tx);
        self.emit(Event::QuestionAsked {
            call_id,
            question: question.to_string(),
            options: options.to_vec(),
            source,
        })
        .await
        .map_err(|_| ToolError::Io)?;
        Ok(rx.await.unwrap_or(None))
    }

    pub(crate) async fn relay_decision(&self, call_id: CallId) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().await.pending.insert(call_id, tx);
        rx
    }

    /// Records an `AllowForSession` grant.
    pub(crate) async fn grant(&self, tool: String, subject: String) {
        self.inner.lock().await.grants.push((tool, subject));
    }

    /// The `AllowForSession` grants in force, as `(tool, subject prefix)`,
    /// oldest first and without repeats, for the app's Settings (T37.45.3).
    pub async fn grants(&self) -> Vec<(String, String)> {
        let mut grants = self.inner.lock().await.grants.clone();
        let mut seen = std::collections::HashSet::new();
        grants.retain(|g| seen.insert(g.clone()));
        grants
    }

    /// Drops every copy of a grant, so the engine asks for the next call
    /// it covered; `GrantRevoked` lets resume drop it too. A grant the
    /// session does not hold is a warning, not an event.
    async fn revoke(&self, tool: String, subject: String) -> Result<(), CoreError> {
        let removed = {
            let mut inner = self.inner.lock().await;
            let before = inner.grants.len();
            inner.grants.retain(|(t, s)| *t != tool || *s != subject);
            inner.grants.len() < before
        };
        if !removed {
            return self
                .emit(Event::Notice {
                    level: Level::Warn,
                    text: format!("no session grant {tool} {subject}"),
                })
                .await;
        }
        self.emit(Event::GrantRevoked { tool, subject }).await
    }

    /// Dedup bookkeeping for a read-only result; `Some` is the pointer text
    /// that replaces the payload.
    pub(crate) async fn dedup_observe(
        &self,
        tool: &str,
        input: &serde_json::Value,
        subject: &str,
        id: cox_protocol::ArchiveId,
        output: &[u8],
    ) -> Option<String> {
        let mut inner = self.inner.lock().await;
        let round = inner.round;
        inner.dedup.observe(tool, input, subject, id, round, output)
    }

    /// Forgets cached reads a write or command may have changed.
    pub(crate) async fn dedup_invalidate(&self, risk: cox_protocol::types::Risk, subject: &str) {
        self.inner.lock().await.dedup.invalidate(risk, subject);
    }

    /// Remembers where a tool result is archived for microcompaction (T8.2).
    pub(crate) async fn remember_archive(&self, call: CallId, archive: ArchiveRef) {
        self.inner.lock().await.archives.insert(call, archive);
    }

    /// Holds a tool's archived image for this round's results message
    /// (T40.5). A field on the session, not on `ToolResult`, because that
    /// type has dozens of literal constructions.
    pub(crate) async fn remember_image(&self, call: CallId, image: Content) {
        self.inner.lock().await.tool_images.insert(call, image);
    }

    /// The live mode (`SetPermissionMode` changes it; the configured one
    /// is only the starting value), which a subagent inherits (T45.1).
    pub(crate) async fn permission_mode(&self) -> PermissionMode {
        self.inner.lock().await.permission_mode
    }

    /// What this session has spent so far, in USD.
    pub(crate) async fn spent(&self) -> f64 {
        self.inner.lock().await.spent_usd
    }

    /// Charges a subagent's (or side job's) cost to this session.
    pub(crate) async fn add_spend(&self, usd: f64) {
        self.inner.lock().await.spent_usd += usd;
    }

    /// Records deferred tools found by `tool_search`; returns the ones that
    /// are new, which is what the next request adds to its tool list.
    pub(crate) async fn discover(&self, names: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut inner = self.inner.lock().await;
        let mut added = Vec::new();
        for name in names {
            if !inner.discovered.contains(&name) {
                inner.discovered.push(name.clone());
                added.push(name);
            }
        }
        added
    }

    async fn run_turn(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        confirm_think: bool,
    ) -> Result<(), CoreError> {
        let turn = TurnId::new();
        let span = tracing::info_span!(
            parent: &self.telemetry_span,
            "invoke_agent cox.turn",
            gen_ai.operation.name = "invoke_agent",
            gen_ai.agent.name = "cox",
            gen_ai.conversation.id = %self.id,
            gen_ai.input.messages = tracing::field::Empty,
            cox.session.id = %self.id,
            cox.turn.id = %turn,
            cox.job = ?self.job,
            cox.tier = ?self.tier,
            cox.turn.stop_reason = tracing::field::Empty,
            error.type = tracing::field::Empty,
            otel.status_code = tracing::field::Empty,
        );
        // A113: the title job reads the prompt as the user typed it.
        let first_prompt = self.title_prompt(&text).await;
        let result = self
            .run_turn_inner(turn, text, attachments, confirm_think)
            .instrument(span.clone())
            .await;
        if result.is_ok()
            && let Some(prompt) = first_prompt
        {
            self.auto_title(&prompt).await;
        }
        // T33.20: `route` advice holds for its own turn only, however the
        // turn ended.
        self.inner.lock().await.routed = None;
        if let Err(error) = &result {
            span.record("error.type", error.to_string());
            span.record("otel.status_code", "ERROR");
            tracing::error!(
                parent: &span,
                event.name = "cox.turn.failed",
                error = %error,
                "agent turn failed"
            );
        }
        result
    }

    async fn run_turn_inner(
        &self,
        turn: TurnId,
        text: String,
        attachments: Vec<Attachment>,
        confirm_think: bool,
    ) -> Result<(), CoreError> {
        self.renew_cancel();
        let user_item = ItemId::new();
        // §1.8 step 1: a hook may block or rewrite the prompt before it
        // touches history; a blocked prompt is still a (refused) turn so
        // every surface sees its `TurnDone`.
        let (text, context) = match hooks::fire(
            self,
            HookEvent::UserPromptSubmit,
            serde_json::json!({ "prompt": text }),
        )
        .await
        {
            HookOutcome::Block { reason } => {
                let tc = self.config.tiers.get(self.tier);
                self.emit_turn_started(
                    turn,
                    self.next_seq().await,
                    self.tier,
                    ModelId(tc.model.clone()),
                )
                .await?;
                self.emit(Event::Notice {
                    level: Level::Warn,
                    text: format!("prompt blocked by hook: {reason}"),
                })
                .await?;
                return self
                    .finish(turn, StopReason::Refusal { detail: reason })
                    .await;
            }
            HookOutcome::Modify { input } => hooks::prompt_rewrite(text, input),
            _ => (text, None),
        };
        record_content(
            "gen_ai.input.messages",
            &serde_json::json!([{"role": "user", "content": &text}]),
        );
        // D5, A103 (T37.24.11): `/think` and the think toggle move this one
        // turn to the think tier through the same per-turn slot `route`
        // advice uses, so every call of the turn follows it and `run_turn`
        // clears it after. A session already on think (`--deep`, architect)
        // keeps no slot, so its earlier thinking blocks are not stripped.
        if confirm_think {
            let mut inner = self.inner.lock().await;
            if inner.overrides.main_tier.unwrap_or(self.tier) != Tier::Think {
                inner.routed = Some(Tier::Think);
            }
        }
        // T9.1: the think tier needs `confirm_think`; without it the turn is
        // refused before any provider call, with the price in the notice.
        // An unknown provider name is a turn-fatal config error instead.
        let route = match self.route_for(Job::Main, confirm_think).await {
            Ok(route) => route,
            Err(RouteError::NeedsConfirm { tier, model }) => {
                self.emit_turn_started(turn, self.next_seq().await, tier, model.clone())
                    .await?;
                let detail = RouteError::NeedsConfirm { tier, model }.notice();
                self.emit(Event::Notice {
                    level: Level::Warn,
                    text: detail.clone(),
                })
                .await?;
                return self.finish(turn, StopReason::Refusal { detail }).await;
            }
            Err(e) => {
                let tc = self.config.tiers.get(self.tier);
                self.emit_turn_started(
                    turn,
                    self.next_seq().await,
                    self.tier,
                    ModelId(tc.model.clone()),
                )
                .await?;
                self.emit(Event::Error {
                    error: CoreError::Config {
                        key: "tiers".into(),
                        message: e.notice(),
                    },
                    fatal: false,
                })
                .await?;
                return self.finish(turn, StopReason::Error).await;
            }
        };
        let route = self.route_turn(route, &text, confirm_think).await?;
        // §1.10 trigger, applied at the next turn's start rather than after
        // `TurnDone` so nothing follows a turn's last event (§1.3 rule 7).
        let (last, max_context) = (
            self.inner.lock().await.last_context_tokens,
            self.provider.capabilities().max_context,
        );
        let due = compact::needs_compaction(last, max_context, self.config.context.compact_at);
        if self.compact_now(due, last, max_context).await? {
            self.compact(compact::Trigger::Auto, None).await?;
        }
        // T37.6, T40.2: an invalid image, or one the wire cannot take, is
        // held back with a notice rather than sent to a model that would
        // reject the whole request. Only what was sent is recorded, so the
        // rollout rebuilds this exact message (invariant 6).
        let (content, attachments, held) = crate::context::user_content(
            text.clone(),
            context,
            attachments,
            &route.model.0,
            self.provider.accepts_images(&route.model.0),
        );
        let seq = {
            let mut inner = self.inner.lock().await;
            inner.state = State::Assembling;
            let start = inner.history.len();
            inner.history.push(Message {
                role: Role::User,
                content,
            });
            let seq = inner.turn_seq + 1;
            inner.turn_marks.push(TurnMark {
                item: user_item,
                start,
                seq,
            });
            inner.provider_calls = 0;
            inner.retried_after_too_long = false;
            inner.turn_seq += 1;
            inner.turn_seq
        };
        // T10.3: index the user text under this turn's number; best-effort,
        // like every index write.
        let _ = self.store.rollout_index(&self.id, seq, &text);
        crate::checkpoint::mark_turn(self, seq);
        let external = self.external.get().cloned();
        let model = external
            .as_ref()
            .map_or_else(|| route.model.clone(), |a| ModelId(a.name().to_string()));
        self.emit_turn_started(turn, seq, route.tier, model).await?;
        self.emit(Event::ItemStarted {
            item: user_item,
            kind: ItemKind::UserMessage {
                text: text.clone(),
                attachments,
            },
        })
        .await?;
        self.emit(Event::ItemDone { item: user_item }).await?;
        for text in held {
            self.emit(Event::Notice {
                level: Level::Warn,
                text,
            })
            .await?;
        }
        if let Some(agent) = external {
            return self.external_turn(agent, turn, route.tier, text).await;
        }

        loop {
            match self.step(turn).await? {
                Step::Continue => {}
                Step::Done => return Ok(()),
            }
        }
    }

    /// An external-agent child's turn (T35.5, EA§3): the driver stands where
    /// the provider would and runs its own tools inside its own sandboxed
    /// process, so its events are recorded as they come; each answer joins
    /// history like a model's, so `run_task` distils it unchanged. Its
    /// `TurnDone` is held back until the usage row (EA§6: `$0`, tokens
    /// only if reported) is written, since nothing may follow `TurnDone`.
    async fn external_turn(
        &self,
        agent: Arc<dyn ExternalAgent>,
        turn: TurnId,
        tier: Tier,
        prompt: String,
    ) -> Result<(), CoreError> {
        let (tx, mut rx) = mpsc::channel(64);
        let started = std::time::Instant::now();
        let (driver, cancel) = (agent.clone(), self.cancel_token());
        let run = tokio::spawn(async move { driver.turn(turn, prompt, tx, cancel).await });
        let mut stop = None;
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::TurnDone { stop: s, .. } => stop = Some(s),
                ev => {
                    if let Event::ItemStarted {
                        kind: ItemKind::AssistantMessage { text },
                        ..
                    } = &ev
                    {
                        self.inner.lock().await.history.push(Message {
                            role: Role::Assistant,
                            content: vec![Content::Text { text: text.clone() }],
                        });
                    }
                    self.emit(ev).await?;
                }
            }
        }
        let reported = match run.await {
            Ok(Ok(reported)) => reported,
            Ok(Err(error)) => {
                self.emit(Event::Error {
                    error,
                    fatal: false,
                })
                .await?;
                stop = Some(StopReason::Error);
                None
            }
            Err(_) => {
                stop = Some(StopReason::Interrupted);
                None
            }
        };
        let mut usage = reported.unwrap_or(cox_protocol::types::Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: false,
            cost_usd: 0.0,
            latency_ms: 0,
        });
        // Billed on the user's own plan, never cox's ledger (EA§6).
        usage.cost_usd = 0.0;
        usage.latency_ms = started.elapsed().as_millis() as u64;
        self.store
            .usage_insert(&cox_protocol::UsageRow {
                session_id: self.id,
                // One call per turn: the driver runs its own loop inside.
                turn: 1,
                job: self.job.clone(),
                tier,
                provider: ProviderId::External,
                model: ModelId(agent.name().to_string()),
                effort: None,
                usage,
            })
            .map_err(|error| CoreError::Store { error })?;
        self.emit(Event::Usage { turn, usage }).await?;
        let stop = stop.unwrap_or(if self.cancel_token().is_cancelled() {
            StopReason::Interrupted
        } else {
            StopReason::EndTurn
        });
        self.finish(turn, stop).await
    }

    /// One provider call and its tool batch. The turn loop is just
    /// `while step() == Continue`; I/O happens only through traits.
    async fn step(&self, turn: TurnId) -> Result<Step, CoreError> {
        if self.cancel_token().is_cancelled() {
            self.set_state(State::Interrupted).await;
            self.finish(turn, StopReason::Interrupted).await?;
            return Ok(Step::Done);
        }
        let (
            history,
            calls_so_far,
            discovered,
            marks,
            archives,
            startup_context,
            routed,
            repomap,
            mode,
        ) = {
            let inner = self.inner.lock().await;
            (
                inner.history.clone(),
                inner.provider_calls,
                inner.discovered.clone(),
                inner.turn_marks.iter().map(|m| m.start).collect::<Vec<_>>(),
                inner.archives.clone(),
                inner.startup_context.clone(),
                inner.routed.is_some(),
                inner.repomap.clone(),
                // T50.3: the live mode, so the model is told what the engine
                // enforces after a `SetPermissionMode`.
                inner.permission_mode,
            )
        };
        if calls_so_far >= self.config.core.max_turns {
            self.finish(turn, StopReason::MaxTurns).await?;
            return Ok(Step::Done);
        }
        {
            let mut inner = self.inner.lock().await;
            inner.state = State::Assembling;
            inner.provider_calls += 1;
            inner.round += 1;
        }
        // T9.1: every provider call routes through the Router; the gate
        // already passed in `run_turn`, so only a bad provider name can fail
        // here and it is turn-fatal, never silent.
        let route = match self.route_for(Job::Main, true).await {
            Ok(route) => route,
            Err(e) => {
                self.emit(Event::Error {
                    error: CoreError::Config {
                        key: "tiers".into(),
                        message: e.notice(),
                    },
                    fatal: false,
                })
                .await?;
                self.finish(turn, StopReason::Error).await?;
                return Ok(Step::Done);
            }
        };
        // One assembly for the first try and T28.3's pre-call retry, so the
        // retried request differs only in the history it is given.
        let build = |history: &[Message], marks: &[usize], microcompact_after: u32| {
            let req_messages = crate::context::microcompact(
                history,
                marks,
                self.config.context.keep_turns,
                microcompact_after,
                &archives,
            );
            // T40.6: on every request, routed or not — an earlier turn's
            // tool image is never resent, and never in a resumed request.
            let req_messages = match marks.last() {
                Some(start) => crate::context::strip_tool_images_before(req_messages, *start),
                None => req_messages,
            };
            let req_messages = match marks.last() {
                Some(start) if routed => {
                    crate::context::strip_thinking_before(req_messages, *start)
                }
                _ => req_messages,
            };
            let mut req = assemble_with_skills(
                &req_messages,
                &self.config,
                route.tier,
                &self.tools,
                &discovered,
                &self.cwd,
                "",
                &Stable {
                    instructions: self.instructions.get().map_or("", String::as_str),
                    skills_index: self.skills_index.get().map_or("", String::as_str),
                    repomap: repomap.as_deref().unwrap_or(""),
                },
                mode,
            );
            req.model = route.model.clone();
            // T22.3: `SessionStart` hook context goes into `system[3]` — the
            // volatile block after the last cache breakpoint (§1.9), so the
            // cached prefix stays byte-stable.
            if !startup_context.is_empty() {
                req.system[3].text.push_str(&startup_context);
            }
            req
        };
        let first = build(
            &history,
            &marks,
            self.config.context.microcompact_after_turns,
        );
        let req = match self.fit_request(first, build).await? {
            compact::Fit::Fits(req) => req,
            compact::Fit::TooBig(tokens) => {
                self.emit(Event::Notice {
                    level: Level::Budget,
                    text: format!(
                        "request not sent: ~{tokens} tokens is over compact_at {} × max_context {} \
                         even after compaction; the last {} turn(s) are kept verbatim",
                        self.config.context.compact_at,
                        self.provider.capabilities().max_context,
                        self.config.context.keep_turns,
                    ),
                })
                .await?;
                self.finish(turn, StopReason::Budget).await?;
                return Ok(Step::Done);
            }
        };
        let provider_span = tracing::info_span!(
            parent: &tracing::Span::current(),
            "chat",
            gen_ai.operation.name = "chat",
            gen_ai.provider.name = provider_name(route.provider),
            gen_ai.request.model = %route.model,
            gen_ai.response.model = tracing::field::Empty,
            gen_ai.response.finish_reasons = tracing::field::Empty,
            gen_ai.usage.input_tokens = tracing::field::Empty,
            gen_ai.usage.output_tokens = tracing::field::Empty,
            gen_ai.input.messages = tracing::field::Empty,
            gen_ai.output.messages = tracing::field::Empty,
            cox.session.id = %self.id,
            cox.turn.id = %turn,
            cox.provider.call.ordinal = calls_so_far + 1,
            cox.usage.cache_read_tokens = tracing::field::Empty,
            cox.usage.cache_write_tokens = tracing::field::Empty,
            cox.usage.estimated = tracing::field::Empty,
            cox.cost.usd = tracing::field::Empty,
            cox.duration_ms = tracing::field::Empty,
            cox.retry.count = tracing::field::Empty,
            cox.retry.delay_ms = tracing::field::Empty,
            error.type = tracing::field::Empty,
            otel.status_code = tracing::field::Empty,
        );
        if let Some(content) = captured_json(&serde_json::json!(&req.messages)) {
            provider_span.record("gen_ai.input.messages", content);
        }
        // T8.3: hash the prefix before the request moves into the stream.
        let prefix_texts: Vec<String> = req.system.iter().map(|b| b.text.clone()).collect();
        let (spent, warned) = {
            let inner = self.inner.lock().await;
            (inner.spent_usd, inner.budget_warned)
        };
        match budget::decide(
            spent,
            self.config.budget.session_usd,
            self.config.budget.warn_at,
            warned,
        ) {
            budget::Decision::Stop => {
                self.finish(turn, StopReason::Budget).await?;
                return Ok(Step::Done);
            }
            budget::Decision::Warn => {
                {
                    self.inner.lock().await.budget_warned = true;
                }
                self.emit(Event::Notice {
                    level: Level::Budget,
                    text: format!(
                        "budget ${spent:.2} of ${:.2}",
                        self.config.budget.session_usd
                    ),
                })
                .await?;
            }
            budget::Decision::Proceed => {}
        }
        let breakdown = self.context_breakdown(&req).await;
        self.emit(Event::ContextBreakdown { turn, breakdown })
            .await?;
        // `consume_provider` starts this item once any streamed thought is
        // over (A91), so the thought is listed ahead of the reply.
        let assistant_item = ItemId::new();
        self.set_state(State::Streaming).await;
        let (ptx, mut prx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.cancel_token();
        let join = tokio::spawn(
            async move { provider.stream(req, ptx, cancel).await }
                .instrument(provider_span.clone()),
        );
        let streamed = match consume_provider(self, &mut prx, assistant_item)
            .instrument(provider_span.clone())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                provider_span.record("error.type", e.to_string());
                provider_span.record("otel.status_code", "ERROR");
                let _ = join.await;
                self.emit(Event::Error {
                    error: e.clone(),
                    fatal: false,
                })
                .await?;
                self.finish(turn, StopReason::Error).await?;
                return Ok(Step::Done);
            }
        };
        let usage = match join.await {
            Ok(Ok(u)) => u,
            Ok(Err(error)) => {
                provider_span.record("error.type", error.to_string());
                provider_span.record("otel.status_code", "ERROR");
                // §1.10: a too-long request compacts and retries once.
                let too_long = matches!(error, ProviderError::ContextTooLong { .. });
                let retried = self.inner.lock().await.retried_after_too_long;
                if too_long && !retried {
                    self.emit(Event::ItemDone {
                        item: assistant_item,
                    })
                    .await?;
                    self.inner.lock().await.retried_after_too_long = true;
                    if self.compact(compact::Trigger::ContextTooLong, None).await? {
                        return Ok(Step::Continue);
                    }
                }
                self.emit(Event::Error {
                    error: CoreError::Provider { error },
                    fatal: false,
                })
                .await?;
                self.finish(turn, StopReason::Error).await?;
                return Ok(Step::Done);
            }
            Err(_) => {
                provider_span.record("error.type", "provider_task_join");
                provider_span.record("otel.status_code", "ERROR");
                self.set_state(State::Interrupted).await;
                self.finish(turn, StopReason::Interrupted).await?;
                return Ok(Step::Done);
            }
        };
        let usage = streamed.usage.unwrap_or(usage);
        let response_model = streamed.response_model.as_ref().unwrap_or(&route.model);
        provider_span.record("gen_ai.response.model", response_model.to_string());
        if let Some(stop) = &streamed.stop {
            provider_span.record("gen_ai.response.finish_reasons", stop_reason_name(stop));
        }
        provider_span.record("gen_ai.usage.input_tokens", usage.input_tokens as u64);
        provider_span.record("gen_ai.usage.output_tokens", usage.output_tokens as u64);
        provider_span.record(
            "cox.usage.cache_read_tokens",
            usage.cache_read_tokens as u64,
        );
        provider_span.record(
            "cox.usage.cache_write_tokens",
            usage.cache_write_tokens as u64,
        );
        provider_span.record("cox.usage.estimated", usage.estimated);
        provider_span.record("cox.cost.usd", usage.cost_usd);
        provider_span.record("cox.duration_ms", usage.latency_ms);
        provider_span.record("cox.retry.count", streamed.retries as u64);
        provider_span.record("cox.retry.delay_ms", streamed.retry_delay_ms);
        provider_span.record("otel.status_code", "OK");
        if let Some(content) = captured_json(&serde_json::json!({
            "role": "assistant",
            "content": &streamed.text,
            "thinking": &streamed.thinking,
            "tool_calls": &streamed.calls,
        })) {
            provider_span.record("gen_ai.output.messages", content);
        }
        tracing::info!(
            parent: &provider_span,
            event.name = "cox.provider.completed",
            gen_ai.provider.name = provider_name(route.provider),
            gen_ai.request.model = %route.model,
            gen_ai.response.model = %response_model,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_read_tokens = usage.cache_read_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            cost_usd = usage.cost_usd,
            latency_ms = usage.latency_ms,
            retry_count = streamed.retries,
            "provider request completed"
        );
        {
            let mut inner = self.inner.lock().await;
            inner.last_context_tokens = usage
                .input_tokens
                .saturating_add(usage.cache_read_tokens)
                .saturating_add(usage.cache_write_tokens)
                .saturating_add(usage.output_tokens);
            inner.last_usage = Some(usage);
        }
        // T8.3: cache share for the status line; a 0-read after a hit diffs
        // the prefix hashes and names the block that broke it.
        let miss = {
            let mut inner = self.inner.lock().await;
            inner.cache_ratio = crate::cache_diag::ratio_of(&usage);
            inner.cache.observe(&prefix_texts, &usage)
        };
        if let Some(text) = miss {
            self.emit(Event::Notice {
                level: Level::Info,
                text,
            })
            .await?;
        }
        self.store
            .usage_insert(&cox_protocol::UsageRow {
                session_id: self.id,
                turn: calls_so_far + 1,
                job: self.job.clone(),
                tier: route.tier,
                provider: self.provider.id(),
                model: route.model.clone(),
                effort: Some(route.effort),
                usage,
            })
            .map_err(|error| CoreError::Store { error })?;
        if budget::counts(route.tier, self.config.budget.cheap_counts) {
            self.inner.lock().await.spent_usd += usage.cost_usd;
        }
        self.emit(Event::Usage { turn, usage }).await?;
        self.emit(Event::ItemDone {
            item: assistant_item,
        })
        .await?;

        if streamed.calls.is_empty() {
            if !streamed.text.is_empty() {
                let mut inner = self.inner.lock().await;
                inner.history.push(Message {
                    role: Role::Assistant,
                    content: vec![Content::Text {
                        text: streamed.text.clone(),
                    }],
                });
            }
            self.index_text(&streamed.text).await;
            self.finish(turn, StopReason::EndTurn).await?;
            return Ok(Step::Done);
        }

        {
            let mut inner = self.inner.lock().await;
            inner.history.push(Message {
                role: Role::Assistant,
                content: {
                    let mut blocks = Vec::new();
                    if !streamed.text.is_empty() {
                        blocks.push(Content::Text {
                            text: streamed.text.clone(),
                        });
                    }
                    for (id, name, input) in &streamed.calls {
                        // T39.2: the signature sits right before its call,
                        // the order `run_signed_tools` writes to the rollout.
                        if let Some(sig) = streamed.signatures.get(id) {
                            blocks.push(Content::Thinking {
                                text: String::new(),
                                signature: Some(sig.clone()),
                            });
                        }
                        blocks.push(Content::ToolUse {
                            id: *id,
                            name: name.clone(),
                            input: input.clone(),
                        });
                    }
                    blocks
                },
            });
            inner.state = State::RunningTools;
        }
        let results = run_signed_tools(self, turn, streamed.calls, &streamed.signatures).await?;
        if self.cancel_token().is_cancelled() {
            self.set_state(State::Interrupted).await;
            self.finish(turn, StopReason::Interrupted).await?;
            return Ok(Step::Done);
        }
        let order: Vec<CallId> = results.iter().map(|(id, _)| *id).collect();
        let mut msg = results_message(results);
        // T40.5: images follow every `ToolResult` (Anthropic wants the
        // results first; the Chat wire sends images as a user message after
        // the tool messages), in call order.
        let images: Vec<Content> = {
            // Taken whole, so an interrupted round leaves nothing behind.
            let mut held = std::mem::take(&mut self.inner.lock().await.tool_images);
            order.iter().filter_map(|id| held.remove(id)).collect()
        };
        if !images.is_empty() {
            if self.provider.accepts_images(&route.model.0) {
                msg.content.extend(images);
            } else {
                self.emit(Event::Notice {
                    level: Level::Warn,
                    text: format!(
                        "{} tool image(s) not sent: {} does not take images on this provider; \
                         each stays archived",
                        images.len(),
                        route.model
                    ),
                })
                .await?;
            }
        }
        // T10.3: tool results are user-role text the user will grep for.
        let joined: String = msg
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        {
            let mut inner = self.inner.lock().await;
            inner.history.push(msg);
        }
        self.index_text(&joined).await;
        Ok(Step::Continue)
    }

    /// The number the next user turn will get; a refused turn (blocked
    /// prompt, unconfirmed think tier) reports it too, since the counter
    /// only moves when a user item lands.
    async fn next_seq(&self) -> u32 {
        self.inner.lock().await.turn_seq + 1
    }

    /// A98: `req`'s window and split, emitted just before it is sent. The
    /// window is the catalog row of `req.model`, else the provider's own
    /// finite `max_context` (a plugin model or a served window the core's
    /// config-only catalog does not carry). The total is the same byte
    /// heuristic compaction weighs a request with, since this crate may not
    /// call the provider's estimator. Reads `req`, never changes it.
    async fn context_breakdown(&self, req: &Request) -> ContextBreakdown {
        let max = self.provider.capabilities().max_context;
        let mut inner = self.inner.lock().await;
        let catalog = inner.catalog.get_or_insert_with(|| {
            // Fail open: the built-in rows are embedded, so a failed load
            // leaves only the provider's number, never a failed turn.
            cox_models::Catalog::load(&self.config, &[], None).unwrap_or_default()
        });
        let window = catalog
            .get(&req.model.0)
            .and_then(|row| row.context_window)
            .or((max > 0 && max < u32::MAX).then_some(max));
        crate::context::breakdown(req, compact::estimate(req), inner.last_usage.as_ref())
            .parts(window)
    }

    async fn emit_turn_started(
        &self,
        turn: TurnId,
        seq: u32,
        tier: Tier,
        model: ModelId,
    ) -> Result<(), CoreError> {
        self.emit(Event::TurnStarted {
            turn,
            seq,
            job: self.job.clone(),
            tier,
            model,
        })
        .await
    }

    /// `/mode architect|editor` (P42, T42.3): architect narrows the live
    /// permission mode to `plan` and moves main turns to think; editor
    /// restores `permissions.mode` and the tier the mode replaced. Tools are
    /// never touched, and think still needs `confirm_think` per turn
    /// (invariant 9). Refused mid-turn: a turn's calls keep one mode.
    async fn switch_mode(&self, arg: Option<&str>) -> Result<(), CoreError> {
        let Some(mode) = arg.and_then(crate::mode::parse) else {
            return self
                .emit(Event::Notice {
                    level: Level::Warn,
                    text: "usage: /mode architect|editor".into(),
                })
                .await;
        };
        let preset = crate::mode::preset(mode);
        let events = {
            let mut inner = self.inner.lock().await;
            if inner.state != State::Idle {
                None
            } else {
                let base = match mode {
                    Mode::Editor => self.config.permissions.mode,
                    Mode::Architect => inner.permission_mode,
                };
                inner.permission_mode = crate::mode::apply(preset, base);
                let before = inner.overrides.main_tier;
                match preset.main_tier {
                    Some(tier) => {
                        if inner.mode_tier.is_none() {
                            inner.mode_tier = Some(before);
                        }
                        inner.overrides.main_tier = Some(tier);
                    }
                    None => {
                        if let Some(saved) = inner.mode_tier.take() {
                            inner.overrides.main_tier = saved;
                        }
                    }
                }
                // Same reason as `/model`: thinking signatures bind to the
                // model that wrote them.
                if inner.overrides.main_tier != before {
                    inner.history = crate::router::strip_thinking(&inner.history);
                }
                inner.mode = mode;
                Some((state_changed(&inner), mode_changed(&inner)))
            }
        };
        match events {
            // `StateChanged` too: resume rebuilds the permission mode from
            // it (T50.4), and surfaces already follow it.
            Some((state, changed)) => {
                self.emit(state).await?;
                self.emit(changed).await
            }
            None => {
                self.emit(Event::Notice {
                    level: Level::Warn,
                    text: "a turn is running; `/mode` waits until it ends".into(),
                })
                .await
            }
        }
    }

    /// A composer `!` line (T25.3). The call takes the model's path
    /// (`run_tools`: `PreToolUse`, the engine, the sandbox, the archive) so
    /// a user command is no more trusted than a model one. Refused outside
    /// `Idle`: a shell result landing mid-turn would split a tool_use from
    /// its tool_result in history.
    async fn user_shell(&self, command: String, share: bool) -> Result<(), CoreError> {
        let input = serde_json::json!({ "command": command });
        let Some(result) = self.user_tool("`!`", "bash", input).await? else {
            return Ok(());
        };
        if share {
            self.push_user_text(format!("$ {command}\n{}", result.visible))
                .await;
        }
        Ok(())
    }

    /// A composer `@name task` line (T45.5): the `agent` tool on the same
    /// path as `!` (`run_tools`: `PreToolUse`, the engine, the budget and
    /// the agent slots), so a user dispatch is no more trusted than a model
    /// one; an unknown name is the tool's own denial, which lists the names
    /// it accepts. The line and the answer join history at its tail, never
    /// inside the cached prefix, so the next model turn sees them.
    async fn user_agent(&self, name: String, task: String) -> Result<(), CoreError> {
        let input = serde_json::json!({ "preset": name, "task": task });
        let label = format!("`@{name}`");
        let Some(result) = self.user_tool(&label, "agent", input).await? else {
            return Ok(());
        };
        self.push_user_text(format!("@{name} {task}\n{}", result.visible))
            .await;
        Ok(())
    }

    /// One user-issued tool call (`!`, `@name`) through `run_tools`, or
    /// `None` with a notice naming `what` when a turn is running: a result
    /// landing mid-turn would split a tool_use from its tool_result in
    /// history.
    async fn user_tool(
        &self,
        what: &str,
        tool: &str,
        input: serde_json::Value,
    ) -> Result<Option<ToolResult>, CoreError> {
        let busy = {
            let mut inner = self.inner.lock().await;
            let busy = inner.state != State::Idle;
            if !busy {
                inner.state = State::RunningTools;
            }
            busy
        };
        if busy {
            self.emit(Event::Notice {
                level: Level::Warn,
                text: format!("a turn is running; {what} waits until it ends"),
            })
            .await?;
            return Ok(None);
        }
        self.renew_cancel();
        let ran = run_tools(
            self,
            TurnId::new(),
            vec![(CallId::new(), tool.into(), input)],
        )
        .await;
        self.inner.lock().await.state = State::Idle;
        Ok(ran?.into_iter().next().map(|(_, result)| result))
    }

    /// Appends one user text message at the history tail.
    async fn push_user_text(&self, text: String) {
        self.inner.lock().await.history.push(Message {
            role: Role::User,
            content: vec![Content::Text { text }],
        });
    }

    pub(crate) async fn set_state(&self, state: State) {
        self.inner.lock().await.state = state;
    }

    async fn finish(&self, turn: TurnId, stop: StopReason) -> Result<(), CoreError> {
        {
            let mut inner = self.inner.lock().await;
            inner.state = State::Finishing;
        }
        if stop == StopReason::EndTurn {
            // §1.8 step 4: `Stop` is informational; its verdict is not applied.
            let _ = hooks::fire(self, HookEvent::Stop, serde_json::json!({})).await;
        }
        self.emit(Event::TurnDone {
            turn,
            stop: stop.clone(),
        })
        .await?;
        let span = tracing::Span::current();
        span.record("cox.turn.stop_reason", stop_reason_name(&stop));
        span.record(
            "otel.status_code",
            if stop == StopReason::Error {
                "ERROR"
            } else {
                "OK"
            },
        );
        tracing::info!(
            event.name = "cox.turn.completed",
            cox.session.id = %self.id,
            cox.turn.id = %turn,
            stop.reason = ?stop,
            "agent turn completed"
        );
        {
            let mut inner = self.inner.lock().await;
            inner.state = State::Idle;
        }
        Ok(())
    }
}

/// Raw model/tool content may contain source and secrets, so standard GenAI
/// content attributes are populated only under the OpenTelemetry opt-in.
pub(crate) fn capture_message_content() -> bool {
    std::env::var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT")
        .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

pub(crate) fn record_content(field: &'static str, value: &serde_json::Value) {
    if let Some(content) = captured_json(value) {
        tracing::Span::current().record(field, content);
    }
}

pub(crate) fn captured_json(value: &serde_json::Value) -> Option<String> {
    capture_message_content().then(|| value.to_string())
}

/// The `snake_case` name of a stop reason, matching its serde tag. The
/// OpenTelemetry GenAI convention wants `gen_ai.response.finish_reasons` to
/// be a stable identifier, so this never uses `Debug`, whose output would
/// leak Rust spelling (`Some(EndTurn)`) and change with the enum.
pub(crate) fn stop_reason_name(stop: &StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTurns => "max_turns",
        StopReason::Interrupted => "interrupted",
        StopReason::Budget => "budget",
        StopReason::Refusal { .. } => "refusal",
        StopReason::Error => "error",
    }
}

fn provider_name(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Anthropic => "anthropic",
        ProviderId::OpenAi => "openai",
        ProviderId::Local => "local",
        ProviderId::Jev => "typesafe",
        ProviderId::External => "external",
    }
}

/// In-memory store for loop tests: no SQLite, same trait.
pub struct MemoryStore {
    /// Every session's rollout, tagged so a resumed child reads only its own.
    events: StdMutex<Vec<(SessionId, Event)>>,
    usage: StdMutex<Vec<cox_protocol::UsageRow>>,
    archive: StdMutex<HashMap<cox_protocol::ArchiveId, Vec<u8>>>,
    /// `(project, name)` → `(path, body)` for `memory_*` (T10.1).
    memory: StdMutex<HashMap<(String, String), (String, String)>>,
    /// `(session, turn, text)` FTS rows (T10.3).
    index: StdMutex<Vec<(String, u32, String)>>,
    /// `checkpoints` rows in insertion order (T26.1).
    checkpoints: StdMutex<Vec<cox_protocol::CheckpointRow>>,
}

impl MemoryStore {
    /// Empty ledger and rollout.
    pub fn new() -> Self {
        Self {
            events: StdMutex::new(Vec::new()),
            usage: StdMutex::new(Vec::new()),
            archive: StdMutex::new(HashMap::new()),
            memory: StdMutex::new(HashMap::new()),
            index: StdMutex::new(Vec::new()),
            checkpoints: StdMutex::new(Vec::new()),
        }
    }

    /// Ledger rows written for this session (test assertion).
    pub fn usage_rows(&self) -> Vec<cox_protocol::UsageRow> {
        self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Indexed texts (test assertion for T10.3 call sites).
    pub fn indexed_texts(&self) -> Vec<(String, u32, String)> {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MemoryStore {
    fn open(_home: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::new())
    }
    fn session_create(&self, _s: &cox_protocol::SessionRow) -> Result<(), StoreError> {
        Ok(())
    }
    fn rollout_append(&self, id: &SessionId, ev: &Event) -> Result<u64, StoreError> {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events.push((*id, ev.clone()));
        Ok(events.len() as u64)
    }
    /// One session's rollout; an id that never wrote reads the whole log,
    /// which tests use to see every session at once.
    fn rollout_read(&self, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        let events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let any = events.iter().any(|(sid, _)| sid == id);
        Ok(events
            .iter()
            .filter(|(sid, _)| !any || sid == id)
            .map(|(_, ev)| ev.clone())
            .collect())
    }
    fn usage_insert(&self, row: &cox_protocol::UsageRow) -> Result<(), StoreError> {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(row.clone());
        Ok(())
    }
    fn archive_put(&self, a: &ArchivePut) -> Result<cox_protocol::ArchiveId, StoreError> {
        let id = cox_protocol::ArchiveId::new();
        self.archive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, a.bytes.clone());
        Ok(id)
    }
    fn archive_get(&self, id: &cox_protocol::ArchiveId) -> Result<Vec<u8>, StoreError> {
        self.archive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or(StoreError::NotFound)
    }
    fn memory_search(
        &self,
        q: &str,
        limit: usize,
    ) -> Result<Vec<cox_protocol::MemoryHit>, StoreError> {
        // Substring stand-in for FTS5: every term must appear in the name or
        // body, most hits first. The real ranking lives in `cox-store`.
        let terms: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
        let mut hits: Vec<(usize, String, String, String)> = self
            .memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|((_, name), (path, body))| {
                if terms.is_empty() {
                    return None;
                }
                let hay = format!("{name}\n{body}").to_lowercase();
                if !terms.iter().all(|t| hay.contains(t)) {
                    return None;
                }
                let score = terms.iter().map(|t| hay.matches(t).count()).sum();
                let snippet: String = body.chars().take(200).collect();
                Some((score, name.clone(), path.clone(), snippet))
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        Ok(hits
            .into_iter()
            .take(limit.max(1))
            .map(|(_, name, path, snippet)| cox_protocol::MemoryHit {
                name,
                path: path.into(),
                snippet,
            })
            .collect())
    }
    fn memory_upsert(
        &self,
        project: &str,
        name: &str,
        path: &str,
        _kind: &str,
        body: &str,
    ) -> Result<(), StoreError> {
        self.memory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (project.to_string(), name.to_string()),
                (path.to_string(), body.to_string()),
            );
        Ok(())
    }
    fn rollout_index(&self, session: &SessionId, turn: u32, text: &str) -> Result<(), StoreError> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.index.lock().unwrap_or_else(|e| e.into_inner()).push((
            session.to_string(),
            turn,
            text.to_string(),
        ));
        Ok(())
    }
    fn checkpoint_insert(&self, row: &cox_protocol::CheckpointRow) -> Result<(), StoreError> {
        self.checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(row.clone());
        Ok(())
    }
    fn checkpoint_list(
        &self,
        session: &SessionId,
    ) -> Result<Vec<cox_protocol::CheckpointRow>, StoreError> {
        Ok(self
            .checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|r| r.session == *session)
            .cloned()
            .collect())
    }
}

#[async_trait::async_trait]
impl Archive for MemoryStore {
    async fn put(&self, put: ArchivePut) -> Result<cox_protocol::ArchiveId, StoreError> {
        self.archive_put(&put)
    }
    async fn get(&self, id: &cox_protocol::ArchiveId) -> Result<Vec<u8>, StoreError> {
        self.archive_get(id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cox_protocol::config::HookConfig;
    use cox_provider::scripted::Scripted;
    use serde_json::Value;

    use super::*;

    /// Records every hook call and always continues (T22.3's claims).
    #[derive(Default)]
    struct Probe(StdMutex<Vec<(HookEvent, Value)>>);

    #[async_trait::async_trait]
    impl Hook for Probe {
        async fn run(&self, event: HookEvent, payload: Value, _timeout: Duration) -> HookOutcome {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((event, payload));
            HookOutcome::Continue
        }
    }

    /// A session over one scripted reply whose hook config enables exactly
    /// the events in `configured` (T22.3 dispatches its two new ones only
    /// where a user configured them).
    fn open(configured: &[&str]) -> (Session, Arc<Probe>) {
        let mut config = cox_protocol::Config::default();
        for name in configured {
            config.hooks.events.insert(
                (*name).to_string(),
                vec![HookConfig {
                    command: "true".into(),
                    ..HookConfig::default()
                }],
            );
        }
        let store = Arc::new(MemoryStore::new());
        let provider =
            Arc::new(Scripted::from_toml("[[turn]]\ntext = \"ok\"\n", "").expect("scenario"));
        let session = Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-turn"),
        )
        .expect("session");
        let probe = Arc::new(Probe::default());
        session.set_hook(probe.clone());
        (session, probe)
    }

    /// `Scripted` on a wire without image input: `Scripted` itself stands
    /// in for a vision wire (T40.7), and `accepts_images` defaults to false.
    struct TextOnly(Scripted);

    #[async_trait::async_trait]
    impl Provider for TextOnly {
        fn id(&self) -> ProviderId {
            self.0.id()
        }
        fn capabilities(&self) -> cox_protocol::types::Caps {
            self.0.capabilities()
        }
        async fn stream(
            &self,
            req: Request,
            sink: mpsc::Sender<cox_protocol::types::ProviderEvent>,
            cancel: CancellationToken,
        ) -> Result<cox_protocol::types::Usage, ProviderError> {
            self.0.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.0.count_tokens(req).await
        }
    }

    /// T37.6 Check: a wire without image input gets the notice and the text
    /// still goes. T40.2: the rollout records only what was sent, so a
    /// resumed session rebuilds the same text-only message.
    #[tokio::test]
    async fn image_on_a_text_only_wire_is_held_back_with_a_notice() {
        let store = Arc::new(MemoryStore::new());
        let scripted = Scripted::from_toml("[[turn]]\ntext = \"ok\"\n", "").expect("scenario");
        let provider = Arc::new(TextOnly(scripted));
        let session = Session::new(
            cox_protocol::Config::default(),
            provider,
            vec![],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-turn"),
        )
        .expect("session");
        let shot = Attachment {
            name: "shot.png".into(),
            media_type: "image/png".into(),
            data_b64: "iVBORw0KGgo=".into(),
        };
        session
            .submit(Submission::UserTurn {
                text: "look".into(),
                attachments: vec![shot],
                confirm_think: false,
            })
            .await
            .expect("turn");
        let events = store.rollout_read(&session.id()).expect("rollout");
        assert!(events.iter().any(|e| matches!(e,
            Event::ItemStarted { kind: ItemKind::UserMessage { attachments, .. }, .. }
                if attachments.is_empty())));
        assert!(events.iter().any(|e| matches!(e,
            Event::Notice { level: Level::Warn, text } if text.contains("does not take images"))));
        let history = &session.inner.lock().await.history;
        assert_eq!(
            history[0].content,
            vec![Content::Text {
                text: "look".into()
            }]
        );
    }

    #[tokio::test]
    async fn session_start_hook_runs_once() {
        let (session, probe) = open(&["SessionStart"]);
        for _ in 0..2 {
            session
                .submit(Submission::SetEffort { effort: None })
                .await
                .expect("submit");
        }
        let seen = probe.0.lock().unwrap_or_else(|e| e.into_inner());
        let starts: Vec<&Value> = seen
            .iter()
            .filter(|(e, _)| *e == HookEvent::SessionStart)
            .map(|(_, p)| p)
            .collect();
        assert_eq!(starts.len(), 1, "one dispatch per session, whatever runs");
        assert_eq!(starts[0]["source"], "startup");
        assert_eq!(starts[0]["session_id"], session.id().to_string());
        assert_eq!(starts[0]["cwd"], "/tmp/cox-turn");
    }

    #[tokio::test]
    async fn notification_hook_gets_turn_done_payload() {
        let (session, probe) = open(&["Notification"]);
        session
            .submit(Submission::UserTurn {
                text: "hi".into(),
                attachments: vec![],
                confirm_think: false,
            })
            .await
            .expect("turn");
        let seen = probe.0.lock().unwrap_or_else(|e| e.into_inner());
        let (_, payload) = seen
            .iter()
            .find(|(e, _)| *e == HookEvent::Notification)
            .expect("Notification fired for TurnDone");
        assert_eq!(payload["kind"], "turn_done");
        assert_eq!(payload["title"], "Turn done");
        assert_eq!(payload["message"], "end_turn");
        assert_eq!(payload["hook_event_name"], "Notification");
    }

    /// The script, plus every request it was sent (P42's prefix claim is
    /// about the bytes the provider saw).
    struct Recording {
        script: Scripted,
        seen: StdMutex<Vec<Request>>,
    }

    #[async_trait::async_trait]
    impl Provider for Recording {
        fn id(&self) -> ProviderId {
            self.script.id()
        }
        fn capabilities(&self) -> cox_protocol::types::Caps {
            self.script.capabilities()
        }
        async fn stream(
            &self,
            req: Request,
            sink: mpsc::Sender<cox_protocol::types::ProviderEvent>,
            cancel: CancellationToken,
        ) -> Result<cox_protocol::types::Usage, ProviderError> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.clone());
            self.script.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.script.count_tokens(req).await
        }
    }

    impl Recording {
        fn seen(&self) -> Vec<Request> {
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    /// A session under `config` over `turns` scripted replies.
    fn recorded(
        config: cox_protocol::Config,
        turns: usize,
    ) -> (Session, Arc<MemoryStore>, Arc<Recording>) {
        let provider = Arc::new(Recording {
            script: Scripted::from_toml(&"[[turn]]\ntext = \"ok\"\n".repeat(turns), "")
                .expect("scenario"),
            seen: StdMutex::new(Vec::new()),
        });
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            config,
            provider.clone(),
            vec![],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-mode"),
        )
        .expect("session");
        (session, store, provider)
    }

    async fn set_mode(session: &Session, name: &str) {
        session
            .submit(Submission::Command {
                command: cox_protocol::types::SlashCommand {
                    name: "mode".into(),
                    args: vec![name.into()],
                },
            })
            .await
            .expect("/mode");
    }

    async fn turn(session: &Session, confirm_think: bool) {
        session
            .submit(Submission::UserTurn {
                text: "plan it".into(),
                attachments: vec![],
                confirm_think,
            })
            .await
            .expect("turn");
    }

    #[tokio::test]
    async fn architect_denies_write_through_the_engine() {
        let (session, store, _) = recorded(cox_protocol::Config::default(), 0);
        let write = ToolCall {
            id: CallId::new(),
            name: "write".into(),
            input: serde_json::json!({"path": "/tmp/cox-mode/a.rs"}),
            risk: cox_protocol::types::Risk::Write,
            subject: "/tmp/cox-mode/a.rs".into(),
            segments: None,
        };
        assert!(!matches!(
            session.decide(&write).await,
            Outcome::Deny { .. }
        ));
        set_mode(&session, "architect").await;
        match session.decide(&write).await {
            Outcome::Deny { reason, .. } => assert!(reason.contains("plan mode"), "{reason}"),
            other => panic!("architect let a write through: {other:?}"),
        }
        let events = store.rollout_read(&session.id()).expect("rollout");
        assert!(events.iter().any(|e| *e
            == Event::ModeChanged {
                mode: Mode::Architect,
                permission_mode: PermissionMode::Plan,
            }));
    }

    #[tokio::test]
    async fn architect_never_widens_a_plan_config() {
        let mut config = cox_protocol::Config::default();
        config.permissions.mode = PermissionMode::Plan;
        let (session, _, _) = recorded(config, 0);
        for name in ["architect", "editor", "architect"] {
            set_mode(&session, name).await;
            assert_eq!(
                session.permission_mode().await,
                PermissionMode::Plan,
                "{name}"
            );
        }
        // A wider mode picked by hand (Shift+Tab) is narrowed again.
        session
            .submit(Submission::SetPermissionMode {
                mode: PermissionMode::Auto,
            })
            .await
            .expect("set mode");
        set_mode(&session, "architect").await;
        assert_eq!(session.permission_mode().await, PermissionMode::Plan);
    }

    #[tokio::test]
    async fn editor_restores_the_configured_mode() {
        let mut config = cox_protocol::Config::default();
        config.permissions.mode = PermissionMode::Auto;
        let (session, _, _) = recorded(config, 0);
        set_mode(&session, "architect").await;
        assert_eq!(session.permission_mode().await, PermissionMode::Plan);
        assert_eq!(
            session.inner.lock().await.overrides.main_tier,
            Some(Tier::Think)
        );
        set_mode(&session, "editor").await;
        assert_eq!(session.permission_mode().await, PermissionMode::Auto);
        assert_eq!(session.inner.lock().await.overrides.main_tier, None);

        // A `/model` pick made before the mode survives leaving it.
        session
            .switch_model(Tier::Cheap, None)
            .await
            .expect("/model cheap");
        set_mode(&session, "architect").await;
        set_mode(&session, "editor").await;
        assert_eq!(
            session.inner.lock().await.overrides.main_tier,
            Some(Tier::Cheap)
        );
    }

    #[tokio::test]
    async fn mode_switch_keeps_prefix_bytes_identical() {
        let (session, _, provider) = recorded(cox_protocol::Config::default(), 2);
        turn(&session, false).await;
        set_mode(&session, "architect").await;
        turn(&session, true).await;
        let seen = provider.seen();
        assert_eq!(seen.len(), 2, "one request per turn");
        assert_eq!(seen[0].tier, Tier::Code);
        assert_eq!(seen[1].tier, Tier::Think, "the mode moved main turns");
        assert_eq!(
            seen[0].system[..3],
            seen[1].system[..3],
            "system[0..2] byte-identical across /mode"
        );
        assert_eq!(seen[0].tools, seen[1].tools, "no tool filtered by mode");
    }

    /// T50.3 Check: after `SetPermissionMode` the next request's volatile
    /// block names the live mode, and the cached prefix does not move.
    #[tokio::test]
    async fn volatile_block_shows_the_live_permission_mode() {
        let (session, _, provider) = recorded(cox_protocol::Config::default(), 2);
        turn(&session, false).await;
        let mode = Submission::SetPermissionMode {
            mode: PermissionMode::Plan,
        };
        session.submit(mode).await.expect("set mode");
        turn(&session, false).await;
        let seen = provider.seen();
        assert_eq!(seen.len(), 2, "one request per turn");
        let configured = format!("{:?}", cox_protocol::Config::default().permissions.mode);
        let volatile = |req: &Request| req.system[3].text.clone();
        assert!(volatile(&seen[0]).contains(&format!("permission_mode={configured}\n")));
        assert!(
            volatile(&seen[1]).contains("permission_mode=Plan\n"),
            "{}",
            volatile(&seen[1])
        );
        assert_eq!(
            seen[0].system[..3],
            seen[1].system[..3],
            "cached prefix moved"
        );
    }

    #[tokio::test]
    async fn architect_think_still_requires_confirmation() {
        let mut config = cox_protocol::Config::default();
        config.core.mode = Mode::Architect;
        let (session, store, provider) = recorded(config, 1);
        assert_eq!(session.permission_mode().await, PermissionMode::Plan);
        turn(&session, false).await;
        assert!(
            provider.seen().is_empty(),
            "no provider call without consent"
        );
        let events = store.rollout_read(&session.id()).expect("rollout");
        assert!(events.iter().any(|e| matches!(
            e,
            Event::TurnDone {
                stop: StopReason::Refusal { .. },
                ..
            }
        )));
        turn(&session, true).await;
        let seen = provider.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].tier, Tier::Think);
    }

    /// Counts `Tool::shutdown` calls (T41.5).
    #[derive(Default)]
    struct Owner(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl Tool for Owner {
        fn spec(&self) -> cox_protocol::types::ToolSpec {
            cox_protocol::types::ToolSpec {
                name: "owner".into(),
                description: String::new(),
                input_schema: Value::Null,
                deferred: false,
                risk: cox_protocol::types::Risk::ReadOnly,
                concurrency: cox_protocol::types::Concurrency::Parallel,
            }
        }
        fn subject(&self, _input: &Value) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: Value,
            _cx: &cox_protocol::traits::ToolCx,
        ) -> Result<cox_protocol::types::ToolOutput, ToolError> {
            Err(ToolError::NotFound)
        }
        fn shutdown(&self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn with_tool(tool: Arc<Owner>) -> Session {
        let store = Arc::new(MemoryStore::new());
        let provider =
            Arc::new(Scripted::from_toml("[[turn]]\ntext = \"ok\"\n", "").expect("scenario"));
        Session::new(
            cox_protocol::Config::default(),
            provider,
            vec![tool],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-turn"),
        )
        .expect("session")
    }

    fn shutdowns(tool: &Owner) -> usize {
        tool.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn end_shuts_down_tools_once() {
        let tool = Arc::new(Owner::default());
        let session = with_tool(tool.clone());
        assert_eq!(shutdowns(&tool), 0);
        session.end();
        assert_eq!(shutdowns(&tool), 1);
        // A second end, or one from a clone of the handle, is not a second shutdown.
        session.end();
        session.clone().end();
        assert_eq!(shutdowns(&tool), 1);
    }

    #[tokio::test]
    async fn child_end_does_not_shut_down_parent_tools() {
        let tool = Arc::new(Owner::default());
        let parent = with_tool(tool.clone());
        let child = parent
            .spawn_child(
                parent.config.clone(),
                parent.tools.clone(),
                Job::Explore,
                Tier::Code,
                None,
                None,
                "explore-1".into(),
                "explore".into(),
            )
            .expect("child");
        child.end();
        assert_eq!(shutdowns(&tool), 0);
        parent.end();
        assert_eq!(shutdowns(&tool), 1);
    }
}
