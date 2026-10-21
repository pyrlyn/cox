// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/theme`: picker preview and revert, the theme editor, saved themes.
//! Separate from `state/mod.rs` because it is the one part of `update` that
//! rewrites the drawn theme and keeps `theme_prev` to undo it.

use super::{Cmd, Modal, State};
use crate::picker::Picker;
use crate::theme_editor::ThemeEditor;

/// T24.2: applies one `/theme` row to `State` without persisting it — the
/// live preview `Esc` (via `theme_prev`) can still undo. A `syntax: <name>`
/// row only ever touches `syntax_theme`; a bare name looks it up in
/// `theme_catalog` and follows the current background unless the file
/// itself pins one.
fn apply_row(state: &mut State, row: &str) {
    if let Some(name) = row.strip_prefix("syntax: ") {
        if let Some(&s) = state.syntax_names.iter().find(|n| **n == name) {
            state.syntax_theme = s;
        }
        return;
    }
    if let Some((_, file)) = state.theme_catalog.iter().find(|(n, _)| n == row) {
        let dark = file.variant.unwrap_or(state.dark);
        state.dark = dark;
        state.theme = file.theme(dark);
    }
}

/// `Esc` in the `/theme` picker or the theme editor: whatever was drawn
/// before `/theme` opened.
pub(super) fn restore_theme(state: &mut State) {
    if let Some((dark, theme, syntax_theme)) = state.theme_prev.take() {
        state.dark = dark;
        state.theme = theme;
        state.syntax_theme = syntax_theme;
    }
}

/// `Ctrl+E` on a `/theme` colour row (T46.7) swaps the picker for the
/// editor on that theme; a `syntax: ` row has no tokens, so it stays put.
/// `theme_prev` was saved when the picker opened and is kept, so the
/// editor's `Esc` restores the same state the picker's would.
pub(super) fn open_theme_editor(state: &mut State) {
    let Some(Modal::Picker(picker)) = &state.modal else {
        return;
    };
    let Some(row) = picker.matches.get(picker.selected) else {
        return;
    };
    let Some((name, file)) = state.theme_catalog.iter().find(|(n, _)| n == row).cloned() else {
        return;
    };
    let builtin = crate::theme::builtin_source(&name).is_some();
    let dark = file.variant.unwrap_or(state.dark);
    let editor = ThemeEditor::open(name, builtin, dark, file);
    if state.theme_prev.is_none() {
        state.theme_prev = Some((state.dark, state.theme, state.syntax_theme));
    }
    state.dark = dark;
    state.theme = editor.theme();
    state.modal = Some(Modal::ThemeEditor(editor));
}

/// Every key that moves the `/theme` picker's selection previews that row.
pub(super) fn preview_theme(state: &mut State, picker: &Picker) {
    if let Some(row) = picker.matches.get(picker.selected).cloned() {
        apply_row(state, &row);
    }
}

/// `Enter` on a `/theme` row: applies it (in case `Enter` came before any
/// navigation ever previewed it) and persists it with `cox config set`
/// semantics — `Cmd::PersistConfig` carries the write to the runtime, which
/// alone has `config_cmd::set`.
pub(super) fn apply_theme_choice(state: &mut State, row: &str) -> Vec<Cmd> {
    apply_row(state, row);
    let key = if row.starts_with("syntax: ") {
        "tui.syntax_theme"
    } else {
        "tui.theme"
    };
    let value = row.strip_prefix("syntax: ").unwrap_or(row).to_string();
    vec![Cmd::PersistConfig {
        key: key.into(),
        value,
    }]
}

