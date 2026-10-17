// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/refine` in the TUI (T66.8, P66): the command grammar, the review
//! overlay and its keys. The proposal, the files and the history belong to
//! `crates/cox` (the TUI never touches the disk or the store); this module
//! only carries a [`Request`] out and a [`Msg`] back. A separate module so
//! `state.rs` and `commands.rs` keep a few lines of glue each.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::cells::Look;
use crate::diff;
use crate::state::{Ask, Cmd};
use crate::text::sanitize;

/// One screen of a page, as in the `Ctrl+G` diff view.
const PAGE: usize = 10;

pub const USAGE: &str = "/refine <text> [--global] | list | undo [<event id>] [--global]";

/// What the user asked of the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A proposal for `text`. Local to this session unless `global`: the
    /// flag is the only way to write what every session reads.
    Propose { text: String, global: bool },
    /// The reviewed proposal is confirmed. Only the review overlay sends it.
    Apply,
    /// This session's refine history.
    List,
    /// Reverses `id`, or the newest applied refine of this session. A
    /// global event needs `global`.
    Undo { id: Option<String>, global: bool },
}

/// What the runtime answers on the feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A proposal to review: the model's reason and a git-style patch.
    Review { evidence: String, diff: String },
    /// Anything else: `Ok` is shown as information, `Err` as a warning.
    Notice(Result<String, String>),
}

/// `args` are the words after `/refine`. `list`, `undo` and the flag are
/// matched as whole words, so a request like "list the failing tests"
/// still proposes.
pub fn parse(args: &[String]) -> Result<Request, &'static str> {
    let global = args.iter().any(|a| a == "--global");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--global")
        .collect();
    match words.as_slice() {
        [] => Err(USAGE),
        ["list"] => Ok(Request::List),
        ["undo"] => Ok(Request::Undo { id: None, global }),
        ["undo", id] => Ok(Request::Undo {
            id: Some((*id).to_string()),
            global,
        }),
        text => Ok(Request::Propose {
            text: text.join(" "),
            global,
        }),
    }
}

/// The proposal on screen, scrolled from its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    evidence: String,
    diff: String,
    scroll: usize,
}

impl Review {
    /// Model text is untrusted, so it is sanitized once, here.
    pub fn new(evidence: &str, diff: &str) -> Self {
        Self {
            evidence: sanitize(evidence),
            diff: sanitize(diff),
            scroll: 0,
        }
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn line_count(&self) -> usize {
        self.diff.lines().count()
    }

    /// The overlay's rows: the reason, the keys, then every entry's diff.
    pub fn lines(&self, look: &Look) -> Vec<Line<'static>> {
        let look = Look {
            show_diffs: true,
            ..*look
        };
        let sep = look.glyphs.sep;
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut out = vec![
            Line::styled(format!(" refine {sep} {}", self.evidence), Style::default()),
            Line::styled(
                format!(" y applies {sep} n or Esc cancels {sep} PageUp/PageDown scroll"),
                dim,
            ),
        ];
        out.extend(
            diff::from_unified(&self.diff)
                .iter()
                .flat_map(|d| diff::lines(d, &look)),
        );
        out
    }

    pub fn wheel(&mut self, up: bool, lines: usize) {
        self.scroll = if up {
            self.scroll.saturating_sub(lines)
        } else {
            (self.scroll + lines).min(self.line_count().saturating_sub(1))
        };
    }

    /// `y` or `Enter` confirms, `n` or `Esc` drops the proposal, paging
    /// scrolls; any other key is ignored so a stray one never applies.
    pub fn key(mut self, key: KeyEvent) -> (Option<Self>, Vec<Cmd>) {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                (None, vec![Cmd::Ask(Ask::Refine(Request::Apply))])
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => (None, Vec::new()),
            KeyCode::PageDown => {
                self.wheel(false, PAGE);
                (Some(self), Vec::new())
            }
            KeyCode::PageUp => {
                self.wheel(true, PAGE);
                (Some(self), Vec::new())
            }
            _ => (Some(self), Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_refine_is_local_unless_the_global_flag_is_given() {
        assert_eq!(
            parse(&words("always run tests first")),
            Ok(Request::Propose {
                text: "always run tests first".into(),
                global: false,
            })
        );
        assert_eq!(
            parse(&words("--global always run tests first")),
            Ok(Request::Propose {
                text: "always run tests first".into(),
                global: true,
            })
        );
    }

    #[test]
    fn list_and_undo_are_whole_words_so_a_sentence_still_proposes() {
        assert_eq!(parse(&words("list")), Ok(Request::List));
        assert_eq!(
            parse(&words("undo")),
            Ok(Request::Undo {
                id: None,
                global: false,
            })
        );
        assert_eq!(
            parse(&words("undo abc --global")),
            Ok(Request::Undo {
                id: Some("abc".into()),
                global: true,
            })
        );
        assert!(matches!(
            parse(&words("list the failing tests")),
            Ok(Request::Propose { global: false, .. })
        ));
        assert_eq!(parse(&[]), Err(USAGE));
    }

    #[test]
    fn only_y_or_enter_applies_and_a_stray_key_keeps_the_review() {
        let review = Review::new(
            "why",
            "diff --git a/x b/x\n--- /dev/null\n+++ x\n@@ -0,0 +1 @@\n+a\n",
        );
        let key = |c| KeyEvent::from(c);
        let (kept, cmds) = review.clone().key(key(KeyCode::Char('q')));
        assert_eq!((kept.as_ref(), cmds), (Some(&review), Vec::new()));
        for apply in [KeyCode::Char('y'), KeyCode::Enter] {
            let (kept, cmds) = review.clone().key(key(apply));
            assert!(kept.is_none());
            assert_eq!(cmds, vec![Cmd::Ask(Ask::Refine(Request::Apply))]);
        }
        for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
            assert_eq!(review.clone().key(key(cancel)), (None, Vec::new()));
        }
    }
}
