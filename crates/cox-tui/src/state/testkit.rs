// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Test-only helpers shared by the `state` submodules' tests: a line, a slash
//! command or a sigil line typed into the composer through `update`, the way
//! a user types it. One copy here, since each `state/<part>.rs` keeps its own
//! tests and several of them type lines.

use crossterm::event::{KeyCode, KeyEvent};

use super::{Cmd, Msg, State, update};

/// Types `text` into the composer and submits it with a plain `Enter`,
/// the way a user queues or sends a message.
pub(super) fn type_line(state: &mut State, text: &str) {
    for c in text.chars() {
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    update(state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
}

/// Types a slash command and submits it. `/` at column 0 opens the
/// palette (`Edit::OpenCommands`) but already left the `/` itself in the
/// composer, so — as `clear_command_emits_cmd_clear` established — `Esc`
/// closes the palette without losing it, then the rest types normally.
pub(super) fn type_command(state: &mut State, line: &str) -> Vec<Cmd> {
    type_after_sigil(state, '/', line)
}

/// `line` after the picker its leading `sigil` opens was closed with
/// `Esc`, which keeps the sigil in the composer; then `Enter`.
pub(super) fn type_after_sigil(state: &mut State, sigil: char, line: &str) -> Vec<Cmd> {
    let rest = line.strip_prefix(sigil).unwrap_or(line);
    update(state, Msg::Key(KeyEvent::from(KeyCode::Char(sigil))));
    update(state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    for c in rest.chars() {
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    update(state, Msg::Key(KeyEvent::from(KeyCode::Enter)))
}
