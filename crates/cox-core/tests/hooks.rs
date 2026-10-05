// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.4 in the loop: hook verdicts reach the turn through the core's call
//! sites, and a broken hook is a warning, never a stopped turn
//! (`broken_hook_is_skipped_not_fatal`, plan.md §1.10 #10).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::traits::Hook;
use cox_protocol::types::{
    Content, Decision, Event, HookEvent, HookOutcome, ItemKind, Level, Role, StopReason, Submission,
};
use serde_json::{Value, json};

/// Answers `on` (`PreToolUse` by default) with `verdict`, everything else with `Continue`,
/// and records every event it saw.
struct Stub {
    on: HookEvent,
    verdict: HookOutcome,
    seen: Mutex<Vec<(HookEvent, Value)>>,
}

impl Stub {
    fn new(verdict: HookOutcome) -> Arc<Self> {
        Self::on(HookEvent::PreToolUse, verdict)
    }

    fn on(on: HookEvent, verdict: HookOutcome) -> Arc<Self> {
        Arc::new(Self {
            on,
            verdict,
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl Hook for Stub {
    async fn run(&self, event: HookEvent, payload: Value, _timeout: Duration) -> HookOutcome {
        self.seen.lock().unwrap().push((event, payload));
        if event == self.on {
            self.verdict.clone()
        } else {
            HookOutcome::Continue
        }
    }
}

/// A source whose events are granted rather than configured (a plugin's,
/// PL§6): interested in `Notification` whatever `[hooks]` says.
#[derive(Default)]
struct Granted {
    seen: Mutex<Vec<HookEvent>>,
}

#[async_trait]
impl Hook for Granted {
    fn interested(&self, event: HookEvent, _config: &cox_protocol::config::HooksConfig) -> bool {
        event == HookEvent::Notification
    }

    async fn run(&self, event: HookEvent, _payload: Value, _timeout: Duration) -> HookOutcome {
        self.seen.lock().unwrap().push(event);
        HookOutcome::Continue
    }
}

async fn run_one_tool(stub: Arc<Stub>) -> Vec<Event> {
    let (session, _store, mut rx) = common::open(
        &common::scenario("one_tool"),
        cox_protocol::Config::default(),
    );
    session.set_hook(stub);
    let running = common::spawn_turn(&session, "one_tool");
    let events = common::drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    events
}

#[tokio::test]
async fn broken_hook_is_skipped_not_fatal() {
    let stub = Stub::new(HookOutcome::Failed {
        error: "exit 7".into(),
    });
    let events = run_one_tool(stub.clone()).await;
    assert!(common::tool_results(&events)[0].0);
    assert!(events.iter().any(|e| matches!(e, Event::Notice { level: Level::Warn, text } if text == "hook PreToolUse skipped: exit 7")));
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    let seen: Vec<&str> = stub
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(e, _)| e.name())
        .collect();
    assert_eq!(
        seen,
        ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]
    );
}

#[tokio::test]
async fn plugin_hook_fires_without_hooks_config() {
    let granted = Arc::new(Granted::default());
    let config = cox_protocol::Config::default();
    assert!(config.hooks.events.is_empty());
    let (session, _store, mut rx) = common::open(&common::scenario("one_tool"), config);
    session.set_hook(granted.clone());
    let running = common::spawn_turn(&session, "one_tool");
    common::drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let seen = granted.seen.lock().unwrap();
    // `Notification` (turn done) is dispatched only on the hook's own say.
    assert!(seen.contains(&HookEvent::Notification), "{seen:?}");
    assert!(!seen.contains(&HookEvent::SessionStart), "{seen:?}");
}

#[tokio::test]
async fn hooks_pre_tool_use_block_fails_the_call_with_the_reason() {
    let stub = Stub::new(HookOutcome::Block {
        reason: "no echoing".into(),
    });
    let events = run_one_tool(stub.clone()).await;
    let (ok, visible) = &common::tool_results(&events)[0];
    assert!(!ok);
    assert!(visible.contains("blocked by hook: no echoing"), "{visible}");
    let seen = stub.seen.lock().unwrap();
    let (_, payload) = seen
        .iter()
        .find(|(e, _)| *e == HookEvent::PreToolUse)
        .unwrap();
    assert_eq!(payload["tool_name"], "echo");
    assert_eq!(payload["tool_input"], json!({ "text": "hi" }));
    assert_eq!(payload["hook_event_name"], "PreToolUse");
    assert!(
        payload["session_id"]
            .as_str()
            .is_some_and(|s| s.len() == 26)
    );
}

#[tokio::test]
async fn hooks_pre_tool_use_modify_rewrites_the_input() {
    let stub = Stub::new(HookOutcome::Modify {
        input: json!({ "text": "rewritten" }),
    });
    let events = run_one_tool(stub.clone()).await;
    let (ok, visible) = &common::tool_results(&events)[0];
    assert!(ok);
    assert!(visible.contains("rewritten"), "{visible}");
    let seen = stub.seen.lock().unwrap();
    let (_, post) = seen
        .iter()
        .find(|(e, _)| *e == HookEvent::PostToolUse)
        .unwrap();
    assert_eq!(post["tool_input"], json!({ "text": "rewritten" }));
    assert_eq!(post["tool_response"]["is_error"], false);
}

#[tokio::test]
async fn hooks_additional_context_rides_as_a_second_block_not_as_the_prompt() {
    let stub = Stub::on(
        HookEvent::UserPromptSubmit,
        HookOutcome::Modify {
            input: json!({ "additional_context": "peer editing x.rs" }),
        },
    );
    let events = run_one_tool(stub).await;
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ItemStarted {
            kind: ItemKind::UserMessage { text, .. },
            ..
        } if text == "one_tool"
    )));
}

#[tokio::test]
async fn hooks_additional_context_reaches_the_model_after_the_prompt() {
    let stub = Stub::on(
        HookEvent::UserPromptSubmit,
        HookOutcome::Modify {
            input: json!({ "prompt": "rewritten", "additional_context": "peer editing x.rs" }),
        },
    );
    let (session, _store, mut rx) = common::open(
        &common::scenario("one_tool"),
        cox_protocol::Config::default(),
    );
    session.set_hook(stub);
    let running = common::spawn_turn(&session, "one_tool");
    common::drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let history = session.history().await;
    assert_eq!(history[0].role, Role::User);
    assert_eq!(
        history[0].content,
        [
            Content::Text {
                text: "rewritten".into()
            },
            Content::Text {
                text: "peer editing x.rs".into()
            },
        ]
    );
}

#[tokio::test]
async fn hooks_permission_request_fires_while_the_turn_waits_for_the_user() {
    let stub = Stub::new(HookOutcome::Continue);
    let (session, _store, mut rx) = common::open(
        &common::scenario("ask_then_approve"),
        cox_protocol::Config::default(),
    );
    session.set_hook(stub.clone());
    let running = common::spawn_turn(&session, "write");
    let call_id = loop {
        if let Event::ApprovalRequired { call, .. } = rx.recv().await.expect("event stream closed")
        {
            break call.id;
        }
    };
    session
        .submit(Submission::Approve {
            call_id,
            decision: Decision::Allow,
        })
        .await
        .expect("approve");
    common::drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let seen = stub.seen.lock().unwrap();
    let (_, payload) = seen
        .iter()
        .find(|(e, _)| *e == HookEvent::PermissionRequest)
        .expect("PermissionRequest fired");
    assert_eq!(payload["tool_name"], "touch");
    assert_eq!(payload["tool_input"]["path"], "a");
}
