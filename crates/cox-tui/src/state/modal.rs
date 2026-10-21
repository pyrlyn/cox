// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Modal`: the one slot `State` draws over the transcript and routes keys
//! to first. The variant list is separate from `update` so adding an overlay
//! does not mean editing the fold; the dialogs' own state lives in
//! `crate::modal`, `crate::picker` and `crate::theme_editor`.

use cox_protocol::ids::SessionId;

use super::Cell;
use crate::modal::{Approval, PluginGrantDialog, Question, RemoveConfirm};
use crate::picker::Picker;
use crate::theme_editor::ThemeEditor;

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
