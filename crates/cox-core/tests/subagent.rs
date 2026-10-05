// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Subagents through the loop (T3.9): the explore preset runs on the cheap
//! tier with read-only tools, its cost lands in the ledger under its own
//! session, and an answer over the cap comes back summarised.

mod common;

use std::time::Duration;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{drain, open, run_with, scenario, spawn_turn, tool_results};
use cox_protocol::agent::AgentDef;
use cox_protocol::errors::WorktreeError;
use cox_protocol::traits::{Worktree, Worktrees};
use cox_protocol::types::{Content, Decision, Event, Job, Source, Submission, Tier};

/// A discovered `.cox/agents/reviewer.md`-shaped definition (T34.1): its
/// `tools` narrows the child to `echo`, and its `model` picks the tier the
/// same way a real file's `model: haiku` would (`cox_protocol::agent::tier_for`).
fn reviewer_def(model: Option<&str>) -> AgentDef {
    AgentDef {
        name: "reviewer".into(),
        description: "reviews a diff".into(),
        tools: vec!["echo".into()],
        model: model.map(str::to_string),
        path: PathBuf::from("<test>/.cox/agents/reviewer.md"),
        body: "You review changes for correctness.".into(),
        disabled: false,
        permission_mode: None,
    }
}

/// A `Worktrees` that records what the loop asked for and answers with a
/// fixed path, so no git runs in this test.
struct Fake(Mutex<Vec<(PathBuf, String, String)>>);

#[async_trait]
impl Worktrees for Fake {
    async fn add(&self, from: &Path, name: &str, owner: &str) -> Result<Worktree, WorktreeError> {
        self.0.lock().expect("lock").push((
            from.to_path_buf(),
            name.to_string(),
            owner.to_string(),
        ));
        Ok(Worktree {
            path: PathBuf::from(format!("/tmp/_worktrees/cox-turn-{name}")),
            branch: name.to_string(),
            main: from.to_path_buf(),
        })
    }
}

/// T27.3: `isolation: "worktree"` asks the provider for a worktree named
/// after the task id and owned by the parent session, and the answer ends
/// with its path and branch. Without a provider the call is refused.
/// Worktree isolation is `Destructive` (T44.1), so an allow rule lets it run.
#[tokio::test]
async fn subagent_worktree_isolation_runs_child_in_its_worktree() {
    let mut config = cox_protocol::Config::default();
    config.permissions.allow = vec!["agent(shell)".into()];
    let (session, _store, mut rx) = open(&scenario("subagent_worktree"), config.clone());
    let fake = Arc::new(Fake(Mutex::new(Vec::new())));
    session.set_worktrees(fake.clone());
    let running = spawn_turn(&session, "subagent_worktree");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");

    let asked = fake.0.lock().expect("lock").clone();
    assert_eq!(asked.len(), 1);
    let (from, name, owner) = &asked[0];
    assert_eq!(from, &PathBuf::from("/tmp/cox-turn"));
    assert_eq!(owner, &format!("cox / {}", session.id()));
    let task = events
        .iter()
        .find_map(|e| match e {
            Event::TaskCreated { task, .. } => Some(task.to_string()),
            _ => None,
        })
        .expect("task created");
    assert_eq!(name, &task, "the worktree is named after the task id");
    assert_eq!(
        tool_results(&events),
        [(
            true,
            format!("edited\n[worktree /tmp/_worktrees/cox-turn-{task}, branch {task}]")
        )]
    );

    let (session, _store, mut rx) = open(&scenario("subagent_worktree"), config);
    let running = spawn_turn(&session, "no provider");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let results = tool_results(&events);
    assert_eq!(results.len(), 1);
    assert!(!results[0].0);
    assert!(
        results[0].1.contains("worktree isolation is not available"),
        "{}",
        results[0].1
    );
}

