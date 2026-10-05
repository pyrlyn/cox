// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Transcript cells (T5.3): how each `Cell` prints at a given width, in the
//! viewport and in scrollback alike. Separate from `view` so the runtime's
//! `insert_before` and the test harness share one renderer, and from `state`
//! so the state machine knows nothing about columns or colours.

use cox_protocol::types::{Diff, Level};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff;
use crate::item_render::ItemRender;
use crate::link;
use crate::markdown;
use crate::state::Cell;
use crate::text;

/// T32.2: `Look` moved to `cox-render` with the renderers that take it;
/// re-exported here so `cells::Look` keeps working.
pub use cox_render::Look;

/// Output longer than head + tail + 1 lines is folded in the middle; the
/// archive keeps the rest (`cox expand <id>`).
const HEAD: usize = 6;
const TAIL: usize = 5;

/// How a plan-mode refusal reaches a tool result: `cox-core`'s `turn.rs`
/// prefixes `permission denied: ` to `permission::Engine`'s plan-mode reason.
const PLAN_DENIAL: &str = "permission denied: plan mode";

fn dim(s: impl Into<String>) -> Line<'static> {
    Line::styled(s.into(), Style::default().add_modifier(Modifier::DIM))
}

/// The tool card's left edge (T24.4): a phase-tinted glyph and a space,
/// inserted as a span of its own so a highlighted body line keeps its own
/// spans untouched — this is what used to be a plain two-column indent.
fn rail(mut line: Line<'static>, glyph: &'static str, style: Style) -> Line<'static> {
    line.spans
        .insert(0, Span::styled(format!("{glyph} "), style));
    line
}

/// The exit code `cox-tools::bash` already appends to its own output
/// (`[exit <code> in <ms>ms]`, `crates/cox-tools/src/bash/mod.rs::render`),
/// read back from the cleaned output's last line rather than re-parsed from
/// the process — the header shows exactly what the tool told the model.
fn bash_exit_code(output: &str) -> Option<&str> {
    let line = output.lines().next_back()?.trim();
    let rest = line.strip_prefix("[exit ")?;
    let (code, _) = rest.split_once(" in ")?;
    Some(code)
}

/// Tools whose subject is the one file they read or change.
const FILE_TOOLS: [&str; 4] = ["read", "write", "edit", "apply_patch"];

/// The syntect token for a tool whose output is the file named by its
/// subject; `None` for every other tool, whose output is not source text.
fn file_token(name: &str, subject: &str) -> Option<String> {
    if !FILE_TOOLS.contains(&name) {
        return None;
    }
    Some(std::path::Path::new(subject).extension()?.to_str()?.into())
}

