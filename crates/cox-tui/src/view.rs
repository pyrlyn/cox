// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `view`: `State` → screen. Pure over the state and a `Buffer`, so the
//! live viewport and the test harness (`render`) draw through the same
//! function and a snapshot is the real screen. Cells print through
//! `cells::cell_lines`, shared with the runtime's `insert_before`, so
//! scrollback and viewport agree. An empty composer shows `KEYMAP` hints
//! for the current context (T24.6).

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use cox_protocol::plugin::Slot;

use crate::cells::cell_lines;
use crate::commands;
use crate::state::{Cell, Modal, State};
use crate::status;

/// Queued messages (T25.1) shown above at once before they collapse to a
/// `+n` summary — enough to see what is coming without pushing the
/// composer off screen.
const QUEUE_SHOWN: usize = 3;

/// One line per queued message (T25.1), oldest first, dim and prefixed
/// `⏸`; past `QUEUE_SHOWN` the rest collapse into one `+n` line. Each
/// message shows only its first line — same reasoning as a picker row, a
/// queued message is a label here, not a place to read the whole thing —
/// and goes through `text::sanitize` like other user text a render site
/// did not type itself.
fn queue_lines(state: &State) -> Vec<Line<'static>> {
    let style = Style::default().fg(state.theme.dim);
    let total = state.queue.len();
    let mut lines: Vec<Line<'static>> = state
        .queue
        .iter()
        .take(QUEUE_SHOWN)
        .map(|text| {
            let first = crate::text::sanitize(text.lines().next().unwrap_or(""));
            Line::styled(format!(" ⏸ {first}"), style)
        })
        .collect();
    if total > QUEUE_SHOWN {
        lines.push(Line::styled(format!(" ⏸ +{}", total - QUEUE_SHOWN), style));
    }
    lines
}

/// The empty composer's line (T24.6): the first keymap rows of the context
/// the keys are in, one per action (T25.5: as bound now), dim, in place of
/// a fixed placeholder — up to five, fewer when they would not fit in
/// `width` (never under three).
fn hints(state: &State, width: u16) -> Line<'static> {
    let sep = format!(" {} ", state.glyphs.sep);
    let mut rows = state.keymap.rows(state.context());
    rows.dedup_by_key(|(_, action)| *action);
    // T46.7: `Ctrl+E` edits a theme only over `/theme`, so no other modal
    // advertises it.
    let themes =
        matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == crate::picker::Kind::Themes);
    rows.retain(|(_, action)| themes || *action != "theme.edit");
    let mut text = String::new();
    for (n, (key, action)) in rows.into_iter().take(5).enumerate() {
        let hint = format!("{key} {}", commands::label(action));
        let next = if n == 0 { hint } else { format!("{sep}{hint}") };
        if n >= 3 && text.len() + next.len() > usize::from(width) {
            break;
        }
        text.push_str(&next);
    }
    Line::styled(text, Style::default().fg(state.theme.dim))
}

/// T22.9: each tool cell's line range (into the unscrolled transcript) to
/// the absolute screen rows it occupies once scrolled by `offset` and
/// clipped to `area` — the space `MouseEvent::row` arrives in, since
/// ratatui anchors an inline viewport to the real cursor row, not zero.
fn record_cell_rows(
    state: &State,
    spans: &[(Range<usize>, usize)],
    offset: usize,
    rows: usize,
    area: Rect,
) {
    let visible = offset..offset + rows;
    let cell_rows = spans
        .iter()
        .filter_map(|(span, i)| {
            let start = span.start.max(visible.start);
            let end = span.end.min(visible.end);
            (start < end).then(|| {
                let top = area.y + u16::try_from(start - offset).unwrap_or(u16::MAX);
                let bottom = area.y + u16::try_from(end - offset).unwrap_or(u16::MAX);
                (top..bottom, *i)
            })
        })
        .collect();
    *state.cell_rows.borrow_mut() = cell_rows;
}

