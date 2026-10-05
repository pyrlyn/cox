// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Every intent (T37.10, DT§4.3) against a scratch `COX_HOME`: a real
//! cox-core session over the Scripted provider writes to a real `cox.db`
//! in a tempdir, the intents drive it through `dispatch`, and the
//! workspace reads the result back. Never the real `~/.cox`, never a
//! keychain.

#[path = "../../cox-core/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_app::{Dispatch, Intent, IntentError, Workspace, dispatch};
use cox_core::Session;
use cox_protocol::errors::WorktreeError;
use cox_protocol::traits::{Store as _, Worktree, WorktreeInfo, Worktrees};
use cox_protocol::types::{
    Attachment, Decision, Effort, Event, ItemKind, PermissionMode, SlashCommand, StopReason,
    Submission, Tier,
};
use cox_protocol::{CallId, Config};
use cox_provider::scripted::Scripted;
use cox_store::Store;
use tokio::sync::mpsc::Receiver;

/// The git side, faked: the workspace only forwards to it.
struct Fake;

#[async_trait]
impl Worktrees for Fake {
    async fn add(&self, from: &Path, _: &str, _: &str) -> Result<Worktree, WorktreeError> {
        Err(WorktreeError::NotARepository {
            dir: from.to_path_buf(),
        })
    }
    async fn list(&self, from: &Path) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        Ok(vec![WorktreeInfo {
            path: from.to_path_buf(),
            branch: Some("main".into()),
            main: true,
            locked: None,
            stale: false,
            merged: false,
            bytes: 42,
        }])
    }
}

const SCRIPT: &str = r#"
[[turn]]
text = "hello from scripted"

[[turn]]
text = "writing"
tool_calls = [{ name = "touch", input = { path = "a" } }]

[[turn]]
text = "done"
"#;

struct Scratch {
    _dir: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    session: Session,
    store: Arc<Store>,
    rx: Receiver<Event>,
}

fn scratch() -> Scratch {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("cox-home");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let mut config = Config::default();
    config.core.workspace_roots = vec![cwd.clone()];
    let store = Arc::new(Store::open(&home).expect("store"));
    let provider = Arc::new(Scripted::from_toml(SCRIPT, "").expect("script"));
    let session = Session::new(
        config,
        provider,
        common::tools(),
        store.clone(),
        store.clone(),
        cwd.clone(),
    )
    .expect("session");
    let rx = session.events().expect("events");
    Scratch {
        _dir: dir,
        home,
        cwd,
        session,
        store,
        rx,
    }
}

/// Runs `intent` the way the controller will: a turn is spawned, anything
/// else awaited.
async fn send(session: &Session, intent: Intent) {
    match dispatch(intent).expect("dispatch") {
        Dispatch::Submit {
            submission,
            spawn: true,
        } => {
            let session = session.clone();
            tokio::spawn(async move { session.submit(submission).await });
        }
        Dispatch::Submit { submission, .. } => session.submit(submission).await.expect("submit"),
        other => panic!("not a submission: {other:?}"),
    }
}

async fn next(rx: &mut Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("event timeout")
        .expect("stream open")
}

/// Events up to the first one `stop` accepts, approving any approval.
async fn until(s: &mut Scratch, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = next(&mut s.rx).await;
        if let Event::ApprovalRequired { call, .. } = &event {
            let approve = Intent::Approve {
                call: call.id,
                decision: Decision::Allow,
            };
            send(&s.session, approve).await;
        }
        let done = stop(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn turn_done(e: &Event) -> bool {
    matches!(e, Event::TurnDone { .. })
}

#[tokio::test]
async fn send_runs_a_turn_recorded_in_the_scratch_home() {
    let mut s = scratch();
    let text = "hello there".to_string();
    send(
        &s.session,
        Intent::Send {
            text,
            attachments: vec![],
            confirm_think: false,
        },
    )
    .await;
    let events = until(&mut s, turn_done).await;
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    let rollout = s.store.rollout_read(&s.session.id()).expect("rollout");
    assert!(rollout.iter().any(|e| matches!(
        e,
        Event::ItemStarted { kind: ItemKind::UserMessage { text, .. }, .. } if text == "hello there"
    )));
}

#[tokio::test]
async fn approve_lets_the_waiting_call_run() {
    let mut s = scratch();
    send(
        &s.session,
        Intent::Send {
            text: "hi".into(),
            attachments: vec![],
            confirm_think: false,
        },
    )
    .await;
    until(&mut s, turn_done).await;
    send(
        &s.session,
        Intent::Send {
            text: "touch a".into(),
            attachments: vec![],
            confirm_think: false,
        },
    )
    .await;
    let events = until(&mut s, turn_done).await;
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ApprovalDecided {
            decision: Decision::Allow,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ToolCallDone { result, .. } if result.ok && result.visible == "touched"
    )));
}