#[cfg(test)]
mod tests {
    use super::super::{Ask, Cmd, Modal, Msg, State, update};
    use crate::picker::Kind;
    use crate::theme;
    use cox_protocol::types::{PermissionMode, SandboxMode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// T24.2 step 3: moving the `/theme` picker's cursor previews a theme
    /// immediately, and `Esc` restores whatever was active before it opened
    /// — a browse that changes nothing must be free to abandon.
    #[test]
    fn theme_picker_preview_reverts_on_esc() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.theme_rows = vec!["cox-dark".into(), "cox-light".into()];
        state.theme_catalog = theme::BUILT_IN_THEMES[..2]
            .iter()
            .map(|(name, src)| ((*name).to_string(), theme::parse_theme_file(src).unwrap()))
            .collect();
        let before = (state.dark, state.theme, state.syntax_theme);

        // `/theme` submitted from the composer, same as `clear_command_emits_cmd_clear`.
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "theme".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert!(
            matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::Themes),
            "/theme with no argument opens the picker"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        assert_ne!(
            (state.dark, state.theme, state.syntax_theme),
            before,
            "moving the cursor previews the selected theme"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(
            (state.dark, state.theme, state.syntax_theme),
            before,
            "Esc restores the theme active before the picker opened"
        );
        assert!(state.modal.is_none());
    }

    /// `/theme` open over the two plain built-ins and one syntax row.
    fn theme_picker_open() -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.theme_rows = vec![
            "cox-dark".into(),
            "cox-light".into(),
            "syntax: base16".into(),
        ];
        state.theme_catalog = theme::BUILT_IN_THEMES[..2]
            .iter()
            .map(|(name, src)| ((*name).to_string(), theme::parse_theme_file(src).unwrap()))
            .collect();
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "theme".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        state
    }

    fn ctrl_e() -> Msg {
        Msg::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))
    }

    /// T46.7: `Ctrl+E` is `expand` elsewhere; over `/theme` it opens the
    /// editor on the highlighted row, and the editor's `Esc` restores what
    /// was drawn before `/theme` opened.
    #[test]
    fn ctrl_e_opens_the_editor_on_the_highlighted_theme() {
        let mut state = theme_picker_open();
        let drawn = state.theme_prev.expect("/theme saved what was drawn");
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        update(&mut state, ctrl_e());
        match &state.modal {
            Some(Modal::ThemeEditor(e)) => {
                assert_eq!(e.name, "cox-light");
                assert!(e.builtin);
            }
            other => panic!("expected the theme editor, got {other:?}"),
        }
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(state.modal, None);
        assert_eq!((state.dark, state.theme, state.syntax_theme), drawn);
    }

    /// T46.7: a built-in is never written in place; its edits go to
    /// `<name>-custom`, each token as the text a theme file holds.
    #[test]
    fn editor_save_emits_save_theme_with_custom_stem_for_builtin() {
        let mut state = theme_picker_open();
        update(&mut state, ctrl_e());
        let dark = match &state.modal {
            Some(Modal::ThemeEditor(e)) => e.dark,
            other => panic!("expected the theme editor, got {other:?}"),
        };
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Down)));
        for _ in 0..16 {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Backspace)));
        }
        for c in "#ff0000".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        assert_eq!(
            state.theme.accent,
            ratatui::style::Color::Rgb(255, 0, 0),
            "the preview follows the edit"
        );
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(
            cmds,
            vec![Cmd::Ask(Ask::SaveTheme {
                stem: "cox-dark-custom".into(),
                dark,
                tokens: vec![("accent".into(), "#ff0000".into())],
            })]
        );
        assert_eq!(state.modal, None);
    }

    /// T46.7: the runtime's answer lists the new file among the colour
    /// themes (before the `syntax: ` rows), applies it and persists it.
    #[test]
    fn saved_theme_is_selectable_without_restart() {
        let mut state = theme_picker_open();
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        let src = theme::builtin_source("cox-dark").unwrap();
        let src =
            theme::set_token(src, "accent", true, ratatui::style::Color::Rgb(1, 2, 3)).unwrap();
        let file = theme::parse_theme_file(&src).unwrap();
        let cmds = update(
            &mut state,
            Msg::ThemeSaved("cox-dark-custom".into(), file.clone()),
        );
        assert_eq!(
            state.theme_rows,
            vec!["cox-dark", "cox-light", "cox-dark-custom", "syntax: base16"]
        );
        assert!(
            state
                .theme_catalog
                .iter()
                .any(|(n, f)| n == "cox-dark-custom" && *f == file)
        );
        assert_eq!(state.theme, file.theme(state.dark));
        assert_eq!(
            cmds,
            vec![Cmd::PersistConfig {
                key: "tui.theme".into(),
                value: "cox-dark-custom".into(),
            }]
        );
    }
}
