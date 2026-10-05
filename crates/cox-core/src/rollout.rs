// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Rebuild `Message` history from a rollout `Event` stream so resume
//! assembles the same `Request` a live session would have (plan.md T2.4).

use std::collections::{HashMap, HashSet};

use cox_protocol::ids::{ArchiveId, CallId, ItemId};
use cox_protocol::types::{
    Content, Decision, Event, ItemKind, Job, Level, Message, PermissionMode, Role, StopReason,
    ToolCall,
};

use crate::context::attached_content;

/// Reconstructed transcript plus the session flags resume must restore.
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    /// Model-visible messages, in order.
    pub messages: Vec<Message>,
    /// The mode of the last recorded `StateChanged` (T50.2, T37.5); `None`
    /// when the rollout never recorded one, as in every rollout written
    /// before T50.2.
    pub permission_mode: Option<PermissionMode>,
    /// Persistent `(tool, subject)` grants from `AllowForSession`.
    pub grants: Vec<(String, String)>,
    /// True when the caller dropped a truncated last JSONL line.
    pub truncated: bool,
    /// The highest main-turn `seq` seen, so a resumed session keeps
    /// numbering where it left off (T26.2).
    pub turns: u32,
    /// Surviving main turns as `(seq, message index, checkpoint count)`.
    /// These are rollout ordinals, not positions after rewind/compaction.
    pub turn_marks: Vec<HistoryTurn>,
    /// The archived repo map of the last `RepoMapBuilt` (P43): resume
    /// reads it back instead of rebuilding, so `system[2]` keeps its bytes.
    pub repomap: Option<ArchiveId>,
}

/// Reconstructed metadata for one user turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryTurn {
    /// The user item that owns this turn, retained for later compaction.
    pub item: ItemId,
    /// The original `Event::TurnStarted::seq`.
    pub seq: u32,
    /// Where this turn begins in [`History::messages`].
    pub message_index: usize,
    /// Files checkpointed during this turn.
    pub checkpoints: usize,
}

impl History {
    /// Rebuilds history from a complete event list.
    pub fn from_events(events: &[Event]) -> Self {
        Self::from_rollout(events, false)
    }

