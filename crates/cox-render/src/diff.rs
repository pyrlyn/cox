// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Diff view (T5.4, T24.5): a `ToolResult.diff` as coloured unified-diff
//! lines under a per-file `± path  +n −m` header, collapsible to that header
//! alone. A replaced line pair shows which words changed; a viewport of at
//! least `SIDE_MIN_WIDTH` columns splits old and new into two panes. The
//! edit card, the approval modal and `Ctrl+G` all print through `lines`.
//! Separate from `cells` because pairing and pane fitting are their own
//! small machine and more than one surface prints a diff; the hunk parse,
//! the pairing and the word diff are `diffmodel`'s, which the desktop app
//! shares.

use std::ops::Range;

use cox_protocol::types::Diff;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::Look;
use crate::diffmodel::{Aligned, Row, align, parse, replaced};
pub use crate::diffstat::counts;
use crate::markdown;

/// Below this many columns a pane is too narrow to read a line of code, so
/// the diff stays stacked whatever `tui.diff` says.
pub const SIDE_MIN_WIDTH: u16 = 120;

/// `tui.diff`: whether a wide viewport may split a diff into two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Side by side from `SIDE_MIN_WIDTH` columns, stacked below.
    #[default]
    Auto,
    /// Same threshold as `Auto`; named so a config can say what it wants.
    Side,
    /// Never split.
    Stacked,
}

impl Mode {
    /// An unknown value is `auto`: a typo costs a layout choice, not a start.
    pub fn parse(s: &str) -> Self {
        match s {
            "side" => Self::Side,
            "stacked" => Self::Stacked,
            _ => Self::Auto,
        }
    }
}

/// How the hunks are laid out; `left`/`right` are the panes' columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Stacked,
    Side { left: usize, right: usize },
}

impl Layout {
    pub fn pick(mode: Mode, width: u16) -> Self {
        if mode == Mode::Stacked || width < SIDE_MIN_WIDTH {
            return Self::Stacked;
        }
        // Two columns of indent (as stacked lines have) and one `│`.
        let usable = usize::from(width) - 3;
        let left = usable / 2;
        Self::Side {
            left,
            right: usable - left,
        }
    }
}

/// Appends `text` to the last span when it has the same style, so a word
/// diff is a handful of spans rather than one per token.
fn push(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    match spans.last_mut() {
        _ if text.is_empty() => {}
        Some(last) if last.style == style => last.content.to_mut().push_str(text),
        _ => spans.push(Span::styled(text.to_string(), style)),
    }
}

/// One side of a replaced pair: its changed `words` in `changed` (the
/// add/remove colour) and the words both lines share dim.
fn marked(text: &str, words: &[Range<usize>], changed: Style) -> Vec<Span<'static>> {
    let same = Style::default().add_modifier(Modifier::DIM);
    let (mut spans, mut at) = (Vec::new(), 0);
    for w in words {
        push(&mut spans, text.get(at..w.start).unwrap_or_default(), same);
        push(&mut spans, text.get(w.clone()).unwrap_or_default(), changed);
        at = w.end;
    }
    push(&mut spans, text.get(at..).unwrap_or_default(), same);
    spans
}

/// The style of a whole line by its first characters.
fn line_style(l: &str, look: &Look) -> Style {
    if l.starts_with("+++") || l.starts_with("---") || l.starts_with('\\') {
        Style::default().add_modifier(Modifier::DIM)
    } else if l.starts_with("@@") {
        Style::default().fg(look.colors.diff_hunk)
    } else if l.starts_with('+') {
        Style::default().fg(look.colors.diff_add)
    } else if l.starts_with('-') {
        Style::default().fg(look.colors.diff_del)
    } else {
        Style::default()
    }
}

/// `spans` cut or padded to exactly `width` columns; tabs become four
/// spaces first so a pane's width is what the terminal draws.
fn fit(spans: Vec<Span<'static>>, width: usize, ellipsis: &'static str) -> Vec<Span<'static>> {
    let spans: Vec<Span<'static>> = spans
        .into_iter()
        .map(|s| Span::styled(s.content.replace('\t', "    "), s.style))
        .collect();
    let total: usize = spans.iter().map(|s| s.content.width()).sum();
    let room = match total > width {
        true => width.saturating_sub(ellipsis.width()),
        false => width,
    };
    let (mut out, mut used) = (Vec::new(), 0);
    'spans: for s in spans {
        let mut text = String::new();
        for c in s.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > room {
                out.push(Span::styled(text, s.style));
                break 'spans;
            }
            used += w;
            text.push(c);
        }
        out.push(Span::styled(text, s.style));
    }
    if total > width {
        out.push(Span::raw(ellipsis));
        used += ellipsis.width();
    }
    out.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    out
}

