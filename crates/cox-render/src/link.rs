// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! OSC 8 hyperlinks (T23.3). A renderer marks a span as a link with
//! [`mark`]; after the frame is drawn, [`apply`] finds the marked cells,
//! clears the mark and — when the terminal speaks OSC 8 — wraps each run in
//! `ESC ] 8 ; ; target ESC \`. The target is derived from the drawn text
//! itself (an `http(s)` URL, or a file path under the workspace), so a link
//! never points anywhere the user cannot read on screen, and text that only
//! the model wrote cannot become one: it is sanitized before drawing and
//! never carries the mark. Separate from `text` because it works on the
//! drawn `Buffer`, not on strings.

use std::num::NonZeroU16;
use std::path::{Component, Path, PathBuf};

use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// The reserved underline colour that marks a link cell. No theme sets an
/// underline colour, and [`apply`] resets it before the terminal sees it.
const MARK: Color = Color::Indexed(8);

/// `span` as a link: the mark [`apply`] looks for. It is invisible; only a
/// run that becomes a hyperlink is underlined, so a terminal without OSC 8
/// shows no promise it cannot keep.
pub fn mark(span: Span<'static>) -> Span<'static> {
    let style = span.style.patch(Style::default().underline_color(MARK));
    span.style(style)
}

fn is_marked(cell: &Cell) -> bool {
    cell.underline_color == MARK
}

/// Strips every link mark in `buf`; with `emit`, turns each marked run
/// into one OSC 8 hyperlink. `cwd` resolves a relative file path; a path
/// that leaves it is left unlinked. A run that touches the right edge may
/// be the first half of a wrapped URL, so it stays plain.
pub fn apply(buf: &mut Buffer, cwd: &Path, emit: bool) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            if !is_marked(&buf[(x, y)]) {
                x += 1;
                continue;
            }
            let start = x;
            let mut text = String::new();
            while x < area.right() && is_marked(&buf[(x, y)]) {
                let cell = &mut buf[(x, y)];
                cell.underline_color = Color::Reset;
                text.push_str(cell.symbol());
                x += 1;
            }
            let target = target(text.trim_end(), cwd);
            let width = NonZeroU16::new(x - start);
            if let (true, Some(target), Some(width), false) =
                (emit, target, width, x == area.right())
            {
                let first = &mut buf[(start, y)];
                first.modifier.insert(Modifier::UNDERLINED);
                first
                    .set_symbol(&format!("\x1b]8;;{target}\x1b\\{text}\x1b]8;;\x1b\\"))
                    .set_diff_option(CellDiffOption::ForcedWidth(width));
                // The first cell prints the whole run; the rest print
                // nothing, whether the frame is diffed or drawn whole.
                for cx in start + 1..x {
                    buf[(cx, y)]
                        .set_symbol("")
                        .set_diff_option(CellDiffOption::Skip);
                }
            }
        }
    }
}

/// The link target for drawn `text`: the URL itself, or `file://` plus the
/// path under `cwd` with `#L<n>` from a `:n` suffix. `None` for anything
/// else, including a character that could end the escape early.
fn target(text: &str, cwd: &Path) -> Option<String> {
    if text.is_empty() || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    if text.starts_with("https://") || text.starts_with("http://") {
        return Some(text.to_string());
    }
    let (path, line) = match text.rsplit_once(':') {
        Some((p, n)) if !p.is_empty() && n.parse::<u32>().is_ok() => (p, Some(n)),
        _ => (text, None),
    };
    if !cwd.is_absolute() {
        return None;
    }
    let full = normalize(&cwd.join(path));
    if !full.starts_with(cwd) {
        return None;
    }
    let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
    Some(format!("file://{}{anchor}", full.display()))
}

/// `..` and `.` resolved lexically: the TUI never touches the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};

    fn drawn(line: Line<'static>, emit: bool) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        Paragraph::new(line).render(buf.area, &mut buf);
        apply(&mut buf, Path::new("/work"), emit);
        buf
    }

    #[test]
    fn osc8_emitted_only_when_supported() {
        let line = || {
            Line::from(vec![
                Span::raw("read "),
                mark(Span::raw("src/a.rs:12")),
                Span::raw(" ok"),
            ])
        };
        let buf = drawn(line(), true);
        assert_eq!(
            buf[(5, 0)].symbol(),
            "\x1b]8;;file:///work/src/a.rs#L12\x1b\\src/a.rs:12\x1b]8;;\x1b\\"
        );
        assert_eq!(buf[(6, 0)].symbol(), "");
        assert_eq!(buf[(16, 0)].symbol(), " ");
        assert!(buf.content.iter().all(|c| !is_marked(c)));
        let plain = drawn(line(), false);
        assert_eq!(plain[(5, 0)].symbol(), "s");
        assert!(plain.content.iter().all(|c| !is_marked(c)));
    }

    #[test]
    fn links_only_what_stays_inside_the_workspace_or_is_http() {
        let cwd = Path::new("/work");
        assert_eq!(target("../etc/passwd", cwd), None);
        assert_eq!(target("/etc/passwd", cwd), None);
        assert_eq!(
            target("https://x.dev/a", cwd).as_deref(),
            Some("https://x.dev/a")
        );
        assert_eq!(
            target("/work/b.rs", cwd).as_deref(),
            Some("file:///work/b.rs")
        );
    }
}
