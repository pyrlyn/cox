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
//!
//! A plugin's `Span.link` (PL§15.2) is the one place the text and the target
//! differ, so [`mark_to`] carries the target itself: it interns the string in
//! a bounded table and writes the id into the underline colour, the only
//! per-cell channel a `Span` has. [`apply`] resolves the id through the same
//! `target` rule as a text-derived link, so a plugin link cannot leave the
//! workspace or use a scheme other than `https://` either.

use std::collections::HashMap;
use std::num::NonZeroU16;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// The reserved underline colour that marks a link cell. No theme sets an
/// underline colour, and [`apply`] resets it before the terminal sees it.
const MARK: Color = Color::Indexed(8);

/// First byte of the underline colour that carries a [`mark_to`] id; the
/// other two bytes are the id. No theme sets an underline colour.
const LINK_TAG: u8 = 0xC8;
/// Distinct plugin link targets kept for the life of the process. A plugin
/// that rotates targets can fill the table and lose its own links, never
/// anyone else's text.
const MAX_TARGETS: usize = 4096;
/// Longest plugin link accepted; the widget caps count span text, not this.
const MAX_LINK_BYTES: usize = 2048;

#[derive(Default)]
struct Targets {
    ids: HashMap<String, u16>,
    by_id: Vec<String>,
}

static TARGETS: LazyLock<Mutex<Targets>> = LazyLock::new(Mutex::default);

/// What a marked cell asks [`apply`] to link.
#[derive(PartialEq, Eq)]
enum Mark {
    /// The drawn text is the target.
    Text,
    /// A plugin-declared target, by table id.
    Link(u16),
}

