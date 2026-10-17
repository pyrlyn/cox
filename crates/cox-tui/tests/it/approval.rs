// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Diff view and approval modal (T5.4): what an edit's diff prints, what the
//! approval modal shows for a bash call, and which `Submission` each key
//! sends — all through the same `update`/`view` the runtime uses.

use std::path::PathBuf;

use cox_protocol::ids::{CallId, SessionId};
use cox_protocol::types::{
    Decision, Diff, Event, PermissionMode, Risk, SandboxMode, Source, Submission, ToolCall,
    ToolResult, Why,
};
use cox_tui::cells::cell_lines;
use cox_tui::state::{Cmd, Modal, Msg, State, update};
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(state: &mut State, code: KeyCode) -> Vec<Cmd> {
    update(state, Msg::Key(KeyEvent::from(code)))
}

fn edit_done(state: &mut State, path: &str, unified: &str) {
    let call_id = CallId::new();
    let call = ToolCall {
        id: call_id,
        name: "edit".into(),
        input: serde_json::json!({"path": path}),
        risk: Risk::Write,
        subject: path.into(),
        segments: None,
    };
    update(state, Msg::Event(Event::ToolCallRequested { call }));
    update(
        state,
        Msg::Event(Event::ToolCallDone {
            call_id,
            result: ToolResult {
                ok: true,
                visible: String::new(),
                archive: None,
                bytes: 0,
                duration_ms: 1,
                diff: Some(Diff {
                    path: PathBuf::from(path),
                    unified: unified.into(),
                }),
                structured: None,
            },
        }),
    );
}

fn transcript(state: &State) -> String {
    state
        .transcript
        .iter()
        .flat_map(|c| cell_lines(c, &state.look(60)))
        .map(|l| l.to_string().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn diff_two_files() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    edit_done(
        &mut state,
        "src/lib.rs",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-fn old() {}\n+fn new() {}\n fn keep() {}\n",
    );
    edit_done(
        &mut state,
        "README.md",
        "--- a/README.md\n+++ b/README.md\n@@ -1 +1,3 @@\n # cox\n+\n+A coxswain.\n",
    );
    let expanded = transcript(&state);
    update(
        &mut state,
        Msg::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
    );
    let collapsed = transcript(&state);
    insta::assert_snapshot!(format!("{expanded}\n---\n{collapsed}"));
}

fn bash_approval() -> (State, CallId) {
    bash_approval_from(None)
}

fn bash_approval_from(source: Option<Source>) -> (State, CallId) {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let call_id = CallId::new();
    let call = ToolCall {
        id: call_id,
        name: "bash".into(),
        input: serde_json::json!({"command": "git push --force"}),
        risk: Risk::Exec,
        subject: "git push --force".into(),
        segments: None,
    };
    update(
        &mut state,
        Msg::Event(Event::ApprovalRequired {
            call,
            why: Why::RuleAsk {
                rule: "Bash(git push:*)".into(),
            },
            source,
        }),
    );
    (state, call_id)
}

/// T27.2: a subagent's prompt names it in the agent colour; the main
/// session's prompt (`modal_bash_approval`) has no prefix.
#[test]
fn approval_modal_shows_source_agent() {
    let (state, _) = bash_approval_from(Some(Source {
        session: SessionId::new(),
        agent: Some("explore-2".into()),
        preset: Some("explore".into()),
    }));
    let Some(Modal::Approval(approval)) = &state.modal else {
        panic!("approval modal open");
    };
    let header = &approval.lines(&state.look(60))[0];
    assert_eq!(header.spans[0].style.fg, Some(state.theme.agent));
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 7)));
}

#[test]
fn modal_bash_approval() {
    let (mut state, _) = bash_approval();
    let asked = buffer_to_string(&render(&state, 60, 7));
    key(&mut state, KeyCode::Char('e'));
    for _ in 0..8 {
        key(&mut state, KeyCode::Backspace);
    }
    for c in " -n".chars() {
        key(&mut state, KeyCode::Char(c));
    }
    let editing = buffer_to_string(&render(&state, 60, 7));
    insta::assert_snapshot!(format!("{asked}\n---\n{editing}"));
}

#[test]
fn y_sends_approve_submission() {
    for (code, decision) in [
        (KeyCode::Char('y'), Decision::Allow),
        (KeyCode::Enter, Decision::Allow),
        (KeyCode::Char('s'), Decision::AllowForSession),
        (
            KeyCode::Char('n'),
            Decision::Deny {
                reason: "denied by user".into(),
            },
        ),
    ] {
        let (mut state, call_id) = bash_approval();
        assert_eq!(
            key(&mut state, code),
            vec![Cmd::Submit(Submission::Approve { call_id, decision })]
        );
        assert!(state.modal.is_none());
    }
}

#[test]
fn modal_edit_resubmits_the_command_as_decision_edit() {
    let (mut state, call_id) = bash_approval();
    key(&mut state, KeyCode::Char('e'));
    for _ in 0..8 {
        key(&mut state, KeyCode::Backspace);
    }
    for c in " -n".chars() {
        key(&mut state, KeyCode::Char(c));
    }
    assert_eq!(
        key(&mut state, KeyCode::Enter),
        vec![Cmd::Submit(Submission::Approve {
            call_id,
            decision: Decision::Edit {
                input: serde_json::json!({"command": "git push -n"}),
            },
        })]
    );
    // Esc while editing only leaves the editor; the call is still pending.
    let (mut state, _) = bash_approval();
    key(&mut state, KeyCode::Char('e'));
    assert!(key(&mut state, KeyCode::Esc).is_empty());
    assert!(state.modal.is_some());
}

/// T24.5: an `edit` awaiting approval shows its `old` → `new` through the
/// same diff renderer as the edit card, word diff included, above the keys.
#[test]
fn modal_edit_approval_shows_the_proposed_diff() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let call = ToolCall {
        id: CallId::new(),
        name: "edit".into(),
        input: serde_json::json!({
            "path": "src/lib.rs",
            "old": "fn keep() {}\nfn old() -> u8 { 1 }",
            "new": "fn keep() {}\nfn new() -> u8 { 2 }",
        }),
        risk: Risk::Write,
        subject: "src/lib.rs".into(),
        segments: None,
    };
    update(
        &mut state,
        Msg::Event(Event::ApprovalRequired {
            call,
            why: Why::Risk { risk: Risk::Write },
            source: None,
        }),
    );
    let frame = buffer_to_string(&render(&state, 60, 14));
    assert!(frame.contains("[y]es  [s]ession  [n]o"), "{frame}");
    insta::assert_snapshot!(frame);
}