#[tokio::test]
async fn subagent_explore_uses_cheap_tier_and_read_only_tools() {
    let (events, store, _) = run_with("subagent_explore", cox_protocol::Config::default()).await;
    let created = events.iter().find_map(|e| match e {
        Event::TaskCreated { tier, label, .. } => Some((*tier, label.clone())),
        _ => None,
    });
    assert_eq!(created, Some((Tier::Cheap, "explore: find x".to_string())));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TaskCompleted { .. }))
    );
    assert_eq!(tool_results(&events), [(true, "result: x".to_string())]);

    let rows = store.usage_rows();
    let main: Vec<_> = rows.iter().filter(|r| r.job == Job::Main).collect();
    let explore: Vec<_> = rows.iter().filter(|r| r.job == Job::Explore).collect();
    assert_eq!(main.len(), 2, "parent: the delegating turn and `done`");
    assert_eq!(
        explore.len(),
        3,
        "child: the refused write, the echo, the answer"
    );
    assert!(explore.iter().all(|r| r.tier == Tier::Cheap));
    let child_id = explore[0].session_id;
    assert!(explore.iter().all(|r| r.session_id == child_id));
    assert!(main.iter().all(|r| r.session_id != child_id));
}

#[tokio::test]
async fn subagent_result_over_cap_is_summarised_on_the_summarize_job() {
    let (events, store, _) = run_with("subagent_summary", cox_protocol::Config::default()).await;
    assert_eq!(tool_results(&events), [(true, "short summary".to_string())]);
    let summary: Vec<_> = store
        .usage_rows()
        .into_iter()
        .filter(|r| r.job == Job::Summarize)
        .collect();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].tier, Tier::Cheap);
}

/// Collects turn events plus late background completions: `TaskCompleted`
/// may arrive after `TurnDone`, so keep receiving until `completed` pairs
/// and their `finished` notices are all in (or time out).
async fn drain_with_background(
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    completed: usize,
) -> Vec<Event> {
    let mut events = drain(rx).await;
    while completed_count(&events) < completed || !finished_notice(&events) {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("background completion timeout")
            .expect("event stream closed");
        events.push(ev);
    }
    events
}

fn completed_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::TaskCompleted { .. }))
        .count()
}

fn finished_notice(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            Event::Notice { text, .. } if text.contains("finished")
        )
    })
}

#[tokio::test]
async fn tasks_background_agent_reports_pointer_then_notice() {
    let (session, store, mut rx) = open(
        &scenario("subagent_background"),
        cox_protocol::Config::default(),
    );
    let running = spawn_turn(&session, "go");
    running.await.expect("join").expect("turn");
    let events = drain_with_background(&mut rx, 1).await;

    let created = events
        .iter()
        .position(|e| matches!(e, Event::TaskCreated { .. }));
    let completed = events
        .iter()
        .position(|e| matches!(e, Event::TaskCompleted { .. }));
    assert!(created < completed, "Created before Completed");
    // The tool result is a short pointer, never the kilobyte answer.
    let results = tool_results(&events);
    assert_eq!(results.len(), 1);
    assert!(results[0].0);
    assert!(results[0].1.contains("background task"), "{}", results[0].1);
    assert!(!results[0].1.contains("CHILD-ANSWER"), "no full result");
    assert!(finished_notice(&events));
    // History holds the pointer line, and no long tool result at all.
    let history = session.history().await;
    assert!(
        history.iter().flat_map(|m| &m.content).all(|c| match c {
            Content::ToolResult { content, .. } => content.len() < 500,
            _ => true,
        }),
        "no full result in context"
    );
    assert!(
        history
            .iter()
            .flat_map(|m| &m.content)
            .any(|c| matches!(c, Content::Text { text } if text.contains("finished"))),
        "completion pointer in history"
    );
    assert!(store.usage_rows().iter().any(|r| r.job == Job::Explore));
}

#[tokio::test]
async fn tasks_two_background_agents_run_concurrently() {
    let (session, _, mut rx) = open(
        &scenario("subagent_background_two"),
        cox_protocol::Config::default(),
    );
    let running = spawn_turn(&session, "go");
    running.await.expect("join").expect("turn");
    let events = drain_with_background(&mut rx, 2).await;

    let labels: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::TaskCreated { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels.len(), 2);
    assert!(labels.iter().any(|l| l.contains("first job")), "{labels:?}");
    assert!(
        labels.iter().any(|l| l.contains("second job")),
        "{labels:?}"
    );
    assert_eq!(completed_count(&events), 2);
    let results = tool_results(&events);
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|(ok, text)| *ok && text.contains("background task")),
        "{results:?}"
    );
}

