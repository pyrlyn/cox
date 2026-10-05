// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/rewind` in the TUI (T26.2): the timeline lists turns newest first with
//! their checkpointed file counts, a choice plus a what-to-restore row
//! becomes `Submission::Rewind`, `Esc Esc` on an empty composer opens the
//! same picker, and `Rewound` cuts the transcript at the chosen turn.

mod common;

use cox_protocol::ids::{CallId, ItemId, TurnId};
use cox_protocol::types::{
    CheckpointFile, CheckpointKind, Event, ItemKind, Job, ModelId, PermissionMode, SandboxMode,
    StopReason, Submission, Tier,
};
use cox_tui::picker::Kind;
use cox_tui::state::{Cell, Cmd, ESC_ESC_TICKS, Modal, Msg, State, update};
use crossterm::event::{KeyCode, KeyEvent};

fn feed(state: &mut State, ev: Event) {
    update(state, Msg::Event(ev));
}

/// One finished user turn with `files` checkpointed paths.
fn turn(state: &mut State, seq: u32, text: &str, files: usize) {
    let turn = TurnId::new();
    feed(
        state,
        Event::TurnStarted {
            turn,
            seq,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        },
    );
    feed(
        state,
        Event::ItemStarted {
            item: ItemId::new(),
            kind: ItemKind::UserMessage {
                text: text.into(),
                attachments: Vec::new(),
            },
        },
    );
    if files > 0 {
        feed(
            state,
            Event::Checkpoint {
                turn,
                call: Some(CallId::new()),
                files: (0..files)
                    .map(|i| CheckpointFile {
                        path: format!("/w/f{i}.rs").into(),
                        kind: CheckpointKind::Pre,
                    })
                    .collect(),
            },
        );
    }
    feed(
        state,
        Event::TurnDone {
            turn,
            stop: StopReason::EndTurn,
        },
    );
}

fn two_turns() -> State {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, 1, "add the cache column", 3);
    turn(
        &mut state,
        2,
        "now a very long prompt that the timeline shortens so the row stays one line",
        0,
    );
    state
}

fn picker_rows(state: &State) -> Vec<String> {
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("no picker");
    };
    picker.matches.clone()
}

#[test]
fn rewind_timeline_snapshot() {
    let mut state = two_turns();
    common::type_line(&mut state, "/rewind");
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("no picker");
    };
    assert_eq!(picker.kind, Kind::Rewind);
    insta::assert_snapshot!(picker_rows(&state).join("\n"));
}

#[test]
fn rewind_choice_then_what_becomes_a_submission() {
    let mut state = two_turns();
    common::type_line(&mut state, "/rewind");
    // Newest first: Down selects T1.
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
    let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(cmds.is_empty());
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("no what-to-restore picker");
    };
    assert_eq!(picker.kind, Kind::RewindWhat);
    // `code` is the second row.
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
    let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(
        cmds,
        vec![Cmd::Submit(Submission::Rewind {
            to_turn: 1,
            code: true,
            conversation: false,
        })]
    );
    assert!(state.modal.is_none());
}

#[test]
fn esc_esc_on_an_empty_composer_opens_the_timeline() {
    let mut state = two_turns();
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(state.modal.is_none());
    for _ in 0..ESC_ESC_TICKS {
        update(&mut state, Msg::Tick);
    }
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::Rewind));

    // Too slow, or with text in the composer, it is just Esc.
    let mut state = two_turns();
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    for _ in 0..=ESC_ESC_TICKS {
        update(&mut state, Msg::Tick);
    }
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(state.modal.is_none());
    let mut state = two_turns();
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('x'))));
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(state.modal.is_none());
}

#[test]
fn rewound_conversation_cuts_the_transcript_at_the_turn() {
    let mut state = two_turns();
    assert_eq!(state.turns.len(), 2);
    feed(
        &mut state,
        Event::Rewound {
            to_turn: 2,
            code: false,
            conversation: true,
            restored: Vec::new(),
            skipped: Vec::new(),
        },
    );
    assert_eq!(state.turns.len(), 1);
    assert!(
        matches!(state.transcript.last(), Some(Cell::User { text, .. }) if text == "add the cache column")
    );
    // A code-only rewind leaves the transcript alone.
    feed(
        &mut state,
        Event::Rewound {
            to_turn: 1,
            code: true,
            conversation: false,
            restored: vec!["/w/f0.rs".into()],
            skipped: Vec::new(),
        },
    );
    assert_eq!(state.turns.len(), 1);
}

/// T26.4: `/undo` is a code-only rewind of the last turn, `/redo` the
/// core's one-step way back; with no turn yet `/undo` only says so.
#[test]
fn undo_and_redo_submit_one_step() {
    let mut empty = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    assert!(common::type_line(&mut empty, "/undo").is_empty());
    assert!(
        matches!(empty.transcript.last(), Some(Cell::Notice { text, .. }) if text.starts_with("undo:"))
    );
    let mut state = two_turns();
    assert!(matches!(
        common::type_line(&mut state, "/undo")[..],
        [Cmd::Submit(Submission::Rewind {
            to_turn: 2,
            code: true,
            conversation: false,
        })]
    ));
    assert!(matches!(
        common::type_line(&mut state, "/redo")[..],
        [Cmd::Submit(Submission::Redo)]
    ));
}