/// Draws `state` into `area`; returns where the cursor goes.
pub fn view(state: &State, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let banner = u16::from(state.banner.is_some());
    // The transcript spans the full width, so one `look` serves it and the
    // approval modal, whose height is its own line count (an edit's diff).
    let look = state.look(area.width);
    let approval = match &state.modal {
        Some(Modal::Approval(a)) => a.lines(&look),
        _ => Vec::new(),
    };
    // T33.8: same reason as `approval` — `view` needs the line count for
    // the band's height before it draws the transcript below it.
    let grant = match &state.modal {
        Some(Modal::PluginGrant(g)) => g.lines(&state.glyphs, &state.theme),
        _ => Vec::new(),
    };
    // T33.33: same reason as `grant` above — a band, not an overlay, so
    // `view` needs its line count before laying out the transcript.
    let remove = match &state.modal {
        Some(Modal::PluginRemove(r)) => r.lines(&state.theme),
        _ => Vec::new(),
    };
    let modal = match &state.modal {
        Some(Modal::Approval(_)) => u16::try_from(approval.len()).unwrap_or(u16::MAX),
        Some(Modal::Question(q)) => q.height(),
        Some(Modal::Picker(p)) => p.height(),
        Some(Modal::PluginGrant(_)) => u16::try_from(grant.len()).unwrap_or(u16::MAX),
        Some(Modal::PluginRemove(_)) => u16::try_from(remove.len()).unwrap_or(u16::MAX),
        Some(Modal::ThemeEditor(e)) => e.height(),
        // The diff view, the agents list, the rollout overlay and a
        // plugin's overlay (T33.24) all take the transcript's rows
        // (`Context::Overlay`), not a band of their own.
        Some(
            Modal::Diff { .. }
            | Modal::Help
            | Modal::Context
            | Modal::Agents { .. }
            | Modal::Transcript { .. }
            | Modal::Plugin { .. },
        )
        | None => 0,
    };
    let composer_rows = u16::try_from(state.composer.line_count().clamp(1, 5)).unwrap_or(5);
    let todo_rows = if state.show_todo {
        u16::try_from(state.todo.len() + 1).unwrap_or(u16::MAX)
    } else {
        0
    };
    // T33.24, PL§8: a bottom `panel`, like the todo panel above — one band,
    // capped at `PANEL_ROWS`, present only while `TogglePanel` has it open.
    let panel_rows = if state.plugin_panel_open.is_some() {
        status::PANEL_ROWS
    } else {
        0
    };
    let queue = queue_lines(state);
    let queue_rows = u16::try_from(queue.len()).unwrap_or(u16::MAX);
    // T46.3: the status command's row, above the built-in one and only
    // while it has something to say, so without it nothing moves.
    let script = status::script_line(state, area.width);
    let script_rows = u16::from(script.is_some());
    let [
        banner_area,
        transcript,
        todo_area,
        panel_area,
        modal_area,
        queue_area,
        composer,
        script_area,
        status,
    ] = Layout::vertical([
        Constraint::Length(banner),
        Constraint::Min(1),
        Constraint::Length(todo_rows),
        Constraint::Length(panel_rows),
        Constraint::Length(modal),
        Constraint::Length(queue_rows),
        Constraint::Length(composer_rows),
        Constraint::Length(script_rows),
        Constraint::Length(1),
    ])
    .areas(area);

    if let Some(b) = &state.banner {
        b.line(&state.theme).render(banner_area, buf);
    }
    let rows = usize::from(transcript.height);
    // T22.9: cleared every draw; only the plain-transcript arm below
    // refills it, so a click behind `Diff`/`Help`/`Agents`/`Transcript`
    // never hits a card that is not actually on screen.
    state.cell_rows.borrow_mut().clear();
    let (lines, offset): (Vec<Line<'static>>, usize) = match &state.modal {
        // The transcript scrolls from its end; the diff view from its start.
        Some(Modal::Diff { text, scroll }) => {
            let lines = crate::diff::view_lines(text, &look);
            let offset = (*scroll).min(lines.len().saturating_sub(rows));
            (lines, offset)
        }
        Some(Modal::Help) => (
            crate::modal::help_lines(&state.glyphs, &state.theme, &state.keymap, area.width),
            0,
        ),
        Some(Modal::Context) => (
            crate::modal::context_lines(&state.status, &state.glyphs, &state.theme, area.width),
            0,
        ),
        // T27.5: `/agents`'s navigable list, one row per `agents_rows` entry;
        // the selected row is marked the same way `Picker::lines` marks
        // its own selection.
        Some(Modal::Agents {
            rows: agent_rows,
            selected,
            ..
        }) => {
            let lines: Vec<Line<'static>> = agent_rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    if i == *selected {
                        Line::styled(
                            format!(" {} {row}", state.glyphs.cursor),
                            Style::default().fg(state.theme.selection),
                        )
                    } else {
                        Line::raw(format!("   {row}"))
                    }
                })
                .collect();
            (lines, 0)
        }
        // T27.5: a sibling session's rollout, replayed into `Cell`s by
        // `state::replay_cells` and drawn through the same `cell_lines`
        // every other transcript cell goes through — no second renderer.
        Some(Modal::Transcript { cells, scroll }) => {
            let lines: Vec<Line<'static>> =
                cells.iter().flat_map(|c| cell_lines(c, &look)).collect();
            let offset = (*scroll).min(lines.len().saturating_sub(rows));
            (lines, offset)
        }
        // T33.24, PL§8: drawn straight into `transcript` below with
        // `plugin_ui::render`, a widget tree rather than `Line`s — this
        // match only needs to leave the band empty here.
        Some(Modal::Plugin { .. }) => (Vec::new(), 0),
        _ => {
            // `Ctrl+E` (T24.4) reaches only the last tool cell, still
            // `Some(bool)` so a folded one keeps its `Ctrl+E` hint; a click
            // (T22.9) force-opens any other cell already in
            // `state.expanded`, so an untouched cell renders as before.
            let last_tool = state
                .transcript
                .iter()
                .rposition(|c| matches!(c, Cell::Tool { .. }));
            let mut lines: Vec<Line<'static>> = Vec::new();
            let mut tool_spans: Vec<(Range<usize>, usize)> = Vec::new();
            for (i, c) in state.transcript.iter().enumerate() {
                let mut look = look;
                if Some(i) == last_tool {
                    look.expand_last = Some(state.expanded.contains(&i));
                } else if state.expanded.contains(&i) {
                    look.expand_last = Some(true);
                }
                let start = lines.len();
                lines.extend(cell_lines(c, &look));
                if matches!(c, Cell::Tool { .. }) {
                    tool_spans.push((start..lines.len(), i));
                }
            }
            let offset = lines.len().saturating_sub(rows + state.scroll);
            record_cell_rows(state, &tool_spans, offset, rows, transcript);
            (lines, offset)
        }
    };
    Paragraph::new(lines)
        .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0))
        .render(transcript, buf);
    // T33.24, PL§8: the overlay draws over the transcript, like `Diff`;
    // a missing or late render (not yet answered, or the slot stopped)
    // leaves a placeholder rather than nothing crashing or blanking.
    if let Some(Modal::Plugin { id }) = &state.modal {
        match status::widget(state, id, Slot::Overlay) {
            Some(w) => crate::plugin_ui::render(w, transcript, buf, &state.theme, state.marks),
            None => {
                crate::modal::plugin_overlay_placeholder(id, &state.theme).render(transcript, buf)
            }
        }
    }

    if state.show_todo {
        Paragraph::new(status::todo_lines(state)).render(todo_area, buf);
    }
    // T33.24, PL§8: the panel band; a not-yet-rendered widget just leaves
    // the band blank rather than blocking on the plugin.
    if let Some(id) = &state.plugin_panel_open
        && let Some(w) = status::widget(state, id, Slot::Panel)
    {
        crate::plugin_ui::render(w, panel_area, buf, &state.theme, state.marks);
    }
    match &state.modal {
        Some(Modal::Approval(_)) => Paragraph::new(approval).render(modal_area, buf),
        Some(Modal::Question(q)) => {
            Paragraph::new(q.lines(&state.glyphs, &state.theme)).render(modal_area, buf)
        }
        Some(Modal::Picker(p)) => {
            Paragraph::new(p.lines(&state.glyphs, &state.theme)).render(modal_area, buf)
        }
        Some(Modal::PluginGrant(_)) => Paragraph::new(grant).render(modal_area, buf),
        Some(Modal::PluginRemove(_)) => Paragraph::new(remove).render(modal_area, buf),
        Some(Modal::ThemeEditor(e)) => {
            Paragraph::new(e.lines(&state.glyphs, &state.theme)).render(modal_area, buf)
        }
        // Drawn over the transcript above, like `Diff`/`Help`; no band
        // here. `Plugin`'s overlay is drawn there too, right after the
        // transcript `Paragraph` above.
        Some(
            Modal::Diff { .. }
            | Modal::Help
            | Modal::Context
            | Modal::Agents { .. }
            | Modal::Transcript { .. }
            | Modal::Plugin { .. },
        )
        | None => {}
    }
    if !queue.is_empty() {
        Paragraph::new(queue).render(queue_area, buf);
    }

    let [prompt, text] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(composer);
    Line::styled(
        state.glyphs.mode(state.mode),
        Style::default().fg(state.theme.mode(state.mode)),
    )
    .render(prompt, buf);
    if state.composer.text().is_empty() {
        hints(state, text.width).render(text, buf);
    } else {
        state.composer.widget().render(text, buf);
    }
    if let Some(script) = script {
        script.render(script_area, buf);
    }
    status::line_at(state, status.width).render(status, buf);

    // T23.3: marks become hyperlinks (or nothing) before the frame leaves.
    crate::link::apply(buf, &state.cwd, state.caps.osc8);
    // One place for every colour on the screen, the composer widget and the
    // syntect spans included.
    crate::color::map_buffer(buf, state.depth);

    let (row, col) = state.composer.cursor();
    let x = text.x + u16::try_from(col).unwrap_or(u16::MAX);
    let y = text.y + u16::try_from(row).unwrap_or(u16::MAX);
    Some(Position::new(
        x.min(text.right().saturating_sub(1)),
        y.min(text.bottom().saturating_sub(1)),
    ))
}

