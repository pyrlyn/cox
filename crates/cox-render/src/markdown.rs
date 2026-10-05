// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Markdown → `StyledDoc` (T5.3, T37.7): pulldown-cmark events become runs
//! tagged with `StyleToken` roles, fenced code goes through syntect. The
//! ratatui lines (`render`, `highlight`, aligned tables) are a thin layout
//! over that doc behind the `ratatui` feature. Separate from the cells so an
//! assistant reply and a compaction summary render through one path and a
//! test can check the mapping on a string.

use std::path::Path;
use std::sync::{LazyLock, OnceLock};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
#[cfg(feature = "ratatui")]
use ratatui::style::{Color, Modifier, Style};
#[cfg(feature = "ratatui")]
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
#[cfg(feature = "ratatui")]
use unicode_width::UnicodeWidthStr;

#[cfg(feature = "ratatui")]
use crate::Look;
use crate::doc::{Block, StyleToken, StyledDoc, StyledLine, StyledSpan, TextKind, TextLine};
use crate::glyph::Glyphs;

static SYNTAXES: LazyLock<syntect::parsing::SyntaxSet> =
    LazyLock::new(two_face::syntax::extra_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);
/// `.tmTheme` files under `~/.cox/themes/` (T24.2 step 4), merged on top of
/// the bundled set by `load_user_themes` once at startup; empty until then,
/// same as an unconfigured `tui.syntax_theme`.
static USER_THEMES: OnceLock<ThemeSet> = OnceLock::new();

/// Reads every `.tmTheme` in `dir` into `USER_THEMES`, so a name that
/// collides with a bundled theme picks the user's file. Fails open: a
/// missing `dir` or any file `syntect` rejects leaves the bundled set as
/// the whole story, never a startup error. Idempotent — a second call
/// (e.g. from a test) is a no-op, matching `glyph`'s "decided once" leak.
pub fn load_user_themes(dir: &Path) {
    if let Ok(set) = ThemeSet::load_from_folder(dir) {
        let _ = USER_THEMES.set(set);
    }
}

