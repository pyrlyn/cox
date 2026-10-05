// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The block model and `TimelinePatch` (DT§4.3): what a UI sees instead of
//! `Event`. Separate from the fold so `cox-ffi` (T37.14) mirrors plain data
//! without the fold's bookkeeping; every type is serde, so a recorded patch
//! stream is a fixture the Swift tests replay (DT§8).

use std::path::PathBuf;

use cox_protocol::ids::{CallId, TaskId};
use cox_protocol::types::{
    ArchiveRef, CompactReason, DecidedBy, Decision, Effort, Level, ModelId, PermissionMode, Risk,
    Source, StopReason, Tier, Usage, Why,
};
use cox_render::diffmodel::DiffModel;
use cox_render::doc::{Block as DocBlock, StyledDoc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plugin_ui::{PluginSlot, WidgetView};
use crate::summary::Icon;
use crate::tasks::TaskKind;
use crate::usage::UsageView;

/// How many trailing lines of output a running `Tool` block keeps.
pub const TAIL_LINES: usize = 5;

/// A block's key. It is derived from the ids in the events that build the
/// block (or, for blocks without one, from the event's position), so a
/// rollout replayed from the start keys every block as the live run did.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub String);

/// One transcript block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    /// The turn's ordinal (`TurnStarted.seq`); 0 before the first turn.
    pub turn: u32,
    pub kind: BlockKind,
}

/// What a block shows (DT§4.3 block table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    /// Attachments by name; their bytes stay in the rollout.
    User {
        text: String,
        attachments: Vec<String>,
    },
    /// `text` is the markdown source, `doc` its parse.
    Assistant {
        text: String,
        doc: StyledDoc,
        /// A plugin's `item:assistant_message` renderer's tree (T52.23.1),
        /// kept as `Tool`'s is; drawn in place of the reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_view: Option<WidgetView>,
    },
    /// `duration_ms` is `None` while the thought streams; its
    /// `ThinkingDone` sets it, and the fold header reads "Thought for 12 s".
    Thinking {
        text: String,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    Tool {
        tool: String,
        /// The one-line summary (`summary::summary`), past tense once done.
        summary: String,
        icon: Icon,
        risk: Risk,
        state: ToolState,
        /// The last `TAIL_LINES` lines of output.
        tail: String,
        archive: Option<ArchiveRef>,
        /// An edit's change as hunks of numbered, highlighted lines, so no
        /// UI parses unified text (T37.23.5).
        diff: Option<DiffModel>,
        duration_ms: u64,
        /// A plugin's `tool:` renderer's tree (T52.23.1, PL§8), sanitized and
        /// bounded like a slot's; the card draws it in place of the generic
        /// one. Display only: never in the rollout or the model's history.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_view: Option<WidgetView>,
    },
    /// Consecutive read/grep/glob/outline calls of one step: "Explored 3
    /// files". The children stay `Tool` blocks right after this one, so
    /// their patches are unchanged; a UI shows them when it expands.
    ToolGroup {
        summary: String,
        children: Vec<BlockId>,
        /// `Running` while any child runs, else `Failed` if any failed.
        state: ToolState,
    },
    Approval {
        call: CallId,
        tool: String,
        summary: String,
        /// The call's input as the model sent it: what Edit… starts from
        /// (T37.27.6).
        input: Value,
        /// The subjects "Allow for session" would grant `tool`, as the
        /// engine records them (`grants_for`): one per command of a split
        /// line. Shown only; the engine alone decides what a grant covers.
        grants: Vec<String>,
        why: Why,
        source: Option<Source>,
        /// `None` while pending.
        decision: Option<Decision>,
        by: Option<DecidedBy>,
    },
    Question {
        call: CallId,
        question: String,
        options: Vec<String>,
        /// `None` while pending.
        answer: Option<String>,
    },
    Task {
        task: TaskId,
        label: String,
        tier: Tier,
        done: bool,
        cost_usd: f64,
        exit_code: Option<i32>,
        /// Read by the Tasks tab and the task card, so neither reads
        /// `exit_code` itself (T58.4.23).
        state: TaskState,
        /// Which the Tasks tab labels it; a click opens a transcript or an
        /// output (T37.29.6).
        kind: TaskKind,
    },
    Compaction {
        before_tokens: u32,
        after_tokens: u32,
        reason: CompactReason,
        /// The summary that replaced the dropped turns; `None` when the
        /// stream did not carry its `Summary` item.
        summary: Option<String>,
    },
    Checkpoint {
        files: Vec<PathBuf>,
    },
    Notice {
        level: Level,
        text: String,
    },
    Error {
        text: String,
        fatal: bool,
    },
    /// Summed over the turn's provider calls; `stop` once it ended.
    TurnMeta {
        model: ModelId,
        tier: Tier,
        usage: Option<Usage>,
        stop: Option<StopReason>,
    },
}