/// T27.2: a child's escalated call reaches the parent's stream labelled
/// with the agent (not the parent's own session), the parent's `Approve`
/// unblocks the child, and the decision is relayed back so the prompt
/// closes.
#[tokio::test]
async fn subagent_approval_carries_source() {
    let (session, _store, mut rx) = open(
        &scenario("subagent_approval"),
        cox_protocol::Config::default(),
    );
    let running = spawn_turn(&session, "run the tests");
    let (call_id, source) = loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        let Event::ApprovalRequired { call, source, .. } = ev else {
            continue;
        };
        let source = source.expect("source");
        if source.agent.is_some() {
            break (call.id, source);
        }
        // The parent's own `agent` call asks first, unlabelled.
        assert_eq!(source.session, session.id());
        session
            .submit(Submission::Approve {
                call_id: call.id,
                decision: Decision::Allow,
            })
            .await
            .expect("approve agent");
    };
    let Source {
        session: asking,
        agent,
        preset,
    } = source;
    assert_ne!(asking, session.id());
    assert_eq!(agent.as_deref(), Some("shell-1"));
    assert_eq!(preset.as_deref(), Some("shell"));
    session
        .submit(Submission::Approve {
            call_id,
            decision: Decision::Allow,
        })
        .await
        .expect("approve");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ApprovalDecided { call_id: id, .. } if *id == call_id))
    );
    assert_eq!(tool_results(&events), [(true, "tests ran".to_string())]);
}

/// T34.1: a name that is not `explore`/`shell` still dispatches when a
/// `.cox/agents`/`.claude/agents` definition discovered it — the child
/// gets exactly the def's own tool allowlist and tier.
#[tokio::test]
async fn agent_dispatches_a_discovered_custom_preset_by_name() {
    let toml = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "look at diff", preset = "reviewer" } }]

# child: the def's own tool
[[turn]]
text = "checking"
tool_calls = [{ name = "echo", input = { text = "diff" } }]

# child: answer
[[turn]]
text = "looks good"

# parent
[[turn]]
text = "done"
"#;
    let (session, store, mut rx) = open(toml, cox_protocol::Config::default());
    session.set_agent_defs(vec![reviewer_def(Some("haiku"))]);
    let running = spawn_turn(&session, "go");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");

    let created = events.iter().find_map(|e| match e {
        Event::TaskCreated { tier, label, .. } => Some((*tier, label.clone())),
        _ => None,
    });
    assert_eq!(
        created,
        Some((Tier::Cheap, "reviewer: look at diff".to_string())),
        "the def's model: haiku picks the cheap tier"
    );
    assert_eq!(tool_results(&events), [(true, "looks good".to_string())]);
    // T34.1: a custom preset's ledger rows are tagged `Job::Agent`; its
    // own `tier`/`model` decides the tier, not this job (D5).
    assert!(store.usage_rows().iter().any(|r| r.job == Job::Agent));
}

// T34.1: `agent_unknown_preset_lists_builtin_and_discovered_names_in_error`
// lives in `crates/cox-core/src/subagent.rs`'s own unit tests instead of
// here. A failed `resolve` makes `risk()` fall back to `Exec` (pre-existing
// behaviour, unchanged by this task: an unresolvable call could be
// anything), which needs an approval answer before a full turn ever reaches
// `call()`'s own error text — the same claim is exact and deterministic one
// level down, over `resolve()` directly, like `subagent_presets_are_explore_and_shell`.

/// T34.1: `tier` may only lower a dispatch's tier, never raise it (D5
/// "never up"). The def's `model` is absent (`inherit`), so its natural
/// tier is the parent's own — `Code` for the top-level session.
#[tokio::test]
async fn agent_tier_override_is_honored_and_clamped() {
    let lower = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "a", preset = "reviewer", tier = "cheap" } }]

[[turn]]
text = "answer a"

[[turn]]
text = "done"
"#;
    let (session, _store, mut rx) = open(lower, cox_protocol::Config::default());
    session.set_agent_defs(vec![reviewer_def(None)]);
    let running = spawn_turn(&session, "go");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let tier = events.iter().find_map(|e| match e {
        Event::TaskCreated { tier, .. } => Some(*tier),
        _ => None,
    });
    assert_eq!(tier, Some(Tier::Cheap), "a lower request is honoured");

    let higher = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "b", preset = "reviewer", tier = "think" } }]

[[turn]]
text = "answer b"

