// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `StyledDoc` (T37.7, DT§4.2): what markdown and syntax highlighting
//! produce before any UI draws them — blocks of runs tagged with the plugin
//! `StyleToken` roles, never a terminal style. Separate from `markdown` so a
//! non-terminal consumer (the desktop app through `cox-app`) can hold the
//! types without ratatui; the ratatui conversion lives in `markdown`, behind
//! the `ratatui` feature the TUI enables.

pub use cox_protocol::plugin::ui::StyleToken;
use serde::{Deserialize, Serialize};

/// A rendered markdown text: top-level blocks in order. A terminal puts one
/// blank line between blocks; a GUI spaces them as it likes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledDoc {
    pub blocks: Vec<Block>,
}

/// One row of runs.
pub type StyledLine = Vec<StyledSpan>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Prose. A heading's `#` run, a list item's marker and a quote's bars
    /// are not in the text: each surface draws them from `kind` and the
    /// line's own fields (A92).
    Text {
        kind: TextKind,
        lines: Vec<TextLine>,
    },
    /// A fenced or indented block, highlighted line by line.
    Code {
        lang: String,
        lines: Vec<StyledLine>,
    },
    /// Plain cell text, header row first; each surface lays out columns.
    Table { rows: Vec<Vec<String>> },
    /// A thematic break.
    Rule,
}

/// What a `Block::Text` started as — a hint for a GUI's spacing and font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    Paragraph,
    Heading(u8),
    List,
    Quote,
}

/// One line of a `Block::Text`: its runs and where it sits. A terminal
/// prints the bars and the marker in front of the runs; a GUI draws a bar
/// per quote and hangs the marker in a gutter. A heading's level is its
/// block's `TextKind::Heading`, drawn on the block's first line only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextLine {
    /// How many quotes the line sits in: one bar each.
    #[serde(skip_serializing_if = "is_default")]
    pub quote: u8,
    /// How deep in nested lists the line is, 0 for a top-level item and
    /// outside any list.
    #[serde(skip_serializing_if = "is_default")]
    pub depth: u8,
    /// A list item's bullet glyph or number (`3.`) on the item's first
    /// line; empty on any other line.
    #[serde(skip_serializing_if = "is_default")]
    pub marker: String,
    pub spans: StyledLine,
}

/// A run of text with one style.
///
/// Serialized fields left at their default are omitted, so a desktop
/// fixture (DT§8) of a streamed reply stays readable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyledSpan {
    pub text: String,
    /// Its colour role.
    #[serde(skip_serializing_if = "is_default")]
    pub token: StyleToken,
    /// A syntect theme colour for highlighted code; wins over `token`. One
    /// highlighter and one theme serve every surface (DT§4.2).
    #[serde(skip_serializing_if = "is_default")]
    pub rgb: Option<[u8; 3]>,
    /// The same run's colour under the theme's light variant, set where a
    /// surface picks by the system appearance (a diff, A95); `rgb` then
    /// holds the dark variant's.
    #[serde(skip_serializing_if = "is_default")]
    pub light: Option<[u8; 3]>,
    #[serde(skip_serializing_if = "is_default")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub strike: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub underline: bool,
    /// An `http(s)` target. Set only on runs whose drawn text *is* the URL
    /// (T23.3), so a link never hides where it goes behind other words.
    #[serde(skip_serializing_if = "is_default")]
    pub link: Option<String>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

impl StyledSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

impl StyledDoc {
    /// Every block as Markdown, joined by a blank line. A streamed reply
    /// arrives as doc blocks, not source, so this is the one writer Copy and
    /// the clients share instead of each keeping its own (T58.4.26).
    pub fn markdown(&self) -> String {
        let blocks: Vec<String> = self.blocks.iter().filter_map(Block::markdown).collect();
        blocks.join("\n\n")
    }
}

impl Block {
    /// This block as Markdown; `None` for a table without rows, which has
    /// nothing to write.
    pub fn markdown(&self) -> Option<String> {
        match self {
            Self::Text { kind, lines } => {
                let lines: Vec<String> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| text_line(line, *kind, i == 0))
                    .collect();
                Some(lines.join("\n"))
            }
            Self::Code { lang, lines } => {
                let body: Vec<String> = lines
                    .iter()
                    .map(|l| l.iter().map(|s| s.text.as_str()).collect())
                    .collect();
                Some(fence(lang, &body.join("\n")))
            }
            Self::Table { rows } => {
                let head = rows.first()?;
                let row = |cells: &[String]| format!("| {} |", cells.join(" | "));
                let rule = vec!["---".to_string(); head.len()];
                let mut out = vec![row(head), row(&rule)];
                out.extend(rows[1..].iter().map(|r| row(r)));
                Some(out.join("\n"))
            }
            Self::Rule => Some("---".to_string()),
        }
    }
}

/// A text line as Markdown: its quotes as `>`, then an item's depth indent
/// and number or `-`, or a heading's `#` run on its first line; a list line
/// that goes on an item is indented under it.
fn text_line(line: &TextLine, kind: TextKind, first: bool) -> String {
    let mut head = "> ".repeat(usize::from(line.quote));
    let mut bold_marks = true;
    if !line.marker.is_empty() {
        let number = line.marker.chars().next().is_some_and(char::is_numeric);
        head.push_str(&"  ".repeat(usize::from(line.depth)));
        head.push_str(if number { &line.marker } else { "-" });
        head.push(' ');
    } else if kind == TextKind::List {
        head.push_str(&"  ".repeat(usize::from(line.depth) + 1));
    } else if let TextKind::Heading(level) = kind {
        if first {
            head.push_str(&"#".repeat(usize::from(level)));
            head.push(' ');
        }
        // A heading is bold by its level, so its spans' bold marks would
        // only double it.
        bold_marks = false;
    }
    head + &line
        .spans
        .iter()
        .map(|s| span(s, bold_marks))
        .collect::<String>()
}

