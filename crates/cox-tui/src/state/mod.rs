// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! TEA state for the TUI (T5.1): `State`, `Msg`, `Cmd` and the pure
//! `update`. No async, no I/O, no terminal: the runtime (`app`) feeds it key
//! and core events and executes the `Cmd`s it returns, and a test feeds it
//! the same `Event`s a real session emits, so every screen is replayable.
//!
//! This file holds `State`, `TurnRow` and the dispatch (`update`, `step`);
//! each kind of input is folded in its own submodule (`event`, `keys`,
//! `mouse`, `submit`, `act`, `plugin`, `theme`) and the plain data types
//! (`msg`, `cell`, `modal`, `status`) are re-exported here, so every
//! `cox_tui::state::*` path stays as it was before the split (#118).

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ops::Range;

use cox_protocol::ids::{CallId, ItemId, TaskId};
use cox_protocol::types::{
    Content, Level, Mode as SessionMode, PermissionMode, Presence, Role, SandboxMode, Tier,
    TodoItem,
};

use crate::banner::Banner;
use crate::cells::Look;
use crate::color::Depth;
use crate::commands::{COMMANDS, Context};
use crate::composer::Composer;
use crate::glyph::{self, Glyphs};
use crate::item_render::ItemRender;
use crate::keymap::Keymap;
use crate::markdown;
use crate::modal::PluginGrantDialog;
use crate::pointer::Pointer;
use crate::term::{Caps, Progress};
use crate::theme::{Theme, ThemeFile};

mod act;
mod cell;
mod event;
mod keys;
mod modal;
mod mouse;
mod msg;
mod plugin;
mod status;
mod submit;
#[cfg(test)]
mod testkit;
mod theme;

pub use cell::Cell;
pub use keys::ESC_ESC_TICKS;
pub use modal::Modal;
pub use msg::{
    Ask, Cmd, GrantDecision, Msg, PluginMgmtRequest, PluginNewRequest, PluginRequest, PluginUiMsg,
};
pub use status::{GitStatus, Notify, Status};
pub use submit::Loop;
pub(crate) use submit::compose;

use act::notice;
use cell::replay_cells;
use event::on_event;
use keys::on_key;
use mouse::on_mouse;
use plugin::{declare_plugin_commands, plugin_command_out};
use submit::loop_tick;
use theme::apply_theme_choice;

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

#[cfg(test)]
mod tests {

    use super::*;

    use cox_core::{History, HistoryTurn};
    use cox_protocol::ids::TurnId;

    use cox_protocol::types::{
        Content, Event, Message, PermissionMode, Role, SandboxMode, StopReason, ToolCall,
    };

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
}
