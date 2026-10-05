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

use cox_protocol::GrantScope;
use cox_protocol::ids::{CallId, ItemId, SessionId, TaskId};
use cox_protocol::plugin::{CommandDecl, CommandOut, KeyDecl, NoticeLevel, RenderIn, Slot, Widget};
use cox_protocol::types::{
    Content, ContextBreakdown, Effort, Event, ItemKind, Level, Mode as SessionMode, PermissionMode,
    Presence, Role, SandboxMode, SlashCommand, StopReason, Submission, Tier, TodoItem, ToolCall,
    ToolResult,
};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::banner::Banner;
use crate::cells::Look;
use crate::color::Depth;
use crate::commands::{self, Action, COMMANDS, Context};
use crate::composer::{Composer, Edit};
use crate::glyph::{self, Glyphs};
use crate::item_render::{CellRef, ItemRender, RenderSource};
use crate::keymap::{self, Keymap};
use crate::markdown;
use crate::modal::{Approval, PluginGrantDialog, Question, QuestionAnswer, RemoveConfirm};
use crate::picker::{self, Kind, Pick, Picker};
use crate::tasks;
use crate::term::{Caps, Progress};
use crate::theme::{Theme, ThemeFile};
use crate::theme_editor::{EditorOutcome, ThemeEditor};
use crate::vim::Mode;

/// One transcript entry. A finished cell leaves the viewport for the
/// terminal's own scrollback (`State::take_finished`).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    User {
        text: String,
        /// Attachment names; the bytes stay with the item.
        attachments: Vec<String>,
    },
    Assistant {
        item: ItemId,
        text: String,
        done: bool,
        /// A plugin's rendering (T33.26), asked once when `done`.
        render: ItemRender,
    },
    Thinking {
        item: ItemId,
        text: String,
        done: bool,
    },
    Tool {
        call: Box<ToolCall>,
        output: String,
        result: Option<ToolResult>,
        /// `State::tick` when the call was requested; elapsed time is ticks.
        started: u64,
        /// A composer `!` line (T25.3) rather than the model asked for it.
        user: bool,
        /// A plugin's rendering (T33.26), asked once when `result` lands.
        render: ItemRender,
    },
    Notice {
        level: Level,
        text: String,
    },
    Error {
        text: String,
        fatal: bool,
    },
    /// A compaction summary standing in for the turns it replaced.
    Summary {
        text: String,
    },
    /// A follow-up delivered to, or a reply from, a background task
    /// (`Event::TaskMessage`, T34.4/SM§6): one line labelled like a
    /// relayed `ApprovalRequired`'s `Source` ("X asks:", `Approval::from_agent`).
    /// `label` and `text` are already sanitized at the boundary in
    /// `update` (T34.4's `deliver`/`message_parent` text is model-controlled
    /// on both ends), mirroring `Modal::Diff`'s text rather than a second
    /// `text::sanitize` pass in `cells::cell_lines`.
    TaskMessage {
        label: String,
        text: String,
        /// `true`: the task itself spoke to the parent (`from == task`,
        /// SM§3's `message_parent`). `false`: a message was delivered to it
        /// (parent/user or a relayed sibling, SM§3's `deliver`).
        from_task: bool,
    },
}

impl Cell {
    pub fn done(&self) -> bool {
        match self {
            Cell::User { .. }
            | Cell::Notice { .. }
            | Cell::Error { .. }
            | Cell::Summary { .. }
            | Cell::TaskMessage { .. } => true,
            // T33.26: a pending plugin render holds the cell in the viewport.
            Cell::Assistant { done, render, .. } => *done && !render.pending(),
            Cell::Thinking { done, .. } => *done,
            Cell::Tool { result, render, .. } => result.is_some() && !render.pending(),
        }
    }
}

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

/// Lines a `PageUp`/`PageDown` moves the diff view (the inline viewport is
/// 15 rows, so a page is a little less).
const DIFF_PAGE: usize = 10;

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

/// Ticks (100 ms each) two `Esc`s may be apart to count as `Esc Esc`.
pub const ESC_ESC_TICKS: u64 = 5;

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

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Key(KeyEvent),
    Paste(String),
    Event(Event),
    Tick,
    Resize(u16, u16),
    /// The live sessions of this project, polled by the runtime (T16.3).
    Agents(Vec<Presence>),
    /// The working tree's branch and counts (T15.2); `None` outside a repo.
    Git(Option<GitStatus>),
    /// The runtime's answer to `Ask::GitDiff` (T15.3); `None` outside a repo.
    Diff(Option<String>),
    /// The terminal gained (`true`) or lost focus (T23.5).
    Focus(bool),
    /// A wheel tick or click (T22.4); `app.rs` only forwards these once
    /// `tui.mouse` actually enabled capture, so `update` need not re-check it.
    Mouse(MouseEvent),
    /// The runtime's answer to `Ask::Rollout` (T27.5): a sibling session's
    /// rollout events, replayed into cells for the read-only `Transcript`
    /// overlay. `crates/cox/src/session.rs` answers this for real with
    /// `Store::rollout_read`; a test can also send it directly.
    Rollout(Vec<Event>),
    /// What the runtime learned about a plugin's UI (T33.23, PL§8).
    Plugin(PluginUiMsg),
    /// The runtime's answer to `Cmd::PluginMgmt` (T33.30 `New`; T33.33
    /// `Update`/`Remove`/`List`): `Ok` is the lines `cox plugin
    /// new`/`update`/`remove`/`list` would have printed, joined; `Err`
    /// names why. Shown as a notice, the same as `Event::Notice`.
    PluginMgmt(Result<String, String>),
    /// The status command's first line (T46.3), `None` when it failed,
    /// timed out or printed nothing; answers `Ask::StatusLine`.
    StatusLine(Option<String>),
    /// The theme editor's file was written (T46.7): its stem and what it
    /// parses to, so `/theme` lists and applies it without a restart.
    ThemeSaved(String, ThemeFile),
    /// The `Dictation`'s answer to a `Cmd::Voice` (T54.6).
    Voice(crate::voice::VoiceMsg),
}

/// The runtime's side of the plugin redraw model (PL§8): `cox-tui` never
/// holds a plugin, so every render arrives here, cached in `State`.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginUiMsg {
    /// A plugin's granted slots, commands and keys after `cox_init`
    /// (T33.25 adds the latter two). `status.left`/`status.right` render
    /// once now, since they are always visible; `panel`/`overlay` (T33.24)
    /// only register here and render later, when `TogglePanel`/
    /// `OpenOverlay` actually shows them. `crates/cox` re-sends this
    /// whenever a session re-inits the plugin, so `update` treats it as the
    /// current, full set, not an addition to the last one.
    Declare {
        plugin: String,
        slots: Vec<Slot>,
        commands: Vec<CommandDecl>,
        keys: Vec<KeyDecl>,
        /// Granted `cox_render_item` targets (T33.26).
        renderers: Vec<String>,
    },
    /// A `cox_render_item` answer (T33.26); `None` keeps the built-in look.
    ItemRendered {
        cell: CellRef,
        widget: Option<Widget>,
    },
    /// `Effects.redraw` or `cox_redraw()`: render this plugin's slots again.
    Redraw { plugin: String },
    /// A `cox_render` answer that came back inside its deadline.
    Rendered {
        plugin: String,
        slot: Slot,
        widget: Widget,
    },
    /// A `cox_render` that timed out, failed or does not exist; the last
    /// good render stays and the miss is counted.
    Missed { plugin: String, slot: Slot },
    /// A `cox_command`/`cox_key` answer (T33.25, PL§8); `None` is a
    /// timeout, an error or a missing export, the same fail-open `Missed`
    /// already gives a render.
    Command {
        plugin: String,
        out: Option<CommandOut>,
    },
}