/// Test harness: the screen `view` would draw at `width`×`height`.
pub fn render(state: &State, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    view(state, area, &mut buf);
    buf
}

/// Rows of a buffer as text, trailing spaces trimmed (snapshot form).
pub fn buffer_to_string(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::PluginSegment;
    use cox_protocol::plugin::{Widget, ui};
    use cox_protocol::types::{PermissionMode, SandboxMode};

    /// A one-line `Widget::Text` a plugin might render (T33.24).
    fn text_widget(t: &str) -> Widget {
        Widget::Text(vec![vec![ui::Span {
            text: t.into(),
            ..ui::Span::default()
        }]])
    }

    /// T25.1 step 2/"Done when": two queued messages render above the
    /// composer, dim and prefixed `⏸`, oldest first.
    #[test]
    fn queue_renders_above_composer() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        state.queue.push_back("first message".to_string());
        state.queue.push_back("second message".to_string());
        let buf = render(&state, 40, 8);
        insta::assert_snapshot!(buffer_to_string(&buf));
    }

    /// T24.6 step 2: the empty composer's hints are the first `KEYMAP` rows
    /// of the context — send/mode/@/`/`/? when idle, interrupt and friends
    /// while a turn runs.
    #[test]
    fn placeholder_hints_follow_context() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let idle = buffer_to_string(&render(&state, 90, 4));
        state.status.busy = true;
        let running = buffer_to_string(&render(&state, 90, 4));
        assert!(idle.contains("Enter send · Shift+Tab mode cycle · @ file · / command · ? help"));
        assert!(running.contains("Esc interrupt · Ctrl+B background · Ctrl+O transcript"));
        insta::assert_snapshot!(format!("{idle}\n---\n{running}"));
    }

    /// T24.6 step 3: `?` on an empty composer draws `KEYMAP` grouped by
    /// context over the transcript; `Esc` closes it, and `?` typed after
    /// text stays a character.
    #[test]
    fn help_overlay_snapshot() {
        use crate::state::{Msg, update};
        use crossterm::event::{KeyCode, KeyEvent};
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let key = |c| Msg::Key(KeyEvent::from(c));
        update(&mut state, key(KeyCode::Char('?')));
        assert_eq!(state.modal, Some(Modal::Help));
        insta::assert_snapshot!(buffer_to_string(&render(&state, 80, 15)));
        update(&mut state, key(KeyCode::Esc));
        assert_eq!(state.modal, None);
        update(&mut state, key(KeyCode::Char('a')));
        update(&mut state, key(KeyCode::Char('?')));
        assert_eq!(
            (state.modal.clone(), state.composer.text()),
            (None, "a?".into())
        );
    }

    /// T33.24, PL§8: the `panel` band is empty until `TogglePanel` opens
    /// it, then draws the plugin's cached render above the composer.
    #[test]
    fn plugin_panel_open_and_closed() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.plugin_status.push(PluginSegment {
            plugin: "acme".into(),
            slot: Slot::Panel,
            widget: Some(text_widget("panel text")),
            misses: 0,
        });
        let closed = buffer_to_string(&render(&state, 40, 10));
        assert!(!closed.contains("panel text"), "{closed}");

        state.plugin_panel_open = Some("acme".into());
        let open = buffer_to_string(&render(&state, 40, 10));
        assert!(open.contains("panel text"), "{open}");

        insta::assert_snapshot!(format!("{closed}\n---\n{open}"));
    }

    /// T33.24, PL§8: `Modal::Plugin` draws full screen over the
    /// transcript, like `Diff`/`Transcript`; before the first render lands
    /// it shows a placeholder rather than a blank screen or a crash.
    #[test]
    fn plugin_overlay_snapshot() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.modal = Some(Modal::Plugin { id: "acme".into() });
        let waiting = buffer_to_string(&render(&state, 40, 10));
        assert!(waiting.contains("acme"), "{waiting}");

        state.plugin_status.push(PluginSegment {
            plugin: "acme".into(),
            slot: Slot::Overlay,
            widget: Some(text_widget("overlay text")),
            misses: 0,
        });
        let rendered = buffer_to_string(&render(&state, 40, 10));
        assert!(rendered.contains("overlay text"), "{rendered}");

        insta::assert_snapshot!(format!("{waiting}\n---\n{rendered}"));
    }

    /// A98: `/context` draws the window, the share and the four parts
    /// as one bar in the theme's context roles, in each theme; the parts
    /// are rescaled to the reported context. With no window it says so.
    #[test]
    fn context_overlay_snapshot() {
        use crate::state::{Msg, update};
        use crate::theme::Theme;
        use cox_protocol::ids::TurnId;
        use cox_protocol::types::{ContextBreakdown, Event};
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let mut breakdown = ContextBreakdown {
            window: Some(200_000),
            total: 34_000,
            system: 3_400,
            tools: 21_500,
            instructions: 1_100,
            history: 8_000,
            cached: 51_000,
        };
        let event = |breakdown| {
            Msg::Event(Event::ContextBreakdown {
                turn: TurnId::new(),
                breakdown,
            })
        };
        update(&mut state, event(breakdown));
        state.status.context_tokens = 68_000;
        state.modal = Some(Modal::Context);
        for (name, theme) in [
            ("dark", Theme::dark()),
            ("light", Theme::light()),
            ("no_color", Theme::mono()),
        ] {
            state.theme = theme;
            let buf = render(&state, 60, 12);
            let text = buffer_to_string(&buf);
            assert!(text.contains("68000 of 200000 tokens · 34%"), "{text}");
            insta::assert_snapshot!(format!("context_overlay_{name}"), format!("{buf:?}"));
        }
        breakdown.window = None;
        update(&mut state, event(breakdown));
        let text = buffer_to_string(&render(&state, 60, 12));
        assert!(text.contains("68000 tokens · window unknown"), "{text}");
    }
}
