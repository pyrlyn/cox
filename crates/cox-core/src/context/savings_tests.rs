// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Token-savings guards for this module: history read dedup (never a false
//! hit), the per-model-family edit tool, and the earlier microcompaction
//! default. Fixtures are fixed, so every byte count asserted is exact.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use cox_protocol::ids::{ArchiveId, CallId};
use cox_protocol::traits::Tool;
use cox_protocol::types::{ArchiveRef, Content, Message, Role, Tier};
use serde_json::{Value, json};

use super::{assemble_with, dedup_reads, microcompact, prefers_apply_patch};

/// A fixed 4 KiB file body.
fn body(tag: &str) -> String {
    (1..=64)
        .map(|n| format!("{n}\t{tag} line {n:03} padding padding padding padding pad\n"))
        .collect()
}

/// One user turn, one tool call, its result, and a short answer.
fn round(name: &str, input: Value, result: &str, is_error: bool) -> (CallId, Vec<Message>) {
    let id = CallId::new();
    let msgs = vec![
        Message {
            role: Role::User,
            content: vec![Content::Text { text: "go".into() }],
        },
        Message {
            role: Role::Assistant,
            content: vec![Content::ToolUse {
                id,
                name: name.into(),
                input,
            }],
        },
        Message {
            role: Role::User,
            content: vec![Content::ToolResult {
                call_id: id,
                content: result.into(),
                is_error,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![Content::Text { text: "ok".into() }],
        },
    ];
    (id, msgs)
}

fn result_text(msgs: &[Message], id: CallId) -> &str {
    msgs.iter()
        .flat_map(|m| &m.content)
        .find_map(|c| match c {
            Content::ToolResult {
                call_id, content, ..
            } if *call_id == id => Some(content.as_str()),
            _ => None,
        })
        .expect("result")
}

fn bytes(msgs: &[Message]) -> usize {
    serde_json::to_string(msgs).expect("json").len()
}

#[test]
fn identical_read_of_unchanged_range_is_deduped_and_shrinks_history() {
    let input = json!({"path": "src/a.rs", "lines": "1-64"});
    let (first, mut h) = round("read", input.clone(), &body("a"), false);
    let (second, more) = round("read", input, &body("a"), false);
    h.extend(more);
    let out = dedup_reads(&h);
    assert_eq!(
        result_text(&out, first),
        body("a"),
        "the earlier block is untouched"
    );
    let r = result_text(&out, second);
    assert!(
        r.starts_with("unchanged:") && r.contains(&first.to_string()),
        "{r}"
    );
    // Savings: the second 4 KiB body is replaced by a ~100-byte reference.
    let saved = bytes(&h) - bytes(&out);
    assert!(saved >= body("a").len() - 150, "saved {saved}");
    // Deterministic: the same history gives the same bytes (cache-stable).
    assert_eq!(dedup_reads(&h), out);
}

/// The main criterion: a re-read is only ever replaced by a result that is
/// byte-identical for the identical call. Every one of these must miss.
#[test]
fn dedup_never_hands_out_another_calls_result() {
    let base = json!({"path": "src/a.rs", "lines": "1-64"});
    let cases: Vec<(&str, Value, String, bool)> = vec![
        // The file changed between the reads (e.g. an edit in between).
        ("read", base.clone(), body("changed"), false),
        // A different range of the same file.
        (
            "read",
            json!({"path": "src/a.rs", "lines": "2-65"}),
            body("a"),
            false,
        ),
        // A different file with, by chance, the same text.
        (
            "read",
            json!({"path": "src/b.rs", "lines": "1-64"}),
            body("a"),
            false,
        ),
        // A different mode.
        (
            "read",
            json!({"path": "src/a.rs", "lines": "1-64", "anchors": true}),
            body("a"),
            false,
        ),
        // Another tool with the same input and text.
        ("grep", base.clone(), body("a"), false),
        // A failed read is never a reference target or a hit.
        ("read", base.clone(), body("a"), true),
    ];
    for (name, input, text, is_error) in cases {
        let (_, mut h) = round("read", base.clone(), &body("a"), false);
        let (second, more) = round(name, input.clone(), &text, is_error);
        h.extend(more);
        let out = dedup_reads(&h);
        assert_eq!(
            result_text(&out, second),
            text,
            "false hit for {name} {input}"
        );
        assert_eq!(out, h, "nothing else changes for {name} {input}");
    }
    // An error first, then a good read: the good one is not deduped against it.
    let (_, mut h) = round("read", base.clone(), &body("a"), true);
    let (second, more) = round("read", base, &body("a"), false);
    h.extend(more);
    assert_eq!(result_text(&dedup_reads(&h), second), body("a"));
}

#[test]
fn small_results_are_left_alone() {
    let input = json!({"path": "x"});
    let (_, mut h) = round("read", input.clone(), "1\tshort", false);
    let (_, more) = round("read", input, "1\tshort", false);
    h.extend(more);
    assert_eq!(dedup_reads(&h), h);
}

#[test]
fn edit_tool_family_detection() {
    for m in [
        "gpt-5.2",
        "openai/gpt-5",
        "gpt-5.3-codex",
        "o3",
        "o4-mini",
        "codex-mini",
    ] {
        assert!(prefers_apply_patch(m), "{m}");
    }
    for m in [
        "claude-sonnet-5",
        "anthropic/claude-opus-4",
        "gemini-3.8-flash",
        "opus",
        "glm-5",
    ] {
        assert!(!prefers_apply_patch(m), "{m}");
    }
}

fn edit_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(cox_tools::read::ReadTool),
        Arc::new(cox_tools::edit::EditTool),
        Arc::new(cox_tools::v4a::ApplyPatchTool),
    ]
}