/// What the TUI asks of a plugin; `crates/cox` serves it and answers with
/// `Msg::Plugin`.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginRequest {
    /// Call `plugin`'s `cox_render` with `input`.
    Render { plugin: String, input: RenderIn },
    /// `/<id>:<name>` (T33.25, PL§8): call `plugin`'s `cox_command`.
    Command {
        plugin: String,
        name: String,
        args: String,
    },
    /// `<leader> <key>` (T33.25, PL§8): call `plugin`'s `cox_key`.
    Key { plugin: String, name: String },
    /// A finished cell (T33.26): call `plugin`'s `cox_render_item`.
    RenderItem {
        plugin: String,
        cell: CellRef,
        target: String,
        source: RenderSource,
        width: u16,
    },
    /// `/plugin remove <id>` confirmed (T33.33, PL§1c): `crates/cox` sends
    /// this after `plugin_cmd::remove_for_tui` succeeds, over the same
    /// channel `Render`/`Command`/`Key` already reach `plugin_ui::serve` on
    /// — the one place holding this session's live hosts — so it can flag
    /// `plugin`'s host stopped without a second channel. No answer rides
    /// back on the feed; the modal's own notice already told the user.
    Stop { plugin: String },
}

/// What the TUI asks the runtime to fetch off-screen; the answer comes back
/// on the feed channel. Kept apart from `Submission`: the core never sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    GitDiff,
    /// `/agents` (T27.5): a sibling session's rollout, the same read
    /// `crates/cox/src/resume.rs` does for `--resume` (`Store::rollout_read`);
    /// `crates/cox/src/session.rs` answers it for real.
    Rollout(SessionId),
    /// `[tui.status_line]` (T46.3): the status command's stdin JSON and
    /// the terminal width, sent only when either changed; the runtime
    /// debounces and answers `Msg::StatusLine`.
    StatusLine {
        input: serde_json::Value,
        columns: u16,
    },
    /// The theme editor's `Enter` (T46.7): write `<themes>/<stem>.toml`
    /// with each `(token, colour)` set for the `dark` or light half; the
    /// runtime checks `stem` and answers `Msg::ThemeSaved` or a notice.
    SaveTheme {
        stem: String,
        dark: bool,
        tokens: Vec<(String, String)>,
    },
}

/// `Modal::PluginGrant`'s `y` (T33.8, PL§3): the full requested capability
/// list to write at `digest` in `scope`. Carried out to `crates/cox` over a
/// channel, the same shape `persist`'s `(String, String)` carries a
/// `/theme` choice out to `config_cmd::set` — this crate never touches the
/// store itself.
#[derive(Debug, Clone, PartialEq)]
pub struct GrantDecision {
    pub plugin_id: String,
    pub digest: String,
    pub scope: GrantScope,
    pub capabilities: Vec<String>,
}

/// `/plugin new`'s request to the runtime (T33.30, PL§13), carried out to
/// `crates/cox` over a channel like `GrantDecision` above. `lang` is a
/// string, not `plugin_new::Lang`, because `cox-tui` cannot depend on
/// `crates/cox`'s types (plan.md §1.1); `crates/cox`'s `session.rs` maps it
/// back with `Lang::from_str`, the one place that mapping happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginNewRequest {
    pub name: String,
    pub lang: String,
    pub with: Vec<String>,
}

/// `/plugin`'s management requests to the runtime (T33.30 `New`; T33.33
/// `Update`/`Remove`/`List`), carried over the one channel `Cmd::PluginMgmt`
/// reaches (`app.rs`) rather than a second channel per subcommand —
/// `crates/cox`'s executor matches on this and calls the same
/// `plugin_new`/`plugin_cmd` functions `cox plugin ...` uses, so there is
/// only one implementation of each. Its answer rides the feed as
/// `Msg::PluginMgmt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMgmtRequest {
    New(PluginNewRequest),
    /// `/plugin update [<id>...] [--all] [--check] [--rollback]` (PL§1b):
    /// always pre-approved — the TUI's raw mode has no stdin to prompt a
    /// widened capability list on, so `crates/cox` runs it the way `--yes`
    /// would.
    Update {
        ids: Vec<String>,
        all: bool,
        check: bool,
        rollback: bool,
    },
    /// `/plugin remove <id> [--keep-data]` (PL§1c): only reached after
    /// `Modal::PluginRemove` confirmed it, so `crates/cox` never prompts
    /// either.
    Remove {
        id: String,
        keep_data: bool,
    },
    /// `/plugin list [--json]`.
    List {
        json: bool,
    },
}

/// The only effects `update` may request; the runtime performs them.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Submit(Submission),
    Quit,
    Clear,
    /// `/fork [turn]` (T26.3): leave for a child session with the history
    /// up to `turn` (`None`: all of it); the binary builds it.
    Fork(Option<u32>),
    /// `/handoff <objective>` (T26.3): leave for a child session seeded
    /// with a cheap-tier summary of this one plus the objective.
    Handoff(String),
    Copy(String),
    Ask(Ask),
    /// `/theme` (T24.2): a picker choice writes `key` in the user config
    /// with `cox config set` semantics. `cox-tui` has no `toml_edit`-editing
    /// path of its own (only `crates/cox` owns the config file); the
    /// runtime carries this to `config_cmd::set`.
    PersistConfig {
        key: String,
        value: String,
    },
    /// OSC 9;4 tab progress (T23.6); only sent when `caps.osc9_4`.
    Progress(Progress),
    /// Ring the terminal (T23.5); `app.rs` picks OSC 9/777 by `Caps`.
    Notify {
        title: String,
        body: String,
    },
    /// `Modal::PluginGrant`'s `y` (T33.8): `app.rs` forwards this to
    /// `crates/cox`, which writes it. `n` never reaches here — `on_key`
    /// alone advances `pending_grants`, since skipping writes nothing.
    PluginGrant(GrantDecision),
    /// A plugin call the runtime makes off-screen (T33.23); `app.rs`
    /// forwards it to `crates/cox`, the only side that holds plugin hosts.
    Plugin(PluginRequest),
    /// `/plugin new|update|remove|list`'s request to the runtime (T33.30
    /// `New`; T33.33 `Update`/`Remove`/`List`): `app.rs` forwards this to
    /// `crates/cox`, which runs the same `plugin_new`/`plugin_cmd`
    /// functions `cox plugin ...` calls and answers on the feed as
    /// `Msg::PluginMgmt`.
    PluginMgmt(PluginMgmtRequest),
    /// Push-to-talk (T54.6): `app.rs` hands this to the `Dictation` it
    /// holds; the answer arrives as `Msg::Voice`.
    Voice(crate::voice::VoiceCmd),
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