/// A span's text with its bold, italic and strike marks; whitespace stays
/// bare because a mark around it would not parse.
fn span(span: &StyledSpan, bold_marks: bool) -> String {
    if span.text.chars().all(char::is_whitespace) {
        return span.text.clone();
    }
    let mut text = span.text.clone();
    if span.bold && bold_marks {
        text = format!("**{text}**");
    }
    if span.italic {
        text = format!("_{text}_");
    }
    if span.strike {
        text = format!("~~{text}~~");
    }
    text
}

/// `body` fenced as `lang`, with a fence longer than any backtick run
/// inside it so the body cannot close it early.
fn fence(lang: &str, body: &str) -> String {
    let (mut longest, mut run) = (0, 0);
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let ticks = "`".repeat((longest + 1).max(3));
    format!("{ticks}{lang}\n{body}\n{ticks}")
}

#[cfg(test)]
mod tests {
    use super::{Block, TextKind};
    use crate::glyph::UNICODE;
    use crate::markdown::{parse, theme_name};

    const FIXTURE: &str = "# Title\n\nSome `code` and **bold [docs](https://x.dev/a)** at \
        <https://x.dev/b>.\n\n## Steps\n\n- one\n- two with `ls`\n  - nested\n\n\
        1. first\n2. second\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n---\n\n\
        | a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn markdown_parses_into_tagged_blocks_without_a_terminal() {
        insta::assert_debug_snapshot!(parse(FIXTURE, theme_name(true, ""), &UNICODE));
    }

    #[test]
    fn only_a_run_that_shows_its_url_carries_the_link() {
        let doc = parse("[docs](https://x.dev/a)", theme_name(true, ""), &UNICODE);
        let links: Vec<_> = doc
            .blocks
            .iter()
            .flat_map(|b| match b {
                super::Block::Text { lines, .. } => {
                    lines.iter().flat_map(|l| l.spans.clone()).collect()
                }
                _ => Vec::new(),
            })
            .filter_map(|s| s.link.map(|l| (s.text, l)))
            .collect();
        let url = "https://x.dev/a".to_string();
        assert_eq!(links, [(url.clone(), url)]);
    }

    /// Each text line as (kind, quote, depth, marker, its runs' text).
    fn lines(markdown: &str) -> Vec<(TextKind, u8, u8, String, String)> {
        let doc = parse(markdown, theme_name(true, ""), &UNICODE);
        doc.blocks
            .into_iter()
            .flat_map(|b| match b {
                Block::Text { kind, lines } => lines
                    .into_iter()
                    .map(|l| {
                        let text = l.spans.iter().map(|s| s.text.as_str()).collect();
                        (kind, l.quote, l.depth, l.marker, text)
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn headings_quotes_and_items_carry_level_and_marker_apart_from_their_text() {
        let s = String::from;
        assert_eq!(
            lines("## Plan\n\n> outer\n>\n> > inner\n\n- one\n  - two\n\n3. three"),
            [
                (TextKind::Heading(2), 0, 0, s(""), s("Plan")),
                (TextKind::Quote, 1, 0, s(""), s("outer")),
                (TextKind::Quote, 2, 0, s(""), s("inner")),
                (TextKind::List, 0, 0, s("•"), s("one")),
                (TextKind::List, 0, 1, s("•"), s("two")),
                (TextKind::List, 0, 0, s("3."), s("three")),
            ]
        );
    }

    #[test]
    fn a_list_in_a_quote_keeps_its_bars_and_the_quote_resumes_after_it() {
        let s = String::from;
        assert_eq!(
            lines("> - item\n>\n> after"),
            [
                (TextKind::List, 1, 0, s("•"), s("item")),
                (TextKind::Quote, 1, 0, s(""), s("after")),
            ]
        );
    }

    fn code(body: &str) -> Block {
        Block::Code {
            lang: "rs".into(),
            lines: body
                .lines()
                .map(|l| vec![super::StyledSpan::plain(l)])
                .collect(),
        }
    }

    #[test]
    fn a_fence_outgrows_the_longest_backtick_run() {
        assert_eq!(
            code("a ``` b\n````").markdown().as_deref(),
            Some("`````rs\na ``` b\n````\n`````")
        );
        assert_eq!(
            code("plain").markdown().as_deref(),
            Some("```rs\nplain\n```")
        );
    }

    #[test]
    fn a_nested_list_keeps_its_depth() {
        let doc = parse("- one\n  - two\n\n3. three", theme_name(true, ""), &UNICODE);
        assert_eq!(doc.markdown(), "- one\n  - two\n\n3. three");
    }

    #[test]
    fn headings_quotes_tables_and_marks_write_their_markdown() {
        let doc = parse(
            "## Plan\n\n> quoted **b** *i*\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---",
            theme_name(true, ""),
            &UNICODE,
        );
        assert_eq!(
            doc.markdown(),
            "## Plan\n\n> quoted **b** _i_\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n---"
        );
        assert_eq!(Block::Table { rows: vec![] }.markdown(), None);
    }
}
