// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Microcompaction tests (T8.2): old results become `Pointer`s in the
//! request copy; the stored history (and its archives) is untouched.

use std::collections::HashMap;

use cox_core::microcompact;
use cox_protocol::ids::{ArchiveId, CallId};
use cox_protocol::types::{ArchiveRef, Content, Message, Role};

fn tool_turn(call: CallId, name: &str, output: &str) -> Vec<Message> {
    vec![
        Message {
            role: Role::User,
            content: vec![Content::Text {
                text: "do it".into(),
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![Content::ToolUse {
                id: call,
                name: name.into(),
                input: serde_json::json!({}),
            }],
        },
        Message {
            role: Role::User,
            content: vec![Content::ToolResult {
                call_id: call,
                content: output.into(),
                is_error: false,
            }],
        },
    ]
}

fn three_turns() -> (
    Vec<Message>,
    Vec<usize>,
    HashMap<CallId, ArchiveRef>,
    Vec<CallId>,
) {
    let mut messages = Vec::new();
    let mut starts = Vec::new();
    let mut archives = HashMap::new();
    let mut calls = Vec::new();
    for (i, out) in ["out0", "out1", "out2"].iter().enumerate() {
        let call = CallId::new();
        starts.push(messages.len());
        messages.extend(tool_turn(call, "echo", out));
        archives.insert(
            call,
            ArchiveRef {
                id: ArchiveId::new(),
                bytes: (10 + i) as u64,
            },
        );
        calls.push(call);
    }
    let _ = messages;
    (messages, starts, archives, calls)
}

#[test]
fn microcompact_old_results_become_pointers() {
    let (messages, starts, archives, _) = three_turns();
    let out = microcompact(&messages, &starts, 1, 1, &archives);
    assert_eq!(out.len(), messages.len(), "message count unchanged");
    let pointers = out
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, Content::Pointer { .. }))
        .count();
    assert_eq!(pointers, 2, "first two turns pointered, last kept");
    for c in &out[8].content {
        assert!(
            matches!(c, Content::ToolResult { content, .. } if content == "out2"),
            "newest result stays full"
        );
    }
}

#[test]
fn microcompact_keeps_last_keep_turns_verbatim() {
    let (messages, starts, archives, _) = three_turns();
    let out = microcompact(&messages, &starts, 2, 0, &archives);
    // keep=2 protects the last two turns even with after=0.
    assert_eq!(&out[3..], &messages[3..]);
    assert!(matches!(out[2].content[0], Content::Pointer { .. }));
}

#[test]
fn microcompact_rollout_untouched_and_expand_works() {
    use cox_protocol::traits::{ArchivePut, Store as _};
    let store = cox_core::MemoryStore::new();
    let bytes = b"full tool output".to_vec();
    let id = store
        .archive_put(&ArchivePut {
            session: cox_protocol::ids::SessionId::new(),
            call: CallId::new(),
            tool: "echo".into(),
            subject: None,
            bytes: bytes.clone(),
        })
        .expect("archive");
    let call = CallId::new();
    let messages = tool_turn(call, "echo", "full tool output");
    let mut archives = HashMap::new();
    archives.insert(
        call,
        ArchiveRef {
            id,
            bytes: bytes.len() as u64,
        },
    );
    let before = messages.clone();
    let out = microcompact(&messages, &[0], 0, 0, &archives);
    assert_eq!(messages, before, "input slice not mutated");
    let pointer = out[0]
        .content
        .iter()
        .chain(&out[1].content)
        .chain(&out[2].content)
        .find_map(|c| match c {
            Content::Pointer { archive, .. } => Some(archive),
            _ => None,
        })
        .expect("old result pointered");
    // `cox expand` reads through the same archive row.
    assert_eq!(store.archive_get(&pointer.id).expect("expand"), bytes);
}

#[test]
fn microcompact_empty_turn_info_is_noop() {
    let (messages, _, archives, _) = three_turns();
    assert_eq!(microcompact(&messages, &[], 2, 6, &archives), messages);
}
