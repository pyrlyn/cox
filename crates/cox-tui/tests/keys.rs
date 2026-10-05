// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Keys and runtime messages the frame tests leave out (A16): quit and
//! paste, the history picker, decisions and model switches arriving from
//! the runtime, task completion, the clock, and the pinned security banner.

use cox_protocol::ids::{CallId, ItemId, TaskId, TurnId};
use cox_protocol::types::{
    DecidedBy, Decision, Event, Job, Level, ModelId, PermissionMode, Risk, SandboxMode, Tier,
    ToolCall, Why,
};
use cox_tui::picker::Kind;
use cox_tui::state::{Cmd, Modal, Msg, State, update};
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

mod common;
use common::type_line;

fn state() -> State {
    State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite)
}

fn ctrl(c: char) -> Msg {
    Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn key(state: &mut State, code: KeyCode) -> Vec<Cmd> {
    update(state, Msg::Key(KeyEvent::from(code)))
}

fn status(state: &State) -> String {
    cox_tui::status::line(state).to_string()
}

#[test]
fn ctrl_d_quits_without_arming() {
    let mut state = state();
    assert_eq!(update(&mut state, ctrl('d')), vec![Cmd::Quit]);
}

#[test]
fn paste_inserts_text_without_submitting() {
    let mut state = state();
    assert!(update(&mut state, Msg::Paste("git log\n--oneline".into())).is_empty());
    assert_eq!(state.composer.text(), "git log\n--oneline");
    assert_eq!(state.composer.line_count(), 2);
}

#[test]
fn ctrl_r_lists_history_newest_first_and_a_choice_refills_the_composer() {
    let mut state = state();
    type_line(&mut state, "first");
    type_line(&mut state, "second");
    assert!(update(&mut state, ctrl('r')).is_empty());
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("Ctrl+R opens the history picker: {:?}", state.modal);
    };
    assert_eq!(picker.kind, Kind::History);
    assert_eq!(picker.matches, ["second", "first"]);
    key(&mut state, KeyCode::Down);
    assert!(key(&mut state, KeyCode::Enter).is_empty());
    assert!(state.modal.is_none());
    assert_eq!(state.composer.text(), "first");
}

/// T25.8: `Ctrl+R` lists this session's own lines first, unchanged, then
/// the prompts of the project's other sessions with their age; choosing
/// one of those puts its whole text back, not the row.
#[test]
fn history_picker_lists_other_sessions_after_current() {
    let mut state = state();
    type_line(&mut state, "first");
    type_line(&mut state, "second");
    state.past_prompts = [
        ("2d", "fix the login bug\nand add a test"),
        ("now", "add a cache column"),
    ]
    .iter()
    .map(|(age, text)| (cox_tui::picker::prompt_entry(age, text), text.to_string()))
    .collect();
    assert!(update(&mut state, ctrl('r')).is_empty());
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("Ctrl+R opens the history picker: {:?}", state.modal);
    };
    insta::assert_snapshot!(picker.matches.join("\n"));
    for _ in 0..2 {
        key(&mut state, KeyCode::Down);
    }
    assert!(key(&mut state, KeyCode::Enter).is_empty());
    assert_eq!(state.composer.text(), "fix the login bug\nand add a test");
}

#[test]
fn approval_decided_by_a_rule_closes_the_modal() {
    let mut state = state();
    let call_id = CallId::new();
    update(
        &mut state,
        Msg::Event(Event::ApprovalRequired {
            call: ToolCall {
                id: call_id,
                name: "bash".into(),
                input: serde_json::json!({"command": "cargo test"}),
                risk: Risk::Exec,
                subject: "cargo test".into(),
                segments: None,
            },
            why: Why::Risk { risk: Risk::Exec },
            source: None,
        }),
    );
    assert!(matches!(state.modal, Some(Modal::Approval(_))));
    update(
        &mut state,
        Msg::Event(Event::ApprovalDecided {
            call_id,
            decision: Decision::Allow,
            by: DecidedBy::Rule,
        }),
    );
    assert!(state.modal.is_none());
}

#[test]
fn model_switched_renames_the_status_only_for_the_running_tier() {
    let mut state = state();
    update(
        &mut state,
        Msg::Event(Event::TurnStarted {
            seq: 1,
            turn: TurnId::new(),
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("claude-sonnet-5".into()),
        }),
    );
    let switch = |tier, to: &str| Event::ModelSwitched {
        tier,
        from: ModelId("x".into()),
        to: ModelId(to.into()),
    };
    update(
        &mut state,
        Msg::Event(switch(Tier::Cheap, "claude-haiku-4-5")),
    );
    assert_eq!(state.status.model, "claude-sonnet-5");
    update(&mut state, Msg::Event(switch(Tier::Code, "claude-opus-5")));
    assert_eq!(state.status.model, "claude-opus-5");
}

