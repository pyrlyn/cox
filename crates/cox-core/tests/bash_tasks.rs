// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `bash` background tasks (T27.1) through the loop with the real
//! `BashTool`: `background: true` and `Submission::Background` both turn
//! the call into a registered task whose completion carries the exit code
//! and the archive row of the full output.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cox_core::{MemoryStore, Session};
use cox_protocol::ids::{ArchiveId, CallId};
use cox_protocol::traits::Archive;
use cox_protocol::types::{Event, Submission};
use cox_provider::scripted::Scripted;
use cox_tools::bash::BashTool;
use tokio::sync::mpsc::Receiver;

/// A session over `toml` whose only tool is the real `bash`, allowed by
/// rule, in a fresh directory under the system temp dir.
fn open(toml: &str) -> (Session, Arc<MemoryStore>, Receiver<Event>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("cox-t27-{}", CallId::new()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let dir = dir.canonicalize().expect("canonical");
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![dir.clone()];
    config.permissions.allow = vec!["Bash".into()];
    let provider = Arc::new(Scripted::from_toml(toml, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        vec![Arc::new(BashTool)],
        store.clone(),
        store.clone(),
        dir.clone(),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, store, rx, dir)
}

fn user_turn(session: &Session) -> tokio::task::JoinHandle<()> {
    let session = session.clone();
    tokio::spawn(async move {
        session
            .submit(Submission::UserTurn {
                text: "go".into(),
                attachments: vec![],
                confirm_think: false,
            })
            .await
            .expect("turn");
    })
}

/// Receives until `stop` holds for an event; returns everything seen.
async fn until(rx: &mut Receiver<Event>, seen: &mut Vec<Event>, stop: impl Fn(&Event) -> bool) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        let done = stop(&ev);
        seen.push(ev);
        if done {
            return;
        }
    }
}

fn completion(events: &[Event]) -> (Option<i32>, ArchiveId) {
    events
        .iter()
        .find_map(|e| match e {
            Event::TaskCompleted {
                exit_code, archive, ..
            } => Some((*exit_code, archive.expect("archived"))),
            _ => None,
        })
        .expect("TaskCompleted")
}

fn pointer_result(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            Event::ToolCallDone { result, .. } => Some(result.visible.clone()),
            _ => None,
        })
        .expect("ToolCallDone")
}

