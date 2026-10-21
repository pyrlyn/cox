// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Wheel scrolling, click-to-unfold and the hover pointer, `tui.mouse`.
//! Split out of `state.rs` because a mouse event reaches the rest of
//! `State` only through `toggle_fold` and each modal's own scroll.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{Cmd, Modal, State, toggle_fold};
use crate::pointer::{Interaction, pointer};

/// Lines (or picker rows) one wheel tick moves (T22.4) — smaller than
/// `DIFF_PAGE` because a tick is a nudge, not a page.
const WHEEL_LINES: usize = 3;

/// A wheel tick (T22.4) or a left click (T22.9): a click only acts with no
/// modal open, like the wheel's own `None` arm below, and only when it
/// lands inside `cell_rows` — `view.rs`'s record of what is actually on
/// screen after the last draw — so anywhere else does nothing.
pub(super) fn on_mouse(state: &mut State, ev: MouseEvent) -> Vec<Cmd> {
    if matches!(ev.kind, MouseEventKind::Moved) {
        return on_move(state, ev.row);
    }
    if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
        if state.modal.is_none() {
            let hit = state
                .cell_rows
                .borrow()
                .iter()
                .find(|(rows, _)| rows.contains(&ev.row))
                .map(|&(_, i)| i);
            if let Some(i) = hit {
                return toggle_fold(state, i);
            }
        }
        return Vec::new();
    }
    // The wheel reuses whichever scroll path the same context's keyboard
    // already has, moving it `WHEEL_LINES` at a time; the plain transcript
    // had none (`state.scroll` was dead until T22.4), so wheel is its first.
    let up = match ev.kind {
        MouseEventKind::ScrollUp => true,
        MouseEventKind::ScrollDown => false,
        _ => return Vec::new(),
    };
    match &mut state.modal {
        Some(Modal::Picker(picker)) => {
            let code = if up { KeyCode::Up } else { KeyCode::Down };
            for _ in 0..WHEEL_LINES {
                picker.key(KeyEvent::new(code, KeyModifiers::NONE));
            }
        }
        Some(Modal::Refine(review)) => review.wheel(up, WHEEL_LINES),
        Some(Modal::Diff { scroll, text }) => {
            *scroll = if up {
                scroll.saturating_sub(WHEEL_LINES)
            } else {
                (*scroll + WHEEL_LINES).min(text.lines().count().saturating_sub(1))
            };
        }
        Some(
            Modal::Approval(_)
            | Modal::Question(_)
            | Modal::PluginGrant(_)
            | Modal::PluginRemove(_)
            | Modal::Help
            | Modal::Context
            | Modal::Agents { .. }
            | Modal::Transcript { .. }
            | Modal::Plugin { .. }
            | Modal::ThemeEditor(_),
        ) => {}
        None => {
            state.scroll = if up {
                state.scroll.saturating_add(WHEEL_LINES)
            } else {
                state.scroll.saturating_sub(WHEEL_LINES)
            };
        }
    }
    Vec::new()
}

