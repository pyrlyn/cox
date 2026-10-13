// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Relay::approve_writes` through a real turn (T59.7): a call that learns
//! its files only while it runs (`rename_symbol`'s server edit) gets every
//! file judged by the session's engine as an edit of that path, one deny
//! refuses them all, and any ask is one approval listing every file. The
//! `Renamer` stub stands in for `rename_symbol`, whose own tests prove it
//! hands its files over before writing; here the gate, the engine and the
//! approval round trip are the real ones.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::{spawn_turn, tool_results};
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{
    Concurrency, Decision, Event, PermissionMode, Risk, Submission, ToolOutput, ToolSpec,
};
use cox_protocol::{Config, ToolCall};
use cox_provider::scripted::Scripted;
use serde_json::Value;

const SCENARIO: &str = r#"
[[turn]]
text = "renaming"
tool_calls = [{ name = "rename_symbol", input = { path = "a.txt" } }]

[[turn]]
text = "done"
"#;

/// Writes `renamed` into `a.txt` and `b.txt`, after handing both to the
/// session the way `rename_symbol` does once its server has answered.
struct Renamer;

#[async_trait]
impl Tool for Renamer {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rename_symbol".into(),
            description: "rename stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input["path"].as_str().unwrap_or_default().into()
    }
    fn defers_write_approval(&self) -> bool {
        true
    }
    async fn call(&self, _input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        // Canonical, as `confine` hands them to the real tool.
        let paths: Vec<PathBuf> = ["a.txt", "b.txt"]
            .iter()
            .map(|f| std::fs::canonicalize(cx.cwd.join(f)).unwrap())
            .collect();
        let relay = cx.relay.as_ref().expect("the session binds a relay");
        relay
            .approve_writes(cx.call, "rename_symbol", &paths)
            .await?;
        for p in &paths {
            std::fs::write(p, "renamed\n").unwrap();
        }
        Ok(ToolOutput {
            text: "renamed".into(),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// A scratch workspace holding `a.txt` and `b.txt`, removed on drop. Not
/// canonicalized: on macOS the temp dir sits behind `/var`, the symlink
/// the engine's subjects must see through.
struct Work(PathBuf);

impl Work {
    fn new(name: &str) -> Work {
        let dir =
            std::env::temp_dir().join(format!("cox-approve-writes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a.txt", "b.txt"] {
            std::fs::write(dir.join(f), "old\n").unwrap();
        }
        Work(dir)
    }
    fn read(&self, f: &str) -> String {
        std::fs::read_to_string(self.0.join(f)).unwrap()
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the scenario's one rename under `config`, answering every approval
/// with `answer`. Returns each approval's call and the tool's `(ok, text)`.
async fn rename(
    work: &Path,
    mut config: Config,
    answer: Decision,
) -> (Vec<ToolCall>, Vec<(bool, String)>) {
    config.core.workspace_roots = vec![work.to_path_buf()];
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        Arc::new(Scripted::from_toml(SCENARIO, "").expect("scenario")),
        vec![Arc::new(Renamer) as Arc<dyn Tool>],
        store.clone(),
        store,
        work.to_path_buf(),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "go");
    let (mut asked, mut events) = (Vec::new(), Vec::new());
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        if let Event::ApprovalRequired { call, .. } = &ev {
            asked.push(call.clone());
            let sub = Submission::Approve {
                call_id: call.id,
                decision: answer.clone(),
            };
            session.submit(sub).await.expect("approve");
        }
        let done = matches!(ev, Event::TurnDone { .. });
        events.push(ev);
        if done {
            break;
        }
    }
    running.await.expect("join").expect("turn");
    (asked, tool_results(&events))
}

fn rules(mode: PermissionMode, allow: &[&str], deny: &[&str]) -> Config {
    let mut config = Config::default();
    config.permissions.mode = mode;
    config.permissions.allow = allow.iter().map(|r| (*r).to_string()).collect();
    config.permissions.deny = deny.iter().map(|r| (*r).to_string()).collect();
    config
}

#[tokio::test]
async fn a_deny_rule_on_the_second_file_refuses_the_rename_and_writes_neither() {
    let work = Work::new("deny");
    // `auto` would write without asking: only the rule can stop `b.txt`.
    let config = rules(PermissionMode::Auto, &[], &["Edit(b.txt)"]);
    let (asked, results) = rename(&work.0, config, Decision::Allow).await;

    assert!(asked.is_empty(), "a deny asks nothing: {asked:?}");
    let (ok, text) = &results[0];
    assert!(!ok, "{text}");
    assert!(text.contains("b.txt: denied by rule Edit(b.txt)"), "{text}");
    assert_eq!(work.read("a.txt"), "old\n");
    assert_eq!(work.read("b.txt"), "old\n");
}

#[tokio::test]
async fn one_approval_lists_every_file_and_a_no_writes_nothing() {
    let work = Work::new("no");
    let config = rules(PermissionMode::Default, &[], &[]);
    let no = Decision::Deny {
        reason: "not now".into(),
    };
    let (asked, results) = rename(&work.0, config, no).await;

    assert_eq!(asked.len(), 1, "one approval for the whole set: {asked:?}");
    assert_eq!(asked[0].name, "rename_symbol");
    assert_eq!(asked[0].subject, "a.txt, b.txt");
    assert_eq!(
        asked[0].input["files"],
        serde_json::json!(["a.txt", "b.txt"])
    );
    assert!(!results[0].0, "{}", results[0].1);
    assert_eq!(work.read("a.txt"), "old\n");
    assert_eq!(work.read("b.txt"), "old\n");
}

#[tokio::test]
async fn a_yes_to_that_one_approval_writes_every_file() {
    let work = Work::new("yes");
    let config = rules(PermissionMode::Default, &[], &[]);
    let (asked, results) = rename(&work.0, config, Decision::Allow).await;

    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(results[0].0, "{}", results[0].1);
    assert_eq!(work.read("a.txt"), "renamed\n");
    assert_eq!(work.read("b.txt"), "renamed\n");
}

#[tokio::test]
async fn edit_allow_rules_on_every_file_let_it_through_without_asking() {
    let work = Work::new("allow");
    let config = rules(
        PermissionMode::Default,
        &["Edit(a.txt)", "Edit(b.txt)"],
        &[],
    );
    let (asked, results) = rename(&work.0, config, Decision::Allow).await;

    assert!(asked.is_empty(), "{asked:?}");
    assert!(results[0].0, "{}", results[0].1);
    assert_eq!(work.read("b.txt"), "renamed\n");
}

#[tokio::test]
async fn plan_mode_still_refuses_before_the_call_runs() {
    let work = Work::new("plan");
    let config = rules(PermissionMode::Plan, &[], &[]);
    let (asked, results) = rename(&work.0, config, Decision::Allow).await;

    assert!(asked.is_empty(), "{asked:?}");
    assert!(results[0].1.contains("plan mode"), "{}", results[0].1);
    assert_eq!(work.read("a.txt"), "old\n");
}
