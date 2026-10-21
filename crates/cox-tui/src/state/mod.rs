// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! TEA state for the TUI (T5.1): `State`, `Msg`, `Cmd` and the pure
//! `update`. No async, no I/O, no terminal: the runtime (`app`) feeds it key
//! and core events and executes the `Cmd`s it returns, and a test feeds it
//! the same `Event`s a real session emits, so every screen is replayable.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use cox_protocol::ids::{CallId, ItemId, SessionId, TaskId};
use cox_protocol::plugin::{CommandDecl, CommandOut, NoticeLevel};
use cox_protocol::types::{
    Content, ContextBreakdown, Effort, Level, Mode as SessionMode, PermissionMode, Presence, Role,
    SandboxMode, SlashCommand, Submission, Tier, TodoItem,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::banner::Banner;
use crate::cells::Look;
use crate::color::Depth;
use crate::commands::{self, Action, COMMANDS, Context};
use crate::composer::{Composer, Edit};
use crate::glyph::{self, Glyphs};
use crate::item_render::ItemRender;
use crate::keymap::Keymap;
use crate::markdown;
use crate::modal::{Approval, PluginGrantDialog, Question, RemoveConfirm};
use crate::picker::{self, Kind, Picker};
use crate::pointer::{Interaction, Pointer, pointer};
use crate::tasks;
use crate::term::{Caps, Progress};
use crate::theme::{Theme, ThemeFile};
use crate::theme_editor::ThemeEditor;

mod cell;
mod event;
mod keys;
mod msg;
#[cfg(test)]
mod testkit;

pub use cell::Cell;
pub use keys::ESC_ESC_TICKS;
pub use msg::{
    Ask, Cmd, GrantDecision, Msg, PluginMgmtRequest, PluginNewRequest, PluginRequest, PluginUiMsg,
};

use cell::replay_cells;
use event::on_event;
use keys::{on_key, toggle_fold};

/// What the status line shows; filled from `TurnStarted`/`Usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub model: String,
    pub tier: Option<Tier>,
    pub context_tokens: u32,
    /// What `ctx N%` is a share of: the model's window from the last
    /// `Event::ContextBreakdown` (A98), a 200k guess until one names it.
    pub context_window: u32,
    /// The last request's split (A98), which `/context` draws.
    pub context: Option<ContextBreakdown>,
    pub cost_usd: f64,
    pub sandbox: SandboxMode,
    pub busy: bool,
    /// Last call's cache share 0..=1 (T8.3), shown as `cache N%`.
    pub cache_ratio: f64,
    /// Session spend cap, in USD (T28.1); the binary sets it from
    /// `budget.session_usd`, so `$` names the spend over the cap.
    pub budget_cap_usd: f64,
    /// Fraction of the cap that warns (T28.1); the binary sets it from
    /// `budget.warn_at`, and the cost segment turns `theme.warn` past it.
    pub budget_warn_at: f64,
    /// `/effort` override for the session (T28.1); `SetEffort` keeps it here
    /// next to the mode the composer already shows, and the line badges it.
    pub effort: Option<Effort>,
}

