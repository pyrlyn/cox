// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Sessions in the TUI (T16.5): `/sessions` lists the preloaded rows,
//! `/resume` opens the picker over them, and a chosen row names the command
//! that resumes it.

mod common;

use cox_protocol::types::{PermissionMode, SandboxMode};
use cox_tui::picker::Kind;
use cox_tui::state::Msg;
use cox_tui::state::{Cell, Modal, State, update};
use crossterm::event::{KeyCode, KeyEvent};

fn preloaded() -> State {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    state.sessions = vec![
        (
            "01BX5ZZKBKACTAV9WEVGEMMVRZ".into(),
            "fix the flaky test · /w · 2h ago · $0.31".into(),
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            "untitled · /w · 3d ago · $0.00".into(),
        ),
    ];
    state
}

fn last_notice(state: &State) -> &str {
    let Some(Cell::Notice { text, .. }) = state.transcript.last() else {
        panic!("no notice cell");
    };
    text
}

#[test]
fn sessions_command_lists_the_preloaded_rows_snapshot() {
    let mut state = preloaded();
    common::type_line(&mut state, "/sessions");
    insta::assert_snapshot!(last_notice(&state));
}

#[test]
fn sessions_command_says_so_when_there_are_none() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    common::type_line(&mut state, "/sessions");
    assert_eq!(last_notice(&state), "no sessions for this project yet");
}

#[test]
fn sessions_resume_opens_the_picker_newest_first_and_a_choice_names_the_command() {
    let mut state = preloaded();
    common::type_line(&mut state, "/resume");
    let Some(Modal::Picker(picker)) = &state.modal else {
        panic!("no picker");
    };
    assert_eq!(picker.kind, Kind::Sessions);
    assert_eq!(picker.matches[0], state.sessions[0].1);
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(state.modal.is_none());
    assert_eq!(
        last_notice(&state),
        "to resume: cox --resume 01BX5ZZKBKACTAV9WEVGEMMVRZ"
    );
}