#[tokio::test]
async fn bash_background_registers_task() {
    let (session, store, mut rx, dir) = open(
        r#"
[[turn]]
text = "starting"
tool_calls = [{ name = "bash", input = { command = "echo started-bg; exit 3", background = true } }]

[[turn]]
text = "done"
"#,
    );
    let turn = user_turn(&session);
    let mut events = Vec::new();
    until(&mut rx, &mut events, |e| {
        matches!(e, Event::TurnDone { .. })
    })
    .await;
    if !events
        .iter()
        .any(|e| matches!(e, Event::TaskCompleted { .. }))
    {
        until(&mut rx, &mut events, |e| {
            matches!(e, Event::TaskCompleted { .. })
        })
        .await;
    }
    until(
        &mut rx,
        &mut events,
        |e| matches!(e, Event::Notice { text, .. } if text.contains("finished")),
    )
    .await;
    turn.await.expect("join");

    let label = events.iter().find_map(|e| match e {
        Event::TaskCreated { label, .. } => Some(label.clone()),
        _ => None,
    });
    assert_eq!(label.as_deref(), Some("bash: echo started-bg; exit 3"));
    let pointer = pointer_result(&events);
    assert!(pointer.contains("background task"), "{pointer}");
    let (exit_code, archive) = completion(&events);
    assert_eq!(exit_code, Some(3));
    let bytes = store.get(&archive).await.expect("archive row");
    assert!(String::from_utf8_lossy(&bytes).contains("started-bg"));
    let notice = events.iter().find_map(|e| match e {
        Event::Notice { text, .. } if text.contains("finished") => Some(text.clone()),
        _ => None,
    });
    let notice = notice.expect("notice");
    assert!(notice.contains("exit 3"), "{notice}");
    assert!(notice.contains(&format!("expand {archive}")), "{notice}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn ctrl_b_detaches_running_call() {
    let (session, store, mut rx, dir) = open(
        r#"
[[turn]]
text = "running"
tool_calls = [{ name = "bash", input = { command = "echo begin; sleep 2; echo woke" } }]

[[turn]]
text = "moved on"
"#,
    );
    let turn = user_turn(&session);
    let mut events = Vec::new();
    // The first streamed chunk proves the call is running (and detachable).
    until(
        &mut rx,
        &mut events,
        |e| matches!(e, Event::ToolCallOutput { delta, .. } if delta.contains("begin")),
    )
    .await;
    let call_id = events
        .iter()
        .find_map(|e| match e {
            Event::ToolCallRequested { call } => Some(call.id),
            _ => None,
        })
        .expect("requested");
    session
        .submit(Submission::Background { call_id })
        .await
        .expect("background");
    until(&mut rx, &mut events, |e| {
        matches!(e, Event::TurnDone { .. })
    })
    .await;
    // The turn finished while `sleep` still ran: no completion yet.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::TaskCompleted { .. })),
        "turn waited for the detached call"
    );
    let pointer = pointer_result(&events);
    assert!(pointer.contains("background task"), "{pointer}");
    until(&mut rx, &mut events, |e| {
        matches!(e, Event::TaskCompleted { .. })
    })
    .await;
    turn.await.expect("join");

    let (exit_code, archive) = completion(&events);
    assert_eq!(exit_code, Some(0));
    let text = String::from_utf8_lossy(&store.get(&archive).await.expect("row")).into_owned();
    assert!(text.contains("begin") && text.contains("woke"), "{text}");
    // The card stopped streaming once detached; the archive has it all.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ToolCallOutput { delta, .. } if delta.contains("woke"))),
        "detached output still streamed to the card"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Whether a process whose command line contains `pattern` is running.
fn running(pattern: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Polls `running(pattern)` until it equals `want` or `within` elapses.
async fn settle(pattern: &str, want: bool, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if running(pattern) == want {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// T38.2: cancellation is turn-scoped, so after turn 2 the shell detached
/// in turn 1 holds a token `interrupt()` no longer reaches. `end()` still
/// reaches it: the shell's process group is killed within the grace period.
#[tokio::test]
async fn ending_the_session_kills_a_shell_detached_in_an_older_turn() {
    // The same fixed margin the surfaces give `wait_tasks_cleared`.
    const GRACE: Duration = Duration::from_secs(5);
    // Unique per test process, so a parallel run or a leftover never matches.
    let sleeper = format!("sleep 4011.{}", std::process::id());
    let (session, _store, mut rx, dir) = open(&format!(
        r#"
[[turn]]
text = "starting"
tool_calls = [{{ name = "bash", input = {{ command = "{sleeper}", background = true }} }}]

[[turn]]
text = "detached"

[[turn]]
text = "second turn"
"#
    ));
    let mut events = Vec::new();
    let turn = user_turn(&session);
    until(&mut rx, &mut events, |e| {
        matches!(e, Event::TurnDone { .. })
    })
    .await;
    turn.await.expect("turn 1");
    assert!(
        settle(&sleeper, true, GRACE).await,
        "the shell never started"
    );
    let turn = user_turn(&session);
    until(&mut rx, &mut events, |e| {
        matches!(e, Event::TurnDone { .. })
    })
    .await;
    turn.await.expect("turn 2");

    session.end();
    session.wait_tasks_cleared(GRACE).await;
    let gone = settle(&sleeper, false, GRACE).await;
    if !gone {
        let _ = std::process::Command::new("pkill")
            .args(["-KILL", "-f", &sleeper])
            .status();
    }
    let _ = std::fs::remove_dir_all(dir);
    assert!(gone, "`{sleeper}` outlived the session");
}