[[turn]]
text = "done"
"#;
    let (session, _store, mut rx) = open(higher, cox_protocol::Config::default());
    session.set_agent_defs(vec![reviewer_def(None)]);
    let running = spawn_turn(&session, "go");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let tier = events.iter().find_map(|e| match e {
        Event::TaskCreated { tier, .. } => Some(*tier),
        _ => None,
    });
    assert_eq!(
        tier,
        Some(Tier::Code),
        "a higher request is clamped to the parent's own tier (D5: never up)"
    );
}

/// T45.1: an `auto`-configured parent switched to `Default` spawns a child
/// that asks before a `Risk::Write` call, as the parent would; a child built
/// from the configured mode ran it unasked.
#[tokio::test]
async fn child_of_default_parent_does_not_run_auto() {
    let toml = r#"
[[turn]]
tool_calls = [{ name = "agent", input = { task = "touch it", preset = "shell", tools = ["touch"] } }]
[[turn]]
tool_calls = [{ name = "touch", input = { path = "/tmp/cox-turn/t45-1" } }]
[[turn]]
text = "touched"
[[turn]]
text = "done"
"#;
    let mut config = cox_protocol::Config::default();
    config.permissions.mode = cox_protocol::types::PermissionMode::Auto;
    let (session, _store, mut rx) = open(toml, config);
    session
        .submit(Submission::SetPermissionMode {
            mode: cox_protocol::types::PermissionMode::Default,
        })
        .await
        .expect("set mode");
    let running = spawn_turn(&session, "touch it");
    let mut child_asked = false;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        match ev {
            Event::ApprovalRequired { call, source, .. } => {
                child_asked |= source.is_some_and(|s| s.agent.is_some());
                session
                    .submit(Submission::Approve {
                        call_id: call.id,
                        decision: Decision::Allow,
                    })
                    .await
                    .expect("approve");
            }
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");
    assert!(child_asked, "the child's write ran without asking");
}

/// T50.2: a finished child woken by `TaskMessage` is never wider than its
/// parent's live mode. The child ran under `Default`; the parent then
/// switched to Plan, so the woken child's write is denied without asking.
/// Before the fix the child was rebuilt from its rollout in `Default` and
/// asked instead.
#[tokio::test]
async fn woken_child_keeps_parent_plan_mode() {
    let toml = r#"
[[turn]]
tool_calls = [{ name = "agent", input = { task = "wait", preset = "shell", tools = ["touch"] } }]
[[turn]]
text = "ready"
[[turn]]
text = "done"
[[turn]]
tool_calls = [{ name = "touch", input = { path = "/tmp/cox-turn/t50-2" } }]
[[turn]]
text = "blocked"
"#;
    let (session, _store, mut rx) = open(toml, cox_protocol::Config::default());
    let running = spawn_turn(&session, "wait");
    let mut task = None;
    loop {
        match next(&mut rx).await {
            Event::TaskCreated { task: t, .. } => task = Some(t),
            Event::ApprovalRequired { call, .. } => {
                let decision = Decision::Allow;
                let sub = Submission::Approve {
                    call_id: call.id,
                    decision,
                };
                session.submit(sub).await.expect("approve");
            }
            Event::TurnDone { .. } => break,
            _ => {}
        }
    }
    running.await.expect("join").expect("turn");
    let task = task.expect("the child was created");
    let plan = cox_protocol::types::PermissionMode::Plan;
    let sub = Submission::SetPermissionMode { mode: plan };
    session.submit(sub).await.expect("set mode");
    let text = "write it now".to_string();
    let (from, hop) = (None, 0);
    let sub = Submission::TaskMessage {
        task,
        from,
        hop,
        text,
    };
    session.submit(sub).await.expect("deliver");
    let mut child_asked = false;
    loop {
        match next(&mut rx).await {
            Event::ApprovalRequired { call, source, .. } => {
                child_asked |= source.is_some_and(|s| s.agent.is_some());
                let reason = "test".to_string();
                let decision = Decision::Deny { reason };
                let sub = Submission::Approve {
                    call_id: call.id,
                    decision,
                };
                session.submit(sub).await.expect("deny");
            }
            Event::TaskCompleted { task: t, .. } if t == task => break,
            _ => {}
        }
    }
    assert!(
        !child_asked,
        "the woken child asked as in Default, not Plan"
    );
}

async fn next(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("event timeout")
        .expect("event stream closed")
}
