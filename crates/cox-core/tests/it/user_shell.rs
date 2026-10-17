// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Composer `!` lines (T25.3) through the loop with the real `BashTool`:
//! the call takes the model's path (engine, sandbox, archive) and only
//! `!!` changes what the next request carries.

use std::sync::Arc;
use std::time::Duration;

use cox_core::{MemoryStore, Session};
use cox_protocol::ids::CallId;
use cox_protocol::types::{Event, Submission, ToolResult};
use cox_provider::scripted::Scripted;
use cox_tools::bash::BashTool;
use tokio::sync::mpsc::Receiver;

/// A session whose only tool is the real `bash`: allowed by rule, `rm`
/// denied by rule, in a fresh directory holding one file.
fn open() -> (Session, Receiver<Event>) {
    let dir = std::env::temp_dir().join(format!("cox-t253-{}", CallId::new()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("marker.txt"), "x").expect("write");
    let dir = dir.canonicalize().expect("canonical");
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![dir.clone()];
    config.permissions.allow = vec!["Bash".into()];
    config.permissions.deny = vec!["Bash(rm:*)".into()];
    let provider = Arc::new(Scripted::from_toml("", "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        vec![Arc::new(BashTool)],
        store.clone(),
        store,
        dir,
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, rx)
}

/// Submits a `!` line and returns the result of its one `bash` call.
async fn shell(
    session: &Session,
    rx: &mut Receiver<Event>,
    command: &str,
    share: bool,
) -> ToolResult {
    let sub = session.clone();
    let command = command.to_string();
    let run = tokio::spawn(async move {
        sub.submit(Submission::UserShell { command, share })
            .await
            .expect("shell");
    });
    let result = loop {
        let ev = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        if let Event::ToolCallDone { result, .. } = ev {
            break result;
        }
    };
    run.await.expect("join");
    result
}

async fn history_json(session: &Session) -> String {
    serde_json::to_string(&session.history().await).expect("serialize")
}

#[tokio::test]
async fn bang_line_runs_sandboxed_and_stays_out_of_history() {
    let (session, mut rx) = open();
    let before = history_json(&session).await;
    let listed = shell(&session, &mut rx, "ls", false).await;
    assert!(listed.ok, "{}", listed.visible);
    assert!(listed.visible.contains("marker.txt"), "{}", listed.visible);
    let refused = shell(&session, &mut rx, "rm marker.txt", false).await;
    assert!(!refused.ok);
    assert!(
        refused.visible.starts_with("permission denied"),
        "{}",
        refused.visible
    );
    assert_eq!(history_json(&session).await, before);
}

#[tokio::test]
async fn bang_bang_line_enters_history() {
    let (session, mut rx) = open();
    let before = session.history().await.len();
    shell(&session, &mut rx, "ls", true).await;
    let history = session.history().await;
    assert_eq!(history.len(), before + 1);
    let text = serde_json::to_string(&history[before]).expect("serialize");
    assert!(
        text.contains("$ ls") && text.contains("marker.txt"),
        "{text}"
    );
}
