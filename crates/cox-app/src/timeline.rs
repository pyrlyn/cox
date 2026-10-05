// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The timeline fold (DT§4.3): `Event` in, keyed blocks updated, the
//! `TimelinePatch`es that describe the change out. Pure and deterministic,
//! so folding a rollout read back from disk yields the patches the live run
//! did — resume and live share one code path.

use std::fmt::Display;

use cox_core::permission::grants_for;
use cox_protocol::ids::{CallId, ItemId};
use cox_protocol::plugin::Widget;
use cox_protocol::types::{Event, ItemKind, TodoItem, ToolCall};
use cox_render::diffmodel;
use cox_render::glyph::UNICODE;
use cox_render::markdown;

use crate::patch::{Block, BlockId, BlockKind, TaskState, TimelinePatch, ToolState, tail};
use crate::summary::{self, Explore, one_line};
use crate::tasks::TaskKind;
use crate::usage::add_to;

/// The block list of one session and what folds events into it.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    blocks: Vec<Block>,
    /// The current turn's ordinal.
    turn: u32,
    /// Events applied so far, less `StateChanged`; keys the blocks no
    /// event id names.
    seq: u64,
    /// The syntect theme code blocks are highlighted with; a diff takes its
    /// dark and light variant (A95).
    theme: String,
    /// Calls not done yet: their input writes the done summary.
    calls: Vec<ToolCall>,
    /// The exploring calls since the last other block.
    explore: Option<Run>,
    /// A `Summary` item's text until its `Compacted` arrives.
    summary: Option<(ItemId, String)>,
    /// Each `todo` result's list with its turn, oldest first: the last is
    /// the plan, and a rewind falls back to the one before it.
    plans: Vec<(u32, Vec<TodoItem>)>,
}

/// A run of consecutive exploring calls; from its second call on it shows
/// as a `ToolGroup` keyed by the first call, so replay keys it the same.
#[derive(Debug, Clone)]
struct Run {
    group: BlockId,
    children: Vec<BlockId>,
    files: Vec<String>,
    searches: usize,
}

pub(crate) fn key(kind: &str, id: impl Display) -> BlockId {
    BlockId(format!("{kind}:{id}"))
}

