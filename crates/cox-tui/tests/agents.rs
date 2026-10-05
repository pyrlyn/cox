// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Live sessions in the TUI (T16.3): `/agents` lists the fed records with
//! their status and files, and the status line counts them only when there
//! are any. T27.2 turns that list into one row per live agent — a sibling
//! session (`Presence`) or a subagent/background task this session started
//! (`TaskCreated`) — so this file also covers the task half of the card.
//! T27.5 turns the list itself into a navigable `Modal::Agents` overlay and
//! adds the read-only `Modal::Transcript` rollout view `Enter` opens on a
//! sibling-session row.

mod common;

use std::str::FromStr;

use cox_protocol::ids::{ItemId, SessionId, TaskId};
use cox_protocol::types::{
    Event, ItemKind, PermissionMode, Presence, PresenceStatus, SandboxMode, Tier,
};
use cox_tui::state::{Ask, Cell, Cmd, Modal, Msg, State, update};
use cox_tui::status;
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent};

fn record(id: &str, pid: u32, status: PresenceStatus, touched: &[&str]) -> Presence {
    Presence {
        session: SessionId::from_str(id).expect("ulid"),
        pid,
        cwd: "/w".into(),
        project: "/w".into(),
        status,
        turn: 3,
        touched: touched.iter().map(|s| (*s).to_string()).collect(),
        updated: 0,
        worktree: None,
    }
}

fn two() -> Vec<Presence> {
    vec![
        record(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            41,
            PresenceStatus::Active,
            &["crates/cox-tui/src/state.rs", "plan.md"],
        ),
        record(
            "01BX5ZZKBKACTAV9WEVGEMMVRZ",
            42,
            PresenceStatus::Waiting,
            &[],
        ),
    ]
}

/// One row per sibling session and one per a live subagent task: name,
/// preset, tier, cost, elapsed, state (plan.md T27.2's narrow card — model,
/// tokens and last tool need a new event and are a follow-up), now inside
/// the navigable `Modal::Agents` overlay (T27.5) instead of a `Notice`.
#[test]
fn agents_overlay_lists_one_row_per_card() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    update(&mut state, Msg::Agents(two()));
    update(
        &mut state,
        Msg::Event(Event::TaskCreated {
            task: TaskId::new(),
            label: "explore: find the flaky test".into(),
            tier: Tier::Cheap,
        }),
    );
    for _ in 0..7 {
        update(&mut state, Msg::Tick);
    }
    common::type_line(&mut state, "/agents");
    let Some(Modal::Agents {
        rows,
        ids,
        selected,
    }) = &state.modal
    else {
        panic!("no agents overlay");
    };
    assert_eq!(*selected, 0);
    // The two sibling sessions carry their `SessionId`; the task row (no
    // resumable id on the wire yet, plan.md §3 P27) is `None`.
    assert_eq!(
        ids,
        &vec![
            Some(SessionId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("ulid")),
            Some(SessionId::from_str("01BX5ZZKBKACTAV9WEVGEMMVRZ").expect("ulid")),
            None,
        ]
    );
    // `record`'s session ids are fixed ULIDs, and the task's own id never
    // prints (only its label does), so the snapshot needs no redaction.
    insta::assert_snapshot!(rows.join("\n"));
}

/// T44.3: a sibling session running in a worktree says which one, by the
/// worktree's directory name behind the status line's `⧉`; one on the main
/// tree keeps the plain row.
#[test]
fn agents_overlay_shows_worktree() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let mut agents = two();
    agents[0].worktree = Some("/w/_worktrees/w-t44".into());
    update(&mut state, Msg::Agents(agents));
    common::type_line(&mut state, "/agents");
    let Some(Modal::Agents { rows, .. }) = &state.modal else {
        panic!("no agents overlay");
    };
    insta::assert_snapshot!(rows.join("\n"));
}