/// The syntect theme to highlight with: `tui.syntax_theme` when it names one
/// of syntect's bundled or user (T24.2) themes, otherwise the `tui.theme`
/// default (`auto` reads as dark because most terminals are).
pub fn theme_name(dark: bool, chosen: &'static str) -> &'static str {
    if known(chosen) {
        return chosen;
    }
    if dark {
        "base16-ocean.dark"
    } else {
        "base16-ocean.light"
    }
}

/// Whether `name` is a bundled or user (T24.2) theme.
fn known(name: &str) -> bool {
    let has = |set: &ThemeSet| set.themes.contains_key(name);
    has(&THEMES) || USER_THEMES.get().is_some_and(has)
}

/// `chosen`'s dark and light variants, for a surface that follows the
/// system appearance instead of one terminal background (A95): a theme named
/// `….dark`/`….light` or `… (dark)`/`… (light)` pairs with its sibling when
/// that is known too, any other known theme serves both, and an unknown one
/// falls back to `theme_name`'s defaults.
pub fn theme_variants(chosen: &str) -> [String; 2] {
    if !known(chosen) {
        return [theme_name(true, ""), theme_name(false, "")].map(str::to_owned);
    }
    let sibling = |from: &str, to: &str| {
        [(".", ""), (" (", ")")]
            .into_iter()
            .find_map(|(open, close)| {
                let stem = chosen.strip_suffix(&format!("{open}{from}{close}"))?;
                Some(format!("{stem}{open}{to}{close}")).filter(|name| known(name))
            })
    };
    [
        sibling("light", "dark").unwrap_or_else(|| chosen.to_owned()),
        sibling("dark", "light").unwrap_or_else(|| chosen.to_owned()),
    ]
}

/// The bundled plus user (T24.2) theme names, for `cox`'s startup warning
/// about an unknown `tui.syntax_theme` — a bad name falls back, it never
/// fails the session.
pub fn themes() -> Vec<String> {
    let mut names: Vec<String> = THEMES.themes.keys().cloned().collect();
    if let Some(user) = USER_THEMES.get() {
        names.extend(user.themes.keys().cloned());
    }
    names
}

/// Parses `text` into a `StyledDoc`: code blocks highlight with the syntect
/// `theme`, list bullets come from `glyphs`.
pub fn parse(text: &str, theme: &str, glyphs: &Glyphs) -> StyledDoc {
    let mut r = Renderer {
        theme,
        glyphs: *glyphs,
        ..Renderer::default()
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(text, opts) {
        r.event(ev);
    }
    r.flush();
    StyledDoc { blocks: r.blocks }
}

/// Renders `text` as lines, unwrapped; trailing blank lines are dropped so a
/// streaming reply never shows a gap under its last paragraph.
#[cfg(feature = "ratatui")]
pub fn render(text: &str, look: &Look) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for block in parse(text, look.theme, &look.glyphs).blocks {
        match block {
            Block::Text { kind, lines } => {
                for (i, line) in lines.iter().enumerate() {
                    let kind = if i == 0 { kind } else { TextKind::Paragraph };
                    out.push(to_line(&prefixed(line, kind, &look.glyphs)));
                }
            }
            Block::Code { lines, .. } => out.extend(lines.iter().map(to_line)),
            Block::Table { rows } => {
                out.extend(table_lines(&rows, &look.glyphs, usize::from(look.width)));
            }
            Block::Rule => out.push(Line::styled(
                look.glyphs.rule.repeat(3),
                Style::default().add_modifier(Modifier::DIM),
            )),
        }
        out.push(Line::default());
    }
    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

#[derive(Default)]
struct Renderer<'a> {
    theme: &'a str,
    glyphs: Glyphs,
    blocks: Vec<Block>,
    /// Whether the last block is a `Block::Text` still taking lines.
    open: bool,
    /// What the next `Block::Text` opens as; `None` reads as a paragraph.
    kind: Option<TextKind>,
    cur: StyledLine,
    /// The next line is flushed even with no runs: a heading's or an item's
    /// first line, which a terminal still prints for its `#` run or marker.
    due: bool,
    /// The open item's depth and marker, until its first line is flushed.
    item: Option<(u8, String)>,
    /// Inherited run style, innermost last; its `text` is unused.
    pens: Vec<StyledSpan>,
    /// Next number per open ordered list (`None` for bullets), innermost last.
    lists: Vec<Option<u64>>,
    /// Open fenced block: language and body so far.
    code: Option<(String, String)>,
    /// Open table: rows so far and the cell being filled.
    table: Option<(Vec<Vec<String>>, String)>,
    quote: usize,
    /// Open `http(s)` link (T23.3): its URL and where its text starts in `cur`.
    link: Option<(String, usize)>,
}

impl Renderer<'_> {
    fn run(&self, text: impl Into<String>) -> StyledSpan {
        StyledSpan {
            text: text.into(),
            ..self.pens.last().cloned().unwrap_or_default()
        }
    }

    fn push(&mut self, set: fn(&mut StyledSpan)) {
        let mut pen = self.run("");
        set(&mut pen);
        self.pens.push(pen);
    }

    fn text(&mut self, s: &str) {
        if let Some((_, body)) = &mut self.code {
            body.push_str(s);
        } else if let Some((_, cell)) = &mut self.table {
            cell.push_str(s);
        } else {
            self.cur.push(self.run(s));
        }
    }

    /// T23.3: the URL is what becomes a hyperlink, so it is always on
    /// screen — an autolink is its own text, any other link gets ` (url)`.
    /// A link cannot hide where it goes behind friendlier words.
    fn end_link(&mut self) {
        let Some((url, at)) = self.link.take() else {
            return;
        };
        let text: String = self
            .cur
            .get(at..)
            .unwrap_or_default()
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        if text == url {
            for span in self.cur.iter_mut().skip(at) {
                span.link = Some(url.clone());
            }
            return;
        }
        let mut target = self.run(url.clone());
        target.link = Some(url);
        self.cur.push(self.run(" ("));
        self.cur.push(target);
        self.cur.push(self.run(")"));
    }

    fn flush(&mut self) {
        if self.cur.is_empty() && !self.due {
            return;
        }
        self.due = false;
        let depth = small(self.lists.len().saturating_sub(1));
        let (depth, marker) = self.item.take().unwrap_or((depth, String::new()));
        let line = TextLine {
            quote: small(self.quote),
            depth,
            marker,
            spans: std::mem::take(&mut self.cur),
        };
        if !self.open {
            let kind = self.kind.unwrap_or(TextKind::Paragraph);
            self.blocks.push(Block::Text {
                kind,
                lines: Vec::new(),
            });
            self.open = true;
        }
        if let Some(Block::Text { lines, .. }) = self.blocks.last_mut() {
            lines.push(line);
        }
    }

    /// Ends the open line and the block it belongs to.
    fn blank(&mut self) {
        self.flush();
        self.open = false;
    }

    /// A block laid out whole; its start already ended the one before it.
    fn close_with(&mut self, block: Block) {
        self.blocks.push(block);
        self.open = false;
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => self.text(&t),
            Event::Code(c) if self.table.is_some() => self.text(&c),
            Event::Code(c) => {
                let token = StyleToken::Accent;
                self.cur.push(StyledSpan {
                    token,
                    ..self.run(c.into_string())
                });
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.blank();
                self.close_with(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.blank();
                self.kind = Some(TextKind::Heading(level as u8));
                self.push(|p| p.bold = true);
                self.due = true;
            }
            Tag::Paragraph if self.lists.is_empty() && self.quote == 0 => {
                self.kind = Some(TextKind::Paragraph);
            }
            Tag::BlockQuote(..) => {
                self.blank();
                self.quote += 1;
                if self.quote == 1 {
                    self.kind = Some(TextKind::Quote);
                }
            }
            Tag::CodeBlock(kind) => {
                self.blank();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.blank();
                    self.kind = Some(TextKind::List);
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => self.glyphs.bullet.to_string(),
                };
                self.item = Some((small(depth), marker));
                self.due = true;
            }
            Tag::Emphasis => self.push(|p| p.italic = true),
            Tag::Strong => self.push(|p| p.bold = true),
            Tag::Strikethrough => self.push(|p| p.strike = true),
            Tag::Link { dest_url, .. } => {
                self.push(|p| p.underline = true);
                if dest_url.starts_with("https://") || dest_url.starts_with("http://") {
                    self.link = Some((dest_url.into_string(), self.cur.len()));
                }
            }
            Tag::Table(_) => {
                self.blank();
                self.table = Some((Vec::new(), String::new()));
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some((rows, _)) = &mut self.table {
                    rows.push(Vec::new());
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.pens.pop();
                self.blank();
                // What follows is not the heading: only its first line takes the `#` run.
                self.kind = None;
            }
            // Inside a list or quote a paragraph break is just a line break.
            TagEnd::Paragraph if self.lists.is_empty() && self.quote == 0 => self.blank(),
            TagEnd::Paragraph | TagEnd::Item => self.flush(),
            TagEnd::BlockQuote(..) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                self.blank();
            }
            TagEnd::CodeBlock => {
                if let Some((lang, body)) = self.code.take() {
                    let rows: Vec<&str> = body.lines().collect();
                    let lines = highlight_runs(&lang, &rows, self.theme);
                    self.close_with(Block::Code { lang, lines });
                }
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                    // A quote the list sat in goes on as a quote.
                    self.kind = (self.quote > 0).then_some(TextKind::Quote);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.pens.pop();
            }
            TagEnd::Link => {
                self.pens.pop();
                self.end_link();
            }
            TagEnd::TableCell => {
                if let Some((rows, cell)) = &mut self.table {
                    let c = std::mem::take(cell);
                    if let Some(row) = rows.last_mut() {
                        row.push(c);
                    }
                }
            }
            TagEnd::Table => {
                if let Some((rows, _)) = self.table.take() {
                    self.close_with(Block::Table { rows });
                }
            }
            _ => {}
        }
    }
}

