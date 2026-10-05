// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Loop tests: Scripted provider + stub tools, golden Event JSONL (T2.1).

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{drain, open, run_with, scenario, spawn_turn, tool_results};
use cox_core::{MemoryStore, Session};
use cox_protocol::traits::{Store as _, Tool};
use cox_protocol::types::{Content, DecidedBy, Decision, Event, StopReason, Submission};
use tokio::sync::mpsc;

async fn run(name: &str) -> (Vec<Event>, Arc<MemoryStore>, Session) {
    run_with(name, cox_protocol::Config::default()).await
}

fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'))
}

fn redact(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) if is_ulid(s) => *s = "<id>".into(),
        // The truncation trailer embeds an archive id mid-string.
        serde_json::Value::String(s) if s.contains("expand #") => {
            let start = s.find("expand #").unwrap_or(0) + "expand #".len();
            if s.len() >= start + 26 && is_ulid(&s[start..start + 26]) {
                s.replace_range(start..start + 26, "<id>");
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if matches!(
                    k.as_str(),
                    "duration_ms"
                        | "input_tokens"
                        | "output_tokens"
                        | "cache_read_tokens"
                        | "cache_write_tokens"
                        | "latency_ms"
                ) {
                    *v = serde_json::json!(0);
                } else if k == "cost_usd" {
                    *v = serde_json::json!(0.0);
                } else if k == "breakdown" {
                    // A98: the split is a byte heuristic over the prompt and
                    // tool schemas, so any wording change would move it; the
                    // numbers are checked in `turn_every_request_emits_its_context_breakdown`.
                    if let serde_json::Value::Object(parts) = v {
                        parts
                            .iter_mut()
                            .filter(|(part, _)| *part != "window")
                            .for_each(|(_, n)| *n = serde_json::json!(0));
                    }
                } else {
                    redact(v);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

fn snapshot(name: &str, events: &[Event]) {
    let mut value = serde_json::to_value(events).expect("events json");
    redact(&mut value);
    insta::with_settings!({
        snapshot_path => "scenarios",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_json_snapshot!(format!("{name}.events"), value);
    });
}

#[tokio::test]
async fn turn_text_only_snapshot() {
    let (events, store, _) = run("text_only").await;
    snapshot("text_only", &events);
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    assert_eq!(store.usage_rows().len(), 1);
}

#[tokio::test]
async fn turn_one_tool_snapshot() {
    let (events, store, _) = run("one_tool").await;
    snapshot("one_tool", &events);
    assert_eq!(store.usage_rows().len(), 2);
}

#[tokio::test]
async fn turn_three_parallel_snapshot() {
    let (events, _, session) = run("three_parallel").await;
    snapshot("three_parallel", &events);
    let done: Vec<_> = tool_results(&events).into_iter().map(|r| r.1).collect();
    assert_eq!(done, ["a", "b", "c"]);
    let history = session.history().await;
    let results = history
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|c| matches!(c, Content::ToolResult { .. }))
        })
        .expect("one user message of tool results");
    let n = results
        .content
        .iter()
        .filter(|c| matches!(c, Content::ToolResult { .. }))
        .count();
    assert_eq!(n, 3);
}

#[tokio::test]
async fn turn_big_tool_output_is_truncated_then_expandable() {
    let mut config = cox_protocol::Config::default();
    config.context.tool_output_visible_bytes = 120;
    config.context.tool_output_head_lines = 2;
    config.context.tool_output_tail_lines = 2;
    let (events, store, _) = run_with("big_tool_output", config).await;
    snapshot("big_tool_output", &events);
    let result = events
        .iter()
        .find_map(|e| match e {
            Event::ToolCallDone { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("tool result");
    let archive = result.archive.expect("archived before truncation");
    assert!(result.visible.contains(&format!("expand #{}", archive.id)));
    assert!(result.visible.len() <= 120);
    // The follow-up `expand` call reads the whole output back from the archive.
    let (tx, _rx) = mpsc::channel(1);
    let cx = cox_tools::tool_cx(
        vec![PathBuf::from("/tmp/cox-turn")],
        PathBuf::from("/tmp/cox-turn"),
        cox_protocol::types::SandboxPolicy {
            mode: cox_protocol::types::SandboxMode::ReadOnly,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        },
        store,
        tokio_util::sync::CancellationToken::new(),
        tx,
        cox_protocol::ids::SessionId::new(),
        cox_protocol::ids::CallId::new(),
    );
    let expanded = cox_tools::expand::ExpandTool
        .call(serde_json::json!({"id": archive.id.to_string()}), &cx)
        .await
        .expect("expand");
    assert_eq!(expanded.text.len() as u64, archive.bytes);
    assert!(expanded.text.starts_with("line 01\n") && expanded.text.ends_with("line 20"));
}

#[tokio::test]
async fn turn_provider_error_snapshot() {
    let (events, _, _) = run("provider_error").await;
    snapshot("provider_error", &events);
    assert!(events.iter().any(|e| matches!(e, Event::Error { .. })));
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::Error,
            ..
        })
    ));
}

