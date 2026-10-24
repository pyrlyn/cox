// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One transcript entry; a finished cell leaves the viewport for the
//! terminal's scrollback. Split out of `state` so the entry type and its
//! pure helpers read apart from the `update` machinery that builds them.

use cox_protocol::ids::ItemId;
use cox_protocol::types::{Event, Level, PermissionMode, SandboxMode, ToolCall, ToolResult};

use super::{Msg, State, update};
use crate::item_render::ItemRender;

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

/// Replays a rollout's events through the same `update`/`Msg::Event` path a
/// live turn uses, into a scratch `State` nobody else sees, so the
/// `/agents` overlay (T27.5) renders through the identical cell-building
/// code instead of a second renderer — the same technique `tests/cells.rs`'s
/// fixture replay already uses.
pub(super) fn replay_cells(events: Vec<Event>) -> Vec<Cell> {
    let mut scratch = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    for ev in events {
        update(&mut scratch, Msg::Event(ev));
    }
    scratch.transcript
}

/// The plain text `y`/`Y` send to the clipboard: the cell's own stored
/// string, not `cells::cell_lines`' wrapped, glyph-prefixed render — a
/// paste elsewhere wants the source text, not this terminal's width.
pub(super) fn cell_text(cell: &Cell) -> &str {
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
