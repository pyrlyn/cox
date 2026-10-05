// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Draws a plugin's `Widget` tree (PL§8) with ratatui's own widgets. Apart
//! from `cox-plugin-api::ui` because that crate builds for wasm32 and knows
//! no terminal; here is where a `StyleToken` becomes a `Theme` colour and
//! every plugin string passes `sanitize_with`, the same boundary as
//! `cells::cell_lines`. A tree over the PL§8 caps draws one placeholder line.
//! `lines` (T33.26) reads a drawn tree back as transcript lines, for a
//! plugin-rendered cell.

use cox_protocol::plugin::ui::{self, StyleToken, Widget};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, LineGauge, List, ListState, Paragraph, Row, StatefulWidget, Table, Widget as Draw,
};

use unicode_width::UnicodeWidthStr;

use crate::text;
use crate::theme::Theme;

/// What an over-cap tree draws instead of itself.
pub const TOO_LARGE: &str = "plugin output too large";

/// Draws `widget` into `area`; `marks` is `-v`'s visible removal glyphs.
pub fn render(widget: &Widget, area: Rect, buf: &mut Buffer, theme: &Theme, marks: bool) {
    let pen = Pen { theme, marks };
    if widget.within_limits() {
        pen.draw(widget, area, buf);
    } else {
        Line::styled(TOO_LARGE, pen.style(StyleToken::Dim)).render(area, buf);
    }
}

/// Rows a transcript cell may give one plugin render (T33.26): bounds the
/// buffer a `Stack` of huge `sizes` would otherwise ask for.
const MAX_ROWS: u16 = 256;

/// `widget` as transcript lines `width` columns wide (T33.26): drawn by
/// `render` into a buffer of the tree's own height, read back row by row.
pub fn lines(widget: &Widget, width: u16, theme: &Theme, marks: bool) -> Vec<Line<'static>> {
    let height = match widget.within_limits() {
        true => rows(widget).clamp(1, MAX_ROWS),
        false => 1,
    };
    let area = Rect::new(0, 0, width.max(1), height);
    let mut buf = Buffer::empty(area);
    render(widget, area, &mut buf, theme, marks);
    (0..height)
        .map(|y| Line::from(row_spans(&buf, y)))
        .collect()
}

/// The rows `widget` fills: a slot has a fixed height, a transcript cell
/// takes the tree's own, which `Stack.sizes` sets where it is given.
fn rows(widget: &Widget) -> u16 {
    let n = |len: usize| u16::try_from(len).unwrap_or(u16::MAX);
    match widget {
        Widget::Text(lines) => n(lines.len()),
        Widget::List { items, .. } => n(items.len()),
        Widget::Table { rows, .. } => n(rows.len()).saturating_add(1),
        Widget::KeyValue(pairs) => n(pairs.len()),
        Widget::Gauge { .. } => 1,
        Widget::Stack {
            vertical: true,
            children,
            sizes,
        } => children
            .iter()
            .enumerate()
            .map(|(i, c)| sizes.get(i).copied().unwrap_or_else(|| rows(c)))
            .fold(0, u16::saturating_add),
        Widget::Stack { children, .. } => children.iter().map(rows).max().unwrap_or(0),
        Widget::Block { child, .. } => rows(child).saturating_add(2),
    }
}

/// Row `y` of `buf` as runs of equal style, trailing blanks dropped; a wide
/// glyph's hidden follower cells are skipped so its width counts once.
pub(crate) fn row_spans(buf: &Buffer, y: u16) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut x = 0;
    while x < buf.area.width {
        let cell = &buf[(x, y)];
        let (symbol, style) = (cell.symbol(), cell.style());
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(symbol),
            _ => spans.push(Span::styled(symbol.to_string(), style)),
        }
        x = x.saturating_add(symbol.width().max(1) as u16);
    }
    while let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end().len();
        if trimmed > 0 {
            last.content.to_mut().truncate(trimmed);
            break;
        }
        spans.pop();
    }
    spans
}

struct Pen<'a> {
    theme: &'a Theme,
    marks: bool,
}

