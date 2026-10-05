// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The approval-policy × sandbox-mode matrix (T4.3, plan.md §1.8 step 8):
//! twelve cells of `exec_path`, then the loop-level behaviour the table
//! promises — `on-failure` runs an `Exec` call confined without asking, a
//! sandbox denial becomes `ApprovalRequired { SandboxDenied }`, and only an
//! explicit `Allow` reruns the command unconfined.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_core::permission::policy::{ExecPath, exec_path};
use cox_core::{Engine, MemoryStore, Outcome, Session};
use cox_protocol::CallId;
use cox_protocol::config::PermissionsConfig;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{
    ApprovalPolicy, Concurrency, DecidedBy, Decision, Event, Level, PermissionMode, Risk,
    SandboxMode, Submission, ToolCall, ToolOutput, ToolSpec, Why,
};
use cox_provider::scripted::Scripted;
use rstest::rstest;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// An `Exec`-risk stub that models a sandboxed build command: confined it
/// fails with `structured.sandbox_denied` (what `bash` reports when the
/// sandbox, not the command, broke it); unconfined it succeeds.
struct Confined;

fn output(text: &str, is_error: bool, denied: bool) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error,
        diff: None,
        structured: denied.then(|| json!({ "sandbox_denied": "Operation not permitted" })),
    }
}

#[async_trait]
impl Tool for Confined {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "confined".into(),
            description: "exec stub that fails under confinement".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        "make build".into()
    }
    async fn call(&self, _input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(if cx.sandbox.mode == SandboxMode::DangerFullAccess {
            output("built", false, false)
        } else {
            output("sandbox-exec: Operation not permitted", true, true)
        })
    }
}

/// §1.8 step 8's twelve cells. `on-failure` is the only policy that trusts
/// the sandbox, and only while there is one; `never` denies every ask;
/// everything else asks up front.
#[rstest]
#[case::untrusted_read_only(ApprovalPolicy::Untrusted, SandboxMode::ReadOnly, ExecPath::Ask)]
#[case::untrusted_workspace(ApprovalPolicy::Untrusted, SandboxMode::WorkspaceWrite, ExecPath::Ask)]
#[case::untrusted_full_access(
    ApprovalPolicy::Untrusted,
    SandboxMode::DangerFullAccess,
    ExecPath::Ask
)]
#[case::on_request_read_only(ApprovalPolicy::OnRequest, SandboxMode::ReadOnly, ExecPath::Ask)]
#[case::on_request_workspace(ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite, ExecPath::Ask)]
#[case::on_request_full_access(
    ApprovalPolicy::OnRequest,
    SandboxMode::DangerFullAccess,
    ExecPath::Ask
)]
#[case::on_failure_read_only(ApprovalPolicy::OnFailure, SandboxMode::ReadOnly, ExecPath::Confined)]
#[case::on_failure_workspace(
    ApprovalPolicy::OnFailure,
    SandboxMode::WorkspaceWrite,
    ExecPath::Confined
)]
#[case::on_failure_full_access(
    ApprovalPolicy::OnFailure,
    SandboxMode::DangerFullAccess,
    ExecPath::Ask
)]
#[case::never_read_only(ApprovalPolicy::Never, SandboxMode::ReadOnly, ExecPath::Deny)]
#[case::never_workspace(ApprovalPolicy::Never, SandboxMode::WorkspaceWrite, ExecPath::Deny)]
#[case::never_full_access(ApprovalPolicy::Never, SandboxMode::DangerFullAccess, ExecPath::Deny)]
fn policy_matrix_exec_paths(
    #[case] policy: ApprovalPolicy,
    #[case] sandbox: SandboxMode,
    #[case] expected: ExecPath,
) {
    assert_eq!(exec_path(policy, sandbox), expected);
}

fn exec_call() -> ToolCall {
    ToolCall {
        id: CallId::new(),
        name: "bash".into(),
        risk: Risk::Exec,
        subject: "make build".into(),
        input: json!({"command": "make build"}),
        segments: None,
    }
}

/// The engine side of the same table: `on-failure` + a real sandbox allows
/// an otherwise-unsettled `Exec` call without asking, and falls back to
/// asking without one.
#[test]
fn policy_engine_follows_the_matrix() {
    let engine = Engine::compile(&PermissionsConfig::default(), None, Path::new("/repo"))
        .expect("defaults compile");
    assert_eq!(
        engine.decide(
            &exec_call(),
            PermissionMode::Default,
            ApprovalPolicy::OnFailure,
            SandboxMode::WorkspaceWrite,
            &[]
        ),
        Outcome::Allow {
            by: DecidedBy::Policy
        }
    );
    assert!(matches!(
        engine.decide(
            &exec_call(),
            PermissionMode::Default,
            ApprovalPolicy::OnFailure,
            SandboxMode::DangerFullAccess,
            &[]
        ),
        Outcome::Ask(Why::Risk { .. })
    ));
    assert!(matches!(
        engine.decide(
            &exec_call(),
            PermissionMode::Default,
            ApprovalPolicy::Never,
            SandboxMode::WorkspaceWrite,
            &[]
        ),
        Outcome::Deny { .. }
    ));
}