/// T34.7/SM§6: a delivered `Event::TaskMessage` updates that task's narrow
/// `/agents` card with a `last:` line instead of a new progress event
/// stream — the row keeps its `preset`/`tier`/`cost`/`elapsed`/`running`
/// shape and grows one more `·` segment.
#[test]
fn task_message_adds_a_last_message_line_to_the_agents_card() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let task = TaskId::new();
    update(
        &mut state,
        Msg::Event(Event::TaskCreated {
            task,
            label: "explore: find the flaky test".into(),
            tier: Tier::Cheap,
        }),
    );
    update(
        &mut state,
        Msg::Event(Event::TaskMessage {
            task,
            from: Some(task),
            hop: 1,
            text: "found it: flaky_sleep in tests/shell.rs".into(),
        }),
    );
    for _ in 0..7 {
        update(&mut state, Msg::Tick);
    }
    common::type_line(&mut state, "/agents");
    let Some(Modal::Agents { rows, .. }) = &state.modal else {
        panic!("no agents overlay");
    };
    insta::assert_snapshot!(rows.join("\n"));
}

#[test]
fn agents_command_says_so_when_alone() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    common::type_line(&mut state, "/agents");
    assert_eq!(state.modal, None);
    let Some(Cell::Notice { text, .. }) = state.transcript.last() else {
        panic!("no notice cell");
    };
    assert_eq!(text, "no live agents");
}

/// `Enter` on a sibling-session row in the `/agents` overlay (T27.5) asks
/// the runtime for that session's rollout (`Cmd::Ask(Ask::Rollout)`); this
/// test sends the reply by hand (`crates/cox/src/session.rs` answers it for
/// real with `Store::rollout_read`) to prove `Msg::Rollout` replays into
/// cells through the ordinary `update`/`Msg::Event` path (no second
/// renderer) and opens them read-only in `Modal::Transcript`; `Esc` closes it.
#[test]
fn agents_overlay_opens_the_selected_rollout() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    update(&mut state, Msg::Agents(two()));
    common::type_line(&mut state, "/agents");
    assert!(matches!(state.modal, Some(Modal::Agents { .. })));

    // The list itself, drawn over the transcript (`Context::Overlay`).
    insta::assert_snapshot!(
        "agents_overlay_list",
        buffer_to_string(&render(&state, 60, 10))
    );

    let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let first = SessionId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("ulid");
    assert_eq!(cmds, vec![Cmd::Ask(Ask::Rollout(first))]);
    // The runtime's answer is still in flight: the list stays open.
    assert!(matches!(state.modal, Some(Modal::Agents { .. })));

    let rollout = vec![Event::ItemStarted {
        item: ItemId::new(),
        kind: ItemKind::UserMessage {
            text: "hi from the sibling session".into(),
            attachments: vec![],
        },
    }];
    update(&mut state, Msg::Rollout(rollout));
    let Some(Modal::Transcript { cells, .. }) = &state.modal else {
        panic!("no transcript overlay");
    };
    assert_eq!(
        cells,
        &vec![Cell::User {
            text: "hi from the sibling session".into(),
            attachments: Vec::new(),
        }]
    );
    // The fed rollout, replayed into cells and drawn read-only.
    insta::assert_snapshot!(
        "agents_overlay_rollout",
        buffer_to_string(&render(&state, 60, 10))
    );

    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(state.modal, None);
}

#[test]
fn status_line_counts_agents_and_flags_one_waiting() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let alone = status::line(&state).to_string();
    assert!(!alone.contains("agent"));
    update(&mut state, Msg::Agents(two()));
    assert!(status::line(&state).to_string().contains("2 agents!"));
    update(&mut state, Msg::Agents(vec![two().remove(0)]));
    let one = status::line(&state).to_string();
    assert!(one.contains("1 agent") && !one.contains('!'));
    update(&mut state, Msg::Agents(Vec::new()));
    assert_eq!(status::line(&state).to_string(), alone);
}
