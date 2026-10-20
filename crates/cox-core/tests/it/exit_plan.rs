// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T67.7: `exit_plan` asks over the event stream. Only an approval moves the
//! session from Plan to Default; the tool itself never sets the mode.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cox_core::Session;
use cox_protocol::types::{Event, PermissionMode, Submission};
use cox_protocol::{Config, Tool};
use cox_provider::scripted::Scripted;
use cox_tools::plan::ExitPlanTool;

use crate::common::spawn_turn;

const SCENARIO: &str = r#"
[[turn]]
text = "ready"
tool_calls = [{ name = "exit_plan", input = { plan = "rename the helper" } }]

[[turn]]
text = "done"
"#;

fn open_session() -> (Session, tokio::sync::mpsc::Receiver<Event>) {
    let mut config = Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let store = Arc::new(cox_core::MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ExitPlanTool::surface())];
    let session = Session::new(
        config,
        Arc::new(Scripted::from_toml(SCENARIO, "").expect("scenario")),
        tools,
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, rx)
}

async fn enter_plan(session: &Session, rx: &mut tokio::sync::mpsc::Receiver<Event>) {
    session
        .submit(Submission::SetPermissionMode {
            mode: PermissionMode::Plan,
        })
        .await
        .expect("plan");
    // Session open can emit notices before the mode change lands.
    for _ in 0..8 {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        if matches!(
            ev,
            Event::StateChanged {
                mode: PermissionMode::Plan,
                ..
            }
        ) {
            return;
        }
    }
    panic!("plan mode was not applied");
}

/// Approval is what leaves Plan mode. Keeping the plan, or dismissing the
/// question, leaves the mode where it was.
#[tokio::test]
async fn exit_plan_requires_approval() {
    let (session, mut rx) = open_session();
    enter_plan(&session, &mut rx).await;
    let running = spawn_turn(&session, "go");
    let mut left = false;
    let mut saw_question = false;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        match ev {
            Event::QuestionAsked {
                call_id, options, ..
            } => {
                saw_question = true;
                assert!(cox_protocol::types::exit_plan_options(&options));
                session
                    .submit(Submission::Answer {
                        call_id,
                        text: Some(cox_protocol::types::EXIT_PLAN_KEEP.into()),
                    })
                    .await
                    .expect("keep");
            }
            Event::StateChanged {
                mode: PermissionMode::Default,
                ..
            } => left = true,
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");
    assert!(saw_question);
    assert!(!left, "keeping the plan must not leave Plan mode");

    let (session, mut rx) = open_session();
    enter_plan(&session, &mut rx).await;
    let running = spawn_turn(&session, "go");
    let mut left = false;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        match ev {
            Event::QuestionAsked { call_id, .. } => {
                session
                    .submit(Submission::Answer {
                        call_id,
                        text: Some(cox_protocol::types::EXIT_PLAN_APPROVE.into()),
                    })
                    .await
                    .expect("approve");
            }
            Event::StateChanged {
                mode: PermissionMode::Default,
                ..
            } => left = true,
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");
    assert!(left, "approval leaves Plan mode");
}
