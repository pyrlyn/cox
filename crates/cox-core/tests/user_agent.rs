// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Composer `@name task` lines (T45.5) through the loop:
//! `Submission::UserAgent` runs the `agent` tool on the model's own path
//! (engine, hooks, slots), its answer joins history at the tail, and it is
//! refused while a turn runs.

mod common;

use std::time::Duration;

use common::{drain, open, spawn_turn, tool_results};
use cox_core::Session;
use cox_protocol::Config;
use cox_protocol::types::{Event, Level, Submission, ToolResult};
use tokio::sync::mpsc::Receiver;

/// Collects events until `pred` matches one (inclusive).
async fn until(rx: &mut Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        let hit = pred(&ev);
        events.push(ev);
        if hit {
            return events;
        }
    }
}

/// Submits `@name task` and returns the result of its one `agent` call.
async fn dispatch(
    session: &Session,
    rx: &mut Receiver<Event>,
    name: &str,
    task: &str,
) -> ToolResult {
    let sub = session.clone();
    let (name, task) = (name.to_string(), task.to_string());
    let run = tokio::spawn(async move {
        sub.submit(Submission::UserAgent { name, task })
            .await
            .expect("user agent");
    });
    let events = until(rx, |e| matches!(e, Event::ToolCallDone { .. })).await;
    run.await.expect("join");
    match events.last() {
        Some(Event::ToolCallDone { result, .. }) => result.clone(),
        other => panic!("expected ToolCallDone, got {other:?}"),
    }
}

#[tokio::test]
async fn user_agent_runs_through_the_engine() {
    let mut config = Config::default();
    config.permissions.deny = vec!["agent".into()];
    let (session, _, mut rx) = open("[[turn]]\ntext = \"never asked\"\n", config);
    let refused = dispatch(&session, &mut rx, "explore", "look around").await;
    assert!(!refused.ok, "{}", refused.visible);
    assert!(
        refused.visible.starts_with("permission denied"),
        "{}",
        refused.visible
    );
}

#[tokio::test]
async fn user_agent_result_enters_history() {
    let (session, _, mut rx) = open("[[turn]]\ntext = \"found it\"\n", Config::default());
    let before = session.history().await.len();
    let answer = dispatch(&session, &mut rx, "explore", "look around").await;
    assert!(answer.ok, "{}", answer.visible);
    assert!(answer.visible.contains("found it"), "{}", answer.visible);
    let history = session.history().await;
    assert_eq!(history.len(), before + 1);
    let text = serde_json::to_string(&history[before]).expect("serialize");
    assert!(
        text.contains("@explore look around") && text.contains("found it"),
        "{text}"
    );
}

#[tokio::test]
async fn user_agent_refused_mid_turn() {
    let (session, _, mut rx) = open(&common::scenario("interrupt"), Config::default());
    let running = spawn_turn(&session, "go");
    let mut events = until(&mut rx, |e| matches!(e, Event::ToolCallRequested { .. })).await;
    let before = session.history().await.len();
    let sub = Submission::UserAgent {
        name: "explore".into(),
        task: "look around".into(),
    };
    session.submit(sub).await.expect("refused, not an error");
    events.extend(
        until(&mut rx, |e| {
            matches!(e, Event::Notice { level: Level::Warn, text }
                if text == "a turn is running; `@explore` waits until it ends")
        })
        .await,
    );
    assert_eq!(session.history().await.len(), before);
    session
        .submit(Submission::Interrupt)
        .await
        .expect("interrupt");
    running.await.expect("join").expect("turn");
    events.extend(drain(&mut rx).await);
    // Only the turn's own `slow` call finished; no `agent` call ran.
    assert_eq!(tool_results(&events).len(), 1, "{events:#?}");
}
