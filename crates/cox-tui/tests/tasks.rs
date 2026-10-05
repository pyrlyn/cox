// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Background task list (T9.2): two running tasks in the status count,
//! `/tasks` output, and the snapshot of the list.

use cox_protocol::ids::TaskId;
use cox_protocol::types::{Event, PermissionMode, SandboxMode, Tier};
use cox_tui::commands;
use cox_tui::state::{Cell, Msg, State, update};

mod common;
use common::type_line;

fn running(state: &mut State) {
    for label in ["explore: find x", "shell: cargo test"] {
        update(
            state,
            Msg::Event(Event::TaskCreated {
                task: TaskId::new(),
                label: label.into(),
                tier: Tier::Cheap,
            }),
        );
    }
}

#[test]
fn tasks_list_shows_two_running_tasks() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    running(&mut state);
    assert!(
        cox_tui::status::line(&state)
            .to_string()
            .contains("2 tasks"),
        "status count"
    );
    assert_eq!(
        commands::parse("/tasks", Tier::Code),
        Some(cox_tui::commands::Action::Tasks)
    );
    type_line(&mut state, "/tasks");
    let Some(Cell::Notice { text, .. }) = state.transcript.last() else {
        panic!("/tasks is a notice cell");
    };
    assert!(text.contains("explore: find x"), "{text}");
    assert!(text.contains("shell: cargo test"), "{text}");
    // Task ids are ULIDs: redact them before snapshotting.
    let redacted = text
        .lines()
        .map(|l| match l.split_once(": ") {
            Some((_, label)) if l.starts_with("- ") => format!("- <task>: {label}"),
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(redacted);
}
