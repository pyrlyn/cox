// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One live session (DT§4.5): built through `cox-session` with the host's
//! keys and login prompt, its events folded into the app's inbox and fed to
//! a [`Controller`], and each intent run the way [`dispatch`] says — a turn
//! spawned and never awaited, a queued turn after the running one, a fork
//! or handoff opened as a new session. Separate from `app.rs`, which owns
//! what outlives one session.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use cox_core::{History, Session};
use cox_protocol::Checkpointer as _;
use cox_protocol::SandboxPolicy;
use cox_protocol::ids::{ArchiveId, SessionId, TaskId};
use cox_protocol::plugin::{CommandOut, NoticeLevel};
use cox_protocol::traits::Store as _;
use cox_protocol::types::{Event, Level, Submission, TodoItem};
use cox_render::diffmodel::DiffModel;
use cox_sanitize::sanitize;
use cox_session::SessionSpec;
use cox_session::acp_session::AcpSession;
use cox_session::plugin_ui::PluginAnswer;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app::{App, AppError};
use crate::changes::{self, Changes};
use crate::costs::{self, TurnCosts};
use crate::info::{self, Info};
use crate::mcp_status::McpRun;
use crate::plugin_ui::{self, Answered, ItemWatch, PluginKey, PluginSlot, PluginUi};
use crate::review;
use crate::status::StatusFold;
use crate::tasks::{self, TaskTarget};
use crate::terminal::{self, TerminalHandle, TerminalSpec};
use crate::{Block, Completer, Completion, Controller, Dispatch, Intent, SessionGrant, Timeline};
use crate::{TimelinePatch, dispatch};

/// The core's own bound (DT§4.5).
const EVENTS: usize = 256;
/// Plugin UI requests and answers in flight (T52.14); a full queue drops a
/// request, which the next redraw asks again.
const PLUGIN_QUEUE: usize = 64;

/// What runs the session: cox's own core, or an external ACP agent's
/// process (T52.4), whose events feed the same timeline. A stored agent
/// session that could not be reattached keeps the reason instead (T52.6).
// One per open session and never moved after `open`, so the core's size
// costs nothing a box would save.
#[allow(clippy::large_enum_variant)]
enum Driver {
    Core(Session),
    Agent {
        id: SessionId,
        agent: String,
        acp: Result<AcpSession, String>,
    },
}

pub struct LiveSession {
    app: Arc<App>,
    driver: Driver,
    /// Shared with each queued turn, which reports when it starts.
    controller: Arc<Controller>,
    completer: Completer,
    /// The granted plugin slots, commands and keys (T52.14, PL§8).
    plugins: Arc<PluginUi>,
    cwd: PathBuf,
    /// The workspace roots a path Review reads is confined to.
    roots: Vec<PathBuf>,
    /// The session's resolved `[sandbox]`, which its terminal panes run
    /// under as its `bash` calls do (T51.3).
    sandbox: SandboxPolicy,
    theme: String,
    warnings: Vec<String>,
    /// The turn spawned last; a queued one starts after it.
    turn: Mutex<Option<JoinHandle<()>>>,
}