/// A depth or count as a `TextLine` field; nesting past 255 saturates.
fn small(n: usize) -> u8 {
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// `line` as a terminal prints it (T5.3): its quote bars, then its item's
/// indent and marker or, on a heading's first line, its `#` run, then its
/// runs. A GUI draws these itself (A92).
#[cfg(feature = "ratatui")]
fn prefixed(line: &TextLine, kind: TextKind, glyphs: &Glyphs) -> StyledLine {
    let mut out = Vec::with_capacity(line.spans.len() + 2);
    if line.quote > 0 {
        let bars = format!("{} ", glyphs.quote).repeat(usize::from(line.quote));
        let token = StyleToken::Dim;
        out.push(StyledSpan {
            token,
            ..StyledSpan::plain(bars)
        });
    }
    if !line.marker.is_empty() {
        let indent = "  ".repeat(usize::from(line.depth));
        out.push(StyledSpan::plain(format!("{indent}{} ", line.marker)));
    }
    if let TextKind::Heading(level) = kind {
        let hashes = "#".repeat(usize::from(level));
        out.push(StyledSpan {
            bold: true,
            ..StyledSpan::plain(format!("{hashes} "))
        });
    }
    out.extend(line.spans.iter().cloned());
    out
}

/// `rows` through syntect, as one run so a multi-line string or comment
/// keeps its state. `token` is a language name or a file extension; an
/// unknown one, or a theme missing from the bundle, falls back to plain text
/// rather than failing. Shared by fenced blocks, file-shaped tool output and
/// diff hunks, so all three highlight identically.
pub fn highlight_runs(token: &str, rows: &[&str], theme: &str) -> Vec<StyledLine> {
    let syntax = SYNTAXES
        .find_syntax_by_token(token)
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    // Split like a ratatui `Line::raw`, so the TUI draws the fallback as before.
    let plain = |l: &str| -> StyledLine { l.lines().map(StyledSpan::plain).collect() };
    let user = USER_THEMES.get().and_then(|set| set.themes.get(theme));
    let Some(theme) = user.or_else(|| THEMES.themes.get(theme)) else {
        return rows.iter().map(|l| plain(l)).collect();
    };
    let mut h = HighlightLines::new(syntax, theme);
    rows.iter()
        .map(|l| {
            // The newline-aware syntaxes want the terminator to close scopes.
            let with_nl = format!("{l}\n");
            match h.highlight_line(&with_nl, &SYNTAXES) {
                Ok(regions) => regions
                    .into_iter()
                    .map(|(st, s)| {
                        let fg = st.foreground;
                        let rgb = Some([fg.r, fg.g, fg.b]);
                        StyledSpan {
                            rgb,
                            ..StyledSpan::plain(s.trim_end_matches('\n'))
                        }
                    })
                    .collect(),
                Err(_) => plain(l),
            }
        })
        .collect()
}

/// `highlight_runs` as ratatui lines.
#[cfg(feature = "ratatui")]
pub fn highlight(token: &str, rows: &[&str], theme: &str) -> Vec<Line<'static>> {
    highlight_runs(token, rows, theme)
        .iter()
        .map(to_line)
        .collect()
}