/// Replays a rollout's events through the same `update`/`Msg::Event` path a
/// live turn uses, into a scratch `State` nobody else sees, so the
/// `/agents` overlay (T27.5) renders through the identical cell-building
/// code instead of a second renderer — the same technique `tests/cells.rs`'s
/// fixture replay already uses.
fn replay_cells(events: Vec<Event>) -> Vec<Cell> {
    let mut scratch = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    for ev in events {
        update(&mut scratch, Msg::Event(ev));
    }
    scratch.transcript
}

/// A wheel tick (T22.4) or a left click (T22.9): a click only acts with no
/// modal open, like the wheel's own `None` arm below, and only when it
/// lands inside `cell_rows` — `view.rs`'s record of what is actually on
/// screen after the last draw — so anywhere else does nothing.
fn on_mouse(state: &mut State, ev: MouseEvent) -> Vec<Cmd> {
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

/// `Cmd::Notify` for `body` when `tui.notify` says to ring now (T23.5).
fn notify(state: &State, body: String) -> Vec<Cmd> {
    let ring = match state.notify {
        Notify::Always => true,
        Notify::Auto => !state.focused,
        Notify::Off => false,
    };
    if !ring {
        return Vec::new();
    }
    vec![Cmd::Notify {
        title: "cox".to_string(),
        body,
    }]
}

fn on_key(state: &mut State, key: KeyEvent) -> Vec<Cmd> {
    // T54.6: a recording owns `Esc` and the voice key's release and repeats
    // before anything else sees them.
    if let Some(cmds) = crate::voice::on_key(state, key) {
        return cmds;
    }
    // A held `Enter`/`Esc` must not repeat-submit or repeat-dismiss under the
    // Kitty keyboard protocol (T23.1); `app.rs` forwards a `Release` only
    // while push-to-talk records, and `voice::on_key` takes it above.
    if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
        return Vec::new();
    }
    // T33.25, PL§8: the key right after `plugin.leader` never reaches the
    // composer, a modal or another binding, matched or not — `<leader>
    // <key>` is the only way a plugin key fires.
    if state.plugin_leader_armed {
        state.plugin_leader_armed = false;
        return match state.keymap.resolve_plugin_key(key) {
            Some((plugin, name)) => vec![Cmd::Plugin(PluginRequest::Key {
                plugin: plugin.to_string(),
                name: name.to_string(),
            })],
            None => Vec::new(),
        };
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // `Ctrl+C` interrupts a running turn; when idle it must be pressed twice.
    if ctrl && key.code == KeyCode::Char('c') {
        if state.status.busy {
            return vec![Cmd::Submit(Submission::Interrupt)];
        }
        if state.ctrl_c_armed {
            return vec![Cmd::Quit];
        }
        state.ctrl_c_armed = true;
        return Vec::new();
    }
    state.ctrl_c_armed = false;
    // A `git` line completes on `Tab` (T15.4) before `Tab` means anything else.
    if key.code == KeyCode::Tab && state.modal.is_none() {
        let line = state.composer.text();
        let found = picker::candidates(&line, state);
        if !found.is_empty() {
            let picker = Picker::open(Kind::Shell, found).with_query(picker::last_word(&line));
            state.modal = Some(Modal::Picker(picker));
            return Vec::new();
        }
        // T25.2: `Tab` completes the `@`/`/` token under the cursor with the
        // picker typing the sigil would have opened, its query pre-filled.
        if let Some((sigil, query)) = state.composer.take_token() {
            let picker = match sigil {
                '@' => Picker::open(
                    Kind::Files,
                    picker::at_candidates(&state.agent_names, &state.files),
                ),
                _ => Picker::open(
                    Kind::Commands,
                    state.commands.iter().map(|(n, ..)| n.clone()).collect(),
                ),
            };
            state.modal = Some(Modal::Picker(picker.with_query(&query)));
            return Vec::new();
        }
    }
    // T46.7: `Ctrl+E` is `expand` everywhere else, a global the keymap
    // below would run over the picker; on `/theme` it opens the editor.
    if key.code == KeyCode::Char('e')
        && key.modifiers == KeyModifiers::CONTROL
        && matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::Themes)
    {
        open_theme_editor(state);
        return Vec::new();
    }
    // T25.5: every other key the TUI owns goes through the keymap; a key it
    // does not claim falls through to the modal or the composer.
    let base = if state.status.busy {
        Context::Running
    } else {
        Context::Idle
    };
    if let Some(action) = state.keymap.resolve(key, base)
        && let Some(cmds) = run(state, action, key)
    {
        return cmds;
    }
    match state.modal.take() {
        Some(Modal::Approval(mut approval)) => match approval.key(key) {
            Some(decision) => vec![Cmd::Submit(Submission::Approve {
                call_id: approval.call.id,
                decision,
            })],
            None => {
                state.modal = Some(Modal::Approval(approval));
                Vec::new()
            }
        },
        Some(Modal::Question(mut question)) => match question.key(key) {
            Some(answer) if state.think_consent == Some(question.call) => {
                think_consent_answered(state, answer)
            }
            // `None` (Esc) dismisses, so the tool call fails instead of
            // succeeding with empty text.
            Some(answer) => vec![Cmd::Submit(Submission::Answer {
                call_id: question.call,
                text: match answer {
                    QuestionAnswer::Text(text) => Some(text),
                    QuestionAnswer::Dismissed => None,
                },
            })],
            None => {
                state.modal = Some(Modal::Question(question));
                Vec::new()
            }
        },
        // T33.8, PL§3: `y`/`n` decide one dialog and `pending_grants`
        // supplies the next, so the queue drains one key at a time without
        // this crate ever writing the grant itself.
        Some(Modal::PluginGrant(grant)) => match grant.key(key) {
            Some(granted) => {
                let text = if granted {
                    format!("granted {}; it will load next session", grant.plugin_id)
                } else {
                    format!("skipped {} for this session", grant.plugin_id)
                };
                state.transcript.push(Cell::Notice {
                    level: Level::Info,
                    text,
                });
                state.modal = state.pending_grants.pop_front().map(Modal::PluginGrant);
                if granted {
                    vec![Cmd::PluginGrant(GrantDecision {
                        plugin_id: grant.plugin_id,
                        digest: grant.digest,
                        scope: grant.scope,
                        capabilities: grant.capabilities,
                    })]
                } else {
                    Vec::new()
                }
            }
            None => {
                state.modal = Some(Modal::PluginGrant(grant));
                Vec::new()
            }
        },
        // T33.33, PL§1c: `y` reaches the runtime only from here — `act()`
        // just opens the modal, never the request itself, so a stray
        // `/plugin remove` can never fire without this confirmation.
        Some(Modal::PluginRemove(confirm)) => match confirm.key(key) {
            Some(true) => {
                vec![Cmd::PluginMgmt(PluginMgmtRequest::Remove {
                    id: confirm.id,
                    keep_data: confirm.keep_data,
                })]
            }
            Some(false) => {
                notice(
                    state,
                    Level::Info,
                    format!("cancelled removing {}", confirm.id),
                );
                Vec::new()
            }
            None => {
                state.modal = Some(Modal::PluginRemove(confirm));
                Vec::new()
            }
        },
        // T24.2: every key that changes the selection previews the row
        // immediately, not only `Enter` — the same live-apply the picker's
        // other kinds do not need, since none of them redraws the screen
        // they came from.
        // T46.7: every accepted keystroke is drawn at once; `Esc` puts
        // back what was drawn before `/theme` opened, like the picker's.
        Some(Modal::ThemeEditor(mut editor)) => match editor.key(key) {
            None => {
                state.theme = editor.theme();
                state.modal = Some(Modal::ThemeEditor(editor));
                Vec::new()
            }
            Some(EditorOutcome::Revert) => {
                restore_theme(state);
                Vec::new()
            }
            Some(EditorOutcome::Save) => {
                state.theme_prev = None;
                let stem = if editor.builtin {
                    format!("{}-custom", editor.name)
                } else {
                    editor.name
                };
                let tokens = editor
                    .edits
                    .iter()
                    .map(|(t, c)| ((*t).to_string(), crate::theme::format_color(*c)))
                    .collect();
                vec![Cmd::Ask(Ask::SaveTheme {
                    stem,
                    dark: editor.dark,
                    tokens,
                })]
            }
        },
        Some(Modal::Picker(mut picker)) if picker.kind == Kind::Themes => {
            match picker.key(key) {
                Pick::Closed => restore_theme(state),
                Pick::Chosen(row) => {
                    state.theme_prev = None;
                    return apply_theme_choice(state, &row);
                }
                Pick::Nothing => {
                    preview_theme(state, &picker);
                    state.modal = Some(Modal::Picker(picker));
                }
            }
            Vec::new()
        }
        Some(Modal::Picker(mut picker)) => {
            match picker.key(key) {
                Pick::Nothing => state.modal = Some(Modal::Picker(picker)),
                // Backspacing out of the picker also removes the `@`/`/`
                // that opened it, as the user meant.
                Pick::Closed if key.code == KeyCode::Backspace && picker.kind != Kind::Shell => {
                    state.composer.key(key, state.status.busy);
                }
                // `Esc` gives back what was typed after the sigil, so a
                // `Tab` that found nothing costs no text (T25.2).
                Pick::Closed if matches!(picker.kind, Kind::Files | Kind::Commands) => {
                    state.composer.insert(&picker.query);
                }
                Pick::Closed => {}
                Pick::Chosen(choice) if picker.kind == Kind::Rewind => {
                    state.rewind_to = picker::turn_of_entry(&choice);
                    state.modal = Some(Modal::Picker(Picker::open(
                        Kind::RewindWhat,
                        picker::REWIND_WHAT.map(String::from).to_vec(),
                    )));
                }
                Pick::Chosen(choice) if picker.kind == Kind::RewindWhat => {
                    if let Some(to_turn) = state.rewind_to.take() {
                        let what = choice.split(' ').next().unwrap_or("");
                        return vec![Cmd::Submit(Submission::Rewind {
                            to_turn,
                            code: what != "talk",
                            conversation: what != "code",
                        })];
                    }
                }
                // T33.30: the second (and only) step of `/plugin new`
                // without `--lang` — `name`/`with` were stashed when the
                // picker opened, the same way `Kind::Rewind` stashes
                // `rewind_to` for `Kind::RewindWhat` above.
                Pick::Chosen(choice) if picker.kind == Kind::PluginLang => {
                    if let Some((name, with)) = state.pending_plugin_new.take() {
                        return vec![Cmd::PluginMgmt(PluginMgmtRequest::New(PluginNewRequest {
                            name,
                            lang: choice,
                            with,
                        }))];
                    }
                }
                Pick::Chosen(choice) => match picker.kind {
                    Kind::Files | Kind::Commands => {
                        let choice = picker::untag(&choice);
                        state.composer.insert(&format!("{choice} "));
                    }
                    // Resuming in place needs `app::run` to return a request;
                    // until then the command is the answer (T16.5).
                    Kind::Sessions => {
                        let id = state
                            .sessions
                            .iter()
                            .find(|(_, row)| *row == choice)
                            .map_or(choice.as_str(), |(id, _)| id.as_str());
                        let text = format!("to resume: cox --resume {id}");
                        notice(state, Level::Info, text);
                    }
                    Kind::History => {
                        let text = state
                            .past_prompts
                            .iter()
                            .find(|(row, _)| *row == choice)
                            .map_or(choice.as_str(), |(_, text)| text.as_str())
                            .to_string();
                        state.composer.set_text(&text);
                    }
                    // `Themes` and `PluginLang` are each intercepted by
                    // their own guarded arm above and never reach this
                    // generic one.
                    Kind::Rewind | Kind::RewindWhat | Kind::Themes | Kind::PluginLang => {}
                    Kind::Shell => {
                        let mut line = state.composer.text();
                        let keep = line.len() - picker::last_word(&line).len();
                        line.truncate(keep);
                        line.push_str(&choice);
                        line.push(' ');
                        state.composer.set_text(&line);
                    }
                },
            }
            Vec::new()
        }
        Some(Modal::Diff { text, scroll }) => {
            let scroll = match key.code {
                KeyCode::Esc | KeyCode::Char('?') => return Vec::new(),
                KeyCode::Char('g') if ctrl => return Vec::new(),
                KeyCode::PageDown => {
                    (scroll + DIFF_PAGE).min(text.lines().count().saturating_sub(1))
                }
                KeyCode::PageUp => scroll.saturating_sub(DIFF_PAGE),
                _ => scroll,
            };
            state.modal = Some(Modal::Diff { text, scroll });
            Vec::new()
        }
        Some(modal @ (Modal::Help | Modal::Context)) => {
            if !matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                state.modal = Some(modal);
            }
            Vec::new()
        }
        Some(Modal::Agents {
            rows,
            ids,
            mut selected,
        }) => {
            let mut cmds = Vec::new();
            match key.code {
                KeyCode::Esc => {}
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                }
                KeyCode::Down => {
                    if selected + 1 < rows.len() {
                        selected += 1;
                    }
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                }
                // A task row's id is `None` (no `SessionId` on the wire
                // yet, plan.md §3 P27): `Enter` on it just keeps the list.
                KeyCode::Enter => {
                    let ask = ids.get(selected).copied().flatten();
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                    if let Some(id) = ask {
                        cmds.push(Cmd::Ask(Ask::Rollout(id)));
                    }
                }
                _ => {
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    })
                }
            }
            cmds
        }
        Some(Modal::Transcript { cells, scroll }) => {
            // No upper clamp here (unlike `Diff`'s raw line count): `cells`
            // wrap at render width, which this pure update has no access
            // to, and `view.rs` already clamps the offset it actually uses.
            let scroll = match key.code {
                KeyCode::Esc | KeyCode::Char('?') => return Vec::new(),
                KeyCode::PageDown => scroll + DIFF_PAGE,
                KeyCode::PageUp => scroll.saturating_sub(DIFF_PAGE),
                _ => scroll,
            };
            state.modal = Some(Modal::Transcript { cells, scroll });
            Vec::new()
        }
        // T33.24, PL§8: `overlay`'s only key — full screen, no scroll of
        // its own, so anything but `Esc` just keeps it open.
        Some(Modal::Plugin { id }) => {
            if key.code != KeyCode::Esc {
                state.modal = Some(Modal::Plugin { id });
            }
            Vec::new()
        }
        None => {
            let esc_idle_empty =
                key.code == KeyCode::Esc && !state.status.busy && state.composer.is_empty();
            // `/loop`'s stop key (T27.4): the same idle-empty `Esc`, ahead of
            // Esc-Esc's rewind-timeline role below, so a running loop is
            // always one `Esc` away.
            if esc_idle_empty && state.active_loop.take().is_some() {
                notice(state, Level::Info, "loop stopped".into());
                return Vec::new();
            }
            // `Esc Esc` on an idle empty composer opens the rewind timeline
            // (T26.2); a lone Esc still reaches the composer for vim.
            if esc_idle_empty {
                let armed = state.esc_armed.take();
                if armed.is_some_and(|t| state.tick.saturating_sub(t) <= ESC_ESC_TICKS) {
                    return open_rewind(state);
                }
                state.esc_armed = Some(state.tick);
            } else {
                state.esc_armed = None;
            }
            // An `Enter` no binding claims (`send` moved elsewhere) is a
            // newline rather than the composer's own submit.
            if key.code == KeyCode::Enter {
                return compose(state, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
            }
            compose(state, key)
        }
    }
}

