// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The coalescing patch queue (DT§4.5) and the reference consumer it is
//! proved against. Separate from the fold so `timeline.rs` stays one event
//! in, its patches out: while a UI is behind, successive patches for one
//! block merge here, so a stalled UI costs memory per changed block, not per
//! token.

use crate::patch::{Block, BlockId, BlockKind, TimelinePatch, tail};

/// Queues `patch`, merging it into the last queued patch for the same block
/// when that yields the state the two would give applied in turn.
pub fn push(queue: &mut Vec<TimelinePatch>, patch: TimelinePatch) {
    let patch = match patch {
        // What sits beside the list outlives a reset of it.
        TimelinePatch::Reset { .. } => {
            queue.retain(beside);
            patch
        }
        TimelinePatch::Upsert { block, after } => {
            while let Some(i) = last_for(queue, &block.id) {
                match &mut queue[i] {
                    TimelinePatch::Upsert { block: queued, .. } => {
                        // Keeps the queued `after`: it was right where that
                        // patch sits, which is where the block now lands.
                        *queued = block;
                        return;
                    }
                    // The whole block supersedes a queued partial change.
                    TimelinePatch::AppendText { .. } | TimelinePatch::DocTail { .. } => {
                        queue.remove(i);
                    }
                    _ => break,
                }
            }
            TimelinePatch::Upsert { block, after }
        }
        TimelinePatch::AppendText { .. } | TimelinePatch::DocTail { .. } => {
            match target(&patch).and_then(|id| last_for(queue, id)) {
                Some(i) => match merge(&mut queue[i], patch) {
                    Ok(()) => return,
                    Err(patch) => patch,
                },
                None => patch,
            }
        }
        TimelinePatch::Remove { .. } => patch,
        TimelinePatch::Usage { .. } => {
            queue.retain(|p| !matches!(p, TimelinePatch::Usage { .. }));
            patch
        }
        TimelinePatch::Status { .. } => {
            queue.retain(|p| !matches!(p, TimelinePatch::Status { .. }));
            patch
        }
        TimelinePatch::PluginSlot { slot } => {
            queue.retain(|p| {
                !matches!(p, TimelinePatch::PluginSlot { slot: queued }
                    if queued.plugin == slot.plugin && queued.slot == slot.slot)
            });
            TimelinePatch::PluginSlot { slot }
        }
    };
    queue.push(patch);
}

/// The meter, the status and the plugin slots: whole states beside the
/// block list, which no block patch or reset changes.
pub(crate) fn beside(patch: &TimelinePatch) -> bool {
    matches!(
        patch,
        TimelinePatch::Usage { .. }
            | TimelinePatch::Status { .. }
            | TimelinePatch::PluginSlot { .. }
    )
}

/// Applies `patch` to a consumer's block list the way a UI does (DT§4.3).
pub fn apply(blocks: &mut Vec<Block>, patch: TimelinePatch) {
    match patch {
        TimelinePatch::Reset { blocks: all } => *blocks = all,
        TimelinePatch::Upsert { block, after } => {
            if let Some(b) = blocks.iter_mut().find(|b| b.id == block.id) {
                *b = *block;
                return;
            }
            let at = match after {
                None => 0,
                Some(after) => blocks
                    .iter()
                    .position(|b| b.id == after)
                    .map_or(blocks.len(), |i| i + 1),
            };
            blocks.insert(at, *block);
        }
        TimelinePatch::Remove { id } => blocks.retain(|b| b.id != id),
        // The meter and the status sit beside the list; a UI keeps them on
        // its own.
        TimelinePatch::Usage { .. }
        | TimelinePatch::Status { .. }
        | TimelinePatch::PluginSlot { .. } => {}
        TimelinePatch::AppendText { .. } | TimelinePatch::DocTail { .. } => {
            let found = target(&patch).and_then(|id| blocks.iter().position(|b| &b.id == id));
            if let Some(i) = found {
                // A patch that does not fit the block's kind changes nothing,
                // as in the fold.
                let _ = apply_content(&mut blocks[i].kind, patch);
            }
        }
    }
}

/// The block a patch changes; `None` for `Reset`, which changes them all,
/// and for `Usage`, `Status` and `PluginSlot`, which change none.
fn target(patch: &TimelinePatch) -> Option<&BlockId> {
    match patch {
        TimelinePatch::Reset { .. }
        | TimelinePatch::Usage { .. }
        | TimelinePatch::Status { .. }
        | TimelinePatch::PluginSlot { .. } => None,
        TimelinePatch::Upsert { block, .. } => Some(&block.id),
        TimelinePatch::AppendText { id, .. }
        | TimelinePatch::DocTail { id, .. }
        | TimelinePatch::Remove { id } => Some(id),
    }
}