#[tokio::test]
async fn turn_max_turns_snapshot() {
    let mut config = cox_protocol::Config::default();
    config.core.max_turns = 1;
    let (events, _, _) = run_with("max_turns", config).await;
    snapshot("max_turns", &events);
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::MaxTurns,
            ..
        })
    ));
}

/// Collects events until `pred` matches one (inclusive).
async fn until(rx: &mut mpsc::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("wait event")
            .expect("closed");
        let hit = pred(&ev);
        events.push(ev);
        if hit {
            return events;
        }
    }
}

#[tokio::test]
async fn turn_interrupt_mid_tool_snapshot() {
    let (session, _, mut rx) = open(&scenario("interrupt"), cox_protocol::Config::default());
    let running = spawn_turn(&session, "interrupt");
    let mut events = until(&mut rx, |e| matches!(e, Event::ToolCallRequested { .. })).await;
    session
        .submit(Submission::Interrupt)
        .await
        .expect("interrupt");
    running.await.expect("join").expect("turn");
    events.extend(drain(&mut rx).await);
    snapshot("interrupt", &events);
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::Interrupted,
            ..
        })
    ));
}

/// Runs `name` until the first `ApprovalRequired`, answers it with
/// `decision`, then drains the turn.
async fn ask_scenario(name: &str, decision: Decision) -> Vec<Event> {
    ask_scenario_with(name, cox_protocol::Config::default(), decision).await
}

async fn ask_scenario_with(
    name: &str,
    config: cox_protocol::Config,
    decision: Decision,
) -> Vec<Event> {
    let (session, _, mut rx) = open(&scenario(name), config);
    let running = spawn_turn(&session, "write");
    let mut events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    let call_id = events
        .iter()
        .find_map(|e| match e {
            Event::ApprovalRequired { call, .. } => Some(call.id),
            _ => None,
        })
        .expect("prompt");
    session
        .submit(Submission::Approve { call_id, decision })
        .await
        .expect("approve");
    running.await.expect("join").expect("turn");
    events.extend(drain(&mut rx).await);
    events
}

#[tokio::test]
async fn turn_write_asks_then_runs_on_allow_snapshot() {
    let events = ask_scenario("ask_then_approve", Decision::Allow).await;
    snapshot("ask_then_approve", &events);
    assert_eq!(tool_results(&events), [(true, "touched".to_string())]);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ApprovalDecided {
            decision: Decision::Allow,
            by: DecidedBy::User,
            ..
        }
    )));
}

#[tokio::test]
async fn turn_write_asks_then_fails_on_deny_snapshot() {
    let events = ask_scenario(
        "ask_then_deny",
        Decision::Deny {
            reason: "no".into(),
        },
    )
    .await;
    snapshot("ask_then_deny", &events);
    assert_eq!(
        tool_results(&events),
        [(false, "permission denied: no".to_string())]
    );
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
}

#[tokio::test]
async fn turn_allow_for_session_covers_the_next_call() {
    let events = ask_scenario("allow_for_session", Decision::AllowForSession).await;
    let prompts = events
        .iter()
        .filter(|e| matches!(e, Event::ApprovalRequired { .. }))
        .count();
    assert_eq!(prompts, 1);
    assert_eq!(tool_results(&events).len(), 2);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ApprovalDecided {
            by: DecidedBy::User,
            decision: Decision::AllowForSession,
            ..
        }
    )));
}

#[tokio::test]
async fn turn_a_revoked_grant_asks_again() {
    let (session, store, mut rx) = open(&scenario("revoke_grant"), cox_protocol::Config::default());
    let first = spawn_turn(&session, "write");
    let events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    let call_id = events
        .iter()
        .find_map(|e| match e {
            Event::ApprovalRequired { call, .. } => Some(call.id),
            _ => None,
        })
        .expect("prompt");
    session
        .submit(Submission::Approve {
            call_id,
            decision: Decision::AllowForSession,
        })
        .await
        .expect("approve");
    first.await.expect("join").expect("turn");
    let covered = drain(&mut rx).await;
    assert!(
        !covered
            .iter()
            .any(|e| matches!(e, Event::ApprovalRequired { .. }))
    );
    let grant = ("touch".to_string(), "a".to_string());
    assert_eq!(session.grants().await, std::slice::from_ref(&grant));

    session
        .submit(Submission::RevokeGrant {
            tool: grant.0.clone(),
            subject: grant.1.clone(),
        })
        .await
        .expect("revoke");
    assert_eq!(
        rx.recv().await,
        Some(Event::GrantRevoked {
            tool: grant.0.clone(),
            subject: grant.1.clone(),
        })
    );
    assert!(session.grants().await.is_empty());

    let third = spawn_turn(&session, "write again");
    let asked = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    assert!(matches!(asked.last(), Some(Event::ApprovalRequired { .. })));
    session.interrupt();
    let _ = third.await;

    // Resume replays the revoke: the grant does not come back.
    let rollout = store.rollout_read(&session.id()).expect("rollout");
    let history = cox_core::History::from_events(&rollout);
    assert!(history.grants.is_empty());
}