impl Status {
    /// The context `ctx N%` and `/context` count (A98): the last call's
    /// reported context, else the core's estimate for the request about to
    /// go out — the same rule as the desktop meter's share.
    pub fn context_used(&self) -> u32 {
        match (self.context_tokens, self.context) {
            (0, Some(b)) => b.total,
            (n, _) => n,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Approval(Approval),
    /// `ask_user` (T22.1): blocks the turn until a key answers or dismisses it.
    Question(Question),
    /// `NeedsApproval` at session open (T33.8, PL§3); more than one queues
    /// in `pending_grants` below, since this is the same one modal slot.
    PluginGrant(PluginGrantDialog),
    /// `/plugin remove <id> [--keep-data]` (T33.33, PL§1c): confirms an
    /// irreversible action before `Cmd::PluginMgmt(PluginMgmtRequest::Remove)`
    /// reaches the runtime.
    PluginRemove(RemoveConfirm),
    /// `/refine` (T66.8): a proposed change drawn as a diff over the
    /// transcript; `y` applies it, `n` or `Esc` drops it.
    Refine(crate::refine::Review),
    Picker(Picker),
    /// `Ctrl+G` (T15.3): the working tree's `git diff HEAD`, drawn over the
    /// transcript; `scroll` is lines from the top.
    Diff {
        text: String,
        scroll: usize,
    },
    /// `?` on an empty composer (T24.6): `KEYMAP` grouped by context, drawn
    /// over the transcript like the diff view.
    Help,
    /// `/context` (A98): the last request's window and split, drawn over
    /// the transcript like `Help`.
    Context,
    /// `/agents` (T27.5): a navigable list of `agents_rows`, replacing the
    /// static T27.2 `Notice`. `Up`/`Down` move `selected`; `Enter` on a
    /// sibling-session row (`ids[selected].is_some()`) asks the runtime for
    /// its rollout. A task row's id is `None` — no `SessionId` on the wire
    /// yet (plan.md §3 P27) — so `Enter` on it is a no-op.
    Agents {
        rows: Vec<String>,
        ids: Vec<Option<SessionId>>,
        selected: usize,
    },
    /// `Enter` on an `/agents` sibling-session row (T27.5): that session's
    /// rollout, replayed into cells the same pipeline a live turn uses
    /// (`Msg::Rollout`), drawn read-only over the transcript like `Diff`;
    /// `scroll` is lines from the top. `Esc` closes it.
    Transcript {
        cells: Vec<Cell>,
        scroll: usize,
    },
    /// `OpenOverlay` (T33.24, PL§8): `id`'s `overlay` slot, full screen like
    /// `Diff`/`Transcript` above; `Esc` closes it. The widget itself is not
    /// carried here — it stays cached in `plugin_status` like every other
    /// slot's last good render, so a redraw never needs to touch `modal`.
    Plugin {
        id: String,
    },
    /// The theme editor (T46.6): a band like `Picker`, drawn from
    /// `ThemeEditor::lines`; `/theme` opens it and saves it (T46.7).
    ThemeEditor(ThemeEditor),
}

/// Lines (or picker rows) one wheel tick moves (T22.4) — smaller than
/// `DIFF_PAGE` because a tick is a nudge, not a page.
const WHEEL_LINES: usize = 3;

/// What the status line shows of the working tree; a mirror of
/// `cox_tools::git::Status` so this crate keeps no `cox-tools` dependency
/// (T15.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone)]
pub struct State {
    pub transcript: Vec<Cell>,
    pub composer: Composer,
    pub status: Status,
    pub modal: Option<Modal>,
    pub mode: PermissionMode,
    /// `(id, label, tier, started, last_message)`: `tier` and `started`
    /// (`tick` at `TaskCreated`) exist only so `/agents` (T27.2) can show a
    /// running task's tier and elapsed time; `/tasks` still reads just the
    /// label. `last_message` (T34.7) is the sanitized first line of the
    /// most recent `Event::TaskMessage` naming this task, either way — no
    /// new progress event stream, SM§6.
    pub tasks: Vec<(TaskId, String, Tier, u64, Option<String>)>,
    /// Recently finished tasks as `/tasks` lines: exit code and the
    /// `/expand` id of a shell task's output (T27.1).
    pub finished_tasks: Vec<String>,
    /// Lines scrolled up from the bottom of the transcript.
    pub scroll: usize,
    pub banner: Option<Banner>,
    /// Workspace-relative paths the `@` picker offers; the runtime walks them.
    pub files: Vec<String>,
    /// Names `@name task` dispatches (T45.6), from `Session::agent_names`
    /// at start, like `files`; the `@` picker lists them first.
    pub agent_names: Vec<String>,
    /// Local branch names for `git checkout <Tab>` (T15.4); the runtime
    /// lists them at start, like `files`.
    pub git_branches: Vec<String>,
    /// The `/` palette as `(name, usage, description)`: the built-in
    /// `COMMANDS` first, then T22.2's markdown file commands appended by the
    /// runtime. A name beyond `COMMANDS` submits `Submission::Command`.
    pub commands: Vec<(String, String, String)>,
    /// A first idle `Ctrl+C` arms; the second quits.
    pub ctrl_c_armed: bool,
    /// 100 ms ticks since start; spinners and elapsed times read it.
    pub tick: u64,
    /// `Ctrl+T`: thinking cells expanded rather than a one-line count.
    pub show_thinking: bool,
    /// `tui.theme` resolved: dark unless the user chose light.
    pub dark: bool,
    /// `tui.glyphs`/`[tui.icons]` resolved; the binary sets it from config.
    pub glyphs: Glyphs,
    /// `tui.syntax_theme`; empty, or a name syntect does not know, means the
    /// default for `dark`.
    pub syntax_theme: &'static str,
    /// `tui.color` resolved: what the terminal can show, applied to the
    /// finished buffer rather than at each render site.
    pub depth: Depth,
    /// `cox_tui::term::Caps` (T23.0) resolved once at startup; `app.rs`
    /// reads `kitty_keyboard` to push/pop the Kitty keyboard protocol
    /// (T23.1), and later P23 tasks read the rest.
    pub caps: Caps,
    /// The semantic colour tokens (T24.1) every styled span picks from;
    /// `dark`/`light` by `tui.theme`, `mono` when `depth` is `NO_COLOR`.
    pub theme: Theme,
    /// `Ctrl+O`: diffs shown in full rather than as their `+n −m` header.
    pub show_diffs: bool,
    /// `tui.diff` (T24.5); the binary sets it from config.
    pub diff_mode: crate::diff::Mode,
    /// `Ctrl+E` (T24.4) toggles the last tool cell's index into this set; a
    /// click (T22.9) toggles whichever cell it lands on. Presence forces
    /// that cell's output open past its fold; `view.rs` reads it per cell.
    pub expanded: HashSet<usize>,
    /// Screen rows each visible tool card occupies, `(rows, transcript
    /// index)` (T22.9), recorded by `view.rs` on every draw so `on_mouse`
    /// can hit-test a click — interior mutability, so `view` keeps its
    /// `&State` every render call site and test already assumes.
    pub cell_rows: RefCell<Vec<(Range<u16>, usize)>>,
    /// Screen rows the transcript band fills with text after the last draw
    /// (T5.9), recorded by `view.rs` beside `cell_rows`; a mouse move over
    /// one is over text that can be selected, over any other row is not.
    pub text_rows: RefCell<Range<u16>>,
    /// The pointer shape last written through OSC 22 (T5.9), so a move that
    /// lands on the same shape writes nothing.
    pub pointer: Pointer,
    /// The `todo` tool's latest list; `/todo` shows it.
    pub todo: Vec<TodoItem>,
    pub show_todo: bool,
    /// `-v`: show a glyph where `text::sanitize` removed something.
    pub marks: bool,
    /// `tui.motion = reduced` (T24.7); the binary sets it from config.
    pub still: bool,
    /// The OSC 9;4 state last sent (T23.6), so only a change is written.
    pub progress: Progress,
    /// The other live sessions of this project (T16.3); the runtime feeds them.
    pub agents: Vec<Presence>,
    /// This project's recent sessions as `(id, picker row)`, newest first;
    /// the runtime fills them like `files` (T16.5).
    pub sessions: Vec<(String, String)>,
    /// `Ctrl+R`'s other-session prompts as `(picker row, full text)`,
    /// newest first; the runtime fills them like `sessions` (T25.8).
    pub past_prompts: Vec<(String, String)>,
    /// The branch and line counts the runtime polls; `None` outside a
    /// repository, so the line is unchanged there.
    pub git: Option<GitStatus>,
    /// The `--worktree` name (T27.3); the status line shows it after the
    /// branch, and only then.
    pub worktree: Option<String>,
    /// The session's title (A113): generated after the first turn or set
    /// with `/rename`; the runtime seeds a resumed session's from the store.
    pub title: Option<String>,
    /// The `/rewind` timeline (T26.2): one row per user turn, oldest first.
    pub turns: Vec<TurnRow>,
    /// The `seq` of the turn in flight, from `TurnStarted`.
    pub current_seq: u32,
    /// The turn chosen in the rewind picker, awaiting the what-to-restore row.
    pub rewind_to: Option<u32>,
    /// The tick of a first `Esc` on an empty composer; a second within
    /// `ESC_ESC_TICKS` opens the rewind timeline.
    pub esc_armed: Option<u64>,
    /// `/theme` candidates (T24.2), in picker order: every `theme_catalog`
    /// name, then every `syntax_names` entry under a `syntax: ` row prefix.
    /// The runtime builds it once at startup, like `files`.
    pub theme_rows: Vec<String>,
    /// Every colour theme `/theme` can preview or apply, `(name, parsed
    /// file)`; built-ins first, so a user file cannot shadow one.
    pub theme_catalog: Vec<(String, ThemeFile)>,
    /// `.tmTheme` names (T24.2 step 4), leaked once at startup like
    /// `syntax_theme` itself so a picker preview never leaks on a keystroke.
    pub syntax_names: Vec<&'static str>,
    /// `(dark, theme, syntax_theme)` saved when `/theme` opens the picker;
    /// `Esc` restores it, a chosen row drops it.
    pub theme_prev: Option<(bool, Theme, &'static str)>,
    /// Messages typed with `Enter` while a turn runs (T25.1), oldest first;
    /// `view.rs` shows them above the composer and each natural `TurnDone`
    /// pops one into the next turn.
    pub queue: VecDeque<String>,
    /// T25.3: a `!` line submitted and not yet requested; the next `bash`
    /// call is its card.
    pub shell: Option<String>,
    /// The running `!` call; the TUI is busy until it is done.
    pub shell_call: Option<CallId>,
    /// Set when `Ctrl+Enter`/`Alt+Enter` interrupts a running turn to send
    /// now (T25.1); the next `TurnDone{Interrupted}` consumes it and joins
    /// the whole queue into one turn instead of leaving it queued.
    pub send_now: bool,
    /// The keys (T25.5): `KEYMAP` with `~/.cox/keybindings.toml` and Claude
    /// Code's `keybindings.json` over it; the binary loads it.
    pub keymap: Keymap,
    /// `tui.notify` (T23.5); the binary sets it from config.
    pub notify: Notify,
    /// Whether the terminal has focus, from focus reporting (DECSET 1004);
    /// assumed until the terminal says otherwise.
    pub focused: bool,
    /// The session's directory; `link::apply` links file paths under it
    /// (T23.3). Empty until the binary sets it, and then nothing is a file link.
    pub cwd: std::path::PathBuf,
    /// `tui.mouse` (T22.4); the binary sets it from config. `app.rs` reads
    /// this once at startup to decide whether to ask the terminal for mouse
    /// reports at all — off leaves the terminal's own text selection
    /// exactly as if cox never touched the mouse.
    pub mouse: bool,
    /// `/loop` (T27.4), if one is running. Named `active_loop` rather than
    /// the card's literal `loop` — a reserved word.
    pub active_loop: Option<Loop>,
    /// `Modal::PluginGrant` dialogs still waiting (T33.8): `on_key` pops the
    /// next one into `modal` once the current one is decided (`y`/`n`).
    pub pending_grants: VecDeque<PluginGrantDialog>,
    /// Plugin `status.left`/`status.right` segments (T33.23, PL§8), in
    /// declaration order, each with its last good render. `view` only ever
    /// reads these; `status::on_plugin` fills them from `Msg::Plugin`.
    pub plugin_status: Vec<crate::status::PluginSegment>,
    /// `plugin.leader` was just pressed (T33.25, PL§8): the next key, match
    /// or not, goes to `Keymap::resolve_plugin_key` instead of anywhere
    /// else — `<leader> <key>` is the only way a plugin key fires.
    pub plugin_leader_armed: bool,
    /// `TogglePanel`'s open plugin, if any (T33.24, PL§8): at most one
    /// panel is shown at a time, the same one-band budget `todo_area`
    /// already spends above the composer.
    pub plugin_panel_open: Option<String>,
    /// The last `Msg::Resize` (columns, rows), defaulted so a `panel`/
    /// `overlay` render has an area before the terminal ever resizes
    /// (T33.24, PL§8: "the render request carries the area size").
    pub term: (u16, u16),
    /// Plugin renderer targets for finished cells, `(plugin, target)` in
    /// declaration order (T33.26, PL§8); the first match wins.
    pub plugin_renderers: Vec<(String, String)>,
    /// `/plugin new <name> [--with ...]` awaiting the language picker's
    /// choice (T33.30), the same two-step shape `rewind_to` above uses for
    /// `/rewind`'s turn-then-what picks.
    pub pending_plugin_new: Option<(String, Vec<String>)>,
    /// The session's mode (P42) as the core last reported it; `mode`
    /// above stays the permission mode.
    pub session_mode: SessionMode,
    /// The user accepted the think price for this architect stretch, so
    /// every turn carries `confirm_think` until the mode leaves architect.
    pub think_confirmed: bool,
    /// The open think-price question's id: its answer stays in the TUI
    /// instead of reaching the core as an `ask_user` reply.
    think_consent: Option<CallId>,
    /// `[tui.status_line]` (T46.3): the user's status command, its last
    /// input and answer; the runtime sets it at startup when a command is
    /// configured. `None` leaves the screen exactly as without the key.
    pub status_script: Option<crate::status::StatusScript>,
    /// Push-to-talk (T54.6): whether a `Dictation` exists, `[voice]
    /// auto_submit`, and the recording in progress.
    pub voice: crate::voice::Voice,
}

/// `/loop`'s running state (T27.4). `interval_ticks`/`next_at` are
/// `State::tick` units (100 ms each — the same clock `cells.rs` already
/// drives elapsed time and spinners from) rather than the wall clock, so a
/// replayed `Msg::Tick` stream behaves identically in a test and for real.
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    pub prompt: String,
    pub interval_ticks: u64,
    pub next_at: u64,
    /// This loop's own spend cap (`/loop`'s `--budget`, default: the
    /// session cap `status.budget_cap_usd`); `budget.session_usd` still
    /// applies underneath, enforced by the core as always.
    pub budget_usd: f64,
    /// `status.cost_usd` when the loop started, so its own cap tracks only
    /// what the loop itself has spent, not the whole session.
    pub started_cost_usd: f64,
    pub iterations: u32,
}

/// `tui.notify` (T23.5): when a finished turn, an approval or a question
/// rings the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    /// Only while the terminal is unfocused.
    Auto,
    Always,
    Off,
}

impl Notify {
    /// An unknown value is `Auto`, like `diff::Mode::parse`.
    pub fn parse(s: &str) -> Notify {
        match s {
            "always" => Notify::Always,
            "off" => Notify::Off,
            _ => Notify::Auto,
        }
    }
}

/// One user turn as the rewind timeline shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRow {
    pub seq: u32,
    /// The user's text.
    pub text: String,
    /// Files checkpointed during the turn.
    pub files: usize,
    /// Where the turn's user cell sits in `transcript`, so a conversation
    /// rewind cuts there.
    pub cell_at: usize,
}

impl State {
    pub fn new(mode: PermissionMode, sandbox: SandboxMode) -> Self {
        Self {
            transcript: Vec::new(),
            composer: Composer::new(),
            status: Status {
                model: String::new(),
                tier: None,
                context_tokens: 0,
                context_window: 200_000,
                context: None,
                cost_usd: 0.0,
                sandbox,
                busy: false,
                cache_ratio: 0.0,
                budget_cap_usd: 5.0,
                budget_warn_at: 0.8,
                effort: None,
            },
            modal: None,
            mode,
            tasks: Vec::new(),
            finished_tasks: Vec::new(),
            scroll: 0,
            banner: None,
            files: Vec::new(),
            agent_names: Vec::new(),
            git_branches: Vec::new(),
            commands: COMMANDS
                .iter()
                .map(|(n, u, d)| (n.to_string(), u.to_string(), d.to_string()))
                .collect(),
            ctrl_c_armed: false,
            turns: Vec::new(),
            current_seq: 0,
            rewind_to: None,
            esc_armed: None,
            tick: 0,
            show_thinking: false,
            dark: true,
            glyphs: glyph::UNICODE,
            syntax_theme: "",
            depth: Depth::True,
            caps: Caps::default(),
            theme: Theme::dark(),
            show_diffs: true,
            diff_mode: crate::diff::Mode::Auto,
            expanded: HashSet::new(),
            cell_rows: RefCell::new(Vec::new()),
            text_rows: RefCell::new(0..0),
            pointer: Pointer::Default,
            todo: Vec::new(),
            show_todo: false,
            marks: false,
            still: false,
            progress: Progress::Idle,
            agents: Vec::new(),
            sessions: Vec::new(),
            past_prompts: Vec::new(),
            git: None,
            worktree: None,
            title: None,
            theme_rows: Vec::new(),
            theme_catalog: Vec::new(),
            syntax_names: Vec::new(),
            theme_prev: None,
            queue: VecDeque::new(),
            shell: None,
            shell_call: None,
            send_now: false,
            keymap: Keymap::default(),
            notify: Notify::Auto,
            cwd: std::path::PathBuf::new(),
            focused: true,
            mouse: true,
            active_loop: None,
            pending_grants: VecDeque::new(),
            plugin_status: Vec::new(),
            plugin_leader_armed: false,
            plugin_panel_open: None,
            term: (80, 24),
            plugin_renderers: Vec::new(),
            pending_plugin_new: None,
            session_mode: SessionMode::Editor,
            think_confirmed: false,
            think_consent: None,
            status_script: None,
            voice: crate::voice::Voice::default(),
        }
    }