/// The last queued patch for `id`; a `Reset` ends the search, as nothing
/// before it survives.
fn last_for(queue: &[TimelinePatch], id: &BlockId) -> Option<usize> {
    for (i, patch) in queue.iter().enumerate().rev() {
        if matches!(patch, TimelinePatch::Reset { .. }) {
            return None;
        }
        if target(patch) == Some(id) {
            return Some(i);
        }
    }
    None
}

/// Folds `later` into `queued`, both for one block; hands `later` back when
/// the pair does not merge.
fn merge(queued: &mut TimelinePatch, later: TimelinePatch) -> Result<(), TimelinePatch> {
    match (queued, later) {
        (TimelinePatch::Upsert { block, .. }, later) => apply_content(&mut block.kind, later),
        (TimelinePatch::AppendText { text, .. }, TimelinePatch::AppendText { text: more, .. }) => {
            text.push_str(&more);
            Ok(())
        }
        (
            TimelinePatch::DocTail { from, blocks, .. },
            TimelinePatch::DocTail {
                from: next,
                blocks: more,
                ..
            },
        ) if next < *from => {
            (*from, *blocks) = (next, more);
            Ok(())
        }
        (
            TimelinePatch::DocTail { from, blocks, .. },
            TimelinePatch::DocTail {
                from: next,
                blocks: more,
                ..
            },
        ) if ((next - *from) as usize) <= blocks.len() => {
            blocks.truncate((next - *from) as usize);
            blocks.extend(more);
            Ok(())
        }
        (_, later) => Err(later),
    }
}

