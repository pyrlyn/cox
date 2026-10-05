// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A frame as an SVG picture: one `<rect>` per background run and one
//! `<text>` per styled run, columns pinned by `textLength` so the picture
//! lines up like the terminal did. Kept apart from `view` because it renders
//! a finished `Buffer` — the same one the snapshot tests compare as text —
//! and nothing at runtime draws through it; `tests/screenshots.rs` writes
//! `docs/screenshots/*.svg` with it.

use std::fmt::Write;

use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

/// Pixels per column and row at `FONT_SIZE`; a 0.6 em monospace advance.
const CELL_W: f32 = 8.4;
const CELL_H: f32 = 18.0;
const PAD: f32 = 12.0;
const FONT_SIZE: u8 = 14;
const FONT: &str = "SF Mono, Menlo, DejaVu Sans Mono, Consolas, monospace";

const DARK: (&str, &str) = ("#d4d4d4", "#1e1e1e");
const LIGHT: (&str, &str) = ("#24292e", "#ffffff");

/// The 16 ANSI colours as a typical dark terminal shows them.
const ANSI: [&str; 16] = [
    "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
    "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
];

fn css(color: Color, fallback: &str) -> String {
    let ansi = |i: usize| ANSI[i].to_string();
    match color {
        Color::Reset => fallback.to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n @ 0..=15) => ansi(usize::from(n)),
        // The 6×6×6 cube, then the 24-step grey ramp.
        Color::Indexed(n @ 16..=231) => {
            let i = n - 16;
            let step = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
            format!(
                "#{:02x}{:02x}{:02x}",
                step(i / 36),
                step(i / 6 % 6),
                step(i % 6)
            )
        }
        Color::Indexed(n) => {
            let v = 8 + 10 * (n - 232);
            format!("#{v:02x}{v:02x}{v:02x}")
        }
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Adjacent cells with one style, `cols` columns wide.
struct Run {
    x: u16,
    cols: u16,
    text: String,
    fg: String,
    bg: String,
    mods: Modifier,
}

fn runs(buf: &Buffer, y: u16, fg0: &str, bg0: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut skip = 0;
    for x in 0..buf.area.width {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let cell = &buf[(buf.area.x + x, buf.area.y + y)];
        let sym = cell.symbol();
        let cols = u16::try_from(sym.width().max(1)).unwrap_or(1);
        // A wide glyph owns the cells after it; ratatui blanks them.
        skip = cols - 1;
        let mods = cell.modifier;
        let (mut fg, mut bg) = (css(cell.fg, fg0), css(cell.bg, bg0));
        if mods.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        match runs.last_mut() {
            Some(run) if run.fg == fg && run.bg == bg && run.mods == mods => {
                run.text.push_str(sym);
                run.cols += cols;
            }
            _ => runs.push(Run {
                x,
                cols,
                text: sym.to_string(),
                fg,
                bg,
                mods,
            }),
        }
    }
    runs
}

/// `buf` as an SVG document; `cursor` draws a block where the caret is.
pub fn buffer_to_svg(buf: &Buffer, cursor: Option<Position>, dark: bool) -> String {
    let (fg0, bg0) = if dark { DARK } else { LIGHT };
    let w = PAD * 2.0 + f32::from(buf.area.width) * CELL_W;
    let h = PAD * 2.0 + f32::from(buf.area.height) * CELL_H;
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" \
         viewBox=\"0 0 {w:.0} {h:.0}\" font-family=\"{FONT}\" font-size=\"{FONT_SIZE}\">\n\
         <rect width=\"100%\" height=\"100%\" rx=\"8\" fill=\"{bg0}\"/>\n"
    );
    for y in 0..buf.area.height {
        let top = PAD + f32::from(y) * CELL_H;
        let baseline = top + CELL_H - 5.0;
        for run in runs(buf, y, fg0, bg0) {
            let left = PAD + f32::from(run.x) * CELL_W;
            let width = f32::from(run.cols) * CELL_W;
            if run.bg != bg0 {
                let _ = writeln!(
                    out,
                    "<rect x=\"{left:.1}\" y=\"{top:.1}\" width=\"{width:.1}\" height=\"{CELL_H}\" fill=\"{}\"/>",
                    run.bg
                );
            }
            if run.text.trim().is_empty() || run.mods.contains(Modifier::HIDDEN) {
                continue;
            }
            let mut attrs = String::new();
            if run.mods.contains(Modifier::BOLD) {
                attrs.push_str(" font-weight=\"bold\"");
            }
            if run.mods.contains(Modifier::ITALIC) {
                attrs.push_str(" font-style=\"italic\"");
            }
            if run.mods.contains(Modifier::DIM) {
                attrs.push_str(" fill-opacity=\"0.6\"");
            }
            if run.mods.contains(Modifier::UNDERLINED) {
                attrs.push_str(" text-decoration=\"underline\"");
            } else if run.mods.contains(Modifier::CROSSED_OUT) {
                attrs.push_str(" text-decoration=\"line-through\"");
            }
            let _ = writeln!(
                out,
                "<text x=\"{left:.1}\" y=\"{baseline:.1}\" textLength=\"{width:.1}\" \
                 lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\" fill=\"{}\"{attrs}>{}</text>",
                run.fg,
                escape(&run.text)
            );
        }
    }
    if let Some(pos) = cursor {
        let left = PAD + f32::from(pos.x) * CELL_W;
        let top = PAD + f32::from(pos.y) * CELL_H;
        let _ = writeln!(
            out,
            "<rect x=\"{left:.1}\" y=\"{top:.1}\" width=\"{CELL_W}\" height=\"{CELL_H}\" fill=\"{fg0}\" fill-opacity=\"0.5\"/>"
        );
    }
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn svg_pins_each_styled_run_to_its_column_and_escapes_markup() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        buf.set_string(0, 0, "a<b", Style::default().fg(Color::Red));
        buf.set_string(
            4,
            0,
            "日本",
            Style::default()
                .bg(Color::Indexed(196))
                .add_modifier(Modifier::BOLD),
        );
        let svg = buffer_to_svg(&buf, Some(Position::new(9, 0)), true);
        assert!(svg.contains("fill=\"#cd3131\">a&lt;b</text>"), "{svg}");
        // The wide glyphs start at column 4 and span four columns.
        assert!(
            svg.contains("x=\"45.6\" y=\"12.0\" width=\"33.6\""),
            "{svg}"
        );
        assert!(svg.contains("fill=\"#ff0000\"/>"), "{svg}");
        assert!(svg.contains("font-weight=\"bold\">日本</text>"), "{svg}");
        assert!(
            svg.contains("x=\"87.6\" y=\"12.0\" width=\"8.4\""),
            "cursor: {svg}"
        );
        assert_eq!(svg.matches("<text").count(), 2, "blank runs are skipped");
    }

    #[test]
    fn svg_indexed_colours_follow_the_xterm_cube_and_grey_ramp() {
        assert_eq!(css(Color::Indexed(16), ""), "#000000");
        assert_eq!(css(Color::Indexed(231), ""), "#ffffff");
        assert_eq!(css(Color::Indexed(232), ""), "#080808");
        assert_eq!(css(Color::Indexed(255), ""), "#eeeeee");
        assert_eq!(css(Color::Reset, "#abc"), "#abc");
    }
}