#[test]
fn task_completed_drops_it_from_the_status_count() {
    let mut state = state();
    let tasks = [TaskId::new(), TaskId::new()];
    for task in tasks {
        update(
            &mut state,
            Msg::Event(Event::TaskCreated {
                task,
                label: "explore".into(),
                tier: Tier::Cheap,
            }),
        );
    }
    assert!(status(&state).contains("2 tasks"));
    update(
        &mut state,
        Msg::Event(Event::TaskCompleted {
            task: tasks[0],
            result_item: ItemId::new(),
            cost_usd: 0.01,
            exit_code: None,
            archive: None,
        }),
    );
    assert_eq!(state.tasks.len(), 1);
    assert!(!status(&state).contains("2 tasks"));
    assert!(status(&state).contains("1 task"), "{}", status(&state));
}

#[test]
fn security_notice_pins_a_banner_but_a_warning_is_a_cell() {
    let mut state = state();
    update(
        &mut state,
        Msg::Event(Event::Notice {
            level: Level::Security,
            text: "sandbox denied a write outside the workspace".into(),
        }),
    );
    assert!(state.banner.is_some());
    assert!(state.transcript.is_empty(), "a banner is not a cell");
    update(
        &mut state,
        Msg::Event(Event::Notice {
            level: Level::Warn,
            text: "hook skipped".into(),
        }),
    );
    assert_eq!(state.transcript.len(), 1);
    let frame = buffer_to_string(&render(&state, 60, 6));
    let first = frame.lines().next().unwrap_or_default();
    assert!(
        first.contains("sandbox denied"),
        "banner on the first row: {frame}"
    );
    assert!(frame.contains("hook skipped"));
}

/// `type_line` presses `Enter` at the end (it submits), so these tests type
/// the letters themselves and hold the modified `Enter` back for the case
/// under test.
fn type_chars(state: &mut State, text: &str) {
    for c in text.chars() {
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
}

#[test]
fn shift_enter_inserts_newline() {
    let mut state = state();
    type_chars(&mut state, "first");
    let cmds = update(
        &mut state,
        Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
    );
    assert!(cmds.is_empty(), "no submit: {cmds:?}");
    assert_eq!(state.composer.line_count(), 2);
    assert_eq!(state.composer.text(), "first\n");
}

// T23.1: reserved for send-now (T25.1); until then it is the same newline
// fallback as `Shift+Enter`/`Alt+Enter`, distinct only from a bare `Enter`.
#[test]
fn ctrl_enter_is_distinct() {
    let mut state = state();
    type_chars(&mut state, "first");
    let cmds = update(
        &mut state,
        Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
    );
    assert!(cmds.is_empty(), "no submit: {cmds:?}");
    assert_eq!(state.composer.line_count(), 2);
    assert_eq!(state.composer.text(), "first\n");

    let submitted = key(&mut state, KeyCode::Enter);
    assert!(
        matches!(submitted.as_slice(), [Cmd::Submit(_)]),
        "bare Enter still submits: {submitted:?}"
    );
}

#[test]
fn tick_advances_the_clock_and_resize_asks_nothing() {
    let mut state = state();
    for _ in 0..3 {
        assert!(update(&mut state, Msg::Tick).is_empty());
    }
    assert_eq!(state.tick, 3);
    assert!(update(&mut state, Msg::Resize(80, 24)).is_empty());
}

/// T25.2: `Shift+Tab` (`BackTab`, with or without `SHIFT`) cycles default →
/// plan → auto → default, and the composer prompt follows the mode.
#[test]
fn backtab_cycles_modes() {
    let mut s = state();
    let mut prompts = Vec::new();
    for want in [
        PermissionMode::Plan,
        PermissionMode::Auto,
        PermissionMode::Default,
    ] {
        prompts.push(buffer_to_string(&render(&s, 40, 3)));
        let cmds = update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        );
        assert_eq!(
            cmds,
            vec![Cmd::Submit(
                cox_protocol::types::Submission::SetPermissionMode { mode: want }
            )]
        );
        assert_eq!(s.mode, want);
    }
    insta::assert_snapshot!(prompts.join("\n---\n"));
    // A bare `Tab` with nothing to complete neither cycles nor types.
    assert!(key(&mut s, KeyCode::Tab).is_empty());
    assert_eq!(s.mode, PermissionMode::Default);
    assert_eq!(s.composer.text(), "");
}

/// T25.2: `Tab` on an `@word` opens the file picker with `word` as its query
/// and the choice replaces it; on a leading `/word` it opens the palette.
#[test]
fn tab_completes_mention() {
    let mut s = state();
    s.files = ["README.md", "src/main.rs"].map(String::from).to_vec();
    s.composer.set_text("look at @mai");
    key(&mut s, KeyCode::Tab);
    match &s.modal {
        Some(Modal::Picker(p)) => {
            assert_eq!((p.kind, p.query.as_str()), (Kind::Files, "mai"));
            assert_eq!(p.matches, ["src/main.rs"]);
        }
        other => panic!("no file picker: {other:?}"),
    }
    key(&mut s, KeyCode::Enter);
    assert_eq!(s.composer.text(), "look at @src/main.rs ");

    s.composer.set_text("/hel");
    key(&mut s, KeyCode::Tab);
    assert!(matches!(&s.modal, Some(Modal::Picker(p)) if p.kind == Kind::Commands));
    // `Esc` gives the query back rather than losing what was typed.
    key(&mut s, KeyCode::Esc);
    assert_eq!(s.composer.text(), "/hel");
}