    /// Seeds the transcript from reconstructed session history so resume is not blank.
    pub fn transcript_from_history(&mut self, history: &cox_core::History) {
        for (message_index, message) in history.messages.iter().enumerate() {
            match message.role {
                Role::User => {
                    let text = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        if let Some(mark) = history
                            .turn_marks
                            .iter()
                            .find(|mark| mark.message_index == message_index)
                        {
                            self.turns.push(TurnRow {
                                seq: mark.seq,
                                text: text.clone(),
                                files: mark.checkpoints,
                                cell_at: self.transcript.len(),
                            });
                        }
                        self.transcript.push(Cell::User {
                            text,
                            attachments: vec![],
                        });
                    }
                }
                Role::Assistant => {
                    for block in &message.content {
                        match block {
                            Content::Text { text } => {
                                self.transcript.push(Cell::Assistant {
                                    item: ItemId::new(),
                                    text: text.clone(),
                                    done: true,
                                    render: ItemRender::Builtin,
                                });
                            }
                            Content::Thinking { text, .. } => {
                                self.transcript.push(Cell::Thinking {
                                    item: ItemId::new(),
                                    text: text.clone(),
                                    done: true,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.current_seq = history.turns;
    }

    /// What `cells::cell_lines` needs for a `width`-column render.
    pub fn look(&self, width: u16) -> Look {
        Look {
            width,
            theme: markdown::theme_name(self.dark, self.syntax_theme),
            glyphs: self.glyphs,
            show_thinking: self.show_thinking,
            show_diffs: self.show_diffs,
            diff: self.diff_mode,
            tick: self.tick,
            still: self.still,
            marks: self.marks,
            colors: self.theme,
            // The generic look shared by a whole render pass does not know
            // which cell is last; `view.rs` overrides it for that one index.
            expand_last: None,
        }
    }

    /// Which `KEYMAP` context the keys are in right now (T24.6).
    pub fn context(&self) -> Context {
        match &self.modal {
            Some(
                Modal::Diff { .. }
                | Modal::Refine(_)
                | Modal::Help
                | Modal::Context
                | Modal::Agents { .. }
                | Modal::Transcript { .. }
                | Modal::Plugin { .. },
            ) => Context::Overlay,
            Some(_) => Context::Modal,
            None if self.status.busy => Context::Running,
            None => Context::Idle,
        }
    }

    /// Finished cells at the head of the transcript, removed so the runtime
    /// can push them into scrollback in order; a streaming cell holds
    /// everything behind it in the viewport.
    pub fn take_finished(&mut self) -> Vec<Cell> {
        let n = self.transcript.iter().take_while(|c| c.done()).count();
        for turn in &mut self.turns {
            turn.cell_at = turn.cell_at.saturating_sub(n);
        }
        self.transcript.drain(..n).collect()
    }

    /// The newest `bash` or `agent` call still waiting for its result.
    fn detachable_call(&self) -> Option<CallId> {
        self.transcript.iter().rev().find_map(|c| match c {
            Cell::Tool {
                call, result: None, ..
            } if matches!(call.name.as_str(), "bash" | "agent") => Some(call.id),
            _ => None,
        })
    }

    fn tool_mut(&mut self, id: CallId) -> Option<&mut Cell> {
        self.transcript
            .iter_mut()
            .rev()
            .find(|c| matches!(c, Cell::Tool { call, .. } if call.id == id))
    }

    fn item_mut(&mut self, id: ItemId) -> Option<&mut Cell> {
        self.transcript.iter_mut().rev().find(|c| {
            matches!(c, Cell::Assistant { item, .. } | Cell::Thinking { item, .. } if *item == id)
        })
    }
}

pub fn update(state: &mut State, msg: Msg) -> Vec<Cmd> {
    let mut cmds = step(state, msg);
    cmds.extend(progress(state));
    cmds.extend(crate::status::script_ask(state));
    cmds
}

/// T23.6: the tab's progress follows what `state` now shows, sent only
/// when it changes and only to a terminal that draws it.
fn progress(state: &mut State) -> Option<Cmd> {
    if !state.caps.osc9_4 {
        return None;
    }
    let want = match (state.status.busy, &state.modal) {
        (false, _) => Progress::Idle,
        (
            true,
            Some(
                Modal::Approval(_)
                | Modal::Question(_)
                | Modal::PluginGrant(_)
                | Modal::PluginRemove(_),
            ),
        ) => Progress::Paused,
        (true, _) => Progress::Busy,
    };
    (want != state.progress).then(|| {
        state.progress = want;
        Cmd::Progress(want)
    })
}

fn step(state: &mut State, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => on_key(state, key),
        Msg::Paste(text) => {
            state.composer.insert(&text);
            Vec::new()
        }
        Msg::Event(ev) => on_event(state, ev),
        Msg::Tick => {
            state.tick += 1;
            crate::item_render::expire(state);
            loop_tick(state)
        }
        // PL§8: a resize is one of the three times a plugin renders; T33.24
        // keeps the new size so a later `panel`/`overlay` render (opened by
        // a command, not this resize) still asks for the right area.
        Msg::Resize(w, h) => {
            state.term = (w, h);
            crate::status::render_requests(state, None)
        }
        Msg::Plugin(PluginUiMsg::ItemRendered { cell, widget }) => {
            crate::item_render::on_rendered(state, cell, widget);
            Vec::new()
        }
        // T33.25, PL§8: a `Command` answer applies `CommandOut` here, not
        // in `status`, which only ever folds slots; everything else
        // (`Declare`'s slots included) still goes through `on_plugin`.
        Msg::Plugin(PluginUiMsg::Command { plugin, out }) => {
            plugin_command_out(state, &plugin, out)
        }
        Msg::Plugin(msg) => {
            if let PluginUiMsg::Declare {
                plugin,
                commands,
                keys,
                renderers,
                ..
            } = &msg
            {
                declare_plugin_commands(state, plugin, commands);
                state.keymap.declare_plugin_keys(plugin, keys);
                crate::item_render::declare(state, plugin, renderers);
            }
            crate::status::on_plugin(state, msg)
        }
        Msg::Agents(agents) => {
            state.agents = agents;
            Vec::new()
        }
        Msg::Git(git) => {
            state.git = git;
            Vec::new()
        }
        // git's output is a tool's: sanitised once, here at the boundary.
        Msg::Diff(text) => {
            let text = crate::text::sanitize(&text.unwrap_or_default());
            state.modal = Some(Modal::Diff { text, scroll: 0 });
            Vec::new()
        }
        Msg::Refine(msg) => {
            match msg {
                crate::refine::Msg::Review { evidence, diff } => {
                    state.modal = Some(Modal::Refine(crate::refine::Review::new(&evidence, &diff)));
                }
                crate::refine::Msg::Notice(Ok(text)) => notice(state, Level::Info, text),
                crate::refine::Msg::Notice(Err(text)) => notice(state, Level::Warn, text),
            }
            Vec::new()
        }
        Msg::Focus(focused) => {
            state.focused = focused;
            Vec::new()
        }
        Msg::Mouse(ev) => on_mouse(state, ev),
        Msg::Rollout(events) => {
            state.modal = Some(Modal::Transcript {
                cells: replay_cells(events),
                scroll: 0,
            });
            Vec::new()
        }
        Msg::PluginMgmt(result) => {
            let (level, text) = match result {
                Ok(text) => (Level::Info, text),
                Err(text) => (Level::Warn, text),
            };
            notice(state, level, text);
            Vec::new()
        }
        // Only the script's own row: the built-in segments never change.
        Msg::StatusLine(line) => {
            if let Some(script) = &mut state.status_script {
                script.line = line;
            }
            Vec::new()
        }
        Msg::ThemeSaved(stem, file) => {
            match state.theme_catalog.iter_mut().find(|(n, _)| *n == stem) {
                Some(entry) => entry.1 = file,
                None => {
                    // After the last colour theme, before the `syntax: ` rows.
                    let at = state
                        .theme_rows
                        .iter()
                        .position(|r| r.starts_with("syntax: "))
                        .unwrap_or(state.theme_rows.len());
                    state.theme_rows.insert(at, stem.clone());
                    state.theme_catalog.push((stem.clone(), file));
                }
            }
            apply_theme_choice(state, &stem)
        }
        Msg::Voice(msg) => crate::voice::on_msg(state, msg),
    }
}

/// A wheel tick (T22.4) or a left click (T22.9): a click only acts with no
/// modal open, like the wheel's own `None` arm below, and only when it
/// lands inside `cell_rows` — `view.rs`'s record of what is actually on
/// screen after the last draw — so anywhere else does nothing.
fn on_mouse(state: &mut State, ev: MouseEvent) -> Vec<Cmd> {
    if matches!(ev.kind, MouseEventKind::Moved) {
        return on_move(state, ev.row);
    }
    if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
        if state.modal.is_none() {
            let hit = state
                .cell_rows
                .borrow()
                .iter()
                .find(|(rows, _)| rows.contains(&ev.row))
                .map(|&(_, i)| i);
            if let Some(i) = hit {
                return toggle_fold(state, i);
            }
        }
        return Vec::new();
    }
    // The wheel reuses whichever scroll path the same context's keyboard
    // already has, moving it `WHEEL_LINES` at a time; the plain transcript
    // had none (`state.scroll` was dead until T22.4), so wheel is its first.
    let up = match ev.kind {
        MouseEventKind::ScrollUp => true,
        MouseEventKind::ScrollDown => false,
        _ => return Vec::new(),
    };
    match &mut state.modal {
        Some(Modal::Picker(picker)) => {
            let code = if up { KeyCode::Up } else { KeyCode::Down };
            for _ in 0..WHEEL_LINES {
                picker.key(KeyEvent::new(code, KeyModifiers::NONE));
            }
        }
        Some(Modal::Refine(review)) => review.wheel(up, WHEEL_LINES),
        Some(Modal::Diff { scroll, text }) => {
            *scroll = if up {
                scroll.saturating_sub(WHEEL_LINES)
            } else {
                (*scroll + WHEEL_LINES).min(text.lines().count().saturating_sub(1))
            };
        }
        Some(
            Modal::Approval(_)
            | Modal::Question(_)
            | Modal::PluginGrant(_)
            | Modal::PluginRemove(_)
            | Modal::Help
            | Modal::Context
            | Modal::Agents { .. }
            | Modal::Transcript { .. }
            | Modal::Plugin { .. }
            | Modal::ThemeEditor(_),
        ) => {}
        None => {
            state.scroll = if up {
                state.scroll.saturating_add(WHEEL_LINES)
            } else {
                state.scroll.saturating_sub(WHEEL_LINES)
            };
        }
    }
    Vec::new()
}

/// A mouse move (T5.9): the pointer shape for what is under `row`, decided
/// by `cox_render::pointer` from the same two facts `on_mouse` acts on — a
/// row in `cell_rows` is where a click folds a card (only with no modal
/// open), a row in `text_rows` is transcript text. No rule of its own: this
/// only builds the `Interaction`. Silent without OSC 22 or with the mouse
/// off, so a terminal that cannot draw it is never sent the bytes.
fn on_move(state: &mut State, row: u16) -> Vec<Cmd> {
    if !(state.mouse && state.caps.osc22) {
        return Vec::new();
    }
    let clickable = state.modal.is_none()
        && state
            .cell_rows
            .borrow()
            .iter()
            .any(|(rows, _)| rows.contains(&row));
    let want = pointer(Interaction {
        clickable,
        selectable: state.text_rows.borrow().contains(&row),
        ..Interaction::default()
    });
    if want == state.pointer {
        return Vec::new();
    }
    state.pointer = want;
    vec![Cmd::Pointer(want)]
}

/// A key for the composer, and what its `Edit` means for the session.
pub(crate) fn compose(state: &mut State, key: KeyEvent) -> Vec<Cmd> {
    match state.composer.key(key, state.status.busy) {
        Edit::Submit(text) => {
            if let Some((name, task)) = agent_line(&state.agent_names, &text) {
                return user_agent(state, name, task);
            }
            let tier = state.status.tier.unwrap_or(Tier::Code);
            // T22.2: a file command's name reaches the core as
            // `Submission::Command`; the T5.5 parser owns the
            // built-ins and would answer these with a notice.
            // T33.25, PL§8: a `/<id>:<name>` line calls the plugin instead.
            match file_command(&state.commands, &text)
                .or_else(|| plugin_command(&state.commands, &text))
                .or_else(|| commands::parse(&text, tier))
            {
                Some(action) => act(state, action),
                // A turn is running: queue instead of submitting
                // (T25.1). A slash command still runs immediately
                // above — `/clear` in particular must reach the
                // queue it is about to empty.
                None if state.status.busy => {
                    state.queue.push_back(text);
                    Vec::new()
                }
                None => vec![Cmd::Submit(user_turn(state, text))],
            }
        }
        Edit::SendNow(text) => send_now(state, text),
        Edit::OpenFiles => {
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::Files,
                picker::at_candidates(&state.agent_names, &state.files),
            )));
            Vec::new()
        }
        Edit::OpenCommands => {
            let names = state.commands.iter().map(|(n, ..)| n.clone()).collect();
            state.modal = Some(Modal::Picker(Picker::open(Kind::Commands, names)));
            Vec::new()
        }
        Edit::OpenHistory => {
            // Newest first: the entry wanted is usually the last one.
            let mut history = state.composer.history().to_vec();
            history.reverse();
            // T25.8: this project's other sessions after this one's own.
            history.extend(state.past_prompts.iter().map(|(row, _)| row.clone()));
            state.modal = Some(Modal::Picker(Picker::open(Kind::History, history)));
            Vec::new()
        }
        Edit::Nothing => Vec::new(),
    }
}

fn set_mode(state: &mut State, mode: PermissionMode) -> Vec<Cmd> {
    state.mode = mode;
    vec![Cmd::Submit(Submission::SetPermissionMode { mode })]
}

/// `Ctrl+Enter`/`Alt+Enter` while a turn runs (T25.1): the composer already
/// cleared itself (`Edit::SendNow`); its text joins the queue's tail so the
/// interrupt this triggers, once `TurnDone{Interrupted}` acknowledges it,
/// flushes everything typed so far as one turn instead of leaving it queued
/// for the next natural finish.
/// `@name task` at line start with a dispatchable `name` and a task
/// (T45.6); anything else — an `@file` mention included — is a normal turn.
fn agent_line(names: &[String], text: &str) -> Option<(String, String)> {
    let (name, task) = text.strip_prefix('@')?.split_once(' ')?;
    let task = task.trim();
    (names.iter().any(|n| n == name) && !task.is_empty())
        .then(|| (name.to_string(), task.to_string()))
}

/// Submits `@name task`; mid-turn it is refused here, as `!` is, since the
/// core would refuse it anyway.
fn user_agent(state: &mut State, name: String, task: String) -> Vec<Cmd> {
    if state.status.busy {
        let text = format!("a turn is running; `@{name}` waits until it ends");
        notice(state, Level::Warn, text);
        return Vec::new();
    }
    vec![Cmd::Submit(Submission::UserAgent { name, task })]
}

fn send_now(state: &mut State, text: String) -> Vec<Cmd> {
    if !text.trim().is_empty() {
        state.queue.push_back(text);
    }
    state.send_now = true;
    vec![Cmd::Submit(Submission::Interrupt)]
}

/// T24.2: applies one `/theme` row to `State` without persisting it — the
/// live preview `Esc` (via `theme_prev`) can still undo. A `syntax: <name>`
/// row only ever touches `syntax_theme`; a bare name looks it up in
/// `theme_catalog` and follows the current background unless the file
/// itself pins one.
fn apply_row(state: &mut State, row: &str) {
    if let Some(name) = row.strip_prefix("syntax: ") {
        if let Some(&s) = state.syntax_names.iter().find(|n| **n == name) {
            state.syntax_theme = s;
        }
        return;
    }
    if let Some((_, file)) = state.theme_catalog.iter().find(|(n, _)| n == row) {
        let dark = file.variant.unwrap_or(state.dark);
        state.dark = dark;
        state.theme = file.theme(dark);
    }
}

/// `Esc` in the `/theme` picker or the theme editor: whatever was drawn
/// before `/theme` opened.
fn restore_theme(state: &mut State) {
    if let Some((dark, theme, syntax_theme)) = state.theme_prev.take() {
        state.dark = dark;
        state.theme = theme;
        state.syntax_theme = syntax_theme;
    }
}

/// `Ctrl+E` on a `/theme` colour row (T46.7) swaps the picker for the
/// editor on that theme; a `syntax: ` row has no tokens, so it stays put.
/// `theme_prev` was saved when the picker opened and is kept, so the
/// editor's `Esc` restores the same state the picker's would.
fn open_theme_editor(state: &mut State) {
    let Some(Modal::Picker(picker)) = &state.modal else {
        return;
    };
    let Some(row) = picker.matches.get(picker.selected) else {
        return;
    };
    let Some((name, file)) = state.theme_catalog.iter().find(|(n, _)| n == row).cloned() else {
        return;
    };
    let builtin = crate::theme::builtin_source(&name).is_some();
    let dark = file.variant.unwrap_or(state.dark);
    let editor = ThemeEditor::open(name, builtin, dark, file);
    if state.theme_prev.is_none() {
        state.theme_prev = Some((state.dark, state.theme, state.syntax_theme));
    }
    state.dark = dark;
    state.theme = editor.theme();
    state.modal = Some(Modal::ThemeEditor(editor));
}

/// Every key that moves the `/theme` picker's selection previews that row.
fn preview_theme(state: &mut State, picker: &Picker) {
    if let Some(row) = picker.matches.get(picker.selected).cloned() {
        apply_row(state, &row);
    }
}

/// `Enter` on a `/theme` row: applies it (in case `Enter` came before any
/// navigation ever previewed it) and persists it with `cox config set`
/// semantics — `Cmd::PersistConfig` carries the write to the runtime, which
/// alone has `config_cmd::set`.
fn apply_theme_choice(state: &mut State, row: &str) -> Vec<Cmd> {
    apply_row(state, row);
    let key = if row.starts_with("syntax: ") {
        "tui.syntax_theme"
    } else {
        "tui.theme"
    };
    let value = row.strip_prefix("syntax: ").unwrap_or(row).to_string();
    vec![Cmd::PersistConfig {
        key: key.into(),
        value,
    }]
}

fn notice(state: &mut State, level: Level, text: String) {
    state.transcript.push(Cell::Notice { level, text });
}

/// `/rewind` and `Esc Esc`: the timeline, newest first.
fn open_rewind(state: &mut State) -> Vec<Cmd> {
    if state.status.busy {
        notice(
            state,
            Level::Warn,
            "rewind: interrupt the turn first".into(),
        );
        return Vec::new();
    }
    if state.turns.is_empty() {
        notice(state, Level::Info, "nothing to rewind yet".into());
        return Vec::new();
    }
    let rows = state
        .turns
        .iter()
        .rev()
        .map(|t| picker::turn_entry(t.seq, t.files, &t.text))
        .collect();
    state.modal = Some(Modal::Picker(Picker::open(Kind::Rewind, rows)));
    Vec::new()
}

/// `/agents` (T27.2, one row per T27.5): one row per live agent — a
/// sibling cox session (T16.1 presence) or a subagent/background task this
/// session started (`state.tasks`, fed by `TaskCreated`/`TaskCompleted`).
/// The narrow row the creator chose over a new `Event::AgentProgress`:
/// name, preset, tier, cost, elapsed, state. Presence carries none of
/// preset/tier/cost/elapsed, so those show `-`; a task's cost shows `-`
/// too while it runs — it is only known once `TaskCompleted` retires it
/// from `state.tasks`. One line each (T27.5: a `Modal::Agents` row cannot
/// span lines, unlike T27.2's two-line `Notice` card), paired with the
/// `SessionId` `Enter` fetches a rollout for — `None` for a task, which has
/// no resumable id on the wire yet (plan.md §3 P27).
fn agents_rows(
    agents: &[Presence],
    tasks: &[(TaskId, String, Tier, u64, Option<String>)],
    tick: u64,
    worktree_glyph: &str,
) -> Vec<(String, Option<SessionId>)> {
    let mut rows: Vec<(String, Option<SessionId>)> = agents
        .iter()
        .map(|a| {
            let mut text = format!(
                "{} · preset - · tier - · cost - · elapsed - · {}",
                a.session,
                a.status.name()
            );
            // T44.3: which worktree the session holds, by its directory name
            // (the full path is the project's `_worktrees/` prefix again),
            // behind the status line's worktree glyph.
            if let Some(name) = a.worktree.as_deref().and_then(std::path::Path::file_name) {
                text.push_str(&format!(" · {worktree_glyph} {}", name.to_string_lossy()));
            }
            (crate::text::sanitize(&text), Some(a.session))
        })
        .collect();
    rows.extend(tasks.iter().map(|(_, label, tier, started, last)| {
        let preset = label.split_once(": ").map_or("-", |(p, _)| p);
        let elapsed = tick.saturating_sub(*started);
        let mut text = format!(
            "{label} · preset {preset} · tier {} · cost - · elapsed {}.{}s · running",
            format!("{tier:?}").to_lowercase(),
            elapsed / 10,
            elapsed % 10
        );
        // T34.7/SM§6: the narrow card grows a last-message line instead of
        // a new progress event stream — already sanitized in `update`, but
        // re-run through `sanitize` with the rest of the row, same as the
        // label above (`crate::text::sanitize` is idempotent).
        if let Some(last) = last {
            text.push_str(&format!(" · last: {last}"));
        }
        (crate::text::sanitize(&text), None)
    }));
    rows
}

/// T22.2: a `/name args` line naming a file command — something
/// `State.commands` carries beyond the built-in `COMMANDS`, which the T5.5
/// parser owns — submits `Submission::Command` for the core, the same shape
/// the parser already produces for built-ins without a dedicated arm. A
/// colon-bearing name is a plugin command (T33.25) instead, never one of
/// these: no built-in or file command name has ever contained one.
fn file_command(commands: &[(String, String, String)], line: &str) -> Option<Action> {
    let mut words = line.strip_prefix('/')?.split_whitespace();
    let name = words.next()?;
    if COMMANDS.iter().any(|(n, ..)| *n == name) || name.contains(':') {
        return None;
    }
    commands.iter().any(|(n, ..)| n == name).then(|| {
        Action::Submit(Submission::Command {
            command: SlashCommand {
                name: name.to_string(),
                args: words.map(str::to_string).collect(),
            },
        })
    })
}

/// T33.25, PL§8: a `/<id>:<name>` line naming a plugin command — the colon
/// is what tells it apart from `file_command`'s names. Guarded the same
/// way against `COMMANDS` so a built-in still wins even over a
/// mis-registered plugin command that dropped its `<id>:` prefix.
fn plugin_command(commands: &[(String, String, String)], line: &str) -> Option<Action> {
    let mut words = line.strip_prefix('/')?.split_whitespace();
    let full = words.next()?;
    if COMMANDS.iter().any(|(n, ..)| *n == full) {
        return None;
    }
    let (plugin, name) = full.split_once(':')?;
    commands
        .iter()
        .any(|(n, ..)| n == full)
        .then(|| Action::PluginCommand {
            plugin: plugin.to_string(),
            name: name.to_string(),
            args: words.collect::<Vec<_>>().join(" "),
        })
}

/// T33.25, PL§8: a plugin's declared commands join `State.commands` after
/// the built-ins and any file commands, as `/<id>:<name>` — appended once,
/// a re-`Declare` never duplicates a name it already added.
fn declare_plugin_commands(state: &mut State, plugin: &str, commands: &[CommandDecl]) {
    for c in commands {
        let full = format!("{plugin}:{}", crate::text::sanitize(&c.name));
        if state.commands.iter().any(|(n, ..)| *n == full) {
            continue;
        }
        state.commands.push((
            full.clone(),
            format!("/{full}"),
            crate::text::sanitize(&c.description),
        ));
    }
}

/// T33.25, PL§8: `CommandOut`'s closed effects for a `cox_command`/
/// `cox_key` answer. `None` — a timeout, an error or a missing export —
/// fails open like a missed render: nothing happens, nothing is shown.
fn plugin_command_out(state: &mut State, plugin: &str, out: Option<CommandOut>) -> Vec<Cmd> {
    match out {
        Some(CommandOut::Prompt { text }) => {
            vec![Cmd::Submit(user_turn(state, crate::text::sanitize(&text)))]
        }
        Some(CommandOut::Compact { focus }) => vec![Cmd::Submit(Submission::Compact {
            focus: focus.map(|f| crate::text::sanitize(&f)),
        })],
        // T33.24, PL§8: opening either slot is a "slot became visible"
        // moment, one of the three times a plugin renders — `render_requests`
        // itself decides whether `plugin` actually has that slot to render.
        Some(CommandOut::TogglePanel) => {
            state.plugin_panel_open = if state.plugin_panel_open.as_deref() == Some(plugin) {
                None
            } else {
                Some(plugin.to_string())
            };
            crate::status::render_requests(state, Some(plugin))
        }
        Some(CommandOut::OpenOverlay) => {
            state.modal = Some(Modal::Plugin {
                id: plugin.to_string(),
            });
            crate::status::render_requests(state, Some(plugin))
        }
        Some(CommandOut::Notice(n)) => {
            let level = match n.level {
                NoticeLevel::Warn => Level::Warn,
                NoticeLevel::Info => Level::Info,
            };
            notice(state, level, crate::text::sanitize(&n.text));
            Vec::new()
        }
        // PL§15.6 case 5: the inspector only exists in the desktop app.
        Some(CommandOut::OpenInspector) => {
            notice(
                state,
                Level::Info,
                format!("plugin {plugin}: the inspector is desktop-only"),
            );
            Vec::new()
        }
        Some(CommandOut::Nothing) | None => Vec::new(),
    }
}

/// A slash command's effect; anything the core owns becomes a `Submit`.
fn act(state: &mut State, action: Action) -> Vec<Cmd> {
    match action {
        Action::Submit(sub) => {
            if let Submission::SetEffort { effort } = &sub {
                state.status.effort = *effort;
            }
            if let Submission::Command { command } = &sub
                && command.name == "clear"
            {
                state.queue.clear();
                // T27.4: a fresh session should not keep firing an old
                // loop's prompt into it.
                state.active_loop = None;
                return vec![Cmd::Clear];
            }
            return vec![Cmd::Submit(sub)];
        }
        Action::Quit => return vec![Cmd::Quit],
        Action::Mode(mode) => return set_mode(state, mode),
        Action::Help => {
            let text = commands::help(&state.keymap);
            notice(state, Level::Info, text);
        }
        Action::Context => state.modal = Some(Modal::Context),
        // The proposal is a model call, so say so before the review opens.
        Action::Refine(request) => {
            if matches!(request, crate::refine::Request::Propose { .. }) {
                notice(
                    state,
                    Level::Info,
                    "refine: asking the cheap tier for a proposal".into(),
                );
            }
            return vec![Cmd::Ask(Ask::Refine(request))];
        }
        Action::Cost => {
            let s = &state.status;
            let text = format!(
                "${:.2} this session · {} tokens in context",
                s.cost_usd, s.context_tokens
            );
            notice(state, Level::Info, text);
        }
        Action::Todo => state.show_todo = !state.show_todo,
        Action::Tasks => {
            let running: Vec<(TaskId, String)> = state
                .tasks
                .iter()
                .map(|(id, label, ..)| (*id, label.clone()))
                .collect();
            notice(
                state,
                Level::Info,
                tasks::list(&running, &state.finished_tasks),
            )
        }
        Action::Vim => {
            let on = state.composer.vim_mode().is_none();
            state.composer.set_vim(on);
        }
        // T27.5: an empty list stays the T27.2 `Notice` (nothing to
        // navigate); otherwise `/agents` opens the navigable overlay.
        Action::Agents => {
            let entries = agents_rows(
                &state.agents,
                &state.tasks,
                state.tick,
                state.glyphs.worktree,
            );
            if entries.is_empty() {
                notice(state, Level::Info, "no live agents".to_string());
            } else {
                let (rows, ids) = entries.into_iter().unzip();
                state.modal = Some(Modal::Agents {
                    rows,
                    ids,
                    selected: 0,
                });
            }
        }
        Action::Sessions => {
            let text = if state.sessions.is_empty() {
                "no sessions for this project yet".to_string()
            } else {
                state
                    .sessions
                    .iter()
                    .map(|(id, row)| format!("{id} · {row}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            notice(state, Level::Info, text);
        }
        Action::Resume => {
            let rows = state.sessions.iter().map(|(_, row)| row.clone()).collect();
            state.modal = Some(Modal::Picker(Picker::open(Kind::Sessions, rows)));
        }
        Action::Notice(text) => notice(state, Level::Warn, text),
        // The core refuses one mid-turn anyway; saying so here keeps the
        // pending card from latching onto the turn's next `bash` call.
        Action::Shell { .. } if state.status.busy => {
            notice(
                state,
                Level::Warn,
                "a turn is running; `!` waits until it ends".into(),
            );
        }
        Action::Shell { cmd, share } => {
            state.shell = Some(cmd.clone());
            return vec![Cmd::Submit(Submission::UserShell {
                command: cmd,
                share,
            })];
        }
        Action::Rewind => return open_rewind(state),
        Action::Undo => match state.turns.last() {
            Some(t) => {
                return vec![Cmd::Submit(Submission::Rewind {
                    to_turn: t.seq,
                    code: true,
                    conversation: false,
                })];
            }
            None => notice(state, Level::Warn, "undo: no turn yet".into()),
        },
        Action::Redo => return vec![Cmd::Submit(Submission::Redo)],
        // A running turn would be cut mid-write, so both wait for it.
        Action::Fork(_) | Action::Handoff(_) if state.status.busy => {
            notice(state, Level::Warn, "interrupt the turn first".into());
        }
        Action::Fork(Some(turn)) if !state.turns.iter().any(|t| t.seq == turn) => {
            let text = format!("fork: no turn T{turn}; /rewind lists them");
            notice(state, Level::Warn, text);
        }
        Action::Fork(turn) => {
            state.queue.clear();
            return vec![Cmd::Fork(turn)];
        }
        Action::Handoff(objective) => {
            state.queue.clear();
            return vec![Cmd::Handoff(objective)];
        }
        Action::Theme(Some(name)) => {
            if state.theme_rows.contains(&name) {
                return apply_theme_choice(state, &name);
            }
            notice(
                state,
                Level::Warn,
                format!("unknown theme {name:?}; /theme lists them"),
            );
        }
        Action::Theme(None) => {
            state.theme_prev = Some((state.dark, state.theme, state.syntax_theme));
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::Themes,
                state.theme_rows.clone(),
            )));
        }
        // T27.4: `interval` is whole seconds (`parse_interval`), so
        // `* 10` (100 ms ticks) never loses precision; a bare `--budget`
        // defaults to the session cap the status line already shows.
        Action::LoopStart {
            interval,
            prompt,
            budget_usd,
        } => {
            let interval_ticks = (interval.as_secs() * 10).max(1);
            let budget_usd = budget_usd.unwrap_or(state.status.budget_cap_usd);
            state.active_loop = Some(Loop {
                prompt,
                interval_ticks,
                next_at: state.tick + interval_ticks,
                budget_usd,
                started_cost_usd: state.status.cost_usd,
                iterations: 0,
            });
            let text = format!(
                "loop started: every {}s, budget ${budget_usd:.2}",
                interval.as_secs()
            );
            notice(state, Level::Info, text);
        }
        Action::LoopStop => match state.active_loop.take() {
            Some(_) => notice(state, Level::Info, "loop stopped".into()),
            None => notice(state, Level::Warn, "no loop running".into()),
        },
        // T33.25, PL§8: `crates/cox`'s `plugin_ui::answer` runs
        // `cox_command` and the answer comes back as
        // `Msg::Plugin(PluginUiMsg::Command)`, applied in `plugin_command_out`.
        Action::PluginCommand { plugin, name, args } => {
            return vec![Cmd::Plugin(PluginRequest::Command { plugin, name, args })];
        }
        // T33.30: `--lang` already named a language, so there is nothing
        // to pick — the request goes straight to the runtime, which is the
        // one place that validates it (`plugin_new::scaffold`'s errors).
        Action::PluginNew {
            name,
            lang: Some(lang),
            with,
        } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::New(PluginNewRequest {
                name,
                lang,
                with,
            }))];
        }
        // No `--lang`: stash `name`/`with` and ask the picker, the same
        // two-step shape `Action::Rewind` uses for its turn-then-what picks.
        Action::PluginNew {
            name,
            lang: None,
            with,
        } => {
            state.pending_plugin_new = Some((name, with));
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::PluginLang,
                picker::PLUGIN_LANGS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )));
        }
        // T33.33: `list`/`update` need no confirmation — the runtime
        // already pre-approves them the way `--yes` would, since the TUI's
        // raw mode has no stdin to prompt a widened capability list on.
        Action::PluginList { json } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::List { json })];
        }
        Action::PluginUpdate {
            ids,
            all,
            check,
            rollback,
        } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::Update {
                ids,
                all,
                check,
                rollback,
            })];
        }
        // `remove` is the one irreversible plugin action, so it asks
        // through `Modal::PluginRemove` first — `on_key`'s handler for it
        // sends `Cmd::PluginMgmt(PluginMgmtRequest::Remove)` only on `y`.
        Action::PluginRemove { id, keep_data } => {
            state.modal = Some(Modal::PluginRemove(RemoveConfirm { id, keep_data }));
        }
        // `/plugin reload` means `/clear`: the plugin host only reloads a
        // manifest at session open, so restarting the cache prefix is the
        // only way to pick up a changed one — mirrors the `command.name ==
        // "clear"` handling below, plus a notice explaining why.
        Action::PluginReload => {
            state.queue.clear();
            state.active_loop = None;
            notice(
                state,
                Level::Info,
                "plugin reload: restarting session to reload plugins".into(),
            );
            return vec![Cmd::Clear];
        }
    }
    Vec::new()
}