/// The hunks of `diff` (no header) in `layout`. Bodies go through the same
/// syntect pass as a fenced block, highlighted by the patched file's
/// extension; a replaced pair shows its word diff instead. The add/remove
/// colour stays on the marker column so a theme can never hide what a
/// line does.
pub fn render(diff: &Diff, look: &Look, layout: Layout) -> Vec<Line<'static>> {
    let (rows, texts) = parse(&diff.unified);
    let aligned = align(&rows);
    let mut bodies: Vec<Option<Vec<Span<'static>>>> =
        match diff.path.extension().and_then(|e| e.to_str()) {
            Some(token) => markdown::highlight(token, &texts, look.theme)
                .into_iter()
                .map(|l| Some(l.spans))
                .collect(),
            None => Vec::new(),
        };
    bodies.resize(texts.len(), None);
    let del = Style::default().fg(look.colors.diff_del);
    let add = Style::default().fg(look.colors.diff_add);
    for pair in replaced(&rows, &aligned, &texts) {
        (bodies[pair.old], bodies[pair.new]) = match pair.words {
            Some((o, n)) => (
                Some(marked(texts[pair.old], &o, del)),
                Some(marked(texts[pair.new], &n, add)),
            ),
            None => (None, None),
        };
    }
    // A content line: `indent` and its marker in the line colour, then its
    // body spans, or the whole line in the line colour when there are none.
    let body_line = |indent: &str, raw: &str, marker: &str, body: usize| -> Vec<Span<'static>> {
        let style = line_style(raw, look);
        match &bodies[body] {
            Some(spans) => {
                let mut out = vec![Span::styled(format!("{indent}{marker}"), style)];
                out.extend(spans.iter().cloned());
                out
            }
            None => vec![Span::styled(format!("{indent}{raw}"), style)],
        }
    };
    let Layout::Side { left, right } = layout else {
        return rows
            .iter()
            .map(|r| match r {
                Row::Meta(l) => Line::styled(format!("  {l}"), line_style(l, look)),
                Row::Body {
                    raw, marker, body, ..
                } => Line::from(body_line("  ", raw, marker, *body)),
            })
            .collect();
    };
    let last = rows.iter().fold(0, |m, r| match r {
        Row::Body { old, new, .. } => m.max(old.unwrap_or(0)).max(new.unwrap_or(0)),
        Row::Meta(_) => m,
    });
    let digits = last.to_string().len();
    let gutter = Style::default().fg(look.colors.dim);
    let ellipsis = look.glyphs.ellipsis;
    // One pane: `  12 +body…`, exactly `width` columns, blank when absent.
    let pane = |row: Option<usize>, old_side: bool, width: usize| -> Vec<Span<'static>> {
        let Some(Row::Body {
            raw,
            marker,
            body,
            old,
            new,
        }) = row.and_then(|i| rows.get(i))
        else {
            return vec![Span::raw(" ".repeat(width))];
        };
        let no = if old_side { old } else { new };
        let no = no.map_or(String::new(), |n| n.to_string());
        let mut spans = vec![Span::styled(format!("{no:>digits$} "), gutter)];
        spans.extend(body_line("", raw, marker, *body));
        fit(spans, width, ellipsis)
    };
    aligned
        .iter()
        .map(|a| match *a {
            Aligned::Meta(i) => {
                let l = match rows.get(i) {
                    Some(Row::Meta(l)) => *l,
                    _ => "",
                };
                let width = left + right + 1;
                let mut spans = vec![Span::raw("  ")];
                spans.extend(fit(
                    vec![Span::styled(l.to_string(), line_style(l, look))],
                    width,
                    ellipsis,
                ));
                Line::from(spans)
            }
            Aligned::Pair(l, r) => {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(pane(l, true, left));
                spans.push(Span::styled("│", gutter));
                spans.extend(pane(r, false, right));
                Line::from(spans)
            }
        })
        .collect()
}

/// The header, plus the hunks when `look.show_diffs`, laid out for
/// `look.width` and `look.diff`.
pub fn lines(diff: &Diff, look: &Look) -> Vec<Line<'static>> {
    let (added, removed) = counts(&diff.unified);
    let g = look.glyphs;
    let mut out = vec![Line::styled(
        format!(
            "  {} {}  +{added} {}{removed}",
            g.diff,
            diff.path.display(),
            g.minus
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if look.show_diffs {
        out.extend(render(diff, look, Layout::pick(look.diff, look.width)));
    }
    out
}

/// A whole-tree patch split at its `diff --git a/x b/y` headers into the
/// per-file `Diff`s `lines` already renders (T15.3). The path is `y`; the
/// `index`/mode lines stay in the text, where `lines` prints them plain.
pub fn from_unified(text: &str) -> Vec<Diff> {
    let mut out: Vec<Diff> = Vec::new();
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let path = rest.rsplit_once(" b/").map_or(rest, |(_, p)| p);
            out.push(Diff {
                path: path.into(),
                unified: String::new(),
            });
        } else if let Some(d) = out.last_mut() {
            d.unified.push_str(l);
            d.unified.push('\n');
        }
    }
    out
}