impl Timeline {
    pub fn new(theme: &str) -> Self {
        Self {
            theme: theme.to_owned(),
            ..Self::default()
        }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The inspector's Plan tab (DT§5.1, T37.29.2): the list the latest
    /// `todo` call left, with each step's state; empty before one.
    pub fn plan(&self) -> &[TodoItem] {
        self.plans.last().map_or(&[], |(_, items)| items)
    }

    /// The whole list as one patch, to heal a consumer that missed some.
    pub fn reset(&self) -> TimelinePatch {
        TimelinePatch::Reset {
            blocks: self.blocks.clone(),
        }
    }

    /// Folds one event; returns the patches it caused, possibly none.
    pub fn apply(&mut self, event: &Event) -> Vec<TimelinePatch> {
        // A session records its opening mode in the rollout only (T50.4),
        // so a replay holds one `StateChanged` the live stream never had;
        // it draws no block, so it takes no key number either and replay
        // still folds to what the live session showed.
        if matches!(event, Event::StateChanged { .. }) {
            return vec![];
        }
        self.seq += 1;
        let seq = self.seq;
        match event {
            Event::TurnStarted {
                turn,
                seq,
                tier,
                model,
                ..
            } => {
                self.turn = *seq;
                let kind = BlockKind::TurnMeta {
                    model: model.clone(),
                    tier: *tier,
                    usage: None,
                    stop: None,
                };
                self.insert(key("turn", turn), kind)
            }
            Event::ItemStarted { item, kind } => {
                let kind = match kind {
                    ItemKind::UserMessage { text, attachments } => BlockKind::User {
                        text: text.clone(),
                        attachments: attachments.iter().map(|a| a.name.clone()).collect(),
                    },
                    ItemKind::AssistantMessage { text } => BlockKind::Assistant {
                        text: text.clone(),
                        doc: markdown::parse(text, &self.theme, &UNICODE),
                        plugin_view: None,
                    },
                    // T39.2 keeps a tool call's signature as an empty signed
                    // item for the provider's history; there is no thought to
                    // show, and no `ThinkingDone` would ever close it.
                    ItemKind::Thinking {
                        text,
                        signature: Some(_),
                    } if text.is_empty() => return vec![],
                    ItemKind::Thinking { text, .. } => BlockKind::Thinking {
                        text: text.clone(),
                        duration_ms: None,
                    },
                    ItemKind::Notice { level, text } => BlockKind::Notice {
                        level: *level,
                        text: text.clone(),
                    },
                    // A summary is shown by its `Compacted`, which follows.
                    ItemKind::Summary { text } => {
                        self.summary = Some((*item, text.clone()));
                        return vec![];
                    }
                    // Calls and results arrive as their own events.
                    ItemKind::ToolCall { .. } | ItemKind::ToolResult { .. } => return vec![],
                };
                self.insert(key("item", item), kind)
            }
            Event::TextDelta { item, text } => self.stream_doc(key("item", item), text),
            Event::ItemDone { item } => {
                // Closing a reply re-sends it whole: the source text reaches
                // the UI and any dropped `DocTail` is healed.
                let id = key("item", item);
                let reply = self.find(&id).map(|i| &self.blocks[i].kind);
                if matches!(reply, Some(BlockKind::Assistant { .. })) {
                    self.update(&id, |_| {})
                } else {
                    vec![]
                }
            }
            Event::ThinkingDelta { item, text } => self.append(key("item", item), text),
            Event::ThinkingDone { item, duration_ms } => self.update(&key("item", item), |k| {
                if let BlockKind::Thinking { duration_ms: d, .. } = k {
                    *d = Some(*duration_ms);
                }
            }),
            Event::ToolCallRequested { call } => {
                let kind = BlockKind::Tool {
                    tool: call.name.clone(),
                    summary: summary::summary(call, None),
                    icon: summary::icon(&call.name),
                    risk: call.risk,
                    state: ToolState::Running,
                    tail: String::new(),
                    archive: None,
                    diff: None,
                    duration_ms: 0,
                    plugin_view: None,
                };
                let run = self.explore.take();
                let id = key("call", call.id);
                let mut out = self.insert(id.clone(), kind);
                if out.is_empty() {
                    self.explore = run;
                    return out;
                }
                self.calls.push(call.clone());
                if let Some(target) = summary::explore(call) {
                    out.extend(self.explore(run, call.id, id, target));
                }
                out
            }
            Event::ToolCallOutput { call_id, delta } => self.append(key("call", call_id), delta),
            Event::ToolCallDone { call_id, result } => {
                let mut out = self.update(&key("question", call_id), |k| {
                    if let BlockKind::Question { answer, .. } = k {
                        *answer = Some(result.visible.clone());
                    }
                });
                let call = self
                    .calls
                    .iter()
                    .position(|c| c.id == *call_id)
                    .map(|i| self.calls.swap_remove(i));
                let id = key("call", call_id);
                let model = result
                    .diff
                    .as_ref()
                    .map(|d| diffmodel::model(d, &self.theme));
                out.extend(self.update(&id, |k| {
                    if let BlockKind::Tool {
                        summary: s,
                        state,
                        tail: t,
                        archive,
                        diff,
                        duration_ms,
                        ..
                    } = k
                    {
                        *state = if result.ok {
                            ToolState::Done
                        } else {
                            ToolState::Failed
                        };
                        *t = tail(&result.visible).to_owned();
                        *archive = result.archive.clone();
                        *diff = model;
                        *duration_ms = result.duration_ms;
                        if let Some(call) = &call {
                            *s = summary::summary(call, Some(result));
                        }
                    }
                }));
                if call.is_some_and(|c| c.name == "todo")
                    && let Some(items) = result.todo_list()
                {
                    self.plans.push((self.turn, items));
                }
                out.extend(self.refresh_group(&id));
                out
            }
            Event::ApprovalRequired { call, why, source } => {
                let kind = BlockKind::Approval {
                    call: call.id,
                    tool: call.name.clone(),
                    summary: one_line(&call.subject),
                    input: call.input.clone(),
                    grants: grants_for(call).into_iter().map(|(_, s)| s).collect(),
                    why: why.clone(),
                    source: source.clone(),
                    decision: None,
                    by: None,
                };
                self.insert(key("approval", call.id), kind)
            }
            Event::ApprovalDecided {
                call_id,
                decision: d,
                by: b,
            } => self.update(&key("approval", call_id), |k| {
                if let BlockKind::Approval { decision, by, .. } = k {
                    *decision = Some(d.clone());
                    *by = Some(*b);
                }
            }),
            Event::QuestionAsked {
                call_id,
                question,
                options,
                ..
            } => {
                let kind = BlockKind::Question {
                    call: *call_id,
                    question: question.clone(),
                    options: options.clone(),
                    answer: None,
                };
                self.insert(key("question", call_id), kind)
            }
            Event::Usage { turn, usage: u } => self.update(&key("turn", turn), |k| {
                if let BlockKind::TurnMeta { usage, .. } = k {
                    *usage = Some(add_to(*usage, u));
                }
            }),
            Event::TurnDone { turn, stop: s } => self.update(&key("turn", turn), |k| {
                if let BlockKind::TurnMeta { stop, .. } = k {
                    *stop = Some(s.clone());
                }
            }),
            Event::Compacted {
                summary,
                before_tokens,
                after_tokens,
                reason,
                ..
            } => {
                let text = self.summary.take().filter(|(id, _)| id == summary);
                let kind = BlockKind::Compaction {
                    before_tokens: *before_tokens,
                    after_tokens: *after_tokens,
                    reason: *reason,
                    summary: text.map(|(_, text)| text),
                };
                self.insert(key("compaction", summary), kind)
            }
            Event::Checkpoint { files, .. } => {
                let files = files.iter().map(|f| f.path.clone()).collect();
                self.insert(key("checkpoint", seq), BlockKind::Checkpoint { files })
            }
            Event::Rewound {
                to_turn,
                conversation: true,
                ..
            } => {
                // `to_turn` itself is undone too (`Submission::Rewind`), so
                // what follows belongs to the turn before it.
                self.explore = None;
                self.plans.retain(|(turn, _)| turn < to_turn);
                self.turn = to_turn.saturating_sub(1);
                let (gone, kept): (Vec<Block>, Vec<Block>) = std::mem::take(&mut self.blocks)
                    .into_iter()
                    .partition(|b| b.turn >= *to_turn);
                self.blocks = kept;
                gone.into_iter()
                    .map(|b| TimelinePatch::Remove { id: b.id })
                    .collect()
            }
            Event::TaskCreated { task, label, tier } => {
                let kind = BlockKind::Task {
                    task: *task,
                    label: label.clone(),
                    tier: *tier,
                    done: false,
                    cost_usd: 0.0,
                    exit_code: None,
                    state: TaskState::Running,
                    kind: TaskKind::of(label),
                };
                self.insert(key("task", task), kind)
            }
            Event::TaskCompleted {
                task,
                cost_usd: c,
                exit_code: e,
                archive,
                ..
            } => self.update(&key("task", task), |k| {
                if let BlockKind::Task {
                    done,
                    cost_usd,
                    exit_code,
                    state,
                    kind,
                    ..
                } = k
                {
                    (*done, *cost_usd, *exit_code) = (true, *c, *e);
                    *state = TaskState::ended(*e);
                    *kind = kind.ended(*e, *archive);
                }
            }),
            Event::Notice { level, text } => {
                let kind = BlockKind::Notice {
                    level: *level,
                    text: text.clone(),
                };
                self.insert(key("notice", seq), kind)
            }
            Event::Error { error, fatal } => {
                let kind = BlockKind::Error {
                    text: error.to_string(),
                    fatal: *fatal,
                };
                self.insert(key("error", seq), kind)
            }
            _ => vec![],
        }
    }

    fn find(&self, id: &BlockId) -> Option<usize> {
        self.blocks.iter().rposition(|b| &b.id == id)
    }

    fn upsert(&self, i: usize) -> Vec<TimelinePatch> {
        let after = i.checked_sub(1).map(|p| self.blocks[p].id.clone());
        vec![TimelinePatch::Upsert {
            block: Box::new(self.blocks[i].clone()),
            after,
        }]
    }

    /// Appends a new block; any new block ends a run of exploring calls.
    fn insert(&mut self, id: BlockId, kind: BlockKind) -> Vec<TimelinePatch> {
        if self.find(&id).is_some() {
            return vec![];
        }
        self.explore = None;
        let turn = self.turn;
        self.blocks.push(Block { id, turn, kind });
        self.upsert(self.blocks.len() - 1)
    }

    /// Adds an exploring call to the run; from the second call on the run's
    /// `ToolGroup` is upserted, inserted in front of its first call.
    fn explore(
        &mut self,
        run: Option<Run>,
        call: CallId,
        id: BlockId,
        target: Explore,
    ) -> Vec<TimelinePatch> {
        let mut run = run.unwrap_or_else(|| Run {
            group: key("group", call),
            children: vec![],
            files: vec![],
            searches: 0,
        });
        run.children.push(id);
        match target {
            Explore::File(path) if !run.files.contains(&path) => run.files.push(path),
            Explore::File(_) => {}
            Explore::Search => run.searches += 1,
        }
        let kind = BlockKind::ToolGroup {
            summary: summary::explored(run.files.len(), run.searches),
            children: run.children.clone(),
            state: self.group_state(&run.children),
        };
        let at = match self.find(&run.group) {
            Some(i) => Some(i),
            None if run.children.len() > 1 => self.find(&run.children[0]).inspect(|&i| {
                let (id, turn) = (run.group.clone(), self.turn);
                self.blocks.insert(
                    i,
                    Block {
                        id,
                        turn,
                        kind: kind.clone(),
                    },
                );
            }),
            None => None,
        };
        self.explore = Some(run);
        let Some(i) = at else {
            return vec![];
        };
        self.blocks[i].kind = kind;
        self.upsert(i)
    }

    /// Re-sends the group holding `child` when its state changed.
    fn refresh_group(&mut self, child: &BlockId) -> Vec<TimelinePatch> {
        let found = self
            .blocks
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, b)| match &b.kind {
                BlockKind::ToolGroup { children, .. } if children.contains(child) => {
                    Some((i, children.clone()))
                }
                _ => None,
            });
        let Some((i, children)) = found else {
            return vec![];
        };
        let next = self.group_state(&children);
        match &mut self.blocks[i].kind {
            BlockKind::ToolGroup { state, .. } if *state != next => *state = next,
            _ => return vec![],
        }
        self.upsert(i)
    }

    fn group_state(&self, children: &[BlockId]) -> ToolState {
        let states: Vec<ToolState> = children
            .iter()
            .filter_map(|c| match &self.blocks[self.find(c)?].kind {
                BlockKind::Tool { state, .. } => Some(*state),
                _ => None,
            })
            .collect();
        if states.contains(&ToolState::Running) {
            ToolState::Running
        } else if states.contains(&ToolState::Failed) {
            ToolState::Failed
        } else {
            ToolState::Done
        }
    }

    /// A plugin's render of block `id` (T52.23.1): kept in the block, so a
    /// `Reset` carries it, and sent as the upsert every block change is.
    pub fn land(&mut self, id: &BlockId, widget: Option<&Widget>) -> Vec<TimelinePatch> {
        self.update(id, |k| crate::plugin_ui::land(k, widget))
    }

    fn update(&mut self, id: &BlockId, f: impl FnOnce(&mut BlockKind)) -> Vec<TimelinePatch> {
        let Some(i) = self.find(id) else {
            return vec![];
        };
        f(&mut self.blocks[i].kind);
        self.upsert(i)
    }

    fn append(&mut self, id: BlockId, delta: &str) -> Vec<TimelinePatch> {
        let Some(i) = self.find(&id) else {
            return vec![];
        };
        match &mut self.blocks[i].kind {
            BlockKind::Thinking { text, .. } => text.push_str(delta),
            BlockKind::Tool { tail: t, .. } => *t = tail(&(t.clone() + delta)).to_owned(),
            _ => return vec![],
        }
        vec![TimelinePatch::AppendText {
            id,
            text: delta.to_owned(),
        }]
    }

    /// Re-parses the reply and sends only the blocks from the first one
    /// that changed.
    fn stream_doc(&mut self, id: BlockId, delta: &str) -> Vec<TimelinePatch> {
        let Some(i) = self.find(&id) else {
            return vec![];
        };
        let BlockKind::Assistant { text, doc, .. } = &mut self.blocks[i].kind else {
            return vec![];
        };
        text.push_str(delta);
        let next = markdown::parse(text, &self.theme, &UNICODE);
        let from = doc
            .blocks
            .iter()
            .zip(&next.blocks)
            .take_while(|(a, b)| a == b)
            .count();
        let blocks = next.blocks[from..].to_vec();
        *doc = next;
        vec![TimelinePatch::DocTail {
            id,
            from: from as u32,
            blocks,
        }]
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::{TaskId, TurnId};
    use cox_protocol::types::{
        CompactReason, Diff, Job, ModelId, Risk, Segments, Tier, TodoState, ToolResult, Why,
    };
    use serde_json::json;

    use super::*;

    fn started(timeline: &mut Timeline, seq: u32) {
        timeline.apply(&Event::TurnStarted {
            turn: TurnId::new(),
            seq,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        });
    }

    #[test]
    fn streamed_markdown_resends_only_the_blocks_after_the_last_closed_one() {
        let mut timeline = Timeline::new("base16-ocean.dark");
        let item = ItemId::new();
        let kind = ItemKind::AssistantMessage {
            text: String::new(),
        };
        timeline.apply(&Event::ItemStarted { item, kind });
        let mut delta = |text: &str| {
            timeline.apply(&Event::TextDelta {
                item,
                text: text.into(),
            })
        };
        delta("# Title\n\nfirst para");
        let patches = delta("graph, still open");
        let [TimelinePatch::DocTail { from, blocks, .. }] = patches.as_slice() else {
            panic!("expected one DocTail, got {patches:?}");
        };
        assert_eq!((*from, blocks.len()), (1, 1), "the heading is frozen");
    }

    #[test]
    fn a_plugin_render_is_kept_in_the_block_and_sent_as_an_upsert() {
        let mut timeline = Timeline::default();
        let item = ItemId::new();
        let kind = ItemKind::AssistantMessage { text: "hi".into() };
        timeline.apply(&Event::ItemStarted { item, kind });
        let id = key("item", item);
        let widget = Widget::Text(vec![]);
        let patches = timeline.land(&id, Some(&widget));
        let [TimelinePatch::Upsert { block, .. }] = patches.as_slice() else {
            panic!("expected one Upsert, got {patches:?}");
        };
        assert!(matches!(
            &block.kind,
            BlockKind::Assistant {
                plugin_view: Some(_),
                ..
            }
        ));
        assert!(matches!(
            &timeline.reset(),
            TimelinePatch::Reset { blocks }
                if matches!(&blocks[0].kind, BlockKind::Assistant { plugin_view: Some(_), .. })
        ));
        assert!(timeline.land(&key("item", ItemId::new()), None).is_empty());
    }

    #[test]
    fn tool_tail_keeps_the_last_five_lines_across_chunks() {
        let mut timeline = Timeline::default();
        let call = ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: serde_json::Value::Null,
            risk: Risk::Exec,
            subject: "seq 1 7\nsecond line".into(),
            segments: None,
        };
        let call_id = call.id;
        timeline.apply(&Event::ToolCallRequested { call });
        for delta in ["1\n2\n3", "\n4\n5\n", "6\n7\n"] {
            timeline.apply(&Event::ToolCallOutput {
                call_id,
                delta: delta.into(),
            });
        }
        let BlockKind::Tool { tail, summary, .. } = &timeline.blocks()[0].kind else {
            panic!("expected a tool block");
        };
        assert_eq!(
            (tail.as_str(), summary.as_str()),
            ("3\n4\n5\n6\n7\n", "Running `seq 1 7`")
        );
    }

    #[test]
    fn rewinding_the_conversation_removes_the_later_turns_blocks() {
        let mut timeline = Timeline::default();
        for seq in 1..=3 {
            started(&mut timeline, seq);
        }
        let patches = timeline.apply(&Event::Rewound {
            to_turn: 2,
            code: false,
            conversation: true,
            restored: vec![],
            skipped: vec![],
        });
        assert_eq!(patches.len(), 2);
        assert!(
            patches
                .iter()
                .all(|p| matches!(p, TimelinePatch::Remove { .. }))
        );
        assert_eq!(timeline.blocks().len(), 1);
    }

    fn request(timeline: &mut Timeline, name: &str, input: serde_json::Value) {
        let call = ToolCall {
            id: CallId::new(),
            name: name.into(),
            input,
            risk: Risk::ReadOnly,
            subject: String::new(),
            segments: None,
        };
        timeline.apply(&Event::ToolCallRequested { call });
    }

    #[test]
    fn a_non_exploring_call_ends_the_group() {
        let mut timeline = Timeline::default();
        request(&mut timeline, "read", json!({"path": "a.rs"}));
        request(&mut timeline, "glob", json!({"pattern": "*.rs"}));
        request(&mut timeline, "bash", json!({"command": "ls"}));
        request(&mut timeline, "read", json!({"path": "b.rs"}));
        let kinds: Vec<&str> = timeline
            .blocks()
            .iter()
            .map(|b| match &b.kind {
                BlockKind::ToolGroup {
                    summary, children, ..
                } => {
                    assert_eq!(
                        (summary.as_str(), children.len()),
                        ("Explored 1 file, 1 search", 2)
                    );
                    "group"
                }
                BlockKind::Tool { tool, .. } => tool.as_str(),
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["group", "read", "glob", "bash", "read"]);
    }

    #[test]
    fn an_approval_carries_the_input_and_what_allow_for_session_grants() {
        let mut timeline = Timeline::default();
        let input = json!({"command": "git status && npm test"});
        let call = ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: input.clone(),
            risk: Risk::Exec,
            subject: "git status && npm test".into(),
            segments: Some(Segments {
                commands: vec!["git status".into(), "npm test".into()],
                opaque: false,
            }),
        };
        timeline.apply(&Event::ApprovalRequired {
            call,
            why: Why::Risk { risk: Risk::Exec },
            source: None,
        });
        let BlockKind::Approval {
            input: carried,
            grants,
            ..
        } = &timeline.blocks()[0].kind
        else {
            panic!("expected an approval block");
        };
        assert_eq!(carried, &input);
        assert_eq!(grants, &["git status", "npm test"]);
    }

    #[test]
    fn compaction_carries_its_summary_item_text() {
        let mut timeline = Timeline::default();
        let item = ItemId::new();
        let kind = ItemKind::Summary {
            text: "we fixed the login".into(),
        };
        assert!(
            timeline
                .apply(&Event::ItemStarted { item, kind })
                .is_empty()
        );
        timeline.apply(&Event::Compacted {
            summary: item,
            dropped: vec![],
            before_tokens: 9000,
            after_tokens: 1200,
            reason: CompactReason::PreCall,
        });
        let BlockKind::Compaction { summary, .. } = &timeline.blocks()[0].kind else {
            panic!("expected a compaction block");
        };
        assert_eq!(summary.as_deref(), Some("we fixed the login"));
    }

    /// A call to `name` whose result carries `list` as its `structured`.
    fn todo(timeline: &mut Timeline, name: &str, list: serde_json::Value) {
        let call = ToolCall {
            id: CallId::new(),
            name: name.into(),
            input: json!({}),
            risk: Risk::ReadOnly,
            subject: String::new(),
            segments: None,
        };
        let call_id = call.id;
        timeline.apply(&Event::ToolCallRequested { call });
        let result = ToolResult {
            ok: true,
            visible: String::new(),
            archive: None,
            bytes: 0,
            duration_ms: 0,
            diff: None,
            structured: Some(Box::new(list)),
        };
        timeline.apply(&Event::ToolCallDone { call_id, result });
    }

    #[test]
    fn a_rust_edits_keyword_run_carries_the_dark_and_the_light_colour() {
        let mut timeline = Timeline::new("base16-ocean.dark");
        let call = ToolCall {
            id: CallId::new(),
            name: "edit".into(),
            input: json!({}),
            risk: Risk::ReadOnly,
            subject: String::new(),
            segments: None,
        };
        let call_id = call.id;
        timeline.apply(&Event::ToolCallRequested { call });
        let diff = Diff {
            path: "src/lib.rs".into(),
            unified: "@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n".into(),
        };
        let result = ToolResult {
            ok: true,
            visible: String::new(),
            archive: None,
            bytes: 0,
            duration_ms: 0,
            diff: Some(diff),
            structured: None,
        };
        timeline.apply(&Event::ToolCallDone { call_id, result });
        let Some(BlockKind::Tool { diff: Some(m), .. }) = timeline.blocks().last().map(|b| &b.kind)
        else {
            panic!("expected an edit block with its diff");
        };
        let spans = &m.hunks[0].lines[1].spans;
        let keyword = spans
            .iter()
            .find(|s| s.text == "fn")
            .expect("the keyword run");
        assert!(keyword.rgb.is_some() && keyword.light.is_some());
        // base16-ocean's two variants differ in their default foreground.
        assert!(spans.iter().any(|s| s.light.is_some() && s.light != s.rgb));
    }

    #[test]
    fn the_plan_is_the_latest_todo_result_and_a_rewind_restores_the_one_before() {
        let mut timeline = Timeline::default();
        assert!(timeline.plan().is_empty());
        let step = |text: &str, state: &str| json!({"id": text, "text": text, "state": state});
        started(&mut timeline, 1);
        let first = json!([step("Read", "in_progress"), step("Test", "pending")]);
        todo(&mut timeline, "todo", first);
        started(&mut timeline, 2);
        todo(
            &mut timeline,
            "todo",
            json!([step("Read", "done"), step("Test", "in_progress")]),
        );
        // Another tool's payload of the same shape is not the plan.
        todo(&mut timeline, "mcp__notes__todo", json!([]));
        let states = |t: &Timeline| t.plan().iter().map(|i| i.state).collect::<Vec<_>>();
        assert_eq!(states(&timeline), [TodoState::Done, TodoState::InProgress]);
        assert_eq!(timeline.plan()[1].text, "Test");
        timeline.apply(&Event::Rewound {
            to_turn: 2,
            code: false,
            conversation: true,
            restored: vec![],
            skipped: vec![],
        });
        assert_eq!(
            states(&timeline),
            [TodoState::InProgress, TodoState::Pending]
        );
    }

    /// A91: a folded reasoning run records its duration from
    /// `ThinkingDone`, and folding the same events read back from a rollout
    /// line by line gives the same value.
    #[test]
    fn folded_thought_records_its_duration_and_replay_keeps_it() {
        let item = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: None,
        };
        let events = [
            Event::ItemStarted { item, kind },
            Event::ThinkingDelta {
                item,
                text: "weigh ".into(),
            },
            Event::ThinkingDelta {
                item,
                text: "options".into(),
            },
            Event::ThinkingDone {
                item,
                duration_ms: 12_000,
            },
            Event::ItemDone { item },
        ];
        let fold = |events: &[Event]| {
            let mut timeline = Timeline::default();
            for ev in events {
                timeline.apply(ev);
            }
            timeline.blocks().to_vec()
        };
        let live = fold(&events);
        let [block] = live.as_slice() else {
            panic!("one thinking block, got {live:?}");
        };
        let BlockKind::Thinking { text, duration_ms } = &block.kind else {
            panic!("a thinking block, got {block:?}");
        };
        assert_eq!(
            (text.as_str(), *duration_ms),
            ("weigh options", Some(12_000))
        );
        let replayed: Vec<Event> = events
            .iter()
            .map(|ev| serde_json::to_string(ev).and_then(|line| serde_json::from_str(&line)))
            .collect::<Result<_, _>>()
            .expect("rollout lines");
        assert_eq!(fold(&replayed), live);
    }

    /// T37.23.14: the empty signed item T39.2 emits before a tool call
    /// opens no thinking block, so none is left waiting for a duration.
    #[test]
    fn signature_only_thinking_item_leaves_no_open_thinking_block() {
        let item = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: Some("sig-1".into()),
        };
        let mut timeline = Timeline::default();
        assert!(
            timeline
                .apply(&Event::ItemStarted { item, kind })
                .is_empty()
        );
        assert!(timeline.apply(&Event::ItemDone { item }).is_empty());
        assert!(
            !timeline
                .blocks()
                .iter()
                .any(|b| matches!(b.kind, BlockKind::Thinking { .. })),
            "{:?}",
            timeline.blocks()
        );
    }

    #[test]
    fn a_subagent_without_an_exit_code_succeeded() {
        let mut timeline = Timeline::new("base16-ocean.dark");
        started(&mut timeline, 1);
        let (agent, shell) = (TaskId::new(), TaskId::new());
        let states = |timeline: &Timeline| -> Vec<TaskState> {
            timeline
                .blocks()
                .iter()
                .filter_map(|b| match b.kind {
                    BlockKind::Task { state, .. } => Some(state),
                    _ => None,
                })
                .collect()
        };
        for (task, label) in [(agent, "explore: find it"), (shell, "cargo test")] {
            timeline.apply(&Event::TaskCreated {
                task,
                label: label.into(),
                tier: Tier::Code,
            });
        }
        assert_eq!(states(&timeline), [TaskState::Running, TaskState::Running]);
        for (task, exit_code) in [(agent, None), (shell, Some(101))] {
            timeline.apply(&Event::TaskCompleted {
                task,
                result_item: ItemId::new(),
                cost_usd: 0.0,
                exit_code,
                archive: None,
            });
        }
        assert_eq!(states(&timeline), [TaskState::Succeeded, TaskState::Failed]);
        assert_eq!(TaskState::ended(Some(0)), TaskState::Succeeded);
    }
}
