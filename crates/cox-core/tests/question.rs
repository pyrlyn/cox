// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `ask_user` over the event stream (T37.4, DT G4): the question is an
//! `Event::QuestionAsked`, the answer a `Submission::Answer`, so a headless
//! driver with no side channel answers it and the rollout keeps both.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::spawn_turn;
use cox_core::{MemoryStore, Session};
use cox_protocol::types::{Event, Submission};
use cox_protocol::{Config, Store, Tool};
use cox_provider::scripted::Scripted;
use cox_tools::ask_user::{Answers, AskUserTool};

const SCENARIO: &str = r#"
[[turn]]
text = "asking"
tool_calls = [{ name = "ask_user", input = { question = "which env?", options = ["staging", "prod"] } }]

[[turn]]
text = "done"
"#;

#[tokio::test]
async fn question_is_asked_and_answered_over_the_stream_and_both_reach_the_rollout() {
    let mut config = Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(AskUserTool::new(Answers::Surface))];
    let session = Session::new(
        config,
        Arc::new(Scripted::from_toml(SCENARIO, "").expect("scenario")),
        tools,
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "go");
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        match ev {
            Event::QuestionAsked {
                call_id, options, ..
            } => {
                assert_eq!(options, ["staging", "prod"]);
                session
                    .submit(Submission::Answer {
                        call_id,
                        text: Some("staging".into()),
                    })
                    .await
                    .expect("answer");
            }
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");

    let rollout = store.rollout_read(&session.id()).expect("rollout");
    let asked = rollout.iter().find_map(|e| match e {
        Event::QuestionAsked {
            call_id, question, ..
        } => Some((*call_id, question.clone())),
        _ => None,
    });
    let (call_id, question) = asked.expect("the rollout has the question");
    assert_eq!(question, "which env?");
    let answered = rollout.iter().find_map(|e| match e {
        Event::ToolCallDone {
            call_id: id,
            result,
        } if *id == call_id => Some(result),
        _ => None,
    });
    let result = answered.expect("the rollout has the answered call");
    assert!(result.ok);
    assert_eq!(result.visible, "staging");
}

#[tokio::test]
async fn an_answer_with_no_question_pending_is_a_warning_not_an_error() {
    let (session, _store, mut rx) = common::open("[[turn]]\ntext = \"a\"\n", Config::default());
    session
        .submit(Submission::Answer {
            call_id: cox_protocol::CallId::new(),
            text: Some("late".into()),
        })
        .await
        .expect("submit");
    let warned = loop {
        match rx.try_recv() {
            Ok(Event::Notice { text, .. }) => break text,
            Ok(_) => continue,
            Err(e) => panic!("no notice: {e:?}"),
        }
    };
    assert!(warned.starts_with("no question pending"), "{warned}");
}

const SUBAGENT: &str = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "pick an env", preset = "explore", tools = ["ask_user"] } }]

# child
[[turn]]
text = "asking"
tool_calls = [{ name = "ask_user", input = { question = "which env?" } }]

# child: answer
[[turn]]
text = "picked"

# parent
[[turn]]
text = "done"
"#;

/// A child's question has no surface of its own: the parent raises it,
/// labelled with the child, and hands the answer back to the child's call.
#[tokio::test]
async fn subagent_question_is_raised_on_the_parent_and_answered_back() {
    let mut config = Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(AskUserTool::new(Answers::Surface))];
    let session = Session::new(
        config,
        Arc::new(Scripted::from_toml(SUBAGENT, "").expect("scenario")),
        tools,
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "go");
    let mut child = None;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        match ev {
            Event::ApprovalRequired { call, .. } => session
                .submit(Submission::Approve {
                    call_id: call.id,
                    decision: cox_protocol::Decision::Allow,
                })
                .await
                .expect("approve"),
            Event::QuestionAsked {
                call_id, source, ..
            } => {
                let source = source.expect("labelled with the child");
                assert_eq!(source.agent.as_deref(), Some("explore-1"));
                child = Some((source.session, call_id));
                session
                    .submit(Submission::Answer {
                        call_id,
                        text: Some("staging".into()),
                    })
                    .await
                    .expect("answer");
            }
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");
    let (child, call_id) = child.expect("the child asked on the parent's stream");
    let answered = store
        .rollout_read(&child)
        .expect("child rollout")
        .into_iter()
        .find_map(|e| match e {
            Event::ToolCallDone {
                call_id: id,
                result,
            } if id == call_id => Some(result),
            _ => None,
        })
        .expect("the child's call finished");
    assert_eq!(answered.visible, "staging");
}