/// A session over the `Confined` stub under a chosen policy and sandbox
/// mode; the caller drives the turn and answers any ask.
fn open_confined(
    approval: ApprovalPolicy,
    sandbox: SandboxMode,
) -> (Session, mpsc::Receiver<Event>) {
    let mut config = cox_protocol::Config::default();
    config.permissions.approval = approval;
    config.sandbox.mode = sandbox;
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-policy")];
    let scenario = common::scenario("confined_exec");
    let provider = Arc::new(Scripted::from_toml(&scenario, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        vec![Arc::new(Confined)],
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-policy"),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, rx)
}

/// Collects events up to (and including) the first one matching `pred`.
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

fn answer(events: &[Event], decision: Decision) -> Option<cox_protocol::types::Submission> {
    let call_id = events.iter().find_map(|e| match e {
        Event::ApprovalRequired { call, .. } => Some(call.id),
        _ => None,
    })?;
    Some(Submission::Approve { call_id, decision })
}

/// `on-failure` with a sandbox: the call runs confined *without* an approval
/// prompt first; the denial then asks; `Allow` reruns it unconfined and the
/// model sees the successful build.
#[tokio::test]
async fn policy_on_failure_denial_asks_then_allow_reruns_unconfined() {
    let (session, mut rx) = open_confined(ApprovalPolicy::OnFailure, SandboxMode::WorkspaceWrite);
    let running = common::spawn_turn(&session, "build it");
    let mut events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    // The first prompt must be the sandbox denial, not a pre-run ask.
    match events.iter().find_map(|e| match e {
        Event::ApprovalRequired { why, .. } => Some(why),
        _ => None,
    }) {
        Some(Why::SandboxDenied { detail }) => {
            assert!(detail.contains("Operation not permitted"), "{detail}");
        }
        other => panic!("expected a sandbox-denial ask, got {other:?}"),
    }
    session
        .submit(answer(&events, Decision::Allow).expect("prompt"))
        .await
        .expect("approve");
    running.await.expect("join").expect("turn");
    events.extend(common::drain(&mut rx).await);

    // The loop reports the rerun's result: ok, with the unconfined output.
    assert_eq!(common::tool_results(&events), [(true, "built".to_string())]);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ApprovalDecided {
            decision: Decision::Allow,
            ..
        }
    )));
}

/// Same denial, answered `Deny`: the confined failure stands as the result
/// the model reads.
#[tokio::test]
async fn policy_on_failure_denial_denied_keeps_confined_failure() {
    let (session, mut rx) = open_confined(ApprovalPolicy::OnFailure, SandboxMode::WorkspaceWrite);
    let running = common::spawn_turn(&session, "build it");
    let mut events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    session
        .submit(
            answer(
                &events,
                Decision::Deny {
                    reason: "keep it confined".into(),
                },
            )
            .expect("prompt"),
        )
        .await
        .expect("deny");
    running.await.expect("join").expect("turn");
    events.extend(common::drain(&mut rx).await);

    assert_eq!(
        common::tool_results(&events),
        [(false, "sandbox-exec: Operation not permitted".to_string())]
    );
}

/// `on-failure` without a sandbox behaves like `on-request`: it asks before
/// the first run (a `Risk` ask, not a `SandboxDenied` one), and denying
/// there never runs the command.
#[tokio::test]
async fn policy_on_failure_full_access_asks_before_running() {
    let (session, mut rx) = open_confined(ApprovalPolicy::OnFailure, SandboxMode::DangerFullAccess);
    let running = common::spawn_turn(&session, "build it");
    let mut events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    assert!(matches!(
        events.iter().find_map(|e| match e {
            Event::ApprovalRequired { why, .. } => Some(why),
            _ => None,
        }),
        Some(Why::Risk { risk: Risk::Exec })
    ));
    session
        .submit(
            answer(
                &events,
                Decision::Deny {
                    reason: "not today".into(),
                },
            )
            .expect("prompt"),
        )
        .await
        .expect("deny");
    running.await.expect("join").expect("turn");
    events.extend(common::drain(&mut rx).await);

    // The command never ran: the result is the denial the model reads.
    let results = common::tool_results(&events);
    assert_eq!(results.len(), 1);
    assert!(!results[0].0);
    assert!(
        results[0].1.contains("permission denied"),
        "{:?}",
        results[0]
    );
}

/// The loud marker: a `danger-full-access` session announces itself with a
/// `Security` notice right after `SessionStarted`.
#[tokio::test]
async fn policy_danger_full_access_is_loud() {
    let (session, mut rx) = open_confined(ApprovalPolicy::OnRequest, SandboxMode::DangerFullAccess);
    let running = common::spawn_turn(&session, "build it");
    // On-request + Exec asks, then parks; answer to let the turn finish.
    let events = until(&mut rx, |e| matches!(e, Event::ApprovalRequired { .. })).await;
    let notice = events.iter().find_map(|e| match e {
        Event::Notice { level, text } => Some((level, text)),
        _ => None,
    });
    match notice {
        Some((level, text)) => {
            assert_eq!(level, &Level::Security);
            assert!(text.contains("danger-full-access"), "{text}");
        }
        None => panic!("no security notice before the first ask: {events:?}"),
    }
    session
        .submit(
            answer(
                &events,
                Decision::Deny {
                    reason: "done".into(),
                },
            )
            .expect("prompt"),
        )
        .await
        .expect("deny");
    running.await.expect("join").expect("turn");
}