/// A keymap action (T25.5); `None` leaves the key to the modal or the
/// composer. With a modal open only the view toggles and quit act, and a
/// binding on a plain character acts only on an empty composer.
fn run(state: &mut State, action: keymap::Action, key: KeyEvent) -> Option<Vec<Cmd>> {
    use keymap::Action as A;
    let plain = matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let global = matches!(action, A::Quit | A::Thinking | A::Transcript | A::Expand);
    if (plain && !state.composer.is_empty()) || (state.modal.is_some() && !global) {
        return None;
    }
    let enter = |modifiers| KeyEvent::new(KeyCode::Enter, modifiers);
    Some(match action {
        A::Quit => vec![Cmd::Quit],
        // T33.25, PL§8: arms `on_key`'s leader check for the very next key.
        A::PluginLeader => {
            state.plugin_leader_armed = true;
            Vec::new()
        }
        A::Thinking => toggle(&mut state.show_thinking),
        A::Transcript => toggle(&mut state.show_diffs),
        A::Expand => {
            let last_tool = state
                .transcript
                .iter()
                .rposition(|c| matches!(c, Cell::Tool { .. }));
            match last_tool {
                Some(i) => toggle_fold(state, i),
                None => Vec::new(),
            }
        }
        A::Diff => vec![Cmd::Ask(Ask::GitDiff)],
        // `Ctrl+B` (T27.1): the newest pending `bash`/`agent` card becomes a
        // background task; the turn goes on without waiting for it.
        A::Background => {
            let call_id = state.detachable_call()?;
            vec![Cmd::Submit(Submission::Background { call_id })]
        }
        // The queue's tail back for editing (T25.1); a non-empty composer
        // keeps its usual line-kill.
        A::Unqueue => {
            if !state.composer.is_empty() {
                return None;
            }
            if let Some(text) = state.queue.pop_back() {
                state.composer.set_text(&text);
            }
            Vec::new()
        }
        A::Help => {
            state.modal = Some(Modal::Help);
            Vec::new()
        }
        A::ModeCycle => set_mode(state, commands::next_mode(state.mode)),
        // In vim's normal and visual modes `Esc` never interrupts (T25.4):
        // there it only cancels a pending command; `Ctrl+C` still does.
        A::Interrupt => {
            let vim_owns_esc = state.composer.vim_mode().is_some_and(|m| m != Mode::Insert);
            if key.code == KeyCode::Esc && vim_owns_esc {
                return None;
            }
            vec![Cmd::Submit(Submission::Interrupt)]
        }
        // The composer decides what an `Enter` does; these hand it the one
        // each action means, whatever key was bound.
        A::Voice => crate::voice::toggle(state),
        A::Send => compose(state, enter(KeyModifiers::NONE)),
        A::Newline => compose(state, enter(KeyModifiers::SHIFT)),
        A::SendNow => compose(state, enter(KeyModifiers::ALT)),
        // Nothing to copy falls through to the composer, same as
        // `A::Background` with no pending call.
        A::Copy => {
            let text = cell_text(state.transcript.last()?).to_string();
            copy_text(state, text)
        }
        A::CopyAll => {
            if state.transcript.is_empty() {
                return None;
            }
            let text: Vec<&str> = state.transcript.iter().map(cell_text).collect();
            copy_text(state, text.join("\n\n"))
        }
    })
}