/// `Msg::Tick` (T27.4): `/loop`'s timer. A due loop fires through the same
/// path an idle `Enter` uses — a direct `Submit`, not the T25.1 queue,
/// which only defers while busy — so a turn still running just waits for a
/// later tick instead of piling up. The loop's own budget is checked first,
/// so a due-but-over-budget tick stops it instead of firing once more.
fn loop_tick(state: &mut State) -> Vec<Cmd> {
    let Some(lp) = state.active_loop.as_ref() else {
        return Vec::new();
    };
    if state.status.cost_usd - lp.started_cost_usd >= lp.budget_usd {
        state.active_loop = None;
        notice(state, Level::Warn, "loop stopped: budget reached".into());
        return Vec::new();
    }
    if state.status.busy || state.tick < lp.next_at {
        return Vec::new();
    }
    let prompt = lp.prompt.clone();
    let interval_ticks = lp.interval_ticks;
    if let Some(lp) = state.active_loop.as_mut() {
        lp.next_at = state.tick + interval_ticks;
        lp.iterations += 1;
    }
    vec![Cmd::Submit(user_turn(state, prompt))]
}

/// A user turn from any TUI source. In architect it carries the think
/// consent the user gave once for the stretch (P42, invariant 9); anywhere
/// else `confirm_think` would move the turn to think, so it stays off.
fn user_turn(state: &State, text: String) -> Submission {
    Submission::UserTurn {
        text,
        attachments: Vec::new(),
        confirm_think: state.session_mode == SessionMode::Architect && state.think_confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{type_after_sigil, type_command, type_line};
    use super::*;
    use crate::theme;
    use cox_core::{History, HistoryTurn};
    use cox_protocol::ids::TurnId;
    use cox_protocol::plugin::KeyDecl;
    use cox_protocol::types::{
        Content, Event, Message, PermissionMode, Role, SandboxMode, StopReason, ToolCall,
        ToolResult,
    };
    use crossterm::event::{KeyCode, KeyEvent};

    /// T45.6: `@<agent> task` at line start runs that agent directly.
    #[test]
    fn at_agent_name_submits_user_agent() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.agent_names = vec!["explore".into(), "reviewer".into()];
        let cmds = type_after_sigil(&mut state, '@', "@explore find the router");
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserAgent {
                name: "explore".into(),
                task: "find the router".into(),
            })]
        );
    }

    /// T45.6: an `@file` mention, an unknown name and a bare agent name
    /// with no task all stay ordinary user turns.
    #[test]
    fn at_file_path_stays_a_user_turn() {
        for line in ["@src/main.rs explain it", "@nope do it", "@explore"] {
            let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
            state.agent_names = vec!["explore".into()];
            state.files = vec!["src/main.rs".into()];
            let cmds = type_after_sigil(&mut state, '@', line);
            assert_eq!(
                cmds,
                vec![Cmd::Submit(Submission::UserTurn {
                    text: line.into(),
                    attachments: Vec::new(),
                    confirm_think: false,
                })],
                "{line}"
            );
        }
    }

    #[test]
    fn transcript_from_history_seeds_user_and_assistant() {
        let messages = [
            Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "hello".into(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![Content::Text {
                    text: "hi there".into(),
                }],
            },
        ];
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.transcript_from_history(&History {
            messages: messages.to_vec(),
            permission_mode: None,
            grants: Vec::new(),
            truncated: false,
            repomap: None,
            archives: Default::default(),
            turns: 4,
            turn_marks: vec![HistoryTurn {
                item: ItemId::new(),
                seq: 4,
                message_index: 0,
                checkpoints: 2,
            }],
        });
        assert_eq!(state.transcript.len(), 2);
        assert!(matches!(
            &state.transcript[0],
            Cell::User { text, .. } if text == "hello"
        ));
        assert!(matches!(
            &state.transcript[1],
            Cell::Assistant { text, done: true, .. } if text == "hi there"
        ));
        assert_eq!(state.turns[0].seq, 4);
        assert_eq!(state.turns[0].files, 2);
        assert_eq!(state.current_seq, 4);
    }

    #[test]
    fn take_finished_rebases_turn_cell_indices() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.transcript.push(Cell::User {
            text: "one".into(),
            attachments: Vec::new(),
        });
        state.transcript.push(Cell::User {
            text: "two".into(),
            attachments: Vec::new(),
        });
        state.turns = vec![
            TurnRow {
                seq: 1,
                text: "one".into(),
                files: 0,
                cell_at: 0,
            },
            TurnRow {
                seq: 2,
                text: "two".into(),
                files: 0,
                cell_at: 1,
            },
        ];

        assert_eq!(state.take_finished().len(), 2);
        assert_eq!(state.turns[0].cell_at, 0);
        assert_eq!(state.turns[1].cell_at, 0);
    }

    #[test]
    fn clear_command_emits_cmd_clear() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        // `/` at column 0 opens the palette; Esc leaves `/` in the composer.
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "clear".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(cmds, vec![Cmd::Clear]);
    }

    /// T24.2 step 3: moving the `/theme` picker's cursor previews a theme
    /// immediately, and `Esc` restores whatever was active before it opened
    /// — a browse that changes nothing must be free to abandon.
    #[test]
    fn theme_picker_preview_reverts_on_esc() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.theme_rows = vec!["cox-dark".into(), "cox-light".into()];
        state.theme_catalog = theme::BUILT_IN_THEMES[..2]
            .iter()
            .map(|(name, src)| ((*name).to_string(), theme::parse_theme_file(src).unwrap()))
            .collect();
        let before = (state.dark, state.theme, state.syntax_theme);

        // `/theme` submitted from the composer, same as `clear_command_emits_cmd_clear`.
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "theme".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert!(
            matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::Themes),
            "/theme with no argument opens the picker"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        assert_ne!(
            (state.dark, state.theme, state.syntax_theme),
            before,
            "moving the cursor previews the selected theme"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(
            (state.dark, state.theme, state.syntax_theme),
            before,
            "Esc restores the theme active before the picker opened"
        );
        assert!(state.modal.is_none());
    }

    /// `/theme` open over the two plain built-ins and one syntax row.
    fn theme_picker_open() -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.theme_rows = vec![
            "cox-dark".into(),
            "cox-light".into(),
            "syntax: base16".into(),
        ];
        state.theme_catalog = theme::BUILT_IN_THEMES[..2]
            .iter()
            .map(|(name, src)| ((*name).to_string(), theme::parse_theme_file(src).unwrap()))
            .collect();
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "theme".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        state
    }

    fn ctrl_e() -> Msg {
        Msg::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))
    }

    /// T46.7: `Ctrl+E` is `expand` elsewhere; over `/theme` it opens the
    /// editor on the highlighted row, and the editor's `Esc` restores what
    /// was drawn before `/theme` opened.
    #[test]
    fn ctrl_e_opens_the_editor_on_the_highlighted_theme() {
        let mut state = theme_picker_open();
        let drawn = state.theme_prev.expect("/theme saved what was drawn");
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        update(&mut state, ctrl_e());
        match &state.modal {
            Some(Modal::ThemeEditor(e)) => {
                assert_eq!(e.name, "cox-light");
                assert!(e.builtin);
            }
            other => panic!("expected the theme editor, got {other:?}"),
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(state.modal, None);
        assert_eq!((state.dark, state.theme, state.syntax_theme), drawn);
    }

    /// T46.7: a built-in is never written in place; its edits go to
    /// `<name>-custom`, each token as the text a theme file holds.
    #[test]
    fn editor_save_emits_save_theme_with_custom_stem_for_builtin() {
        let mut state = theme_picker_open();
        update(&mut state, ctrl_e());
        let dark = match &state.modal {
            Some(Modal::ThemeEditor(e)) => e.dark,
            other => panic!("expected the theme editor, got {other:?}"),
        };
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        for _ in 0..16 {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Backspace)));
        }
        for c in "#ff0000".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        assert_eq!(
            state.theme.accent,
            ratatui::style::Color::Rgb(255, 0, 0),
            "the preview follows the edit"
        );
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(
            cmds,
            vec![Cmd::Ask(Ask::SaveTheme {
                stem: "cox-dark-custom".into(),
                dark,
                tokens: vec![("accent".into(), "#ff0000".into())],
            })]
        );
        assert_eq!(state.modal, None);
    }

    /// T46.7: the runtime's answer lists the new file among the colour
    /// themes (before the `syntax: ` rows), applies it and persists it.
    #[test]
    fn saved_theme_is_selectable_without_restart() {
        let mut state = theme_picker_open();
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        let src = theme::builtin_source("cox-dark").unwrap();
        let src =
            theme::set_token(src, "accent", true, ratatui::style::Color::Rgb(1, 2, 3)).unwrap();
        let file = theme::parse_theme_file(&src).unwrap();
        let cmds = update(
            &mut state,
            Msg::ThemeSaved("cox-dark-custom".into(), file.clone()),
        );
        assert_eq!(
            state.theme_rows,
            vec!["cox-dark", "cox-light", "cox-dark-custom", "syntax: base16"]
        );
        assert!(
            state
                .theme_catalog
                .iter()
                .any(|(n, f)| n == "cox-dark-custom" && *f == file)
        );
        assert_eq!(state.theme, file.theme(state.dark));
        assert_eq!(
            cmds,
            vec![Cmd::PersistConfig {
                key: "tui.theme".into(),
                value: "cox-dark-custom".into(),
            }]
        );
    }

    /// T33.30's Done-when: `/plugin new <name>` with no `--lang` opens the
    /// language picker, and choosing a row reaches
    /// `Cmd::PluginMgmt(PluginMgmtRequest::New)` with that language — the
    /// one `Cmd` `crates/cox`'s executor turns into a
    /// `plugin_new::scaffold`/`write` call, so there is no second
    /// implementation of the mapping. `fake_executor` stands in for that
    /// runtime executor (`cox-tui` cannot call `plugin_new::scaffold`
    /// itself — it does not depend on `crates/cox`) and records what it
    /// would have been called with.
    #[test]
    fn tui_plugin_new_calls_shared_scaffold() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/plugin new demo");
        assert!(
            matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::PluginLang),
            "/plugin new with no --lang opens the language picker"
        );

        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));

        let mut calls: Vec<(String, String, Vec<String>)> = Vec::new();
        let fake_executor =
            |req: &PluginNewRequest| (req.name.clone(), req.lang.clone(), req.with.clone());
        for cmd in &cmds {
            if let Cmd::PluginMgmt(PluginMgmtRequest::New(req)) = cmd {
                calls.push(fake_executor(req));
            }
        }
        assert_eq!(
            calls,
            vec![("demo".to_string(), "rust".to_string(), Vec::new())],
            "exactly one call, with the picker's chosen language"
        );
        assert!(state.modal.is_none(), "the picker closes after a pick");
    }

    /// T25.1 step 1/3: `Enter` while a turn runs queues instead of
    /// submitting, and each natural `TurnDone` drains the queue's head in
    /// the order the messages were typed.
    #[test]
    fn queued_messages_drain_in_order() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        type_line(&mut state, "second");
        assert_eq!(
            state.queue,
            VecDeque::from(["first".to_string(), "second".to_string()])
        );
        assert!(state.composer.is_empty());

        let done = |stop| {
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop,
            })
        };
        let cmds = update(&mut state, done(StopReason::EndTurn));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "first".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert_eq!(state.queue, VecDeque::from(["second".to_string()]));

        let cmds = update(&mut state, done(StopReason::EndTurn));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "second".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert!(state.queue.is_empty());
    }

    /// T25.1 step 4: `Ctrl+Enter` while a turn runs interrupts it and, once
    /// the interrupt lands, joins the queue with whatever was still in the
    /// composer into one turn.
    #[test]
    fn send_now_interrupts_and_flushes() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        for c in "second".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }

        let cmds = update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
        );
        assert_eq!(cmds, vec![Cmd::Submit(Submission::Interrupt)]);
        assert!(state.composer.is_empty());
        assert_eq!(
            state.queue,
            VecDeque::from(["first".to_string(), "second".to_string()])
        );

        let cmds = update(
            &mut state,
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop: StopReason::Interrupted,
            }),
        );
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "first\n\nsecond".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert!(state.queue.is_empty());
    }

    /// T25.1 step 1: `Ctrl+U` on an empty composer pops the queue's tail
    /// back for editing.
    #[test]
    fn ctrl_u_unqueues_last() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        type_line(&mut state, "second");

        let cmds = update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        );
        assert!(cmds.is_empty());
        assert_eq!(state.composer.text(), "second");
        assert_eq!(state.queue, VecDeque::from(["first".to_string()]));
    }

    /// T27.4: `/loop`'s timer fires a direct `Submit` (not the T25.1 queue)
    /// once the session is idle and the tick it scheduled arrives; earlier
    /// ticks are silent.
    #[test]
    fn loop_enqueues_when_due_and_idle() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        // The shortest interval `parse_interval` accepts, 1s = 10 ticks.
        type_command(&mut state, "/loop 1s go");
        assert!(state.active_loop.is_some());
        for _ in 0..9 {
            assert!(update(&mut state, Msg::Tick).is_empty());
        }
        assert_eq!(
            update(&mut state, Msg::Tick),
            vec![Cmd::Submit(Submission::UserTurn {
                text: "go".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
    }

    /// T27.4: `/loop stop` and an idle empty-composer `Esc` both end a
    /// running loop; `Esc` takes it before Esc-Esc's rewind-timeline role.
    #[test]
    fn loop_stop_and_esc_both_end_a_running_loop() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/loop 1h go");
        assert!(state.active_loop.is_some());
        type_command(&mut state, "/loop stop");
        assert!(state.active_loop.is_none());

        type_command(&mut state, "/loop 1h go");
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(cmds.is_empty());
        assert!(state.active_loop.is_none());
        assert!(
            state.esc_armed.is_none(),
            "the stop consumed the Esc, not the Esc-Esc rewind timer"
        );
    }

    /// T27.4: once the loop's own spend equals its budget, the next tick
    /// stops it instead of firing another turn.
    #[test]
    fn loop_stops_itself_when_its_own_budget_is_spent() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/loop 1s go --budget 1.00");
        state.status.cost_usd = 1.00;
        assert_eq!(update(&mut state, Msg::Tick), Vec::new());
        assert!(state.active_loop.is_none());
    }

    /// T22.2: a markdown file command joins the `/` palette after the
    /// built-ins, a chosen row inserts `/name `, and `Enter` submits it as
    /// `Submission::Command { name, args }` — args tokenized like any line.
    #[test]
    fn palette_lists_file_commands() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.commands.push((
            "review".into(),
            "/review [pr]".into(),
            "review a pull request".into(),
        ));
        assert_eq!(
            state.commands.first().map(|(n, ..)| n.as_str()),
            Some("model"),
            "built-in COMMANDS come first"
        );
        assert_eq!(
            state.commands.last().map(|(n, ..)| n.as_str()),
            Some("review"),
            "file commands are appended after them"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        for c in "rev".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let rows = match &state.modal {
            Some(Modal::Picker(p)) => p.matches.clone(),
            other => panic!("the palette is open, got {other:?}"),
        };
        assert!(rows.contains(&"review".to_string()), "{rows:?}");

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(state.composer.text(), "/review ");
        for c in "pr-1".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::Command {
                command: SlashCommand {
                    name: "review".into(),
                    args: vec!["pr-1".into()],
                },
            })]
        );
    }

    /// T33.25, PL§8: even a plugin command mis-registered without its
    /// `<id>:` prefix (bypassing `declare_plugin_commands`'s own naming)
    /// never shadows a built-in — the same guard `file_command` already
    /// gives a markdown command — and the real built-in still dispatches
    /// with plugin commands present in the table.
    #[test]
    fn builtin_command_wins_over_plugin() {
        let commands = vec![("quit".to_string(), String::new(), String::new())];
        assert_eq!(plugin_command(&commands, "/quit"), None);

        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state
            .commands
            .push(("acme:quit".into(), "/acme:quit".into(), String::new()));
        assert_eq!(type_command(&mut state, "/quit"), vec![Cmd::Quit]);
    }

    /// T33.25, PL§8: `/<id>:<name>` calls the plugin (`Cmd::Plugin`, not
    /// the core), and its `CommandOut::Prompt` answer submits a `UserTurn`,
    /// like a markdown command's own text would.
    #[test]
    fn plugin_command_prompt_submits_user_turn() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state
            .commands
            .push(("acme:go".into(), "/acme:go".into(), "go".into()));

        let cmds = type_command(&mut state, "/acme:go do it");
        assert_eq!(
            cmds,
            vec![Cmd::Plugin(PluginRequest::Command {
                plugin: "acme".into(),
                name: "go".into(),
                args: "do it".into(),
            })]
        );

        let cmds = update(
            &mut state,
            Msg::Plugin(PluginUiMsg::Command {
                plugin: "acme".into(),
                out: Some(CommandOut::Prompt {
                    text: "go go go".into(),
                }),
            }),
        );
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "go go go".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
    }

    /// T33.25, PL§8: a plugin key fires only as `<leader> <key>` — the bare
    /// key on an empty composer does nothing plugin-related, the same as
    /// any other unbound letter.
    #[test]
    fn plugin_key_only_under_leader() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.keymap.declare_plugin_keys(
            "acme",
            &[KeyDecl {
                key: "r".into(),
                name: "reset".into(),
                description: String::new(),
            }],
        );
        let plugin_key = Cmd::Plugin(PluginRequest::Key {
            plugin: "acme".into(),
            name: "reset".into(),
        });

        let r = || Msg::Key(KeyEvent::from(KeyCode::Char('r')));
        assert!(!update(&mut state, r()).contains(&plugin_key));

        let leader = Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(update(&mut state, leader), Vec::new());
        assert!(state.plugin_leader_armed);
        assert_eq!(update(&mut state, r()), vec![plugin_key]);
        assert!(!state.plugin_leader_armed, "one key disarms it");
    }

    /// T33.24, PL§8: `TogglePanel` flips `plugin_panel_open` for the
    /// plugin that sent it (twice returns to closed); `OpenOverlay` opens
    /// `Modal::Plugin`. Neither needs a declared slot to be safe — with
    /// none, `render_requests` just has nothing to ask for.
    #[test]
    fn plugin_command_toggles_panel_and_opens_overlay() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let toggle = |state: &mut State| {
            update(
                state,
                Msg::Plugin(PluginUiMsg::Command {
                    plugin: "acme".into(),
                    out: Some(CommandOut::TogglePanel),
                }),
            )
        };
        assert!(toggle(&mut state).is_empty());
        assert_eq!(state.plugin_panel_open.as_deref(), Some("acme"));
        assert!(toggle(&mut state).is_empty());
        assert_eq!(state.plugin_panel_open, None);

        update(
            &mut state,
            Msg::Plugin(PluginUiMsg::Command {
                plugin: "acme".into(),
                out: Some(CommandOut::OpenOverlay),
            }),
        );
        assert_eq!(state.modal, Some(Modal::Plugin { id: "acme".into() }));
    }

    /// T33.24, PL§8: `overlay`'s only key is `Esc`; it closes with no other
    /// side effect, the same close `Diff`/`Transcript` already have.
    #[test]
    fn esc_closes_plugin_overlay() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.modal = Some(Modal::Plugin { id: "acme".into() });
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(cmds, Vec::new());
        assert_eq!(state.modal, None);
    }

    /// T23.6: busy on a turn, paused while an approval waits, busy again
    /// once it is answered, cleared when the turn ends — each only once,
    /// and nothing at all for a terminal without OSC 9;4.
    #[test]
    fn progress_sequence_follows_turn_state() {
        let turn = TurnId::new();
        let events = || {
            let call = ToolCall {
                id: CallId::new(),
                name: "bash".into(),
                input: serde_json::Value::Null,
                risk: cox_protocol::types::Risk::Exec,
                subject: "cargo test".into(),
                segments: None,
            };
            let call_id = call.id;
            vec![
                Event::TurnStarted {
                    seq: 1,
                    turn,
                    job: cox_protocol::types::Job::Main,
                    tier: cox_protocol::types::Tier::Code,
                    model: cox_protocol::types::ModelId("m".into()),
                },
                Event::TextDelta {
                    item: cox_protocol::ids::ItemId::new(),
                    text: "working".into(),
                },
                Event::ApprovalRequired {
                    call,
                    why: cox_protocol::types::Why::Risk {
                        risk: cox_protocol::types::Risk::Exec,
                    },
                    source: None,
                },
                Event::ApprovalDecided {
                    call_id,
                    decision: cox_protocol::types::Decision::Allow,
                    by: cox_protocol::types::DecidedBy::User,
                },
                Event::TurnDone {
                    turn,
                    stop: StopReason::EndTurn,
                },
            ]
        };
        let progress = |state: &mut State| -> Vec<Progress> {
            events()
                .into_iter()
                .flat_map(|ev| update(state, Msg::Event(ev)))
                .filter_map(|c| match c {
                    Cmd::Progress(p) => Some(p),
                    _ => None,
                })
                .collect()
        };
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc9_4 = true;
        assert_eq!(
            progress(&mut state),
            [
                Progress::Busy,
                Progress::Paused,
                Progress::Busy,
                Progress::Idle
            ]
        );
        let mut plain = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert!(progress(&mut plain).is_empty());
        assert_eq!(crate::term::progress(Progress::Busy), "\x1b]9;4;3;0\x1b\\");
    }

    /// T22.4: a wheel tick reuses whichever context is open's own scroll —
    /// the plain transcript's `state.scroll`, the diff view's `scroll`
    /// field, or a picker's `selected` row — moving `WHEEL_LINES` at a time.
    #[test]
    fn update_mouse_wheel_scrolls_overlay() {
        fn wheel(up: bool) -> Msg {
            Msg::Mouse(MouseEvent {
                kind: if up {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        }

        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        update(&mut state, wheel(true));
        assert_eq!(state.scroll, WHEEL_LINES);
        update(&mut state, wheel(false));
        assert_eq!(state.scroll, 0);
        // Never underflows past the bottom.
        update(&mut state, wheel(false));
        assert_eq!(state.scroll, 0);

        state.modal = Some(Modal::Diff {
            text: (0..20)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            scroll: 5,
        });
        update(&mut state, wheel(true));
        assert_eq!(
            state.modal,
            Some(Modal::Diff {
                text: (0..20)
                    .map(|n| format!("line {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                scroll: 5 - WHEEL_LINES,
            })
        );

        state.modal = Some(Modal::Picker(Picker::open(
            Kind::Files,
            vec!["a".into(), "b".into(), "c".into(), "d".into()],
        )));
        update(&mut state, wheel(false));
        let Some(Modal::Picker(picker)) = &state.modal else {
            panic!("picker closed");
        };
        assert_eq!(picker.selected, WHEEL_LINES.min(3));
        update(&mut state, wheel(true));
        let Some(Modal::Picker(picker)) = &state.modal else {
            panic!("picker closed");
        };
        assert_eq!(picker.selected, 0);
    }

    /// T22.9: a click on a folded tool card's own rows (from `cell_rows`,
    /// which `view()` records) forces it open via `state.expanded`, the
    /// same set `Ctrl+E` flips; a second click folds it back.
    #[test]
    fn update_mouse_click_unfolds_card() {
        use cox_protocol::types::Risk;
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let id = CallId::new();
        update(
            &mut state,
            Msg::Event(Event::ToolCallRequested {
                call: ToolCall {
                    id,
                    name: "bash".into(),
                    input: serde_json::json!({}),
                    risk: Risk::Exec,
                    subject: "seq 20".into(),
                    segments: None,
                },
            }),
        );
        let body = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        update(
            &mut state,
            Msg::Event(Event::ToolCallOutput {
                call_id: id,
                delta: format!("{body}\n[exit 0 in 8ms]"),
            }),
        );
        update(
            &mut state,
            Msg::Event(Event::ToolCallDone {
                call_id: id,
                result: ToolResult {
                    ok: true,
                    visible: String::new(),
                    archive: None,
                    bytes: 0,
                    duration_ms: 8,
                    diff: None,
                    structured: None,
                },
            }),
        );

        let area = ratatui::layout::Rect::new(0, 0, 60, 30);
        let click = |row: u16| {
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        // Unfolding changes the transcript's line count, so the row to
        // click is read back from `cell_rows` after each draw.
        let card_row = |state: &State| -> u16 {
            let mut buf = ratatui::buffer::Buffer::empty(area);
            crate::view::view(state, area, &mut buf);
            state.cell_rows.borrow()[0].0.start
        };

        let row = card_row(&state);
        update(&mut state, click(row));
        assert!(
            state.expanded.contains(&0),
            "a click on the folded card should force it open"
        );

        let row = card_row(&state);
        update(&mut state, click(row));
        assert!(
            !state.expanded.contains(&0),
            "a second click should fold the card back"
        );
    }

    /// T5.9: what the tests below move over — rows 2..4 are a tool card (a
    /// click folds it), rows 4..8 transcript text, anything else empty.
    fn pointer_state(osc22: bool) -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc22 = osc22;
        *state.cell_rows.borrow_mut() = vec![(2..4, 0)];
        *state.text_rows.borrow_mut() = 2..8;
        state
    }

    fn moved(row: u16) -> Msg {
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn mouse_move_asks_for_the_pointer_the_shared_rule_picks() {
        use cox_render::pointer::Pointer;
        let mut state = pointer_state(true);
        assert_eq!(
            update(&mut state, moved(2)),
            vec![Cmd::Pointer(Pointer::Action)],
            "a foldable card row is clickable, which outranks selectable"
        );
        assert_eq!(
            update(&mut state, moved(5)),
            vec![Cmd::Pointer(Pointer::Text)]
        );
        assert_eq!(
            update(&mut state, moved(12)),
            vec![Cmd::Pointer(Pointer::Default)]
        );
    }

    #[test]
    fn mouse_move_inside_the_same_shape_writes_nothing() {
        let mut state = pointer_state(true);
        update(&mut state, moved(5));
        assert_eq!(update(&mut state, moved(6)), Vec::new());
        assert_eq!(update(&mut state, moved(5)), Vec::new());
        // The arrow is the starting shape, so empty space first asks for none.
        let mut fresh = pointer_state(true);
        assert_eq!(update(&mut fresh, moved(12)), Vec::new());
    }

    #[test]
    fn mouse_move_writes_nothing_without_osc22_or_with_the_mouse_off() {
        let mut without = pointer_state(false);
        assert_eq!(update(&mut without, moved(2)), Vec::new());
        assert_eq!(without.pointer, cox_render::pointer::Pointer::Default);

        let mut off = pointer_state(true);
        off.mouse = false;
        assert_eq!(update(&mut off, moved(2)), Vec::new());
    }

    #[test]
    fn an_open_modal_makes_a_card_row_plain_text() {
        use cox_render::pointer::Pointer;
        let mut state = pointer_state(true);
        state.modal = Some(Modal::Help);
        assert_eq!(
            update(&mut state, moved(2)),
            vec![Cmd::Pointer(Pointer::Text)],
            "a click does nothing behind a modal, so the row is not clickable"
        );
    }

    #[test]
    fn view_records_the_rows_that_carry_transcript_text() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        for text in ["one", "two"] {
            state.transcript.push(Cell::Notice {
                level: Level::Info,
                text: text.into(),
            });
        }
        let area = ratatui::layout::Rect::new(0, 4, 40, 20);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        crate::view::view(&state, area, &mut buf);
        let rows = state.text_rows.borrow().clone();
        assert!(
            rows.start >= area.y && !rows.is_empty() && rows.end < area.bottom(),
            "{rows:?} should be a band of text inside the viewport, not all of it"
        );
    }

    /// T66.8: `/refine <text>` asks the runtime, the runtime's proposal
    /// opens the review overlay, and only `y` sends the apply; `n` drops it.
    #[test]
    fn refine_asks_then_reviews_then_applies_only_on_y() {
        use crate::refine::{Msg as RefineMsg, Request};
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert_eq!(
            type_command(&mut state, "/refine run tests first"),
            vec![Cmd::Ask(Ask::Refine(Request::Propose {
                text: "run tests first".into(),
                global: false,
            }))]
        );
        let proposal = || {
            Msg::Refine(RefineMsg::Review {
                evidence: "seen twice".into(),
                diff: "diff --git a/x b/x\n--- /dev/null\n+++ x\n@@ -0,0 +1 @@\n+a\n".into(),
            })
        };
        update(&mut state, proposal());
        assert!(matches!(state.modal, Some(Modal::Refine(_))));
        assert_eq!(state.context(), Context::Overlay);

        let key = |c| Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(update(&mut state, key('x')), Vec::new());
        assert!(state.modal.is_some(), "a stray key keeps the review");
        assert_eq!(
            update(&mut state, key('y')),
            vec![Cmd::Ask(Ask::Refine(Request::Apply))]
        );
        assert!(state.modal.is_none());

        update(&mut state, proposal());
        assert_eq!(update(&mut state, key('n')), Vec::new());
        assert!(state.modal.is_none(), "n sends nothing");
    }

    /// T66.8: `--global` is the only way to ask for the global scope, and a
    /// bare `/refine` says how.
    #[test]
    fn refine_global_needs_the_flag_and_a_bare_refine_prints_usage() {
        use crate::refine::Request;
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert_eq!(
            type_command(&mut state, "/refine be concise --global"),
            vec![Cmd::Ask(Ask::Refine(Request::Propose {
                text: "be concise".into(),
                global: true,
            }))]
        );
        assert_eq!(type_command(&mut state, "/refine"), Vec::new());
        assert!(matches!(
            state.transcript.last(),
            Some(Cell::Notice { text, .. }) if text.starts_with("/refine <text>")
        ));
    }
}