    /// Same as [`from_events`], plus a truncation flag from the JSONL reader.
    pub fn from_rollout(events: &[Event], truncated: bool) -> Self {
        let mut dropped = HashSet::new();
        for ev in events {
            if let Event::Compacted { dropped: ids, .. } = ev {
                dropped.extend(ids.iter().copied());
            }
        }

        let mut items: HashMap<ItemId, ItemKind> = HashMap::new();
        let mut messages = Vec::new();
        // The user item each message belongs to, so `Compacted.dropped`
        // (user item ids) removes whole turns, tool blocks included.
        let mut turn_of: Vec<Option<ItemId>> = Vec::new();
        let mut current_turn: Option<ItemId> = None;
        let mut pending_results: Vec<Content> = Vec::new();
        let mut calls: HashMap<CallId, ToolCall> = HashMap::new();
        let mut grants = Vec::new();
        let mut permission_mode = None;
        let mut repomap = None;
        let mut turns = 0u32;
        let mut current_seq = 0u32;
        let mut item_seq: HashMap<ItemId, u32> = HashMap::new();
        let mut checkpoint_counts: HashMap<u32, usize> = HashMap::new();
        // Where each main turn starts in `messages`, for `Rewound`.
        let mut starts: Vec<(u32, usize)> = Vec::new();

        for ev in events {
            match ev {
                Event::TurnStarted {
                    seq,
                    job: Job::Main,
                    ..
                } => {
                    flush_results(&mut messages, &mut pending_results);
                    turns = turns.max(*seq);
                    current_seq = *seq;
                    starts.push((*seq, messages.len()));
                }
                // The user's cut (T26.2): history stops at `to_turn`, the
                // rollout keeps every line, so the cut is replayed here.
                Event::Rewound {
                    to_turn,
                    conversation: true,
                    ..
                } => {
                    flush_results(&mut messages, &mut pending_results);
                    if let Some(at) = starts.iter().position(|(seq, _)| seq >= to_turn) {
                        let len = starts[at].1;
                        messages.truncate(len);
                        turn_of.truncate(len);
                        starts.truncate(at);
                    }
                }
                Event::ItemStarted { item, kind } => {
                    flush_results(&mut messages, &mut pending_results);
                    items.insert(*item, kind.clone());
                }
                Event::TextDelta { item, text } => {
                    if let Some(ItemKind::AssistantMessage { text: acc }) = items.get_mut(item) {
                        acc.push_str(text);
                    }
                }
                Event::ItemDone { item } => {
                    let Some(kind) = items.remove(item) else {
                        continue;
                    };
                    if dropped.contains(item) {
                        // Keep turn tracking so the dropped turn's assistant
                        // messages still attach to it and the `Compacted`
                        // handler filters them; otherwise they orphan to the
                        // previous turn and survive the rebuild.
                        if matches!(
                            kind,
                            ItemKind::UserMessage { .. } | ItemKind::Summary { .. }
                        ) {
                            current_turn = Some(*item);
                        }
                        continue;
                    }
                    match kind {
                        // T40.2: the recorded attachments are the ones the
                        // live turn sent, rebuilt by the same function.
                        ItemKind::UserMessage { text, attachments } => {
                            current_turn = Some(*item);
                            item_seq.insert(*item, current_seq);
                            messages.push(Message {
                                role: Role::User,
                                content: attached_content(text, None, &attachments),
                            });
                            turn_of.push(current_turn);
                        }
                        ItemKind::Summary { text } => {
                            current_turn = Some(*item);
                            item_seq.insert(*item, 0);
                            messages.push(Message {
                                role: Role::User,
                                content: vec![Content::Text { text }],
                            });
                            turn_of.push(current_turn);
                        }
                        ItemKind::AssistantMessage { text } if !text.is_empty() => {
                            messages.push(Message {
                                role: Role::Assistant,
                                content: vec![Content::Text { text }],
                            });
                            turn_of.push(current_turn);
                        }
                        // T39.2: only a signed block is replayed; unsigned
                        // thinking is display-only, as in the live history.
                        ItemKind::Thinking {
                            text,
                            signature: Some(signature),
                        } => {
                            let signature = Some(signature);
                            append_assistant_block(
                                &mut messages,
                                Content::Thinking { text, signature },
                            );
                            turn_of.resize(messages.len(), current_turn);
                        }
                        _ => {}
                    }
                }
                Event::ToolCallRequested { call } => {
                    calls.insert(call.id, call.clone());
                    append_tool_use(&mut messages, call);
                    turn_of.resize(messages.len(), current_turn);
                }
                Event::Compacted {
                    summary, dropped, ..
                } => {
                    // In memory the summary sits in front of the kept turns;
                    // in the rollout it was appended last. Mirror memory.
                    flush_results(&mut messages, &mut pending_results);
                    turn_of.resize(messages.len(), current_turn);
                    let gone: HashSet<ItemId> = dropped.iter().copied().collect();
                    let mut front = Vec::new();
                    let mut rest = Vec::new();
                    for (msg, turn) in messages.drain(..).zip(turn_of.drain(..)) {
                        if turn == Some(*summary) {
                            front.push((msg, turn));
                        } else if !turn.is_some_and(|t| gone.contains(&t)) {
                            rest.push((msg, turn));
                        }
                    }
                    for (msg, turn) in front.into_iter().chain(rest) {
                        messages.push(msg);
                        turn_of.push(turn);
                    }
                    starts = starts_from(&turn_of, &item_seq);
                }
                Event::StateChanged { mode, .. } => permission_mode = Some(*mode),
                Event::Checkpoint { files, .. } => {
                    *checkpoint_counts.entry(current_seq).or_default() += files.len();
                }
                Event::ToolCallDone { call_id, result } => {
                    pending_results.push(Content::ToolResult {
                        call_id: *call_id,
                        content: result.visible.clone(),
                        is_error: !result.ok,
                    });
                }
                Event::ApprovalDecided {
                    call_id,
                    decision: Decision::AllowForSession,
                    ..
                } => {
                    if let Some(call) = calls.get(call_id) {
                        grants.extend(crate::permission::grants_for(call));
                    }
                }
                Event::GrantRevoked { tool, subject } => {
                    grants.retain(|(t, s)| t != tool || s != subject);
                }
                Event::RepoMapBuilt { archive, .. } => repomap = Some(*archive),
                Event::TurnDone { stop, .. } => {
                    if *stop == StopReason::Interrupted {
                        pending_results.clear();
                    } else {
                        flush_results(&mut messages, &mut pending_results);
                        turn_of.resize(messages.len(), current_turn);
                    }
                }
                _ => {}
            }
        }
        flush_results(&mut messages, &mut pending_results);
        turn_of.resize(messages.len(), current_turn);
        let mut seen = HashSet::new();
        let turn_marks = messages
            .iter()
            .zip(&turn_of)
            .enumerate()
            .filter_map(|(message_index, (message, item))| {
                let item = (*item)?;
                let seq = *item_seq.get(&item)?;
                (message.role == Role::User && seq != 0 && seen.insert(item)).then(|| HistoryTurn {
                    item,
                    seq,
                    message_index,
                    checkpoints: checkpoint_counts.get(&seq).copied().unwrap_or_default(),
                })
            })
            .collect();

        Self {
            messages,
            permission_mode,
            grants,
            truncated,
            turns,
            turn_marks,
            repomap,
        }
    }