/// Where a tool call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// Where a task stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Succeeded,
    Failed,
}

impl TaskState {
    /// A finished task: no exit code (a subagent, which has none) is a
    /// success, as is `0`.
    pub fn ended(exit_code: Option<i32>) -> Self {
        match exit_code {
            None | Some(0) => Self::Succeeded,
            Some(_) => Self::Failed,
        }
    }
}

/// One change to the block list, keyed by id so a UI's identity is stable
/// and a dropped patch is healed by `Reset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TimelinePatch {
    /// The whole list: open, resume, or healing after a gap.
    Reset {
        blocks: Vec<Block>,
    },
    /// Replaces the block with this id in place, or inserts it after
    /// `after` (`None`: first).
    Upsert {
        block: Box<Block>,
        after: Option<BlockId>,
    },
    /// Appends to a `Thinking` block's text or a `Tool` block's tail (which
    /// then keeps its last `TAIL_LINES` lines).
    AppendText {
        id: BlockId,
        text: String,
    },
    /// Replaces an `Assistant` doc's blocks from index `from` on: blocks a
    /// stream has closed are never re-sent.
    DocTail {
        id: BlockId,
        from: u32,
        blocks: Vec<DocBlock>,
    },
    Remove {
        id: BlockId,
    },
    /// The token meter's whole state (DS§7), beside the block list; a
    /// queue keeps only the latest.
    Usage {
        usage: Box<UsageView>,
    },
    /// The session's status beside the block list (DT§4.3); a queue keeps
    /// only the latest.
    Status {
        status: Status,
    },
    /// One plugin slot's whole state (T52.14, PL§8) beside the block list;
    /// a queue keeps only the latest per plugin and slot.
    PluginSlot {
        slot: Box<PluginSlot>,
    },
}

/// What the composer shows about the session, not about any one block
/// (`crate::status` folds it). `None` fields are not known yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Turns queued behind the running one that have not started yet.
    pub queued: u32,
    /// The permission mode in force.
    pub mode: Option<PermissionMode>,
    /// The mode ⇧⇥ asks for (`cox_permission::next_mode`), decided here so
    /// the app only sends it back as `Intent::SetMode`.
    pub next_mode: Option<PermissionMode>,
    /// The model the main turn runs on: the configured `code` tier's, then
    /// whatever the latest main turn ran on or `/model` switched to.
    pub model: Option<ModelId>,
    /// What a person calls that model (`Claude Sonnet 5`), from the model
    /// catalog (A111); `None` when the catalog has no name for it, and the
    /// app shows the id.
    #[serde(default)]
    pub model_name: Option<String>,
    /// The chip's form of `model_name` (`Sonnet 5`): no leading `Claude `,
    /// no trailing ` (latest)` (A111, A116). The full name stays on
    /// `model_name` so the TUI and ACP do not change.
    #[serde(default)]
    pub short_name: Option<String>,
    /// The effort that model runs at: the `/effort` override, else the
    /// `code` tier's configured effort.
    pub effort: Option<Effort>,
}

/// The last `TAIL_LINES` lines of `text`, a trailing newline kept so the
/// next appended chunk starts its own line.
pub fn tail(text: &str) -> &str {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let start = body
        .rmatch_indices('\n')
        .nth(TAIL_LINES - 1)
        .map_or(0, |(i, _)| i + 1);
    &text[start..]
}
