// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What the runtime feeds `update` (`Msg`) and what `update` asks the
//! runtime to do (`Cmd`), with the request and answer types they carry.
//! Types only, kept apart from `State` and `update` so the TEA vocabulary
//! reads in one place; `state` re-exports every name at its old path.

use cox_protocol::GrantScope;
use cox_protocol::ids::SessionId;
use cox_protocol::plugin::{CommandDecl, CommandOut, KeyDecl, RenderIn, Slot, Widget};
use cox_protocol::types::{Event, Presence, Submission};
use crossterm::event::{KeyEvent, MouseEvent};

use super::GitStatus;
use crate::item_render::{CellRef, RenderSource};
use crate::pointer::Pointer;
use crate::term::Progress;
use crate::theme::ThemeFile;

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
    /// The runtime's answer to `Ask::Refine` (T66.8).
    Refine(crate::refine::Msg),
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
    /// `/refine` (T66.8): propose, apply the reviewed proposal, list or
    /// undo; the runtime answers `Msg::Refine`.
    Refine(crate::refine::Request),
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
    /// OSC 22 pointer shape (T5.9); only sent when `caps.osc22` and the
    /// shape differs from the last one written.
    Pointer(Pointer),
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
