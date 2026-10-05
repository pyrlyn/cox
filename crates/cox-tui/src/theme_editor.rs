// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The theme editor modal (T46.6): the 17 colour tokens of one theme with a
//! swatch and the current value, the selected one edited inline. Every
//! keystroke that parses is applied to the file's overrides at once, so the
//! caller can redraw with [`ThemeEditor::theme`] as a live preview; one that
//! does not parse is flagged and leaves the old colour in place.
//!
//! Separate from `picker.rs` and `modal.rs`: it owns a `ThemeFile` and its
//! own edit list, which none of the other modals need. Opening it from
//! `/theme` and writing the file are the caller's (T46.7).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::glyph::Glyphs;
use crate::text::sanitize;
use crate::theme::{TOKENS, Theme, ThemeFile, TrueColorOverrides, format_color, parse_color};

/// Token rows drawn at once; the rest scroll, like the picker's list.
const VISIBLE: usize = 10;
/// Longer than any colour spelling `parse_color` accepts (`lightmagenta`).
const MAX_INPUT: usize = 16;

/// What a key asked of the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorOutcome {
    /// `Enter`/`Ctrl+S` with a valid input: write `edits` out.
    Save,
    /// `Esc`: drop every edit and restore what was drawn before.
    Revert,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeEditor {
    /// The theme's catalog name.
    pub name: String,
    /// A built-in cannot be written in place; the caller picks a new stem.
    pub builtin: bool,
    /// Which half of `[tokens]` is edited and previewed.
    pub dark: bool,
    pub file: ThemeFile,
    /// Index into [`TOKENS`].
    pub selected: usize,
    /// First token row on screen.
    top: usize,
    pub input: String,
    pub invalid: bool,
    /// Every accepted change, one entry per token, last value wins.
    pub edits: Vec<(&'static str, Color)>,
}

impl ThemeEditor {
    pub fn open(name: String, builtin: bool, dark: bool, file: ThemeFile) -> Self {
        let mut editor = Self {
            name,
            builtin,
            dark,
            file,
            selected: 0,
            top: 0,
            input: String::new(),
            invalid: false,
            edits: Vec::new(),
        };
        editor.move_to(0);
        editor
    }

    /// The preview: what the file draws with every accepted edit applied.
    pub fn theme(&self) -> Theme {
        self.file.theme(self.dark)
    }

    pub fn key(&mut self, key: KeyEvent) -> Option<EditorOutcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Some(EditorOutcome::Revert),
            KeyCode::Enter => return (!self.invalid).then_some(EditorOutcome::Save),
            KeyCode::Char('s') if ctrl => return (!self.invalid).then_some(EditorOutcome::Save),
            KeyCode::Up | KeyCode::BackTab => {
                self.move_to((self.selected + TOKENS.len() - 1) % TOKENS.len());
            }
            KeyCode::Down | KeyCode::Tab => self.move_to((self.selected + 1) % TOKENS.len()),
            KeyCode::Backspace => {
                self.input.pop();
                self.apply();
            }
            KeyCode::Char(c)
                if !ctrl
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && c.is_ascii_graphic()
                    && self.input.len() < MAX_INPUT =>
            {
                self.input.push(c);
                self.apply();
            }
            _ => {}
        }
        None
    }

    /// Moves the cursor, dropping an unparsed input for the value in use.
    fn move_to(&mut self, selected: usize) {
        self.selected = selected;
        if selected < self.top {
            self.top = selected;
        } else if selected >= self.top + VISIBLE {
            self.top = selected + 1 - VISIBLE;
        }
        self.input = format_color(self.theme().colors()[selected]);
        self.invalid = false;
    }

    fn apply(&mut self) {
        let Some(color) = parse_color(&self.input) else {
            self.invalid = true;
            return;
        };
        self.invalid = false;
        let token = TOKENS[self.selected];
        self.overrides().set(token, color);
        match self.edits.iter_mut().find(|(t, _)| *t == token) {
            Some(edit) => edit.1 = color,
            None => self.edits.push((token, color)),
        }
    }

    fn overrides(&mut self) -> &mut TrueColorOverrides {
        if self.dark {
            &mut self.file.dark
        } else {
            &mut self.file.light
        }
    }

    pub fn height(&self) -> u16 {
        u16::try_from(2 + VISIBLE.min(TOKENS.len())).unwrap_or(u16::MAX)
    }

    pub fn lines(&self, g: &Glyphs, theme: &Theme) -> Vec<Line<'static>> {
        // The ASCII set swaps the box-drawing rule for `-`; its swatch
        // follows, since a terminal that cannot draw `─` cannot draw `█`.
        let ascii = g.rule.is_ascii();
        let variant = if self.dark { "dark" } else { "light" };
        let mut lines = vec![Line::styled(
            format!(" theme {} {} {variant}", sanitize(&self.name), g.sep),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        let colors = self.theme().colors();
        let end = (self.top + VISIBLE).min(TOKENS.len());
        for i in self.top..end {
            let mine = i == self.selected;
            let cursor = if mine { g.cursor } else { " " };
            let mut spans = vec![
                Span::raw(format!(" {cursor} {:<12}", TOKENS[i])),
                Span::styled(
                    if ascii { "##" } else { "██" },
                    Style::default().fg(colors[i]),
                ),
            ];
            if mine {
                let value = format!(" {}{}", self.input, g.caret);
                spans.push(Span::styled(value, Style::default().fg(theme.selection)));
                if self.invalid {
                    spans.push(Span::styled(
                        "  invalid colour",
                        Style::default().fg(theme.error),
                    ));
                }
            } else {
                spans.push(Span::raw(format!(" {}", format_color(colors[i]))));
            }
            lines.push(Line::from(spans));
        }
        let arrows = if ascii { "Up/Down" } else { "↑↓" };
        let sep = g.sep;
        lines.push(Line::styled(
            format!(" {arrows} move {sep} Enter save {sep} Esc revert"),
            Style::default().fg(theme.dim),
        ));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::{ASCII, UNICODE};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::{Paragraph, Widget};

    fn press(editor: &mut ThemeEditor, code: KeyCode) -> Option<EditorOutcome> {
        editor.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_in(editor: &mut ThemeEditor, text: &str) {
        for c in text.chars() {
            press(editor, KeyCode::Char(c));
        }
    }

    fn clear(editor: &mut ThemeEditor) {
        while !editor.input.is_empty() {
            press(editor, KeyCode::Backspace);
        }
    }

    fn editor() -> ThemeEditor {
        ThemeEditor::open("cox-dark".into(), true, true, ThemeFile::default())
    }

    fn render(editor: &ThemeEditor, g: &Glyphs) -> String {
        let lines = editor.lines(g, &Theme::dark());
        let mut term = Terminal::new(TestBackend::new(48, editor.height())).expect("test terminal");
        term.draw(|f| Paragraph::new(lines).render(f.area(), f.buffer_mut()))
            .expect("draw");
        crate::view::buffer_to_string(term.backend().buffer())
    }

    #[test]
    fn editor_preview_follows_valid_input() {
        let mut editor = editor();
        press(&mut editor, KeyCode::Down);
        press(&mut editor, KeyCode::Down);
        assert_eq!(TOKENS[editor.selected], "accent");
        clear(&mut editor);
        type_in(&mut editor, "#ff0000");
        assert!(!editor.invalid);
        assert_eq!(editor.theme().accent, Color::Rgb(255, 0, 0));
        assert_eq!(editor.edits, vec![("accent", Color::Rgb(255, 0, 0))]);
        assert_eq!(
            press(&mut editor, KeyCode::Enter),
            Some(EditorOutcome::Save)
        );
    }

    #[test]
    fn editor_invalid_colour_is_flagged_and_not_applied() {
        let mut editor = editor();
        let before = editor.theme().text;
        clear(&mut editor);
        type_in(&mut editor, "zz");
        assert!(editor.invalid);
        assert_eq!(editor.theme().text, before);
        assert_eq!(press(&mut editor, KeyCode::Enter), None);
        assert!(render(&editor, &UNICODE).contains("invalid colour"));
    }

    #[test]
    fn editor_esc_reverts() {
        let mut editor = editor();
        assert_eq!(
            press(&mut editor, KeyCode::Esc),
            Some(EditorOutcome::Revert)
        );
    }

    #[test]
    fn editor_up_wraps_and_keeps_the_cursor_on_screen() {
        let mut editor = editor();
        press(&mut editor, KeyCode::Up);
        assert_eq!(TOKENS[editor.selected], "mode_bypass");
        assert!(render(&editor, &UNICODE).contains("mode_bypass"));
    }

    #[test]
    fn theme_editor_dark() {
        insta::assert_snapshot!(render(&editor(), &UNICODE));
    }

    #[test]
    fn theme_editor_ascii() {
        insta::assert_snapshot!(render(&editor(), &ASCII));
    }
}
