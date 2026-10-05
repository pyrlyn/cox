// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Diff view (T15.3): `Ctrl+G` asks the runtime for the working tree's diff
//! and the answer opens a modal over the transcript, rendered through the
//! same per-file blocks an edit result uses. T24.5 adds the word diff and
//! the side-by-side layout the same renderer picks by width.

use cox_protocol::types::{Diff, PermissionMode, SandboxMode};
use cox_tui::diff;
use cox_tui::state::{Ask, Cmd, Modal, Msg, State, update};
use cox_tui::theme::Theme;
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::text::Line;

const PATCH: &str = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-fn old() {}\n+fn new() {}\n fn keep() {}\ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,3 @@\n # cox\n+\n+A coxswain.\n";

fn ctrl_g(state: &mut State) -> Vec<Cmd> {
    update(
        state,
        Msg::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
    )
}

fn scroll(state: &State) -> usize {
    match &state.modal {
        Some(Modal::Diff { scroll, .. }) => *scroll,
        other => panic!("not the diff view: {other:?}"),
    }
}

#[test]
fn diff_view_shows_a_two_file_patch_as_two_headed_blocks() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    assert_eq!(ctrl_g(&mut state), vec![Cmd::Ask(Ask::GitDiff)]);
    assert!(state.modal.is_none(), "the view waits for the answer");
    update(&mut state, Msg::Diff(Some(PATCH.into())));
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 18)));
    // A second Ctrl+G while the view is open closes it instead of asking.
    assert!(ctrl_g(&mut state).is_empty());
    assert!(state.modal.is_none());
}

#[test]
fn diff_view_scrolls_by_page_and_esc_closes_it() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    update(&mut state, Msg::Diff(Some(PATCH.into())));
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::PageDown)));
    let down = scroll(&state);
    assert!(down > 0 && down < PATCH.lines().count(), "{down}");
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::PageUp)));
    assert_eq!(scroll(&state), 0);
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(state.modal.is_none());
}

#[test]
fn diff_view_says_no_changes_for_an_empty_or_absent_diff() {
    for answer in [Some(String::new()), None] {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        update(&mut state, Msg::Diff(answer));
        let frame = buffer_to_string(&render(&state, 60, 8));
        assert!(frame.contains("no changes"), "{frame}");
    }
}

/// T24.5: a hunk with a replaced pair, an unpaired removal and an addition,
/// numbered from line 10 so the side-by-side gutter has two digits.
const WIDE: &str = "diff --git a/src/loop.rs b/src/loop.rs\n--- a/src/loop.rs\n+++ b/src/loop.rs\n@@ -10,5 +10,5 @@\n fn step(&mut self) {\n-    let total = price * count;\n-    self.log(total);\n+    let total = price * quantity + tax;\n     self.emit(total);\n+    self.ledger.record(total);\n }\n";

fn diff_view(width: u16) -> String {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    ctrl_g(&mut state);
    update(&mut state, Msg::Diff(Some(WIDE.into())));
    buffer_to_string(&render(&state, width, 14))
}

/// Spans as text with their role marked: `{+added+}`, `[-removed-]`,
/// `~unchanged~`; anything else (the marker, syntect colours) plain.
fn marked(line: &Line<'_>, theme: &Theme) -> String {
    line.spans
        .iter()
        .map(|s| {
            let t = s.content.as_ref();
            if s.style.fg == Some(theme.diff_add) && t.trim() != "+" {
                format!("{{+{t}+}}")
            } else if s.style.fg == Some(theme.diff_del) && t.trim() != "-" {
                format!("[-{t}-]")
            } else if s.style.add_modifier.contains(Modifier::DIM) {
                format!("~{t}~")
            } else {
                t.to_string()
            }
        })
        .collect()
}

#[test]
fn word_diff_highlights_changed_words() {
    let state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let look = state.look(80);
    let d = Diff {
        path: "src/loop.rs".into(),
        unified: "@@ -1 +1 @@\n-let total = price * count;\n+let total = price * quantity + tax;\n"
            .into(),
    };
    let out = diff::lines(&d, &look);
    let span = |row: usize, text: &str| {
        out[row]
            .spans
            .iter()
            .find(|s| s.content.contains(text))
            .map(|s| s.style)
            .unwrap_or_else(|| panic!("no span {text:?} in {:?}", out[row]))
    };
    assert_eq!(span(2, "count").fg, Some(state.theme.diff_del));
    assert_eq!(span(3, "quantity").fg, Some(state.theme.diff_add));
    assert!(span(3, "price").add_modifier.contains(Modifier::DIM));
    insta::assert_snapshot!(
        out.iter()
            .map(|l| marked(l, &state.theme))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn side_by_side_at_140_columns() {
    insta::assert_snapshot!(diff_view(140));
}

#[test]
fn stacked_at_80_columns() {
    insta::assert_snapshot!(diff_view(80));
}