/// `span` as a link: the mark [`apply`] looks for. It is invisible; only a
/// run that becomes a hyperlink is underlined, so a terminal without OSC 8
/// shows no promise it cannot keep.
pub fn mark(span: Span<'static>) -> Span<'static> {
    let style = span.style.patch(Style::default().underline_color(MARK));
    span.style(style)
}

/// `span` as a link to `link`, a plugin's `Span.link` (PL§15.2): an
/// `https://` URL or a relative path. Anything else, or a full table, leaves
/// `span` as plain text. Unlike [`mark`] the target is not the drawn text, so
/// a terminal without OSC 8 underlines the run (and [`apply`] never links one
/// whose target resolves outside the workspace).
pub fn mark_to(span: Span<'static>, link: &str) -> Span<'static> {
    let Some(id) = intern(link) else { return span };
    let [hi, lo] = id.to_be_bytes();
    let style = span
        .style
        .patch(Style::default().underline_color(Color::Rgb(LINK_TAG, hi, lo)));
    span.style(style)
}

/// Whether `link` has a form a plugin may declare. The workspace check
/// waits for [`apply`], which knows `cwd`.
fn plugin_form(link: &str) -> bool {
    if link.is_empty()
        || link.len() > MAX_LINK_BYTES
        || link.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    if let Some(rest) = link.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let path = match link.rsplit_once(':') {
        Some((p, n)) if n.parse::<u32>().is_ok() => p,
        _ => link,
    };
    !path.is_empty() && !path.starts_with('/') && !path.contains([':', '\\'])
}

fn intern(link: &str) -> Option<u16> {
    if !plugin_form(link) {
        return None;
    }
    let mut t = TARGETS.lock().ok()?;
    if let Some(id) = t.ids.get(link) {
        return Some(*id);
    }
    let id = u16::try_from(t.by_id.len())
        .ok()
        .filter(|_| t.by_id.len() < MAX_TARGETS)?;
    t.ids.insert(link.to_string(), id);
    t.by_id.push(link.to_string());
    Some(id)
}

fn lookup(id: u16) -> Option<String> {
    TARGETS.lock().ok()?.by_id.get(usize::from(id)).cloned()
}

fn mark_of(cell: &Cell) -> Option<Mark> {
    match cell.underline_color {
        MARK => Some(Mark::Text),
        Color::Rgb(LINK_TAG, hi, lo) => Some(Mark::Link(u16::from_be_bytes([hi, lo]))),
        _ => None,
    }
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
            let Some(mark) = mark_of(&buf[(x, y)]) else {
                x += 1;
                continue;
            };
            let start = x;
            let mut text = String::new();
            // Two adjacent plugin links with different targets are two runs.
            while x < area.right() && mark_of(&buf[(x, y)]).as_ref() == Some(&mark) {
                let cell = &mut buf[(x, y)];
                cell.underline_color = Color::Reset;
                text.push_str(cell.symbol());
                x += 1;
            }
            let (target, hides_target) = match mark {
                Mark::Text => (target(text.trim_end(), cwd), false),
                Mark::Link(id) => (lookup(id).and_then(|l| target(&l, cwd)), true),
            };
            let width = NonZeroU16::new(x - start);
            match (emit, target, width, x == area.right()) {
                (true, Some(target), Some(width), false) => {
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
                // A declared link carries a target the text does not show, so
                // without OSC 8 the underline is the only hint it exists.
                (_, Some(_), _, _) if hides_target => {
                    for cx in start..x {
                        buf[(cx, y)].modifier.insert(Modifier::UNDERLINED);
                    }
                }
                _ => {}
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
        assert!(buf.content.iter().all(|c| mark_of(c).is_none()));
        let plain = drawn(line(), false);
        assert_eq!(plain[(5, 0)].symbol(), "s");
        assert!(plain.content.iter().all(|c| mark_of(c).is_none()));
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

    fn plugin_line(emit: bool, links: &[(&str, &str)]) -> Buffer {
        let spans = links
            .iter()
            .map(|(text, link)| mark_to(Span::raw(text.to_string()), link));
        drawn(Line::from(spans.collect::<Vec<_>>()), emit)
    }

    #[test]
    fn plugin_link_target_need_not_be_the_drawn_text() {
        let buf = plugin_line(true, &[("docs", "https://x.dev/a"), (" ", "")]);
        assert_eq!(
            buf[(0, 0)].symbol(),
            "\x1b]8;;https://x.dev/a\x1b\\docs\x1b]8;;\x1b\\"
        );
        assert_eq!(buf[(1, 0)].symbol(), "");
        assert!(buf.content.iter().all(|c| mark_of(c).is_none()));
    }

    #[test]
    fn plugin_link_without_osc8_is_underlined_text() {
        let buf = plugin_line(false, &[("docs", "https://x.dev/a")]);
        assert_eq!(buf[(0, 0)].symbol(), "d");
        assert!((0..4).all(|x| buf[(x, 0)].modifier.contains(Modifier::UNDERLINED)));
        assert!(!buf[(4, 0)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn adjacent_plugin_links_stay_separate_runs() {
        let buf = plugin_line(true, &[("one", "https://a.dev"), ("two", "https://b.dev")]);
        assert!(buf[(0, 0)].symbol().contains("https://a.dev\x1b\\one"));
        assert!(buf[(3, 0)].symbol().contains("https://b.dev\x1b\\two"));
    }

    #[test]
    fn plugin_link_forms_outside_the_contract_draw_as_plain_text() {
        for bad in [
            "http://x.dev",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "/etc/passwd",
            "https://x.dev/\x1b]8;;",
            "https://",
            "a b",
            "",
        ] {
            assert_eq!(mark_to(Span::raw("t"), bad), Span::raw("t"), "{bad:?}");
        }
        let long = format!("https://x.dev/{}", "a".repeat(MAX_LINK_BYTES));
        assert_eq!(mark_to(Span::raw("t"), &long), Span::raw("t"));
        // A form that passes but leaves the workspace is dropped by `apply`.
        let buf = plugin_line(true, &[("up", "../etc/passwd")]);
        assert_eq!(buf[(0, 0)].symbol(), "u");
        assert!(!buf[(0, 0)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn plugin_relative_path_links_under_the_workspace_with_a_line_anchor() {
        let buf = plugin_line(true, &[("a.rs", "src/a.rs:3")]);
        assert!(
            buf[(0, 0)]
                .symbol()
                .contains("file:///work/src/a.rs#L3\x1b\\a.rs")
        );
    }
}