/// A mouse move (T5.9): the pointer shape for what is under `row`, decided
/// by `cox_render::pointer` from the same two facts `on_mouse` acts on — a
/// row in `cell_rows` is where a click folds a card (only with no modal
/// open), a row in `text_rows` is transcript text. No rule of its own: this
/// only builds the `Interaction`. Silent without OSC 22 or with the mouse
/// off, so a terminal that cannot draw it is never sent the bytes.
fn on_move(state: &mut State, row: u16) -> Vec<Cmd> {
    if !(state.mouse && state.caps.osc22) {
        return Vec::new();
    }
    let clickable = state.modal.is_none()
        && state
            .cell_rows
            .borrow()
            .iter()
            .any(|(rows, _)| rows.contains(&row));
    let want = pointer(Interaction {
        clickable,
        selectable: state.text_rows.borrow().contains(&row),
        ..Interaction::default()
    });
    if want == state.pointer {
        return Vec::new();
    }
    state.pointer = want;
    vec![Cmd::Pointer(want)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::{Kind, Picker};
    use crate::state::{Msg, update};
    use cox_protocol::ids::CallId;
    use cox_protocol::types::{Event, PermissionMode, SandboxMode, ToolCall, ToolResult};

    /// T22.4: a wheel tick reuses whichever context is open's own scroll —
    /// the plain transcript's `state.scroll`, the diff view's `scroll`
    /// field, or a picker's `selected` row — moving `WHEEL_LINES` at a time.
    #[test]
    fn update_mouse_wheel_scrolls_overlay() {
        fn wheel(up: bool) -> Msg {
            Msg::Mouse(MouseEvent {
                kind: if up {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        }

        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        update(&mut state, wheel(true));
        assert_eq!(state.scroll, WHEEL_LINES);
        update(&mut state, wheel(false));
        assert_eq!(state.scroll, 0);
        // Never underflows past the bottom.
        update(&mut state, wheel(false));
        assert_eq!(state.scroll, 0);

        state.modal = Some(Modal::Diff {
            text: (0..20)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            scroll: 5,
        });
        update(&mut state, wheel(true));
        assert_eq!(
            state.modal,
            Some(Modal::Diff {
                text: (0..20)
                    .map(|n| format!("line {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                scroll: 5 - WHEEL_LINES,
            })
        );

        state.modal = Some(Modal::Picker(Picker::open(
            Kind::Files,
            vec!["a".into(), "b".into(), "c".into(), "d".into()],
        )));
        update(&mut state, wheel(false));
        let Some(Modal::Picker(picker)) = &state.modal else {
            panic!("picker closed");
        };
        assert_eq!(picker.selected, WHEEL_LINES.min(3));
        update(&mut state, wheel(true));
        let Some(Modal::Picker(picker)) = &state.modal else {
            panic!("picker closed");
        };
        assert_eq!(picker.selected, 0);
    }

    /// T22.9: a click on a folded tool card's own rows (from `cell_rows`,
    /// which `view()` records) forces it open via `state.expanded`, the
    /// same set `Ctrl+E` flips; a second click folds it back.
    #[test]
    fn update_mouse_click_unfolds_card() {
        use cox_protocol::types::Risk;
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let id = CallId::new();
        update(
            &mut state,
            Msg::Event(Event::ToolCallRequested {
                call: ToolCall {
                    id,
                    name: "bash".into(),
                    input: serde_json::json!({}),
                    risk: Risk::Exec,
                    subject: "seq 20".into(),
                    segments: None,
                },
            }),
        );
        let body = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        update(
            &mut state,
            Msg::Event(Event::ToolCallOutput {
                call_id: id,
                delta: format!("{body}\n[exit 0 in 8ms]"),
            }),
        );
        update(
            &mut state,
            Msg::Event(Event::ToolCallDone {
                call_id: id,
                result: ToolResult {
                    ok: true,
                    visible: String::new(),
                    archive: None,
                    bytes: 0,
                    duration_ms: 8,
                    diff: None,
                    structured: None,
                },
            }),
        );

        let area = ratatui::layout::Rect::new(0, 0, 60, 30);
        let click = |row: u16| {
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        // Unfolding changes the transcript's line count, so the row to
        // click is read back from `cell_rows` after each draw.
        let card_row = |state: &State| -> u16 {
            let mut buf = ratatui::buffer::Buffer::empty(area);
            crate::view::view(state, area, &mut buf);
            state.cell_rows.borrow()[0].0.start
        };

        let row = card_row(&state);
        update(&mut state, click(row));
        assert!(
            state.expanded.contains(&0),
            "a click on the folded card should force it open"
        );

        let row = card_row(&state);
        update(&mut state, click(row));
        assert!(
            !state.expanded.contains(&0),
            "a second click should fold the card back"
        );
    }

    /// T5.9: what the tests below move over — rows 2..4 are a tool card (a
    /// click folds it), rows 4..8 transcript text, anything else empty.
    fn pointer_state(osc22: bool) -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc22 = osc22;
        *state.cell_rows.borrow_mut() = vec![(2..4, 0)];
        *state.text_rows.borrow_mut() = 2..8;
        state
    }

    fn moved(row: u16) -> Msg {
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn mouse_move_asks_for_the_pointer_the_shared_rule_picks() {
        use cox_render::pointer::Pointer;
        let mut state = pointer_state(true);
        assert_eq!(
            update(&mut state, moved(2)),
            vec![Cmd::Pointer(Pointer::Action)],
            "a foldable card row is clickable, which outranks selectable"
        );
        assert_eq!(
            update(&mut state, moved(5)),
            vec![Cmd::Pointer(Pointer::Text)]
        );
        assert_eq!(
            update(&mut state, moved(12)),
            vec![Cmd::Pointer(Pointer::Default)]
        );
    }

    #[test]
    fn mouse_move_inside_the_same_shape_writes_nothing() {
        let mut state = pointer_state(true);
        update(&mut state, moved(5));
        assert_eq!(update(&mut state, moved(6)), Vec::new());
        assert_eq!(update(&mut state, moved(5)), Vec::new());
        // The arrow is the starting shape, so empty space first asks for none.
        let mut fresh = pointer_state(true);
        assert_eq!(update(&mut fresh, moved(12)), Vec::new());
    }

    #[test]
    fn mouse_move_writes_nothing_without_osc22_or_with_the_mouse_off() {
        let mut without = pointer_state(false);
        assert_eq!(update(&mut without, moved(2)), Vec::new());
        assert_eq!(without.pointer, cox_render::pointer::Pointer::Default);

        let mut off = pointer_state(true);
        off.mouse = false;
        assert_eq!(update(&mut off, moved(2)), Vec::new());
    }

    #[test]
    fn an_open_modal_makes_a_card_row_plain_text() {
        use cox_render::pointer::Pointer;
        let mut state = pointer_state(true);
        state.modal = Some(Modal::Help);
        assert_eq!(
            update(&mut state, moved(2)),
            vec![Cmd::Pointer(Pointer::Text)],
            "a click does nothing behind a modal, so the row is not clickable"
        );
    }
}
