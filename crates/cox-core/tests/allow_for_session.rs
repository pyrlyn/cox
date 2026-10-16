// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T69.2: `permissions.allow_for_session_persists` writes the approved
//! command prefixes through the installed `AllowRuleStore`, and nothing else.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{scenario, spawn_turn};
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::{CoreError, ToolError};
use cox_protocol::traits::{AllowRuleStore, Tool, ToolCx};
use cox_protocol::types::{
    ApprovalPolicy, Concurrency, Decision, Event, Risk, Segments, Submission, ToolOutput, ToolSpec,
};
use cox_provider::scripted::Scripted;
use serde_json::Value;

/// `bash`'s real parser and subject with a body that runs nothing, always
/// rated `Exec` so the `untrusted` policy asks about it.
struct SplitShell;

#[async_trait]
impl Tool for SplitShell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, input: &Value) -> String {
        cox_tools::bash::BashTool.subject(input)
    }
    fn segments(&self, input: &Value) -> Option<Segments> {
        cox_tools::bash::BashTool.segments(input)
    }
    fn risk(&self, _input: &Value) -> Risk {
        Risk::Exec
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: "ran".into(),
            is_error: false,
            structured: None,
            diff: None,
        })
    }
}

/// The user config's `allow` list in memory; skips a duplicate like the real
/// writer in `cox-config`.
#[derive(Default)]
struct Rules(Mutex<Vec<String>>);

impl AllowRuleStore for Rules {
    fn append_allow(&self, rule: &str) -> Result<bool, CoreError> {
        let mut rules = self.0.lock().expect("rules");
        if rules.iter().any(|r| r == rule) {
            return Ok(false);
        }
        rules.push(rule.into());
        Ok(true)
    }
}

struct Failing;

impl AllowRuleStore for Failing {
    fn append_allow(&self, _rule: &str) -> Result<bool, CoreError> {
        Err(CoreError::Config {
            key: "permissions.allow".into(),
            message: "read-only file".into(),
        })
    }
}

/// Runs the `bash` scenario with `command`, answers its approval with
/// `decision`, and returns the events.
async fn run(
    persists: bool,
    deny: &[&str],
    store: Arc<dyn AllowRuleStore>,
    command: &str,
    decision: Decision,
) -> Vec<Event> {
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    config.permissions.approval = ApprovalPolicy::Untrusted;
    config.permissions.allow_for_session_persists = persists;
    config.permissions.deny = deny.iter().map(|d| d.to_string()).collect();
    let toml = scenario("allow_for_session_bash").replace("cargo test && git status", command);
    let provider = Arc::new(Scripted::from_toml(&toml, "").expect("scenario"));
    let backing = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        vec![Arc::new(SplitShell)],
        backing.clone(),
        backing,
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    session.set_allow_rule_store(store);
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "go");
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        if let Event::ApprovalRequired { call, .. } = &ev {
            let call_id = call.id;
            session
                .submit(Submission::Approve {
                    call_id,
                    decision: decision.clone(),
                })
                .await
                .expect("approve");
        }
        let done = matches!(ev, Event::TurnDone { .. });
        events.push(ev);
        if done {
            break;
        }
    }
    running.await.expect("join").expect("turn");
    events.extend(drain_rest(&mut rx));
    events
}

fn drain_rest(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> Vec<Event> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn rules(store: &Rules) -> Vec<String> {
    store.0.lock().expect("rules").clone()
}

#[tokio::test]
async fn allow_for_session_flag_off_writes_nothing() {
    let store = Arc::new(Rules::default());
    run(
        false,
        &[],
        store.clone(),
        "cargo test",
        Decision::AllowForSession,
    )
    .await;
    assert!(rules(&store).is_empty());
}

#[tokio::test]
async fn allow_for_session_flag_on_saves_one_prefix_rule_per_command_once() {
    let store = Arc::new(Rules::default());
    let line = "cargo test && git status";
    run(true, &[], store.clone(), line, Decision::AllowForSession).await;
    assert_eq!(rules(&store), ["Bash(cargo test:*)", "Bash(git status:*)"]);
    // A later approval of the same line, in another session, adds nothing.
    run(true, &[], store.clone(), line, Decision::AllowForSession).await;
    assert_eq!(rules(&store), ["Bash(cargo test:*)", "Bash(git status:*)"]);
}

#[tokio::test]
async fn allow_for_session_a_plain_allow_saves_nothing() {
    let store = Arc::new(Rules::default());
    run(true, &[], store.clone(), "cargo test", Decision::Allow).await;
    assert!(rules(&store).is_empty());
}

#[tokio::test]
async fn allow_for_session_an_opaque_line_is_not_saved() {
    let store = Arc::new(Rules::default());
    let line = "Get-Content a.txt | Select-String x";
    let input = serde_json::json!({ "command": line });
    let segments = cox_tools::bash::BashTool
        .segments(&input)
        .expect("segments");
    assert!(segments.opaque, "the fixture must be an opaque line");
    let events = run(true, &[], store.clone(), line, Decision::AllowForSession).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ApprovalRequired { .. })),
        "the line must reach the approval it is answered at"
    );
    assert!(rules(&store).is_empty());
}

#[tokio::test]
async fn allow_for_session_a_denied_line_is_neither_asked_nor_saved() {
    let store = Arc::new(Rules::default());
    let events = run(
        true,
        &["Bash(git push:*)"],
        store.clone(),
        "cargo test && git push",
        Decision::AllowForSession,
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ApprovalRequired { .. }))
    );
    assert!(rules(&store).is_empty());
}

#[tokio::test]
async fn allow_for_session_a_failed_write_warns_and_still_runs_the_call() {
    let events = run(
        true,
        &[],
        Arc::new(Failing),
        "cargo test",
        Decision::AllowForSession,
    )
    .await;
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Notice { text, .. } if text.contains("could not save the approval")
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ToolCallDone { .. }))
    );
}
