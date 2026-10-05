// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cox CI dashboard demo: a scrollable table of recent pull requests and their CI status
//! (mock data). Up/Down (or k/j) move, q or Esc quits.

use std::io;

use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::Constraint,
    style::{Color, Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Row, Table, TableState},
    DefaultTerminal, Frame,
};

/// CI result of a pull request's latest run.
#[derive(Clone, Copy)]
enum Status {
    Green,
    Red,
}

/// One row of the dashboard.
struct PullRequest {
    number: u32,
    status: Status,
    title: &'static str,
}

/// Mock data standing in for `gh pr list` / the Actions API.
const PULL_REQUESTS: &[PullRequest] = &[
    PullRequest {
        number: 64,
        status: Status::Green,
        title: "fix: stop hanging test after #62",
    },
    PullRequest {
        number: 63,
        status: Status::Green,
        title: "docs: document plugin loading",
    },
    PullRequest {
        number: 62,
        status: Status::Red,
        title: "feat: parallel task runner",
    },
    PullRequest {
        number: 61,
        status: Status::Red,
        title: "build(deps): bump schemars to 1.2",
    },
    PullRequest {
        number: 60,
        status: Status::Green,
        title: "ci: pin shared workflows",
    },
    PullRequest {
        number: 59,
        status: Status::Red,
        title: "build(deps): bump wasmtime to 48",
    },
    PullRequest {
        number: 58,
        status: Status::Green,
        title: "ci: migrate to reusable pipeline",
    },
    PullRequest {
        number: 57,
        status: Status::Green,
        title: "refactor: split config loader",
    },
    PullRequest {
        number: 56,
        status: Status::Red,
        title: "feat: remote cache backend",
    },
    PullRequest {
        number: 55,
        status: Status::Green,
        title: "test: cover lockfile edge cases",
    },
    PullRequest {
        number: 54,
        status: Status::Green,
        title: "fix: respect NO_COLOR",
    },
    PullRequest {
        number: 53,
        status: Status::Green,
        title: "chore: tidy clippy lints",
    },
];

/// App state: which row is selected (TableState also keeps the scroll offset).
struct App {
    state: TableState,
}

impl App {
    fn new() -> Self {
        Self {
            state: TableState::default().with_selected(Some(0)),
        }
    }

    fn next(&mut self) {
        let last = PULL_REQUESTS.len().saturating_sub(1);
        let i = self.state.selected().map_or(0, |i| (i + 1).min(last));
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = self.state.selected().map_or(0, |i| i.saturating_sub(1));
        self.state.select(Some(i));
    }

    /// Main loop: draw a frame, wait for a key, update state, repeat until quit.
    fn run(mut self, mut terminal: DefaultTerminal) -> io::Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            if let Event::Key(key) = event::read()? {
                // Ignore key-release events (sent on Windows) so each press moves once.
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => self.next(),
                    KeyCode::Up | KeyCode::Char('k') => self.previous(),
                    _ => {}
                }
            }
        }
    }

    /// Render the dashboard: PR number, colored CI status, title.
    fn draw(&mut self, frame: &mut Frame) {
        let header = Row::new(["PR", "Status", "Title"])
            .style(Style::new().bold())
            .bottom_margin(1);
        let rows = PULL_REQUESTS.iter().map(|pr| {
            let status = match pr.status {
                Status::Green => Cell::from("● green").style(Style::new().fg(Color::Green)),
                Status::Red => Cell::from("● red").style(Style::new().fg(Color::Red)),
            };
            Row::new([
                Cell::from(format!("#{}", pr.number)),
                status,
                Cell::from(pr.title),
            ])
        });
        let red = PULL_REQUESTS
            .iter()
            .filter(|pr| matches!(pr.status, Status::Red))
            .count();
        // Fixed PR and status columns; the title takes the remaining width.
        let widths = [
            Constraint::Length(5),
            Constraint::Length(9),
            Constraint::Fill(1),
        ];
        let table = Table::new(rows, widths)
            .header(header)
            .block(
                Block::bordered()
                    .title(" cox: recent pull requests ")
                    .title_bottom(Line::from(format!(" {red} red · Up/Down scroll · q quit "))),
            )
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol(">> ");
        // Stateful render: Ratatui reads the selection and updates the scroll offset.
        frame.render_stateful_widget(table, frame.area(), &mut self.state);
    }
}

fn main() -> io::Result<()> {
    // init() enters raw mode + alternate screen and installs a panic hook that restores
    // the terminal; restore() undoes it on normal exit.
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
    ratatui::restore();
    result
}
