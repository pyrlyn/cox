// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `ask_user` modal (T22.1): what the modal shows for a question with
//! options, and which `Submission::Answer` each key sends — through the same
//! `update`/`view` the runtime uses, exactly like `approval.rs` covers the
//! approval modal.

use cox_protocol::ids::{CallId, SessionId};
use cox_protocol::types::{Event, PermissionMode, SandboxMode, Source, Submission};
use cox_tui::state::{Cmd, Msg, State, update};
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent};

fn answer(call_id: CallId, text: Option<String>) -> Cmd {
    Cmd::Submit(Submission::Answer { call_id, text })
}

fn key(state: &mut State, code: KeyCode) -> Vec<Cmd> {
    update(state, Msg::Key(KeyEvent::from(code)))
}

fn question_asked() -> (State, CallId) {
    question_asked_from(None)
}

fn question_asked_from(agent: Option<String>) -> (State, CallId) {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let call_id = CallId::new();
    update(
        &mut state,
        Msg::Event(Event::QuestionAsked {
            call_id,
            question: "which environment?".into(),
            options: vec!["staging".into(), "production".into()],
            source: agent.map(|agent| Source {
                session: SessionId::new(),
                agent: Some(agent),
                preset: None,
            }),
        }),
    );
    (state, call_id)
}

#[test]
fn modal_question_with_options() {
    let (state, _) = question_asked();
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 7)));
}

/// T34.3: a subagent's `ask_user` call names it in the agent colour, the
/// same way `approval_modal_shows_source_agent` (`approval.rs`) covers a
/// relayed approval; the main session's own question has no prefix.
#[test]
fn ask_user_surface_shows_which_agent_is_asking() {
    let (state, _) = question_asked_from(Some("explore-2".into()));
    let Some(cox_tui::state::Modal::Question(question)) = &state.modal else {
        panic!("question modal open");
    };
    let header = &question.lines(&state.glyphs, &state.theme)[0];
    assert_eq!(header.spans[0].style.fg, Some(state.theme.agent));
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 7)));
}

#[test]
fn question_digit_picks_the_option_and_sends_answer() {
    let (mut state, call_id) = question_asked();
    assert_eq!(
        key(&mut state, KeyCode::Char('2')),
        vec![answer(call_id, Some("production".into()))]
    );
    assert!(state.modal.is_none());
}

#[test]
fn question_enter_sends_the_typed_free_text() {
    let (mut state, call_id) = question_asked();
    for c in "canary".chars() {
        key(&mut state, KeyCode::Char(c));
    }
    assert_eq!(
        key(&mut state, KeyCode::Enter),
        vec![answer(call_id, Some("canary".into()))]
    );
    assert!(state.modal.is_none());
}

#[test]
fn question_esc_dismisses_with_no_answer() {
    let (mut state, call_id) = question_asked();
    assert_eq!(key(&mut state, KeyCode::Esc), vec![answer(call_id, None)]);
    assert!(state.modal.is_none());
}