    /// A `Notice` to emit when the last rollout line was truncated.
    pub fn truncated_notice(&self) -> Option<Event> {
        self.truncated.then(|| Event::Notice {
            level: Level::Warn,
            text: "last rollout line was truncated and dropped".into(),
        })
    }
}

fn starts_from(turn_of: &[Option<ItemId>], item_seq: &HashMap<ItemId, u32>) -> Vec<(u32, usize)> {
    let mut seen = HashSet::new();
    turn_of
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let item = (*item)?;
            let seq = *item_seq.get(&item)?;
            (seq != 0 && seen.insert(item)).then_some((seq, index))
        })
        .collect()
}

fn flush_results(messages: &mut Vec<Message>, pending: &mut Vec<Content>) {
    if pending.is_empty() {
        return;
    }
    messages.push(Message {
        role: Role::User,
        content: std::mem::take(pending),
    });
}

fn append_tool_use(messages: &mut Vec<Message>, call: &ToolCall) {
    append_assistant_block(
        messages,
        Content::ToolUse {
            id: call.id,
            name: call.name.clone(),
            input: call.input.clone(),
        },
    );
}

/// Adds a block to the assistant message the live loop built in one piece
/// (text, then each call's signed thinking and `ToolUse`), opening one when
/// the turn had no text.
fn append_assistant_block(messages: &mut Vec<Message>, block: Content) {
    if let Some(last) = messages.last_mut()
        && last.role == Role::Assistant
    {
        last.content.push(block);
        return;
    }
    messages.push(Message {
        role: Role::Assistant,
        content: vec![block],
    });
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::{CallId, ItemId, TurnId};
    use cox_protocol::types::{CompactReason, ItemKind, StopReason, ToolResult};

    use super::*;

    fn user_item(id: ItemId, text: &str) -> Event {
        Event::ItemStarted {
            item: id,
            kind: ItemKind::UserMessage {
                text: text.into(),
                attachments: vec![],
            },
        }
    }

    #[test]
    fn resume_truncated_last_line_emits_notice() {
        let h = History::from_rollout(&[], true);
        let Event::Notice { level, text } = h.truncated_notice().expect("notice") else {
            panic!("expected notice");
        };
        assert_eq!(level, Level::Warn);
        assert!(text.contains("truncated"));
        assert!(History::from_events(&[]).truncated_notice().is_none());
    }

    /// P43: the last map built wins; a rollout without one has none.
    #[test]
    fn resume_keeps_the_last_repomap_archive() {
        use cox_protocol::types::RepoMapReason;
        let (first, second) = (ArchiveId::new(), ArchiveId::new());
        let built = |archive, reason| Event::RepoMapBuilt {
            archive,
            bytes: 10,
            reason,
        };
        let events = vec![
            built(first, RepoMapReason::SessionStart),
            user_item(ItemId::new(), "hi"),
            built(second, RepoMapReason::Refresh),
        ];
        assert_eq!(History::from_events(&events).repomap, Some(second));
        assert_eq!(History::from_events(&events[1..2]).repomap, None);
    }

    #[test]
    fn old_rollout_without_mode_record_has_no_mode() {
        let events = vec![user_item(ItemId::new(), "hi")];
        assert_eq!(History::from_events(&events).permission_mode, None);
    }

    #[test]
    fn resume_compacted_dropped_items_skipped() {
        let keep = ItemId::new();
        let drop = ItemId::new();
        let events = vec![
            user_item(drop, "old"),
            Event::ItemDone { item: drop },
            user_item(keep, "new"),
            Event::ItemDone { item: keep },
            Event::Compacted {
                summary: ItemId::new(),
                dropped: vec![drop],
                before_tokens: 10,
                after_tokens: 2,
                reason: CompactReason::Manual,
            },
        ];
        let h = History::from_events(&events);
        assert_eq!(h.messages.len(), 1);
        assert_eq!(
            h.messages[0].content,
            vec![Content::Text { text: "new".into() }]
        );
    }

    #[test]
    fn resume_marks_keep_original_seq_after_compaction() {
        let old = ItemId::new();
        let keep = ItemId::new();
        let summary = ItemId::new();
        let events = vec![
            Event::TurnStarted {
                turn: TurnId::new(),
                seq: 4,
                job: Job::Main,
                tier: cox_protocol::types::Tier::Code,
                model: cox_protocol::types::ModelId("m".into()),
            },
            user_item(old, "old"),
            Event::ItemDone { item: old },
            Event::TurnStarted {
                turn: TurnId::new(),
                seq: 7,
                job: Job::Main,
                tier: cox_protocol::types::Tier::Code,
                model: cox_protocol::types::ModelId("m".into()),
            },
            user_item(keep, "keep"),
            Event::ItemDone { item: keep },
            Event::ItemStarted {
                item: summary,
                kind: ItemKind::Summary {
                    text: "summary".into(),
                },
            },
            Event::ItemDone { item: summary },
            Event::Compacted {
                summary,
                dropped: vec![old],
                before_tokens: 10,
                after_tokens: 2,
                reason: CompactReason::Manual,
            },
        ];

        let history = History::from_events(&events);
        assert_eq!(history.turns, 7);
        assert_eq!(history.turn_marks.len(), 1);
        assert_eq!(history.turn_marks[0].item, keep);
        assert_eq!(history.turn_marks[0].seq, 7);
        assert_eq!(history.turn_marks[0].message_index, 1);
    }

    #[test]
    fn resume_interrupt_drops_unflushed_tool_results() {
        let user = ItemId::new();
        let asst = ItemId::new();
        let call = CallId::new();
        let events = vec![
            user_item(user, "go"),
            Event::ItemDone { item: user },
            Event::ItemStarted {
                item: asst,
                kind: ItemKind::AssistantMessage {
                    text: String::new(),
                },
            },
            Event::TextDelta {
                item: asst,
                text: "x".into(),
            },
            Event::ItemDone { item: asst },
            Event::ToolCallRequested {
                call: ToolCall {
                    id: call,
                    name: "echo".into(),
                    input: serde_json::json!({}),
                    risk: cox_protocol::types::Risk::ReadOnly,
                    subject: String::new(),
                    segments: None,
                },
            },
            Event::ToolCallDone {
                call_id: call,
                result: ToolResult {
                    ok: false,
                    visible: "cancelled".into(),
                    archive: None,
                    bytes: 0,
                    duration_ms: 0,
                    diff: None,
                    structured: None,
                },
            },
            Event::TurnDone {
                turn: TurnId::new(),
                stop: StopReason::Interrupted,
            },
        ];
        let h = History::from_events(&events);
        assert_eq!(h.messages.len(), 2);
        assert!(matches!(
            h.messages[1].content.last(),
            Some(Content::ToolUse { .. })
        ));
    }

    /// T39.2: a signed thinking item lands right before the `ToolUse` it was
    /// streamed with, in the same assistant message; unsigned thinking is
    /// display-only and stays out of history.
    #[test]
    fn resume_rebuilds_signed_thinking_before_tool_use() {
        let user = ItemId::new();
        let call = |id: CallId| Event::ToolCallRequested {
            call: ToolCall {
                id,
                name: "echo".into(),
                input: serde_json::json!({}),
                risk: cox_protocol::types::Risk::ReadOnly,
                subject: String::new(),
                segments: None,
            },
        };
        let thinking = |item: ItemId, text: &str, signature: Option<&str>| {
            [
                Event::ItemStarted {
                    item,
                    kind: ItemKind::Thinking {
                        text: text.into(),
                        signature: signature.map(Into::into),
                    },
                },
                Event::ItemDone { item },
            ]
        };
        let (a, b) = (CallId::new(), CallId::new());
        let mut events = vec![user_item(user, "go"), Event::ItemDone { item: user }];
        events.extend(thinking(ItemId::new(), "musing", None));
        events.extend(thinking(ItemId::new(), "", Some("sig-a")));
        events.push(call(a));
        events.push(call(b));
        events.extend(thinking(ItemId::new(), "", Some("sig-c")));
        let c = CallId::new();
        events.push(call(c));
        let h = History::from_events(&events);
        let signed = |sig: &str| Content::Thinking {
            text: String::new(),
            signature: Some(sig.into()),
        };
        let used = |id: CallId| Content::ToolUse {
            id,
            name: "echo".into(),
            input: serde_json::json!({}),
        };
        assert_eq!(h.messages.len(), 2);
        assert_eq!(h.messages[1].role, Role::Assistant);
        assert_eq!(
            h.messages[1].content,
            vec![signed("sig-a"), used(a), used(b), signed("sig-c"), used(c)]
        );
    }
}