impl Pen<'_> {
    fn style(&self, token: StyleToken) -> Style {
        let t = self.theme;
        Style::default().fg(match token {
            StyleToken::Text => t.text,
            StyleToken::Dim => t.dim,
            StyleToken::Accent => t.accent,
            StyleToken::User => t.user,
            StyleToken::Agent => t.agent,
            StyleToken::Tool => t.tool,
            StyleToken::Ok => t.ok,
            StyleToken::Warn => t.warn,
            StyleToken::Error => t.error,
            StyleToken::DiffAdd => t.diff_add,
            StyleToken::DiffDel => t.diff_del,
            StyleToken::DiffHunk => t.diff_hunk,
            StyleToken::Border => t.border,
            StyleToken::Selection => t.selection,
        })
    }

    /// `sanitize` keeps `\n` and `\t`; inside one span either would break
    /// the cell grid, so both become a space.
    fn span(&self, s: &ui::Span) -> Span<'static> {
        let mut style = self.style(s.style);
        if s.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if s.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        let clean = text::sanitize_with(&s.text, self.marks).replace(['\n', '\t'], " ");
        Span::styled(clean, style)
    }

    fn line(&self, spans: &[ui::Span]) -> Line<'static> {
        Line::from(spans.iter().map(|s| self.span(s)).collect::<Vec<_>>())
    }

    fn draw(&self, widget: &Widget, area: Rect, buf: &mut Buffer) {
        match widget {
            Widget::Text(lines) => {
                Paragraph::new(lines.iter().map(|l| self.line(l)).collect::<Vec<_>>())
                    .render(area, buf)
            }
            Widget::List { items, selected } => {
                let list = List::new(items.iter().map(|l| self.line(l)))
                    .highlight_style(
                        self.style(StyleToken::Selection)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");
                StatefulWidget::render(
                    list,
                    area,
                    buf,
                    &mut ListState::default().with_selected(*selected),
                );
            }
            Widget::Table {
                header,
                rows,
                widths,
            } => {
                let cols = rows
                    .iter()
                    .map(Vec::len)
                    .chain([header.len()])
                    .max()
                    .unwrap_or(0);
                let widths = (0..cols).map(|i| {
                    widths
                        .get(i)
                        .map_or(Constraint::Fill(1), |w| Constraint::Length(*w))
                });
                let row = |cells: &[ui::Span]| {
                    Row::new(cells.iter().map(|c| self.span(c)).collect::<Vec<_>>())
                };
                let table = Table::new(rows.iter().map(|r| row(r)), widths)
                    .header(row(header).style(Style::default().add_modifier(Modifier::BOLD)));
                Draw::render(table, area, buf);
            }
            Widget::KeyValue(pairs) => {
                let keys: Vec<Span> = pairs.iter().map(|(k, _)| self.span(k)).collect();
                let pad = keys.iter().map(Span::width).max().unwrap_or(0);
                let lines = keys.into_iter().zip(pairs).map(|(k, (_, v))| {
                    let gap = " ".repeat(pad - k.width() + 1);
                    let mut spans = vec![
                        k.patch_style(Style::new().add_modifier(Modifier::BOLD)),
                        Span::raw(format!(":{gap}")),
                    ];
                    spans.extend(v.iter().map(|s| self.span(s)));
                    Line::from(spans)
                });
                Paragraph::new(lines.collect::<Vec<_>>()).render(area, buf);
            }
            Widget::Gauge { ratio, label } => {
                // `LineGauge` panics outside 0..=1; a NaN would fail the clamp too.
                let ratio = if ratio.is_finite() {
                    ratio.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                LineGauge::default()
                    .ratio(ratio)
                    .label(self.span(label))
                    .filled_style(self.style(StyleToken::Accent))
                    .unfilled_style(self.style(StyleToken::Dim))
                    // A distinct filled glyph so the ratio reads under `NO_COLOR`.
                    .filled_symbol(symbols::line::THICK_HORIZONTAL)
                    .render(area, buf);
            }
            Widget::Stack {
                vertical,
                children,
                sizes,
            } => {
                let dir = if *vertical {
                    Direction::Vertical
                } else {
                    Direction::Horizontal
                };
                let fit = (0..children.len()).map(|i| {
                    sizes
                        .get(i)
                        .map_or(Constraint::Fill(1), |s| Constraint::Length(*s))
                });
                let areas = Layout::new(dir, fit).split(area);
                for (child, a) in children.iter().zip(areas.iter()) {
                    self.draw(child, *a, buf);
                }
            }
            Widget::Block { title, child } => {
                let mut block = Block::bordered().border_style(self.style(StyleToken::Border));
                if let Some(t) = title {
                    block = block.title(self.span(t));
                }
                let inner = block.inner(area);
                block.render(area, buf);
                self.draw(child, inner, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::buffer_to_string;

    fn s(text: &str, style: StyleToken) -> ui::Span {
        ui::Span {
            text: text.into(),
            style,
            ..ui::Span::default()
        }
    }

    fn text(t: &str) -> Widget {
        Widget::Text(vec![vec![s(t, StyleToken::Text)]])
    }

    fn drawn(widget: &Widget, theme: &Theme, w: u16, h: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        render(widget, buf.area, &mut buf, theme, false);
        buf
    }

    /// One tree holding every widget kind, so each theme is one snapshot.
    fn every_kind() -> Widget {
        let bold = ui::Span {
            bold: true,
            ..s("bold", StyleToken::Accent)
        };
        let italic = ui::Span {
            italic: true,
            ..s(" italic", StyleToken::Warn)
        };
        Widget::Stack {
            vertical: true,
            sizes: vec![3, 3, 3, 2, 1, 1],
            children: vec![
                Widget::Block {
                    title: Some(s("block", StyleToken::Accent)),
                    child: Box::new(Widget::Text(vec![vec![bold, italic]])),
                },
                Widget::List {
                    items: vec![
                        vec![s("one", StyleToken::Text)],
                        vec![s("two", StyleToken::Ok)],
                        vec![s("three", StyleToken::Error)],
                    ],
                    selected: Some(1),
                },
                Widget::Table {
                    header: vec![s("name", StyleToken::Text), s("size", StyleToken::Text)],
                    rows: vec![
                        vec![s("a.rs", StyleToken::DiffAdd), s("12", StyleToken::Dim)],
                        vec![s("b.rs", StyleToken::DiffDel), s("7", StyleToken::Dim)],
                    ],
                    widths: vec![10],
                },
                Widget::KeyValue(vec![
                    (
                        s("model", StyleToken::Dim),
                        vec![s("cheap", StyleToken::Tool)],
                    ),
                    (
                        s("cost", StyleToken::Dim),
                        vec![s("$0.01", StyleToken::Warn)],
                    ),
                ]),
                Widget::Gauge {
                    ratio: 0.4,
                    label: s("ctx", StyleToken::Text),
                },
                Widget::Stack {
                    vertical: false,
                    sizes: vec![6],
                    children: vec![text("left"), text("right")],
                },
            ],
        }
    }

    #[test]
    fn every_widget_kind_under_each_theme() {
        let tree = every_kind();
        assert!(tree.within_limits());
        for (name, theme) in [
            ("dark", Theme::dark()),
            ("light", Theme::light()),
            ("no_color", Theme::mono()),
        ] {
            let buf = drawn(&tree, &theme, 30, 13);
            insta::assert_snapshot!(format!("every_widget_kind_{name}"), format!("{buf:?}"));
        }
    }

    #[test]
    fn widget_text_is_sanitized() {
        let buf = drawn(
            &text("\u{1b}[31mred\u{1b}[0m \u{202e}evil\u{202c}"),
            &Theme::dark(),
            20,
            1,
        );
        assert_eq!(buffer_to_string(&buf), "red evil");
    }

    #[test]
    fn oversize_widget_renders_placeholder() {
        let deep = (0..ui::MAX_DEPTH).fold(text("x"), |w, _| Widget::Block {
            title: None,
            child: Box::new(w),
        });
        let wide = Widget::Stack {
            vertical: true,
            sizes: vec![],
            children: vec![text("x"); ui::MAX_NODES],
        };
        let long = text(&"x".repeat(ui::MAX_TEXT_BYTES + 1));
        for tree in [deep, wide, long] {
            assert!(!tree.within_limits());
            assert_eq!(
                buffer_to_string(&drawn(&tree, &Theme::dark(), 30, 3)),
                format!("{TOO_LARGE}\n\n")
            );
        }
    }
}