/// The `Ctrl+G` view: a key line, then every file's headed block, or
/// `no changes`. Always expanded, whatever `Ctrl+O` last did.
pub fn view_lines(text: &str, look: &Look) -> Vec<Line<'static>> {
    let look = Look {
        show_diffs: true,
        ..*look
    };
    let sep = look.glyphs.sep;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut out = vec![Line::styled(
        format!(" git diff HEAD {sep} PageUp/PageDown scroll {sep} Esc closes"),
        dim,
    )];
    let files = from_unified(text);
    if files.is_empty() {
        out.push(Line::styled("  no changes", dim));
    }
    out.extend(files.iter().flat_map(|d| lines(d, &look)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diffmodel::WORD_DIFF_CAP;

    #[test]
    fn counts_skip_file_markers() {
        let d = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n+more\n context\n";
        assert_eq!(counts(d), (2, 1));
    }

    fn look() -> Look {
        Look {
            width: 80,
            theme: markdown::theme_name(true, ""),
            glyphs: crate::glyph::UNICODE,
            show_thinking: false,
            show_diffs: true,
            diff: Mode::Auto,
            tick: 0,
            still: false,
            marks: false,
            colors: crate::theme::Theme::dark(),
            expand_last: None,
        }
    }

    #[test]
    fn an_added_hunk_body_is_highlighted_under_a_coloured_marker() {
        let d = Diff {
            path: "src/x.rs".into(),
            unified: "@@ -1 +1,2 @@\n let x = 1;\n+fn main() {}\n".to_string(),
        };
        let out = lines(&d, &look());
        let added = &out[3];
        assert_eq!(added.to_string(), "  +fn main() {}");
        assert_eq!(added.spans[0].style.fg, Some(look().colors.diff_add));
        // `fn` is a keyword: syntect split the body into spans of its own.
        assert!(added.spans.len() > 2, "{:?}", added.spans);
    }

    #[test]
    fn a_pair_past_the_word_diff_cap_keeps_its_line_colour() {
        let long = "x".repeat(WORD_DIFF_CAP + 1);
        let d = Diff {
            path: "NOTES".into(),
            unified: format!("@@ -1 +1 @@\n-{long}\n+{long}y\n"),
        };
        let out = lines(&d, &look());
        assert_eq!(out[3].spans.len(), 1, "{:?}", out[3].spans);
        assert_eq!(out[3].spans[0].style.fg, Some(look().colors.diff_add));
    }

    #[test]
    fn fit_cuts_with_an_ellipsis_and_pads_to_the_exact_width() {
        let w = |spans: &[Span<'_>]| spans.iter().map(|s| s.content.width()).sum::<usize>();
        let cut = fit(vec![Span::raw("abcdef\tg")], 5, "…");
        assert_eq!(w(&cut), 5);
        assert_eq!(Line::from(cut).to_string(), "abcd…");
        let padded = fit(vec![Span::raw("ab")], 5, "…");
        assert_eq!(Line::from(padded).to_string(), "ab   ");
    }

    #[test]
    fn layout_is_side_only_from_120_columns_and_never_when_stacked() {
        assert_eq!(Layout::pick(Mode::Auto, 119), Layout::Stacked);
        assert_eq!(
            Layout::pick(Mode::Side, 121),
            Layout::Side {
                left: 59,
                right: 59
            }
        );
        assert_eq!(Layout::pick(Mode::Stacked, 200), Layout::Stacked);
        assert_eq!(Mode::parse("bogus"), Mode::Auto);
    }

    #[test]
    fn from_unified_splits_a_patch_at_its_headers_and_keeps_hunks() {
        let patch = "diff --git a/src/x.rs b/src/x.rs\nindex 1..2 100644\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/new file b/new file\nnew file mode 100644\n--- /dev/null\n+++ b/new file\n@@ -0,0 +1 @@\n+hello\n";
        let files = from_unified(patch);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, std::path::Path::new("src/x.rs"));
        assert!(files[0].unified.starts_with("index 1..2"));
        assert_eq!(counts(&files[0].unified), (1, 1));
        assert_eq!(files[1].path, std::path::Path::new("new file"));
        assert_eq!(counts(&files[1].unified), (1, 0));
        assert!(from_unified("").is_empty());
    }

    #[test]
    fn a_diff_of_an_unknown_file_type_stays_plain() {
        let d = Diff {
            path: "NOTES".into(),
            unified: "@@ -1 +1 @@\n+hello\n".to_string(),
        };
        let out = lines(&d, &look());
        assert_eq!(out[2].to_string(), "  +hello");
        assert_eq!(out[2].spans.len(), 1);
    }
}