fn names_for(model: &str, mode: &str) -> (Vec<String>, usize) {
    let mut config = cox_protocol::Config::default();
    config.tiers.code.model = model.into();
    config.context.edit_tools = mode.into();
    let req = assemble_with(
        &[],
        &config,
        Tier::Code,
        &edit_tools(),
        &[],
        Path::new("/w"),
        "d",
    );
    let names = req.tools.iter().map(|t| t.name.clone()).collect();
    (names, req.system[0].text.len())
}

#[test]
fn one_edit_tool_per_model_family_and_both_on_request() {
    let (claude, claude_bytes) = names_for("claude-sonnet-5", "auto");
    assert_eq!(claude, ["edit", "read"]);
    let (gpt, gpt_bytes) = names_for("gpt-5.2", "auto");
    assert_eq!(gpt, ["apply_patch", "read"]);
    let (both, both_bytes) = names_for("claude-sonnet-5", "both");
    assert_eq!(both, ["apply_patch", "edit", "read"]);
    // Savings: the dropped schema's bytes leave system[0].
    let patch = cox_tools::v4a::ApplyPatchTool.spec();
    let edit = cox_tools::edit::EditTool.spec();
    assert!(both_bytes - claude_bytes >= patch.description.len());
    assert!(both_bytes - gpt_bytes >= edit.description.len());
}

#[test]
fn only_one_present_is_never_dropped() {
    let mut config = cox_protocol::Config::default();
    config.tiers.code.model = "gpt-5.2".into();
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(cox_tools::edit::EditTool)];
    let req = assemble_with(&[], &config, Tier::Code, &tools, &[], Path::new("/w"), "d");
    assert_eq!(req.tools.len(), 1, "edit stays when apply_patch is absent");
}

/// Ten turns of one 4 KiB read each, all archived: the default of 4 turns
/// pointers two more turns than the old 6, and saves their bodies.
#[test]
fn earlier_microcompaction_shrinks_history_by_two_bodies() {
    let mut h = Vec::new();
    let mut marks = Vec::new();
    let mut archives = HashMap::new();
    for t in 0..10 {
        marks.push(h.len());
        let (id, msgs) = round(
            "read",
            json!({"path": format!("f{t}.rs")}),
            &body("x"),
            false,
        );
        archives.insert(
            id,
            ArchiveRef {
                id: ArchiveId::new(),
                bytes: body("x").len() as u64,
            },
        );
        h.extend(msgs);
    }
    let keep = cox_protocol::Config::default().context.keep_turns;
    let after = cox_protocol::Config::default()
        .context
        .microcompact_after_turns;
    assert_eq!(after, 4, "the shipped default");
    let old = bytes(&microcompact(&h, &marks, keep, 6, &archives));
    let new = bytes(&microcompact(&h, &marks, keep, after, &archives));
    let saved = old - new;
    let per_body = body("x").len();
    assert!(
        saved > 2 * (per_body - 200) && saved < 2 * per_body + 200,
        "two bodies saved, got {saved}"
    );
}