/// Applies an `AppendText` or `DocTail` to the block it names.
fn apply_content(kind: &mut BlockKind, patch: TimelinePatch) -> Result<(), TimelinePatch> {
    match (kind, patch) {
        (BlockKind::Thinking { text, .. }, TimelinePatch::AppendText { text: more, .. }) => {
            text.push_str(&more);
        }
        (BlockKind::Tool { tail: t, .. }, TimelinePatch::AppendText { text: more, .. }) => {
            t.push_str(&more);
            *t = tail(t).to_owned();
        }
        (BlockKind::Assistant { doc, .. }, TimelinePatch::DocTail { from, blocks, .. })
            if from as usize <= doc.blocks.len() =>
        {
            doc.blocks.truncate(from as usize);
            doc.blocks.extend(blocks);
        }
        (_, patch) => return Err(patch),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cox_protocol::types::Risk;
    use cox_render::doc::{Block as DocBlock, StyledDoc};

    use super::*;
    use crate::patch::ToolState;

    fn id(s: &str) -> BlockId {
        BlockId(s.into())
    }

    fn block(key: &str, kind: BlockKind) -> TimelinePatch {
        TimelinePatch::Upsert {
            block: Box::new(Block {
                id: id(key),
                turn: 1,
                kind,
            }),
            after: None,
        }
    }

    fn thinking(key: &str, text: &str) -> TimelinePatch {
        block(
            key,
            BlockKind::Thinking {
                text: text.into(),
                duration_ms: None,
            },
        )
    }

    fn append(key: &str, text: &str) -> TimelinePatch {
        TimelinePatch::AppendText {
            id: id(key),
            text: text.into(),
        }
    }

    fn doc_tail(key: &str, from: u32, texts: &[&str]) -> TimelinePatch {
        TimelinePatch::DocTail {
            id: id(key),
            from,
            blocks: texts.iter().map(|t| para(t)).collect(),
        }
    }

    fn para(text: &str) -> DocBlock {
        DocBlock::Table {
            rows: vec![vec![text.into()]],
        }
    }

    fn coalesced(patches: &[TimelinePatch]) -> Vec<TimelinePatch> {
        let mut queue = Vec::new();
        for p in patches {
            push(&mut queue, p.clone());
        }
        queue
    }

    fn mirror(start: &[Block], patches: &[TimelinePatch]) -> Vec<Block> {
        let mut blocks = start.to_vec();
        for p in patches {
            apply(&mut blocks, p.clone());
        }
        blocks
    }

    #[test]
    fn appends_to_one_block_merge_and_other_blocks_keep_their_own() {
        let queue = coalesced(&[append("a", "1"), append("b", "x"), append("a", "2")]);
        assert_eq!(queue, vec![append("a", "12"), append("b", "x")]);
    }

    #[test]
    fn content_patches_fold_into_a_queued_upsert() {
        let tool = BlockKind::Tool {
            tool: "bash".into(),
            summary: String::new(),
            icon: crate::Icon::Shell,
            risk: Risk::Exec,
            state: ToolState::Running,
            tail: String::new(),
            archive: None,
            diff: None,
            duration_ms: 0,
            plugin_view: None,
        };
        let queue = coalesced(&[
            thinking("t", "a"),
            block("c", tool),
            append("t", "b"),
            append("c", "1\n2\n3\n"),
            append("c", "4\n5\n6\n"),
        ]);
        let blocks = mirror(&[], &queue);
        assert_eq!(queue.len(), 2);
        assert!(matches!(&blocks[1].kind, BlockKind::Thinking { text, .. } if text == "ab"));
        assert!(
            matches!(&blocks[0].kind, BlockKind::Tool { tail, .. } if tail == "2\n3\n4\n5\n6\n")
        );
    }

    #[test]
    fn doc_tails_merge_from_the_earliest_changed_block() {
        let mut queue = coalesced(&[doc_tail("r", 1, &["a", "b", "c"]), doc_tail("r", 2, &["x"])]);
        assert_eq!(queue, vec![doc_tail("r", 1, &["a", "x"])]);
        push(&mut queue, doc_tail("r", 0, &["y"]));
        assert_eq!(queue, vec![doc_tail("r", 0, &["y"])]);
    }

    #[test]
    fn a_later_upsert_replaces_the_queued_one_and_drops_partial_changes() {
        let queue = coalesced(&[
            append("t", "lost"),
            thinking("u", "u"),
            thinking("t", "whole"),
            thinking("u", "u2"),
        ]);
        assert_eq!(queue, vec![thinking("u", "u2"), thinking("t", "whole")]);
    }

    #[test]
    fn only_the_latest_usage_state_stays_queued() {
        let meter = |calls| TimelinePatch::Usage {
            usage: Box::new(crate::UsageView {
                session: crate::Tally {
                    calls,
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        let queue = coalesced(&[meter(1), append("t", "a"), meter(2), append("t", "b")]);
        assert_eq!(queue, vec![append("t", "ab"), meter(2)]);
    }

    #[test]
    fn only_the_latest_status_stays_queued_and_a_reset_keeps_it() {
        let status = |queued| TimelinePatch::Status {
            status: crate::Status {
                queued,
                ..Default::default()
            },
        };
        let reset = TimelinePatch::Reset { blocks: vec![] };
        let queue = coalesced(&[status(1), append("t", "a"), status(2), reset.clone()]);
        assert_eq!(queue, vec![status(2), reset]);
    }

    #[test]
    fn only_the_latest_patch_per_plugin_slot_stays_queued() {
        use cox_protocol::plugin::Slot;
        let slot = |plugin: &str, slot, stopped| TimelinePatch::PluginSlot {
            slot: Box::new(crate::PluginSlot {
                plugin: plugin.into(),
                slot,
                view: None,
                visible: true,
                stopped,
            }),
        };
        let reset = TimelinePatch::Reset { blocks: vec![] };
        let queue = coalesced(&[
            slot("a", Slot::StatusLeft, false),
            slot("b", Slot::StatusLeft, false),
            slot("a", Slot::Panel, false),
            slot("a", Slot::StatusLeft, true),
            reset.clone(),
        ]);
        assert_eq!(
            queue,
            vec![
                slot("b", Slot::StatusLeft, false),
                slot("a", Slot::Panel, false),
                slot("a", Slot::StatusLeft, true),
                reset,
            ]
        );
    }

    #[test]
    fn coalescing_reaches_the_state_of_applying_every_patch() {
        let assistant = BlockKind::Assistant {
            plugin_view: None,
            text: String::new(),
            doc: StyledDoc {
                blocks: vec![para("p")],
            },
        };
        let start = mirror(&[], &[thinking("t", "0"), block("r", assistant)]);
        let patches = [
            append("t", "1"),
            doc_tail("r", 1, &["a", "b"]),
            append("t", "2"),
            TimelinePatch::Remove { id: id("t") },
            thinking("t", "again"),
            append("t", "!"),
            doc_tail("r", 2, &["c"]),
            doc_tail("r", 0, &["d", "e"]),
            doc_tail("r", 1, &["f"]),
        ];
        let queue = coalesced(&patches);
        assert!(queue.len() < patches.len());
        assert_eq!(mirror(&start, &queue), mirror(&start, &patches));
    }
}