impl LiveSession {
    /// Opens a session in `cwd` (resuming `resume`) and starts draining it.
    pub(crate) async fn open(
        app: Arc<App>,
        cwd: PathBuf,
        resume: Option<(SessionId, History)>,
        theme: String,
    ) -> Result<Arc<Self>, AppError> {
        let config = app.config(&cwd)?;
        let mut timeline = Timeline::new(&theme);
        let mut status = StatusFold::open(&config);
        if let Some((id, _)) = &resume {
            for event in cox_store::Store::open(&app.home)?.rollout_read(id)? {
                timeline.apply(&event);
                status.apply(&event);
            }
        }
        let tried: Vec<String> = match config.mcp.enabled {
            true => cox_session::mcp_servers(&config, &cwd)
                .servers
                .into_keys()
                .collect(),
            false => Vec::new(),
        };
        let (login, keys) = (Arc::clone(&app.host), Arc::clone(&app.host));
        let (asks, requests) = mpsc::channel(PLUGIN_QUEUE);
        let (answered, answers) = mpsc::channel(PLUGIN_QUEUE);
        let plugins = Arc::new(PluginUi::new(asks));
        let spec = SessionSpec {
            config,
            cwd: cwd.clone(),
            home: app.home.clone(),
            worktree: false,
            answer: None,
            questions: true,
            resume,
            mcp_login: Some(Arc::new(move |url: &str| login.open_url(url))),
            plugin_ui: Some(plugin_ui::serve_ui(
                Arc::clone(&plugins),
                requests,
                answered,
            )),
            client: None,
            surface: "app".into(),
            tools: crate::browser::browser_tools(app.host.browser()),
        };
        let keys: cox_session::Keys = Arc::new(move |section: &str| keys.secret(section));
        let opened = cox_session::open_with_keys(spec, Some(keys)).await?;
        let notices: Vec<String> = opened
            .warnings
            .iter()
            .filter_map(|w| match w {
                cox_session::Warning::Mcp(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        app.record_mcp(&cwd, McpRun::new(tried, &notices));
        let session = opened.session;
        // Review's hunk revert (T51.19): cox-render's diff, which the core cannot reach.
        session.set_hunk_reverter(crate::review::RenderHunks::shared());
        let events = session.events().ok_or(AppError::EventsTaken)?;
        let events = tee(Arc::clone(&app), session.id(), events, Arc::clone(&plugins));
        let claude_home = cox_config::load::home_dir().join(".claude");
        let owner = Arc::clone(&app);
        let mut completer = Completer::load(&cwd, &app.home, &claude_home);
        // After the built-ins and command files, never in place of one.
        completer.extend(plugins.completions());
        let live = Arc::new(Self {
            completer,
            plugins,
            controller: Arc::new(Controller::open(timeline, status, events)),
            warnings: opened.warnings.iter().map(ToString::to_string).collect(),
            turn: Mutex::new(None),
            sandbox: cox_session::sandbox_policy(&opened.config),
            roots: opened.config.core.workspace_roots,
            app,
            driver: Driver::Core(session),
            cwd,
            theme,
        });
        owner.register(&live);
        pump(Arc::downgrade(&live), answers);
        Ok(live)
    }

    /// A session driven by the external agent `agent` (T52.4): its events
    /// go through the same inbox tee, timeline and controller as a cox
    /// session's. No plugin UI, cox commands or terminal policy of its own:
    /// the agent brings its own. `resume` reopens a stored one (T52.6): its
    /// rollout fills the timeline first, as for a cox session.
    pub(crate) async fn open_agent(
        app: Arc<App>,
        cwd: PathBuf,
        agent: &str,
        theme: String,
        resume: Option<SessionId>,
    ) -> Result<Arc<Self>, AppError> {
        let config = app.config(&cwd)?;
        let mut timeline = Timeline::new(&theme);
        let mut status = StatusFold::open(&config);
        let mut turns = 0;
        if let Some(id) = &resume {
            for event in cox_store::Store::open(&app.home)?.rollout_read(id)? {
                if let Event::TurnStarted { seq, .. } = &event {
                    turns = turns.max(*seq);
                }
                timeline.apply(&event);
                status.apply(&event);
            }
        }
        let resume = resume.map(|id| (id, turns));
        let opened = crate::external::open(&app, &config, &cwd, agent, resume).await?;
        // The session's asks carry this id as their source (T52.5).
        let id = opened.id;
        let events = tee(
            Arc::clone(&app),
            id,
            opened.events,
            Arc::new(PluginUi::new(mpsc::channel(1).0)),
        );
        let (asks, _) = mpsc::channel(1);
        let owner = Arc::clone(&app);
        let roots = opened.roots;
        let live = Arc::new(Self {
            completer: Completer::default(),
            plugins: Arc::new(PluginUi::new(asks)),
            controller: Arc::new(Controller::open(timeline, status, events)),
            warnings: Vec::new(),
            turn: Mutex::new(None),
            sandbox: cox_session::agent_policy(&config),
            roots,
            app,
            driver: Driver::Agent {
                id,
                agent: agent.to_string(),
                acp: opened.acp,
            },
            cwd,
            theme,
        });
        owner.register(&live);
        Ok(live)
    }

    pub fn id(&self) -> SessionId {
        match &self.driver {
            Driver::Core(session) => session.id(),
            Driver::Agent { id, .. } => *id,
        }
    }

    /// The external agent driving this session, if one does (T52.4).
    pub fn agent(&self) -> Option<&str> {
        match &self.driver {
            Driver::Core(_) => None,
            Driver::Agent { agent, .. } => Some(agent),
        }
    }

    /// Why this stored agent session opened read-only (T52.6): the agent
    /// cannot `session/load`, or could not start. `None` while it runs,
    /// and for every cox session.
    pub fn read_only(&self) -> Option<&str> {
        match &self.driver {
            Driver::Agent { acp: Err(why), .. } => Some(why),
            _ => None,
        }
    }

    /// Cox's own core; an agent's session has none, so what needs it is
    /// refused by `what`'s name.
    fn core(&self, what: &'static str) -> Result<&Session, AppError> {
        match &self.driver {
            Driver::Core(session) => Ok(session),
            Driver::Agent { agent, .. } => Err(AppError::Unsupported {
                agent: agent.clone(),
                intent: what,
            }),
        }
    }

    /// What was skipped while the session was built (D14).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Every block; the next pull continues from here.
    pub fn snapshot(&self) -> Vec<Block> {
        self.controller.snapshot()
    }

    /// The next batch, at most one per frame; `None` once closed.
    pub async fn next_patches(&self) -> Option<Vec<TimelinePatch>> {
        self.controller.next_patches().await
    }

    /// Returns at once for a turn; a fork or handoff returns its child.
    pub async fn send(&self, intent: Intent) -> Result<Option<Arc<Self>>, AppError> {
        let session = match &self.driver {
            Driver::Core(session) => session,
            Driver::Agent { id, agent, acp } => {
                let acp = acp.as_ref().map_err(String::as_str);
                return crate::external::send(&self.app, *id, agent, acp, intent).map(|()| None);
            }
        };
        if let Intent::Command { line } = &intent {
            // `/<id>:<name>` is the plugin's (PL§8); no built-in has a colon.
            if self.plugins.command(line) {
                return Ok(None);
            }
        }
        let parent = session.id();
        let home = &self.app.home;
        let child = match dispatch(intent)? {
            Dispatch::Submit {
                submission,
                spawn: false,
            } => {
                session.submit(submission).await?;
                return Ok(None);
            }
            Dispatch::Submit { submission, .. } => {
                self.spawn(submission, false);
                return Ok(None);
            }
            Dispatch::Queue(submission) => {
                self.spawn(submission, true);
                return Ok(None);
            }
            Dispatch::Fork { turn } => cox_session::fork(home, &self.cwd, parent, turn)?,
            Dispatch::Handoff { objective } => {
                let summary = session.handoff_summary(&objective).await;
                cox_session::handoff(home, &self.cwd, parent, &objective, summary.as_deref())?
            }
        };
        let (app, cwd) = (Arc::clone(&self.app), self.cwd.clone());
        Self::open(app, cwd, Some(child), self.theme.clone())
            .await
            .map(Some)
    }

    /// What the inspector's Changes tab lists (T37.29.1): the blocks, the
    /// checkpoint rows, the rollout's `write` inputs (T37.29.7) and, through
    /// git, the linked worktree.
    pub async fn changes(&self) -> Result<Changes, AppError> {
        let store = self.app.workspace().store();
        let rows = store.checkpoint_rows(&self.id())?;
        let written = changes::written(&store.rollout_read(&self.id())?);
        let worktree = cox_tools::git::linked(&self.cwd).await;
        Ok(changes::build(
            &self.snapshot(),
            &rows,
            &written,
            &self.canonical_cwd(),
            worktree,
        ))
    }

    /// Review's diff of a Changes-tab `path` (T37.28.2, A101): the net
    /// change from the session's first checkpoint copy to the file on disk,
    /// read the way a checkpoint reads it, confined to the workspace roots.
    /// `None` for a path the session never changed or a side over the cap.
    pub async fn review(&self, path: &str) -> Result<Option<DiffModel>, AppError> {
        let checkpointer = cox_tools::checkpoint::GitCheckpointer::new(self.app.home.clone());
        let paths = [path.to_owned()];
        let Some(now) = checkpointer
            .preimages(&self.roots, &self.cwd, &paths)
            .await
            .pop()
        else {
            return Ok(None);
        };
        let store = self.app.workspace().store();
        let rows = store.checkpoint_rows(&self.id())?;
        let Some(row) = review::base(&rows, &now.path) else {
            return Ok(None);
        };
        let before = review::kept(row, |id| store.archive_get(id))?;
        let cwd = self.canonical_cwd();
        let shown = now.path.strip_prefix(&cwd).unwrap_or(&now.path);
        Ok(review::diff(shown, &before, &now.before, &self.theme))
    }

    /// Checkpoint paths are confined, so canonical; paths are shown
    /// relative to this.
    fn canonical_cwd(&self) -> PathBuf {
        std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone())
    }

    /// What the inspector's Plan tab lists (T37.29.2): the todo list the
    /// latest `todo` call left, as the timeline folded it.
    pub fn plan(&self) -> Vec<TodoItem> {
        self.controller.plan()
    }

    /// What a Tasks-tab click on `task` opens (T37.29.6): read from this
    /// session's rollout and its children in the store.
    pub fn open_task(&self, task: TaskId) -> Result<Option<TaskTarget>, AppError> {
        let store = self.app.workspace().store();
        let events = store.rollout_read(&self.id())?;
        let children = store.children(&self.id())?;
        Ok(tasks::open(&events, task, &children, |child| {
            // A child whose rollout cannot be read is paired with no task.
            store
                .rollout_read(child)
                .ok()
                .and_then(|e| tasks::first_prompt(&e))
        }))
    }

    /// A finished shell's output, as `cox expand <id>` prints it
    /// (T37.22.6): the archived bytes as text, with the escape sequences and
    /// bidi overrides a command could write stripped before the app shows it.
    pub fn output(&self, archive: &ArchiveId) -> Result<String, AppError> {
        let bytes = self.app.workspace().store().archive_get(archive)?;
        Ok(cox_sanitize::sanitize(&String::from_utf8_lossy(&bytes)))
    }

    /// The Context tab's cost history (T37.29.3.2): this session's ledger
    /// rows by turn, with its subagents' rows from their child sessions,
    /// and the project's spend today and this week in local time
    /// (T37.29.3.3). The project is the git checkout the session runs in,
    /// else its folder, as `/sessions` scopes it. `[budget]`'s caps come with
    /// the session's and the month's spend against them (T37.29.3.4).
    pub fn turn_costs(&self) -> Result<TurnCosts, AppError> {
        let store = self.app.workspace().store();
        let own = store.usage_ledger(&self.id())?;
        let children = store.children(&self.id())?;
        let children = children
            .iter()
            .map(|child| store.usage_ledger(child))
            .collect::<Result<Vec<_>, _>>()?;
        let now = chrono::Local::now();
        let month = costs::month_spend(store, &now)?;
        let mut costs = costs::build(&own, &children, &self.app.config(&self.cwd)?.budget, month);
        let root =
            cox_config::load::find_git_root(&self.cwd).unwrap_or_else(|| self.canonical_cwd());
        let (today, week) = costs::periods(&now);
        costs.project = costs::footnote(
            &cox_ext::memory::slug_for(&self.cwd),
            store.project_spend(&root, &today)?,
            store.project_spend(&root, &week)?,
        );
        Ok(costs)
    }

    /// What the inspector's Info tab lists (T37.29.5): the id, cwd and
    /// rollout file, the linked worktree through git, and the config layers
    /// with `cox-config`'s provenance as Settings reads it.
    pub async fn info(&self) -> Result<Info, AppError> {
        let settings = crate::settings::view(&self.app.user_config(), &self.cwd)?;
        let worktree = cox_tools::git::linked(&self.cwd).await;
        let rollout = self.app.workspace().store().rollout_path(&self.id());
        Ok(info::build(
            self.id(),
            &self.cwd,
            worktree,
            &settings,
            rollout,
            &cox_config::load::home_dir(),
        ))
    }

    /// A terminal pane (T51.3): the user's login shell in this session's
    /// cwd, under this session's sandbox policy — the policy `bash` runs
    /// under, bare only when the session chose `danger-full-access`. The
    /// app never opens a session in a worktree of its own, so the writable
    /// roots are the workspace roots, as for its tools. What the shell
    /// prints goes to the pane alone, never into this session's events.
    pub fn open_terminal(&self, cols: u16, rows: u16) -> Result<TerminalHandle, AppError> {
        let env_shell = std::env::var("SHELL").ok();
        let spec = TerminalSpec {
            shell: terminal::shell(env_shell.as_deref())?,
            cwd: self.cwd.clone(),
            policy: self.sandbox.clone(),
            roots: self.roots.clone(),
            writable_roots: self.roots.clone(),
            cols,
            rows,
        };
        Ok(terminal::open(&spec)?)
    }

    /// This session's `AllowForSession` grants, as its core holds them
    /// (T37.45.3), with its title for the Settings row.
    pub async fn grants(&self) -> Vec<SessionGrant> {
        let Driver::Core(core) = &self.driver else {
            return Vec::new();
        };
        let (session, store) = (self.id(), self.app.workspace().store());
        // A session with no ledger row yet has no title to show.
        let title = store.session_info(&session).ok().and_then(|i| i.title);
        core.grants()
            .await
            .into_iter()
            .map(|(tool, subject)| SessionGrant {
                session,
                title: title.clone(),
                tool,
                subject,
            })
            .collect()
    }

    /// Revokes a grant through the core (`Submission::RevokeGrant`), so the
    /// engine asks for the next call it covered.
    pub async fn revoke(&self, tool: &str, subject: &str) -> Result<(), AppError> {
        let revoke = Submission::RevokeGrant {
            tool: tool.to_string(),
            subject: subject.to_string(),
        };
        Ok(self.core("RevokeGrant")?.submit(revoke).await?)
    }

    /// The plugin keys granted in this session (T52.14), for the leader
    /// map and the help list.
    pub fn plugin_keys(&self) -> Vec<PluginKey> {
        self.plugins.keys()
    }

    /// `<leader> <key>`: asks `plugin`'s `cox_key`, whose answer lands like
    /// a command's. False when the key is not granted.
    pub fn plugin_key(&self, plugin: &str, name: &str) -> bool {
        self.plugins.key(plugin, name)
    }

    /// The window is now `width`×`height` cells; a shown panel or overlay
    /// renders again for it (PL§8 "resized").
    pub fn plugin_area(&self, width: u16, height: u16) {
        let slots = self.plugins.step(|s| s.resize(width, height));
        self.patch_slots(slots);
    }

    /// Esc on a plugin overlay.
    pub fn close_plugin_overlay(&self) {
        let slots = self.plugins.step(|s| s.close_overlay());
        self.patch_slots(slots);
    }

    fn patch_slots(&self, slots: Vec<PluginSlot>) {
        for slot in slots {
            let slot = Box::new(slot);
            self.controller.push(TimelinePatch::PluginSlot { slot });
        }
    }

    /// One answer from the serve thread: a slot's render or miss, a redraw,
    /// or a command's closed effect (PL§4).
    async fn on_plugin(&self, Answered { block, answer }: Answered) {
        match answer {
            PluginAnswer::Command { plugin, out } => self.plugin_effect(&plugin, out).await,
            // A miss lands `None`, which leaves the generic card.
            PluginAnswer::ItemRendered { widget, .. } => {
                if let Some(id) = block {
                    self.controller.land(&id, widget.as_ref());
                }
            }
            other => {
                let slots = self.plugins.step(|s| s.fold(other));
                self.patch_slots(slots);
            }
        }
    }

    /// `CommandOut`'s effects, as the TUI runs them (T33.25): a prompt is a
    /// turn queued like the user's own, a compaction is `/compact`'s. A
    /// timeout or an error (`None`) does nothing.
    async fn plugin_effect(&self, plugin: &str, out: Option<CommandOut>) {
        let intent = match out {
            Some(CommandOut::Prompt { text }) => Intent::Send {
                text: sanitize(&text),
                attachments: Vec::new(),
                confirm_think: false,
            },
            Some(CommandOut::Compact { focus }) => Intent::Compact {
                focus: focus.map(|f| sanitize(&f)),
            },
            Some(CommandOut::TogglePanel) => {
                let slots = self.plugins.step(|s| s.toggle_panel(plugin));
                self.patch_slots(slots);
                return;
            }
            Some(CommandOut::OpenOverlay) => {
                let slots = self.plugins.step(|s| s.open_overlay(plugin));
                self.patch_slots(slots);
                return;
            }
            Some(CommandOut::Notice(notice)) => {
                let level = match notice.level {
                    NoticeLevel::Warn => Level::Warn,
                    NoticeLevel::Info => Level::Info,
                };
                // A session that ended has no stream left to tell.
                if let Ok(session) = self.core("Notice") {
                    let _ = session.notice(level, sanitize(&notice.text)).await;
                }
                return;
            }
            Some(CommandOut::Nothing) | None => return,
        };
        match dispatch(intent) {
            Ok(Dispatch::Submit {
                submission,
                spawn: true,
            }) => self.spawn(submission, true),
            // A failed compaction says so in the event stream.
            Ok(Dispatch::Submit { submission, .. }) => {
                if let Ok(session) = self.core("Compact") {
                    let _ = session.submit(submission).await;
                }
            }
            // An empty prompt is dropped; neither intent dispatches otherwise.
            _ => {}
        }
    }

    /// `/` commands and `@` files for the composer's token.
    pub fn complete(&self, token: &str, limit: usize) -> Vec<Completion> {
        self.completer.complete(token, limit)
    }

    /// The command palette's rows: `items` (the window's actions and
    /// sessions) and this session's `/` commands and `@` files, ranked for
    /// `query`, at most `per_kind` of each kind (T37.44.13).
    pub fn palette(
        &self,
        query: &str,
        items: Vec<crate::PaletteItem>,
        per_kind: usize,
    ) -> Vec<crate::PaletteHit> {
        self.completer.palette(query, items, per_kind)
    }

    /// This session's earlier prompts, newest first, for ↑ in an empty
    /// composer (T37.24.6).
    pub fn history(&self, limit: usize) -> Result<Vec<String>, AppError> {
        Ok(self.app.workspace().prompts(self.id(), limit)?)
    }

    /// Stops the pull; the session keeps running (DT§4.5).
    pub fn close(&self) {
        self.controller.close();
    }

    /// Quitting: ends every turn and kills what it detached (T38.2).
    pub fn end(&self) {
        match &self.driver {
            Driver::Core(session) => session.end(),
            Driver::Agent { acp: Ok(acp), .. } => acp.end(),
            Driver::Agent { acp: Err(_), .. } => {}
        }
        self.close();
    }

    /// Spawns a turn (R9.4.3); `queued` waits for the one spawned last and
    /// counts in the status patch until it starts.
    fn spawn(&self, submission: Submission, queued: bool) {
        // Only a core session's intents and plugin commands spawn turns.
        let Driver::Core(session) = &self.driver else {
            return;
        };
        let session = session.clone();
        let controller = queued.then(|| Arc::clone(&self.controller));
        let mut last = self.turn.lock().unwrap_or_else(PoisonError::into_inner);
        let before = last.take().filter(|_| queued);
        if let Some(controller) = &controller {
            controller.enqueue();
        }
        *last = Some(tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            if let Some(controller) = controller {
                controller.dequeue();
            }
            // A turn that fails says so in the event stream.
            let _ = session.submit(submission).await;
        }));
    }
}

/// Feeds the plugin serve thread's answers to `live` until it is gone.
fn pump(live: Weak<LiveSession>, mut answers: mpsc::Receiver<Answered>) {
    tokio::spawn(async move {
        while let Some(answer) = answers.recv().await {
            let Some(live) = live.upgrade() else {
                break;
            };
            live.on_plugin(answer).await;
        }
    });
}

/// The inbox folds every event before the timeline sees it. A closed
/// timeline refuses at once, so the core never waits on a view.
fn tee(
    app: Arc<App>,
    id: SessionId,
    mut from: mpsc::Receiver<Event>,
    plugins: Arc<PluginUi>,
) -> mpsc::Receiver<Event> {
    let (tx, rx) = mpsc::channel(EVENTS);
    tokio::spawn(async move {
        let mut items = ItemWatch::default();
        while let Some(event) = from.recv().await {
            app.apply(id, &event);
            let finished = items.apply(&event);
            // Err only once the view closed; the inbox keeps following.
            let _ = tx.send(event).await;
            // After the send, so the block is in the timeline by the time a
            // plugin's answer names it.
            if let Some((block, target, source)) = finished {
                plugins.render_item(block, &target, source);
            }
        }
        app.expire(id);
    });
    rx
}