#[tokio::test]
async fn set_mode_and_set_effort_change_the_session_state() {
    let mut s = scratch();
    send(
        &s.session,
        Intent::SetMode {
            mode: PermissionMode::Plan,
        },
    )
    .await;
    let events = until(&mut s, |e| matches!(e, Event::StateChanged { .. })).await;
    assert!(matches!(
        events.last(),
        Some(Event::StateChanged {
            mode: PermissionMode::Plan,
            ..
        })
    ));
    send(
        &s.session,
        Intent::SetEffort {
            effort: Some(Effort::High),
        },
    )
    .await;
    let events = until(&mut s, |e| matches!(e, Event::StateChanged { .. })).await;
    assert!(matches!(
        events.last(),
        Some(Event::StateChanged {
            effort: Some(Effort::High),
            ..
        })
    ));
}

#[tokio::test]
async fn interrupt_redo_and_a_file_command_are_harmless_when_idle() {
    let s = scratch();
    for intent in [
        Intent::Interrupt,
        Intent::Redo,
        Intent::Command {
            line: "/no-such-command".into(),
        },
    ] {
        match dispatch(intent).expect("dispatch") {
            Dispatch::Submit { submission, .. } => {
                s.session.submit(submission).await.expect("submit");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn rewind_drops_the_conversation_from_a_turn() {
    let mut s = scratch();
    send(
        &s.session,
        Intent::Send {
            text: "one".into(),
            attachments: vec![],
            confirm_think: false,
        },
    )
    .await;
    until(&mut s, turn_done).await;
    let rewind = Intent::Rewind {
        to_turn: 1,
        code: false,
        conversation: true,
    };
    send(&s.session, rewind).await;
    let events = until(&mut s, |e| matches!(e, Event::Rewound { .. })).await;
    assert!(matches!(
        events.last(),
        Some(Event::Rewound {
            to_turn: 1,
            conversation: true,
            ..
        })
    ));
}

#[tokio::test]
async fn fork_and_handoff_seed_children_the_workspace_lists() {
    let mut s = scratch();
    send(
        &s.session,
        Intent::Send {
            text: "hello fork".into(),
            attachments: vec![],
            confirm_think: false,
        },
    )
    .await;
    until(&mut s, turn_done).await;
    let parent = s.session.id();

    let Dispatch::Fork { turn } = dispatch(Intent::Fork { turn: Some(1) }).expect("fork") else {
        panic!("fork is a lineage call");
    };
    let (child, history) = cox_session::fork(&s.home, &s.cwd, parent, turn).expect("fork");
    assert_eq!(history.turns, 1, "the child starts from the parent's turn");
    let handoff = dispatch(Intent::Handoff {
        objective: "ship it".into(),
    })
    .expect("handoff");
    let Dispatch::Handoff { objective } = handoff else {
        panic!("handoff is a lineage call");
    };
    let (heir, _) =
        cox_session::handoff(&s.home, &s.cwd, parent, &objective, Some("sum")).expect("handoff");

    let ws = Workspace::open(&s.home, Arc::new(Fake)).expect("workspace");
    let projects = ws.projects(50).expect("projects");
    assert_eq!(projects.len(), 1, "{projects:?}");
    assert_eq!(projects[0].root, s.cwd);
    assert_eq!(projects[0].sessions, 3);
    let ids: Vec<String> = ws
        .sessions(&s.cwd, 50)
        .expect("sessions")
        .into_iter()
        .inspect(|row| assert_eq!(row.held_by, None, "this process drives it"))
        .map(|row| row.info.id)
        .collect();
    for id in [parent, child, heir] {
        assert!(ids.contains(&id.to_string()), "{id} in {ids:?}");
    }
    let hits = ws.search("hello fork", 10).expect("search");
    assert!(
        hits.iter().any(|h| h.session.id == parent.to_string()),
        "{hits:?}"
    );
    let trees = ws.worktrees(&s.cwd).await.expect("worktrees");
    assert_eq!(trees[0].bytes, 42);
}

#[test]
fn every_intent_maps_to_its_submission() {
    let call = CallId::new();
    let turn = |text: &str| Submission::UserTurn {
        text: text.into(),
        attachments: vec![],
        confirm_think: false,
    };
    let now = |submission| {
        Ok(Dispatch::Submit {
            submission,
            spawn: false,
        })
    };
    let spawned = |submission| {
        Ok(Dispatch::Submit {
            submission,
            spawn: true,
        })
    };
    let shot = Attachment {
        name: "shot.png".into(),
        media_type: "image/png".into(),
        data_b64: "iVBORw0KGgo=".into(),
    };
    let cases: Vec<(Intent, Result<Dispatch, IntentError>)> = vec![
        (
            Intent::Send {
                text: "hi".into(),
                attachments: vec![],
                confirm_think: false,
            },
            spawned(turn("hi")),
        ),
        (
            Intent::Send {
                text: "plan it".into(),
                attachments: vec![],
                confirm_think: true,
            },
            spawned(Submission::UserTurn {
                text: "plan it".into(),
                attachments: vec![],
                confirm_think: true,
            }),
        ),
        (
            Intent::Send {
                text: " ".into(),
                attachments: vec![],
                confirm_think: false,
            },
            Err(IntentError::Empty),
        ),
        (
            Intent::Approve {
                call,
                decision: Decision::AllowForSession,
            },
            now(Submission::Approve {
                call_id: call,
                decision: Decision::AllowForSession,
            }),
        ),
        (
            Intent::Answer {
                question: call,
                text: Some("main".into()),
            },
            now(Submission::Answer {
                call_id: call,
                text: Some("main".into()),
            }),
        ),
        (Intent::Interrupt, now(Submission::Interrupt)),
        (
            Intent::Queue {
                text: "next".into(),
                attachments: vec![],
                confirm_think: false,
            },
            Ok(Dispatch::Queue(turn("next"))),
        ),
        (
            Intent::Queue {
                text: String::new(),
                attachments: vec![shot.clone()],
                confirm_think: true,
            },
            Ok(Dispatch::Queue(Submission::UserTurn {
                text: String::new(),
                attachments: vec![shot],
                confirm_think: true,
            })),
        ),
        (
            Intent::Queue {
                text: " ".into(),
                attachments: vec![],
                confirm_think: false,
            },
            Err(IntentError::Empty),
        ),
        (
            Intent::Compact { focus: None },
            now(Submission::Compact { focus: None }),
        ),
        (
            Intent::SetMode {
                mode: PermissionMode::Auto,
            },
            now(Submission::SetPermissionMode {
                mode: PermissionMode::Auto,
            }),
        ),
        (
            Intent::SwitchModel {
                tier: Tier::Think,
                model: None,
            },
            now(Submission::SwitchModel {
                tier: Tier::Think,
                model: None,
            }),
        ),
        (
            Intent::SetEffort { effort: None },
            now(Submission::SetEffort { effort: None }),
        ),
        (
            Intent::Rewind {
                to_turn: 3,
                code: true,
                conversation: false,
            },
            now(Submission::Rewind {
                to_turn: 3,
                code: true,
                conversation: false,
            }),
        ),
        (Intent::Redo, now(Submission::Redo)),
        (
            Intent::RevertFile {
                path: "a.rs".into(),
                to_turn: 2,
            },
            now(Submission::RevertFile {
                path: "a.rs".into(),
                to_turn: 2,
            }),
        ),
        (
            Intent::Fork { turn: None },
            Ok(Dispatch::Fork { turn: None }),
        ),
        (
            Intent::Handoff {
                objective: "x".into(),
            },
            Ok(Dispatch::Handoff {
                objective: "x".into(),
            }),
        ),
        (
            Intent::Handoff {
                objective: "".into(),
            },
            Err(IntentError::Empty),
        ),
        (
            Intent::Background { call },
            now(Submission::Background { call_id: call }),
        ),
        (
            Intent::Shell {
                command: "ls".into(),
                share: false,
            },
            spawned(Submission::UserShell {
                command: "ls".into(),
                share: false,
            }),
        ),
        (
            Intent::Command {
                line: "!!cargo test".into(),
            },
            spawned(Submission::UserShell {
                command: "cargo test".into(),
                share: true,
            }),
        ),
        (
            Intent::Command {
                line: "/review src main".into(),
            },
            spawned(Submission::Command {
                command: SlashCommand {
                    name: "review".into(),
                    args: vec!["src".into(), "main".into()],
                },
            }),
        ),
        (
            Intent::Command {
                line: "hello".into(),
            },
            Err(IntentError::NotACommand("hello".into())),
        ),
        (
            Intent::Rename {
                title: "Fix the ledger".into(),
            },
            now(Submission::Rename {
                title: "Fix the ledger".into(),
            }),
        ),
        (
            Intent::Command {
                line: "/rename Fix  the ledger".into(),
            },
            now(Submission::Rename {
                title: "Fix the ledger".into(),
            }),
        ),
        (
            Intent::Rename { title: " ".into() },
            Err(IntentError::Empty),
        ),
    ];
    for (intent, want) in cases {
        assert_eq!(dispatch(intent.clone()), want, "{intent:?}");
    }
}