/// `cell` as wrapped lines at `look.width`. Every string that came from the
/// model, a tool or a file passes `text::sanitize` here, at the boundary.
pub fn cell_lines(cell: &Cell, look: &Look) -> Vec<Line<'static>> {
    let clean = |s: &str| text::sanitize_with(s, look.marks);
    let g = look.glyphs;
    let lines = match cell {
        Cell::User { text, attachments } => {
            let mut lines = vec![Line::styled(
                format!("{} {}", g.user, clean(text)),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            lines.extend(
                attachments
                    .iter()
                    .map(|a| dim(format!("  {} {}", g.attach, clean(a)))),
            );
            lines
        }
        // T33.26: a plugin's rendering stands in for the markdown; the
        // text the model wrote is untouched in the cell and the rollout.
        Cell::Assistant {
            render: ItemRender::Plugin(w),
            ..
        } => crate::plugin_ui::lines(w, look.width, &look.colors, look.marks),
        Cell::Assistant { text, .. } => markdown::render(&clean(text), look),
        Cell::Thinking { text, done, .. } if !look.show_thinking => {
            // No tokenizer here; four bytes a token is the usual estimate.
            let tokens = text.len() / 4;
            let verb = match *done {
                true => "thought".to_string(),
                false => format!("thinking{}", g.ellipsis),
            };
            vec![dim(format!(
                "{} {verb} (~{tokens} tokens {} Ctrl+T)",
                g.think, g.sep
            ))]
        }
        Cell::Thinking { text, .. } => clean(text)
            .lines()
            .map(|l| dim(format!("{} {l}", g.think)))
            .collect(),
        // T25.2: plan mode refusing a write is the mode working, not a
        // failure — one dim line saying what would have run, no error card.
        Cell::Tool {
            call,
            result: Some(r),
            ..
        } if !r.ok && r.visible.starts_with(PLAN_DENIAL) => vec![Line::styled(
            format!(
                "{} planned: {} {}",
                g.plan,
                clean(&call.name),
                clean(&call.subject)
            ),
            Style::default().fg(look.colors.dim),
        )],
        Cell::Tool {
            call,
            output,
            result,
            started,
            user,
            render,
        } => {
            let sep = g.sep;
            // The card's phase: no result yet, a result that succeeded, or
            // one that failed. The rail and the header share its tint, so
            // a failed call is red top to bottom without a second lookup.
            let phase_ok = result.as_ref().map(|r| r.ok);
            let (rail_glyph, tint) = match phase_ok {
                None if look.still => (g.spin(0), look.colors.tool),
                None => (g.spin(look.tick), look.colors.tool),
                Some(true) => (g.quote, look.colors.tool),
                Some(false) => (g.quote, look.colors.error),
            };
            let rail_style = Style::default().fg(tint);
            let output = clean(output);
            // T25.3: the user's own `!` line reads as the shell prompt it is.
            let (lead, subject) = match user {
                true => ("$ ".to_string(), clean(&call.subject)),
                false => (
                    format!("{} {} ", g.tool, clean(&call.name)),
                    clean(&call.subject),
                ),
            };
            let mut tail = String::new();
            if let Some(r) = result {
                if let Some(d) = r.diff.as_ref() {
                    let (added, removed) = diff::counts(&d.unified);
                    tail.push_str(&format!(" {sep} +{added} {}{removed}", g.minus));
                }
                tail.push_str(&format!(" {sep} {}ms", r.duration_ms));
                if call.name == "bash"
                    && let Some(code) = bash_exit_code(&output)
                {
                    tail.push_str(&format!(" {sep} exit {code}"));
                }
            }
            // T23.3: a file tool's subject is a link when the header fits;
            // a truncated one would link a path cut short.
            let full = format!("{lead}{subject}{tail}");
            let width = usize::from(look.width.max(1));
            let head = match FILE_TOOLS.contains(&call.name.as_str()) && full.width() <= width {
                true => Line::from(vec![
                    Span::raw(lead),
                    link::mark(Span::raw(subject)),
                    Span::raw(tail),
                ]),
                false => Line::raw(text::truncate(&full, width, g.ellipsis)),
            };
            let mut lines = vec![head.style(Style::default().fg(tint))];
            let out: Vec<&str> = output.lines().collect();
            // A tool that prints a file prints source: highlight it by the
            // subject's extension, railed like plain output.
            let token = file_token(&call.name, &call.subject);
            let body = |rows: &[&str]| -> Vec<Line<'static>> {
                match token.as_deref() {
                    Some(t) => markdown::highlight(t, rows, look.theme)
                        .into_iter()
                        .map(|l| rail(l, rail_glyph, rail_style))
                        .collect(),
                    None => rows
                        .iter()
                        .map(|l| rail(Line::raw((*l).to_string()), rail_glyph, rail_style))
                        .collect(),
                }
            };
            // An error shows its output whole rather than hide the reason it
            // failed; otherwise `Ctrl+E` on the one eligible cell does.
            let force_open = phase_ok == Some(false) || look.expand_last == Some(true);
            // T33.26: a plugin's rendering replaces the output body only;
            // the header, the diff and the footer (with its `cox expand`)
            // stay built-in, so a renderer cannot hide what ran or changed.
            if let ItemRender::Plugin(w) = render {
                let inner = look.width.saturating_sub(2);
                lines.extend(
                    crate::plugin_ui::lines(w, inner, &look.colors, look.marks)
                        .into_iter()
                        .map(|l| rail(l, rail_glyph, rail_style)),
                );
            } else if !force_open && out.len() > HEAD + TAIL + 1 {
                lines.extend(body(&out[..HEAD]));
                let hidden = out.len() - HEAD - TAIL;
                let hint = if look.expand_last == Some(false) {
                    format!(" {sep} Ctrl+E")
                } else {
                    result
                        .as_ref()
                        .and_then(|r| r.archive.as_ref())
                        .map(|a| format!(" {sep} /expand {}", a.id))
                        .unwrap_or_default()
                };
                lines.push(rail(
                    Line::styled(
                        format!("{} {hidden} more lines{hint}", g.ellipsis),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                    rail_glyph,
                    rail_style,
                ));
                lines.extend(body(&out[out.len() - TAIL..]));
            } else {
                lines.extend(body(&out));
            }
            if let Some(d) = result.as_ref().and_then(|r| r.diff.as_ref()) {
                let d = Diff {
                    path: d.path.clone(),
                    unified: clean(&d.unified),
                };
                lines.extend(diff::lines(&d, look));
            }
            match result {
                Some(r) => {
                    let mark = if r.ok { g.ok } else { g.fail };
                    let expand = r
                        .archive
                        .as_ref()
                        .map(|a| format!(" {sep} cox expand {}", a.id))
                        .unwrap_or_default();
                    // T26.4: a change the user can take back says how.
                    let undo = match r.ok && r.diff.is_some() {
                        true => format!(" {sep} /undo"),
                        false => String::new(),
                    };
                    lines.push(rail(
                        dim(format!(
                            "{mark} {}B {}ms{expand}{undo}",
                            r.bytes, r.duration_ms
                        )),
                        rail_glyph,
                        rail_style,
                    ));
                }
                None => {
                    let elapsed = look.tick.saturating_sub(*started);
                    let shown = match look.still {
                        true => "running".to_string(),
                        false => format!("{}.{}s", elapsed / 10, elapsed % 10),
                    };
                    lines.push(rail(dim(shown), rail_glyph, rail_style));
                }
            }
            lines
        }
        Cell::Notice { level, text } => {
            let style = match level {
                Level::Info => Style::default().add_modifier(Modifier::DIM),
                Level::Warn => Style::default().fg(look.colors.warn),
                Level::Budget => Style::default().fg(look.colors.accent),
                Level::Security => Style::default().fg(look.colors.error),
            };
            let tag = format!("[{}] ", format!("{level:?}").to_lowercase());
            let pad = " ".repeat(tag.width());
            clean(text)
                .lines()
                .enumerate()
                .map(|(i, l)| {
                    let prefix = if i == 0 { &tag } else { &pad };
                    Line::styled(format!("{prefix}{l}"), style)
                })
                .collect()
        }
        Cell::Error { text, fatal } => vec![Line::styled(
            format!(
                "{} {}{}",
                g.fail,
                clean(text),
                if *fatal { " (session ended)" } else { "" }
            ),
            Style::default().fg(look.colors.error),
        )],
        Cell::Summary { text } => {
            let mut lines = vec![dim(format!(
                "{d} compacted; earlier turns summarised as {d}",
                d = g.dash
            ))];
            lines.extend(clean(text).lines().map(|l| dim(format!("  {l}"))));
            lines
        }
        // T34.7/SM§6: `label`/`text` are already sanitized in `update`
        // (state.rs), like `Modal::Diff`'s text — no second `clean()` pass
        // here. Styled like `Approval::from_agent`'s "X asks:" prefix
        // (`modal.rs`): bold, `theme.agent`, inserted before the body.
        Cell::TaskMessage {
            label,
            text,
            from_task,
        } => {
            let bold = Style::default().add_modifier(Modifier::BOLD);
            let prefix = if *from_task {
                format!("{label} says: ")
            } else {
                format!("→ {label}: ")
            };
            let pad = " ".repeat(prefix.width());
            text.lines()
                .enumerate()
                .map(|(i, l)| {
                    if i == 0 {
                        Line::from(vec![
                            Span::styled(prefix.clone(), bold.fg(look.colors.agent)),
                            Span::raw(l.to_string()),
                        ])
                    } else {
                        Line::raw(format!("{pad}{l}"))
                    }
                })
                .collect()
        }
    };
    wrap(lines, look.width)
}

/// Word-wraps styled lines to `width` columns by display width, keeping each
/// span's style; continuation rows repeat the line's leading indent so
/// wrapped list items stay under their marker. Words wider than a row are
/// split at a character boundary rather than overflowing.
pub fn wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut out = Vec::new();
    for line in lines {
        let indent: String = line
            .spans
            .first()
            .map(|s| s.content.chars().take_while(|c| *c == ' ').collect())
            .unwrap_or_default();
        let indent = if indent.width() >= width {
            String::new()
        } else {
            indent
        };
        let mut cur: Vec<Span<'static>> = Vec::new();
        let mut col = 0;
        // A `Line::styled` keeps its style on the line, not the spans; the
        // rebuilt lines are plain, so each span inherits it here.
        let base = line.style;
        for span in line.spans {
            let style = base.patch(span.style);
            for word in span.content.split_inclusive(' ') {
                let w = word.trim_end().width();
                if col > 0 && col + w > width {
                    out.push(Line::from(std::mem::take(&mut cur)));
                    if !indent.is_empty() {
                        cur.push(Span::raw(indent.clone()));
                    }
                    col = indent.width();
                }
                let mut rest = word;
                while rest.width() > width - col {
                    let mut take = 0;
                    let mut acc = 0;
                    for (i, ch) in rest.char_indices() {
                        let cw = ch.width().unwrap_or(0);
                        if acc + cw > width - col {
                            break;
                        }
                        acc += cw;
                        take = i + ch.len_utf8();
                    }
                    if take == 0 {
                        break;
                    }
                    cur.push(Span::styled(rest[..take].to_string(), style));
                    out.push(Line::from(std::mem::take(&mut cur)));
                    if !indent.is_empty() {
                        cur.push(Span::raw(indent.clone()));
                    }
                    col = indent.width();
                    rest = &rest[take..];
                }
                col += rest.width();
                cur.push(Span::styled(rest.to_string(), style));
            }
        }
        out.push(Line::from(cur));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn wrap_breaks_at_words_and_keeps_the_indent() {
        let lines = wrap(vec![Line::raw("  • one two three four")], 12);
        assert_eq!(text(&lines), ["  • one two ", "  three four"]);
    }

    #[test]
    fn wrap_splits_a_word_wider_than_the_row_by_display_width() {
        let lines = wrap(vec![Line::raw("ééééééé 漢字漢字")], 4);
        assert_eq!(text(&lines), ["éééé", "ééé ", "漢字", "漢字"]);
    }

    #[test]
    fn wrap_keeps_the_line_style_on_every_span() {
        let red = Style::default().fg(ratatui::style::Color::Red);
        let lines = wrap(vec![Line::styled("aa bb", red)], 3);
        assert!(lines.iter().flat_map(|l| &l.spans).all(|s| s.style == red));
    }

    #[test]
    fn wrap_keeps_span_styles_across_the_break() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let lines = wrap(vec![Line::from(vec![Span::styled("aa bb", bold)])], 3);
        assert_eq!(text(&lines), ["aa ", "bb"]);
        assert_eq!(lines[1].spans[0].style, bold);
    }
}