/// The plain text `y`/`Y` send to the clipboard: the cell's own stored
/// string, not `cells::cell_lines`' wrapped, glyph-prefixed render — a
/// paste elsewhere wants the source text, not this terminal's width.
fn cell_text(cell: &Cell) -> &str {
    match cell {
        Cell::User { text, .. }
        | Cell::Assistant { text, .. }
        | Cell::Thinking { text, .. }
        | Cell::Notice { text, .. }
        | Cell::Error { text, .. }
        | Cell::Summary { text }
        | Cell::TaskMessage { text, .. } => text,
        Cell::Tool { output, .. } => output,
    }
}

/// `Cmd::Copy` when the terminal draws OSC 52 (T23.0 `caps.osc52`); a
/// terminal without it would just print the escape as visible text, so it
/// gets a notice instead of bytes it cannot use.
fn copy_text(state: &mut State, text: String) -> Vec<Cmd> {
    if !state.caps.osc52 {
        notice(
            state,
            Level::Info,
            "clipboard: terminal does not support OSC 52".into(),
        );
        return Vec::new();
    }
    vec![Cmd::Copy(text)]
}

fn toggle(flag: &mut bool) -> Vec<Cmd> {
    *flag = !*flag;
    Vec::new()
}

/// Flips transcript index `i`'s membership in `state.expanded` (T22.9),
/// shared by `Ctrl+E` (always the last tool cell) and a click (whichever
/// cell `on_mouse` hit-tested).
fn toggle_fold(state: &mut State, i: usize) -> Vec<Cmd> {
    if !state.expanded.remove(&i) {
        state.expanded.insert(i);
    }
    Vec::new()
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

fn on_event(state: &mut State, ev: Event) -> Vec<Cmd> {
    if let Some(banner) = Banner::from_event(&ev) {
        state.banner = Some(banner);
        return Vec::new();
    }
    let mut cmds = Vec::new();
    match ev {
        Event::ItemStarted { item, kind } => match kind {
            ItemKind::UserMessage { text, attachments } => {
                state.turns.push(TurnRow {
                    seq: state.current_seq,
                    text: text.clone(),
                    files: 0,
                    cell_at: state.transcript.len(),
                });
                state.transcript.push(Cell::User {
                    text,
                    attachments: attachments.into_iter().map(|a| a.name).collect(),
                });
            }
            ItemKind::AssistantMessage { text } => state.transcript.push(Cell::Assistant {
                item,
                text,
                done: false,
                render: ItemRender::Builtin,
            }),
            // T39.2 keeps a tool call's signature as an empty signed item
            // for the provider's history: a replay token, not something the
            // model said. A streamed thought also starts empty, but unsigned.
            ItemKind::Thinking {
                text,
                signature: Some(_),
            } if text.is_empty() => {}
            ItemKind::Thinking { text, .. } => state.transcript.push(Cell::Thinking {
                item,
                text,
                done: false,
            }),
            ItemKind::Summary { text } => state.transcript.push(Cell::Summary { text }),
            ItemKind::Notice { level, text } => state.transcript.push(Cell::Notice { level, text }),
            // Tool items arrive as `ToolCallRequested`/`ToolCallDone` too;
            // those carry the streamed output, so they own the cell.
            ItemKind::ToolCall { .. } | ItemKind::ToolResult { .. } => {}
        },
        Event::TextDelta { item, text } | Event::ThinkingDelta { item, text } => {
            if let Some(Cell::Assistant { text: t, .. } | Cell::Thinking { text: t, .. }) =
                state.item_mut(item)
            {
                t.push_str(&text);
            }
        }
        // The desktop's fold header shows the duration (A91); the TUI's
        // thinking cell has no header to put it in.
        Event::ThinkingDone { .. } => {}
        Event::ItemDone { item } => {
            if let Some(Cell::Assistant { done, .. } | Cell::Thinking { done, .. }) =
                state.item_mut(item)
            {
                *done = true;
            }
            cmds = crate::item_render::ask(state, CellRef::Item(item));
        }
        Event::ToolCallRequested { call } => {
            let user = call.name == "bash" && state.shell.take().is_some();
            if user {
                state.shell_call = Some(call.id);
                state.status.busy = true;
            }
            state.transcript.push(Cell::Tool {
                call: Box::new(call),
                output: String::new(),
                result: None,
                started: state.tick,
                user,
                render: ItemRender::Builtin,
            });
        }
        Event::ToolCallOutput { call_id, delta } => {
            if let Some(Cell::Tool { output, .. }) = state.tool_mut(call_id) {
                output.push_str(&delta);
            }
        }
        Event::ToolCallDone { call_id, result } => {
            let mut todo = None;
            if let Some(Cell::Tool {
                call, result: r, ..
            }) = state.tool_mut(call_id)
            {
                if call.name == "todo" && result.ok {
                    todo = result.todo_list();
                }
                *r = Some(result);
            }
            if let Some(todo) = todo {
                state.todo = todo;
            }
            cmds = crate::item_render::ask(state, CellRef::Call(call_id));
            // A `!` line has no `TurnDone`; its card closing ends it, and
            // a message queued behind it goes out as a turn would release it.
            if state.shell_call == Some(call_id) {
                state.shell_call = None;
                state.status.busy = false;
                if let Some(text) = state.queue.pop_front() {
                    cmds.push(Cmd::Submit(user_turn(state, text)));
                }
            }
        }
        Event::ApprovalRequired { call, why, source } => {
            cmds = notify(state, format!("approval: {} {}", call.name, call.subject));
            let agent = source.and_then(|s| s.agent);
            state.modal = Some(Modal::Approval(Approval::new(call, why).from_agent(agent)));
        }
        Event::ApprovalDecided { .. } => state.modal = None,
        Event::TurnStarted {
            seq, tier, model, ..
        } => {
            state.current_seq = seq;
            state.status.busy = true;
            // A `!` the core refused never got its card.
            state.shell = None;
            state.status.tier = Some(tier);
            state.status.model = model.to_string();
        }
        Event::TurnDone { stop, .. } => {
            state.status.busy = false;
            // An interrupt is the user's own doing; they are already here.
            if stop != StopReason::Interrupted {
                cmds = notify(state, "turn done".to_string());
            }
            cmds.extend(turn_done_cmds(state, stop));
        }
        Event::Usage { usage, .. } => {
            state.status.cost_usd += usage.cost_usd;
            state.status.context_tokens =
                usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
            state.status.cache_ratio = cox_core::cache_diag::ratio_of(&usage);
        }
        Event::ModelSwitched { tier, to, .. } => {
            if state.status.tier == Some(tier) {
                state.status.model = to.to_string();
            }
        }
        // T22.1, DT G4: `ask_user` (or a subagent's, T34.3) waits in a modal.
        Event::QuestionAsked {
            call_id,
            question,
            options,
            source,
        } => {
            cmds = notify(state, format!("question: {question}"));
            state.modal = Some(Modal::Question(
                Question::new(call_id, question, options).from_agent(source.and_then(|s| s.agent)),
            ));
        }
        // The core's word on mode and effort replaces what a key or a
        // slash command set ahead of it (DT G5).
        Event::StateChanged { mode, effort } => {
            state.mode = mode;
            state.status.effort = effort;
        }
        // A113: the status line shows it; the sessions picker reads the store.
        Event::TitleSet { title, .. } => state.title = Some(title),
        // A98: the status line's share and the `/context` overlay.
        Event::ContextBreakdown { breakdown, .. } => {
            if let Some(window) = breakdown.window.filter(|w| *w > 0) {
                state.status.context_window = window;
            }
            state.status.context = Some(breakdown);
        }
        // T33.20: the `TurnStarted` that follows already carries the
        // advised tier and model, so the status line needs nothing more.
        Event::Advised { .. } => {}
        Event::TaskCreated { task, label, tier } => {
            state.tasks.push((task, label, tier, state.tick, None));
        }
        Event::TaskCompleted {
            task,
            exit_code,
            archive,
            ..
        } => {
            if let Some(i) = state.tasks.iter().position(|(t, ..)| *t == task) {
                let (_, label, ..) = state.tasks.remove(i);
                let line = tasks::finished_line(task, &label, exit_code, archive);
                state.finished_tasks.push(line);
                let over = state
                    .finished_tasks
                    .len()
                    .saturating_sub(tasks::FINISHED_KEPT);
                state.finished_tasks.drain(..over);
            }
        }
        // T34.7/SM§6: `task` is always the task this message concerns —
        // the addressee delivered to (`deliver`), or, when a child speaks
        // to the parent (`message_parent`), its own id doubling as `from`
        // (`tasks.rs`: "the parent has no id"). Resolved once here to the
        // task's registered label, since a finished task is already gone
        // from `state.tasks` by the time an in-flight reply renders.
        Event::TaskMessage {
            task, from, text, ..
        } => {
            let label = state
                .tasks
                .iter()
                .find(|(t, ..)| *t == task)
                .map_or_else(|| task.to_string(), |(_, label, ..)| label.clone());
            let label = crate::text::sanitize(&label);
            let text = crate::text::sanitize(&text);
            if let Some(entry) = state.tasks.iter_mut().find(|(t, ..)| *t == task) {
                entry.4 = Some(text.lines().next().unwrap_or("").to_string());
            }
            state.transcript.push(Cell::TaskMessage {
                label,
                from_task: from == Some(task),
                text,
            });
        }
        Event::Notice { level, text } => state.transcript.push(Cell::Notice { level, text }),
        Event::Error { error, fatal } => state.transcript.push(Cell::Error {
            text: error.to_string(),
            fatal,
        }),
        Event::Checkpoint { files, .. } => {
            if let Some(turn) = state.turns.last_mut() {
                turn.files += files.len();
            }
        }
        Event::Rewound {
            to_turn,
            conversation,
            ..
        } => {
            if conversation && let Some(at) = state.turns.iter().position(|t| t.seq >= to_turn) {
                let cut = state.turns[at].cell_at;
                state.transcript.truncate(cut);
                state.turns.truncate(at);
            }
        }
        Event::ModeChanged {
            mode,
            permission_mode,
        } => mode_changed(state, mode, permission_mode),
        Event::SessionStarted { .. }
        | Event::Compacted { .. }
        | Event::GrantRevoked { .. }
        | Event::RepoMapBuilt { .. } => {}
    }
    cmds
}

/// `Event::TurnDone` (T25.1): a natural finish drains the queue's head as
/// the next turn; an interrupt from `send_now` instead joins everything
/// queued (composer text included, folded in by `send_now`) into the one
/// turn `Ctrl+Enter`/`Alt+Enter` asked for. A plain `Ctrl+C` interrupt
/// (`send_now` unset) leaves the queue untouched — the user cancelled, they
/// did not ask to send it.
fn turn_done_cmds(state: &mut State, stop: StopReason) -> Vec<Cmd> {
    let text = match stop {
        StopReason::Interrupted if state.send_now => {
            state.send_now = false;
            let joined = state.queue.drain(..).collect::<Vec<_>>().join("\n\n");
            (!joined.is_empty()).then_some(joined)
        }
        StopReason::Interrupted => None,
        _ => state.queue.pop_front(),
    };
    match text {
        Some(text) => vec![Cmd::Submit(user_turn(state, text))],
        None => Vec::new(),
    }
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

/// `Event::ModeChanged` (P42): the core's word on the mode. Entering
/// architect asks the think price once per stretch, reusing the `ask_user`
/// modal; leaving it forgets the answer, so the next stretch asks again.
fn mode_changed(state: &mut State, mode: SessionMode, permission_mode: PermissionMode) {
    state.mode = permission_mode;
    state.session_mode = mode;
    if mode != SessionMode::Architect {
        state.think_confirmed = false;
        return;
    }
    let asking = matches!((&state.modal, state.think_consent),
        (Some(Modal::Question(q)), Some(id)) if q.call == id);
    if state.think_confirmed || asking {
        return;
    }
    let question = format!(
        "architect mode runs main turns on the think tier ({}); use it?",
        cox_core::router::THINK_PRICE
    );
    if state.modal.is_none() {
        let id = CallId::new();
        state.think_consent = Some(id);
        state.modal = Some(Modal::Question(Question::new(
            id,
            question,
            vec!["yes".into(), "no".into()],
        )));
    } else {
        // The one modal slot is taken: the first turn's refusal names the
        // price instead, and `/mode architect` asks again.
        notice(
            state,
            Level::Warn,
            "architect: think price not confirmed yet; run /mode architect again".into(),
        );
    }
}

/// The think-price question's answer (P42). Yes confirms think for the
/// rest of this architect stretch; anything else leaves architect.
fn think_consent_answered(state: &mut State, answer: QuestionAnswer) -> Vec<Cmd> {
    state.think_consent = None;
    let yes = matches!(&answer, QuestionAnswer::Text(t)
        if matches!(t.trim().to_ascii_lowercase().as_str(), "y" | "yes"));
    if yes {
        state.think_confirmed = true;
        return Vec::new();
    }
    vec![Cmd::Submit(Submission::Command {
        command: SlashCommand {
            name: "mode".into(),
            args: vec!["editor".into()],
        },
    })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use cox_core::{History, HistoryTurn};
    use cox_protocol::ids::TurnId;
    use cox_protocol::types::{Content, Message, PermissionMode, Role, SandboxMode};
    use crossterm::event::{KeyCode, KeyEvent};

    /// Types `text` into the composer and submits it with a plain `Enter`,
    /// the way a user queues or sends a message.
    fn type_line(state: &mut State, text: &str) {
        for c in text.chars() {
            update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    }

    /// Types a slash command and submits it. `/` at column 0 opens the
    /// palette (`Edit::OpenCommands`) but already left the `/` itself in the
    /// composer, so — as `clear_command_emits_cmd_clear` established — `Esc`
    /// closes the palette without losing it, then the rest types normally.
    fn type_command(state: &mut State, line: &str) -> Vec<Cmd> {
        type_after_sigil(state, '/', line)
    }

    /// `line` after the picker its leading `sigil` opens was closed with
    /// `Esc`, which keeps the sigil in the composer; then `Enter`.
    fn type_after_sigil(state: &mut State, sigil: char, line: &str) -> Vec<Cmd> {
        let rest = line.strip_prefix(sigil).unwrap_or(line);
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(sigil))));
        update(state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in rest.chars() {
            update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(state, Msg::Key(KeyEvent::from(KeyCode::Enter)))
    }

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

    /// T27.1: `Ctrl+B` backgrounds the newest pending `bash` card; with no
    /// such card it is not swallowed as a background request.
    #[test]
    fn ctrl_b_backgrounds_the_pending_bash_card() {
        use cox_protocol::types::{Risk, ToolCall};
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let ctrl_b = || Msg::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        // A pending call only exists inside a running turn, and `Ctrl+B` is a
        // running-turn key (T25.5).
        state.status.busy = true;
        assert_eq!(update(&mut state, ctrl_b()), Vec::new());
        let call_id = CallId::new();
        update(
            &mut state,
            Msg::Event(Event::ToolCallRequested {
                call: ToolCall {
                    id: call_id,
                    name: "bash".into(),
                    input: serde_json::json!({"command": "sleep 5"}),
                    risk: Risk::Exec,
                    subject: "sleep 5".into(),
                    segments: None,
                },
            }),
        );
        assert_eq!(
            update(&mut state, ctrl_b()),
            vec![Cmd::Submit(Submission::Background { call_id })]
        );
    }

    /// T23.5: `auto` rings on a finished turn, an approval and a question
    /// only while the terminal is unfocused; `always` ignores focus, `off`
    /// never rings, and an interrupt the user made never does.
    #[test]
    fn update_emits_notify_only_when_unfocused() {
        let turn_done = |stop| {
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop,
            })
        };
        let rings = |cmds: &[Cmd]| cmds.iter().any(|c| matches!(c, Cmd::Notify { .. }));
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert!(!rings(&update(&mut state, turn_done(StopReason::EndTurn))));

        update(&mut state, Msg::Focus(false));
        assert_eq!(
            update(&mut state, turn_done(StopReason::EndTurn)),
            vec![Cmd::Notify {
                title: "cox".into(),
                body: "turn done".into(),
            }]
        );
        assert!(!rings(&update(
            &mut state,
            turn_done(StopReason::Interrupted)
        )));
        let call = ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: serde_json::Value::Null,
            risk: cox_protocol::types::Risk::Exec,
            subject: "cargo test".into(),
            segments: None,
        };
        let why = cox_protocol::types::Why::Risk {
            risk: cox_protocol::types::Risk::Exec,
        };
        let cmds = update(
            &mut state,
            Msg::Event(Event::ApprovalRequired {
                call,
                why,
                source: None,
            }),
        );
        assert!(
            matches!(&cmds[..], [Cmd::Notify { body, .. }] if body == "approval: bash cargo test")
        );
        let question = Msg::Event(Event::QuestionAsked {
            call_id: CallId::new(),
            question: "which?".into(),
            options: Vec::new(),
            source: None,
        });
        assert!(rings(&update(&mut state, question)));

        update(&mut state, Msg::Focus(true));
        state.notify = Notify::Always;
        assert!(rings(&update(&mut state, turn_done(StopReason::EndTurn))));
        state.notify = Notify::Off;
        update(&mut state, Msg::Focus(false));
        assert!(!rings(&update(&mut state, turn_done(StopReason::EndTurn))));
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

    /// T23.4: `y` on an empty composer copies the last cell still held as
    /// plain text, only when the terminal draws OSC 52; without the
    /// capability it gets a notice instead of bytes it cannot use, and a
    /// non-empty composer keeps typing "y" rather than copying.
    #[test]
    fn overlay_y_emits_copy_of_cell() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc52 = true;
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "cell text".into(),
        });
        let y = || Msg::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(update(&mut state, y()), vec![Cmd::Copy("cell text".into())]);

        let mut unsupported = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        unsupported.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "cell text".into(),
        });
        assert_eq!(update(&mut unsupported, y()), Vec::new());
        assert!(matches!(
            unsupported.transcript.last(),
            Some(Cell::Notice { text, .. }) if text.contains("does not support OSC 52")
        ));

        let mut typing = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        typing.caps.osc52 = true;
        typing.composer.insert("yak");
        assert_eq!(update(&mut typing, y()), Vec::new());
        assert_eq!(typing.composer.text(), "yaky");
    }

    /// T23.4: `Shift+Y` joins every cell still held; nothing held is a
    /// no-op rather than a notice about an empty clipboard.
    #[test]
    fn shift_y_emits_copy_of_the_whole_transcript() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc52 = true;
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "one".into(),
        });
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "two".into(),
        });
        let shift_y = Msg::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        assert_eq!(
            update(&mut state, shift_y),
            vec![Cmd::Copy("one\n\ntwo".into())]
        );

        let mut empty = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        empty.caps.osc52 = true;
        let shift_y = Msg::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        assert_eq!(update(&mut empty, shift_y), Vec::new());
    }

    /// A98: `Event::ContextBreakdown` sets the window `ctx N%` divides by
    /// and keeps the split; one with no window leaves the last known one,
    /// and `/context` opens the overlay without asking the core.
    #[test]
    fn context_breakdown_sets_the_window_and_the_split() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let event = |window| {
            Msg::Event(Event::ContextBreakdown {
                turn: TurnId::new(),
                breakdown: ContextBreakdown {
                    window,
                    total: 50_000,
                    system: 5_000,
                    tools: 20_000,
                    instructions: 1_000,
                    history: 24_000,
                    cached: 0,
                },
            })
        };
        update(&mut state, event(Some(1_000_000)));
        assert_eq!(state.status.context_window, 1_000_000);
        assert_eq!(state.status.context_used(), 50_000, "estimate before usage");
        update(&mut state, event(None));
        assert_eq!(state.status.context_window, 1_000_000);
        assert_eq!(state.status.context.and_then(|b| b.window), None);
        assert_eq!(type_command(&mut state, "/context"), Vec::new());
        assert_eq!(state.modal, Some(Modal::Context));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(state.modal, None);
    }

    #[test]
    fn empty_signed_thinking_draws_no_cell() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let before = state.transcript.len();
        let signed = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: Some("sig".into()),
        };
        update(
            &mut state,
            Msg::Event(Event::ItemStarted { item: signed, kind }),
        );
        update(&mut state, Msg::Event(Event::ItemDone { item: signed }));
        assert_eq!(state.transcript.len(), before);

        // A streamed thought starts empty too, but unsigned: it keeps its cell.
        let streamed = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: None,
        };
        update(
            &mut state,
            Msg::Event(Event::ItemStarted {
                item: streamed,
                kind,
            }),
        );
        assert!(matches!(
            state.transcript.last(),
            Some(Cell::Thinking { item, .. }) if *item == streamed
        ));
    }

    /// P42: entering architect asks the think price once; a yes rides on
    /// every architect turn, a repeat `/mode architect` does not ask again,
    /// leaving forgets it, and a no sends `/mode editor`.
    #[test]
    fn architect_confirmation_is_asked_once() {
        let key = |c| Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        let architect = Event::ModeChanged {
            mode: SessionMode::Architect,
            permission_mode: PermissionMode::Plan,
        };
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        update(&mut state, Msg::Event(architect.clone()));
        assert!(matches!(state.modal, Some(Modal::Question(_))));
        assert_eq!(state.mode, PermissionMode::Plan);
        // `1` picks "yes"; the answer never reaches the core.
        assert_eq!(update(&mut state, key('1')), Vec::new());
        assert!(state.modal.is_none());
        let confirmed = |state: &State| {
            matches!(
                user_turn(state, "go".into()),
                Submission::UserTurn {
                    confirm_think: true,
                    ..
                }
            )
        };
        assert!(confirmed(&state));

        update(&mut state, Msg::Event(architect.clone()));
        assert!(state.modal.is_none(), "asked once per stretch");

        update(
            &mut state,
            Msg::Event(Event::ModeChanged {
                mode: SessionMode::Editor,
                permission_mode: PermissionMode::Default,
            }),
        );
        assert!(!confirmed(&state), "editor turns never confirm think");

        update(&mut state, Msg::Event(architect));
        assert!(matches!(state.modal, Some(Modal::Question(_))));
        // `2` picks "no": leave architect.
        assert_eq!(
            update(&mut state, key('2')),
            vec![Cmd::Submit(Submission::Command {
                command: SlashCommand {
                    name: "mode".into(),
                    args: vec!["editor".into()],
                },
            })]
        );
        assert!(!confirmed(&state));
    }
}