/// Markdown uses three roles; each keeps the terminal look it had before
/// `StyledDoc` existed, so no transcript snapshot moves.
#[cfg(feature = "ratatui")]
fn to_span(s: &StyledSpan) -> Span<'static> {
    let mut st = match s.token {
        StyleToken::Dim => Style::default().add_modifier(Modifier::DIM),
        StyleToken::Accent => Style::default().fg(Color::Cyan),
        _ => Style::default(),
    };
    if let Some([r, g, b]) = s.rgb {
        st = st.fg(Color::Rgb(r, g, b));
    }
    for (on, m) in [
        (s.bold, Modifier::BOLD),
        (s.italic, Modifier::ITALIC),
        (s.strike, Modifier::CROSSED_OUT),
        (s.underline, Modifier::UNDERLINED),
    ] {
        if on {
            st = st.add_modifier(m);
        }
    }
    let span = Span::styled(s.text.clone(), st);
    if s.link.is_some() {
        crate::link::mark(span)
    } else {
        span
    }
}

#[cfg(feature = "ratatui")]
fn to_line(line: &StyledLine) -> Line<'static> {
    Line::from(line.iter().map(to_span).collect::<Vec<_>>())
}

/// Columns padded to their widest cell, header bold over a rule; wider
/// than `width`, `Header: value` records instead, since a wrapped table row
/// no longer lines up with anything.
#[cfg(feature = "ratatui")]
fn table_lines(rows: &[Vec<String>], g: &Glyphs, width: usize) -> Vec<Line<'static>> {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let natural = widths.iter().sum::<usize>() + 2 * cols.saturating_sub(1);
    if let Some((head, body)) = rows.split_first()
        && !body.is_empty()
        && natural > width
    {
        return record_lines(head, body);
    }
    let fmt = |r: &Vec<String>| {
        (0..cols)
            .map(|c| {
                let s = r.get(c).map_or("", String::as_str);
                format!("{s}{}", " ".repeat(widths[c].saturating_sub(s.width())))
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if i == 0 {
            out.push(Line::styled(
                fmt(r),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            out.push(Line::styled(
                widths
                    .iter()
                    // Repeated to the column width, not past it: an
                    // overridden rule glyph may be two columns wide.
                    .map(|w| g.rule.repeat(w / g.rule.width().max(1)))
                    .collect::<Vec<_>>()
                    .join("  "),
                Style::default().add_modifier(Modifier::DIM),
            ));
        } else {
            out.push(Line::raw(fmt(r)));
        }
    }
    out
}

/// Each row as one `Header: value` line per column, a blank line between
/// rows (Codex's narrow-table fallback).
#[cfg(feature = "ratatui")]
fn record_lines(head: &[String], body: &[Vec<String>]) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    for (i, row) in body.iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        for (c, value) in row.iter().enumerate() {
            let key = head.get(c).map_or("", String::as_str);
            out.push(Line::from(vec![
                Span::styled(format!("{key}:"), bold),
                Span::raw(format!(" {value}")),
            ]));
        }
    }
    out
}

#[cfg(all(test, feature = "ratatui"))]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    fn look(glyphs: Glyphs) -> Look {
        Look {
            width: 80,
            theme: theme_name(true, ""),
            glyphs,
            show_thinking: false,
            show_diffs: true,
            diff: crate::diff::Mode::Auto,
            tick: 0,
            still: false,
            marks: false,
            colors: crate::theme::Theme::dark(),
            expand_last: None,
        }
    }

    #[test]
    fn headings_lists_and_inline_code_keep_their_markers() {
        let lines = render(
            "# Title\n\nSome `code` here\n\n- one\n- two\n  - nested\n\n1. a\n2. b",
            &look(crate::glyph::UNICODE),
        );
        assert_eq!(
            text(&lines),
            [
                "# Title",
                "",
                "Some code here",
                "",
                "• one",
                "• two",
                "  • nested",
                "",
                "1. a",
                "2. b"
            ]
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn fenced_code_is_highlighted_per_line() {
        let lines = render(
            "```rust\nfn main() {}\nlet x = 1;\n```",
            &look(crate::glyph::UNICODE),
        );
        assert_eq!(text(&lines), ["fn main() {}", "let x = 1;"]);
        // `fn` is a keyword, so syntect gave it a colour of its own.
        assert!(lines[0].spans.len() > 1);
    }

    #[test]
    fn tables_align_columns_under_a_bold_header() {
        let lines = render(
            "| a | bb |\n|---|---|\n| ccc | d |",
            &look(crate::glyph::UNICODE),
        );
        assert_eq!(text(&lines), ["a    bb", "───  ──", "ccc  d"]);
    }

    #[test]
    fn the_ascii_set_replaces_every_markdown_glyph() {
        let lines = render(
            "- one\n\n---\n\n> quoted\n\n| a | bb |\n|---|---|\n| ccc | d |",
            &look(crate::glyph::ASCII),
        );
        let rendered = text(&lines).join("\n");
        assert!(rendered.is_ascii(), "{rendered:?}");
        assert!(rendered.contains("- one"), "{rendered:?}");
        assert!(rendered.contains("| quoted"), "{rendered:?}");
        assert!(rendered.contains("---  --"), "{rendered:?}");
    }

    #[test]
    fn open_fence_while_streaming_still_renders_as_code() {
        let lines = render("text\n\n```sh\necho hi", &look(crate::glyph::UNICODE));
        assert_eq!(text(&lines), ["text", "", "echo hi"]);
    }

    #[test]
    fn a_file_extension_highlights_like_a_language_token() {
        let by_ext = highlight("rs", &["fn main() {}"], theme_name(true, ""));
        let by_lang = highlight("rust", &["fn main() {}"], theme_name(true, ""));
        assert!(by_ext[0].spans.len() > 1);
        assert_eq!(by_ext[0].spans[0].style, by_lang[0].spans[0].style);
    }

    #[test]
    fn an_unknown_theme_renders_plain_instead_of_failing() {
        assert_eq!(theme_name(true, "no-such-theme"), "base16-ocean.dark");
        assert_eq!(theme_name(false, "no-such-theme"), "base16-ocean.light");
        assert_eq!(theme_name(true, "InspiredGitHub"), "InspiredGitHub");
        let lines = highlight("rs", &["fn main() {}"], "no-such-theme");
        assert_eq!(text(&lines), ["fn main() {}"]);
        assert_eq!(lines[0].spans.len(), 1);
    }
}