#[tokio::test]
async fn turn_edited_input_goes_back_through_the_rules() {
    let mut config = cox_protocol::Config::default();
    config.permissions.ask = vec!["echo(hi)".into()];
    let events = ask_scenario_with(
        "one_tool",
        config,
        Decision::Edit {
            input: serde_json::json!({"text": "bye"}),
        },
    )
    .await;
    // `echo(bye)` matches no ask rule and is read-only, so the edited call
    // runs without a second prompt.
    let prompts = events
        .iter()
        .filter(|e| matches!(e, Event::ApprovalRequired { .. }))
        .count();
    assert_eq!(prompts, 1);
    assert_eq!(tool_results(&events), [(true, "bye".to_string())]);
}

#[tokio::test]
async fn turn_plan_mode_denies_write_without_prompt() {
    let mut config = cox_protocol::Config::default();
    config.permissions.mode = cox_protocol::types::PermissionMode::Plan;
    let (events, _, _) = run_with("ask_then_deny", config).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ApprovalRequired { .. }))
    );
    let results = tool_results(&events);
    assert_eq!(results.len(), 1);
    assert!(
        !results[0].0 && results[0].1.contains("plan mode"),
        "{results:?}"
    );
}

#[tokio::test]
async fn turn_all_tool_results_return_in_one_message() {
    let (_, _, session) = run("three_parallel").await;
    let with_results: Vec<_> = session
        .history()
        .await
        .into_iter()
        .filter(|m| {
            m.content
                .iter()
                .any(|c| matches!(c, Content::ToolResult { .. }))
        })
        .collect();
    assert_eq!(with_results.len(), 1);
    assert_eq!(
        with_results[0]
            .content
            .iter()
            .filter(|c| matches!(c, Content::ToolResult { .. }))
            .count(),
        3
    );
}

#[tokio::test]
async fn turn_no_event_after_turn_done() {
    let (session, _, mut rx) = open(&scenario("text_only"), cox_protocol::Config::default());
    session
        .submit(Submission::UserTurn {
            text: "hi".into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("submit");
    let events = drain(&mut rx).await;
    assert!(matches!(events.last(), Some(Event::TurnDone { .. })));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn turn_every_request_has_a_usage_row() {
    let (events, store, _) = run("one_tool").await;
    let usage_events = events
        .iter()
        .filter(|e| matches!(e, Event::Usage { .. }))
        .count();
    assert_eq!(usage_events, store.usage_rows().len());
    assert_eq!(usage_events, 2);
}

/// A98: each request is preceded by its window, taken from the model
/// catalog, and a split that is not empty and sums to its total.
#[tokio::test]
async fn turn_every_request_emits_its_context_breakdown() {
    let config = cox_protocol::Config::default();
    let catalog = cox_models::Catalog::load(&config, &[], None).expect("catalog");
    let window = catalog
        .get(&config.tiers.code.model)
        .and_then(|row| row.context_window);
    assert!(window.is_some(), "the default code model has a catalog row");
    let (events, _, _) = run("one_tool").await;
    let splits: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::ContextBreakdown { breakdown, .. } => Some(*breakdown),
            _ => None,
        })
        .collect();
    let usages = events
        .iter()
        .filter(|e| matches!(e, Event::Usage { .. }))
        .count();
    assert_eq!((splits.len(), usages), (2, 2), "one per request");
    for b in &splits {
        assert_eq!(b.window, window);
        assert!(b.system > 0 && b.tools > 0 && b.history > 0, "{b:?}");
        assert_eq!(b.system + b.tools + b.instructions + b.history, b.total);
    }
    assert!(
        splits[1].history > splits[0].history,
        "the tool round grew it"
    );
    let first_split = events
        .iter()
        .position(|e| matches!(e, Event::ContextBreakdown { .. }));
    let first_usage = events.iter().position(|e| matches!(e, Event::Usage { .. }));
    assert!(first_split < first_usage, "emitted before the reply");
}

#[tokio::test]
async fn sessions_index_captures_user_and_assistant_text() {
    let (_, store, _) = run("text_only").await;
    let indexed = store.indexed_texts();
    assert_eq!(indexed.len(), 2, "{indexed:?}");
    assert!(indexed[0].2.contains("text_only"), "{indexed:?}");
    assert_eq!(indexed[0].1, 1);
    assert!(indexed[1].2.contains("hello from scripted"), "{indexed:?}");
    assert_eq!(indexed[1].1, 1);
}
