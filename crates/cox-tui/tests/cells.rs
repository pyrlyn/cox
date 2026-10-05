// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cell snapshots (T5.3): every cell kind rendered from the golden event
//! stream in `fixtures/events/transcript.jsonl`, so a change to how a cell
//! prints shows up as a snapshot diff rather than in a user's terminal.

use cox_protocol::ids::{CallId, TaskId};
use cox_protocol::types::{
    Event, PermissionMode, Risk, SandboxMode, Submission, Tier, ToolCall, ToolResult,
};
use cox_tui::cells::cell_lines;
use cox_tui::state::{Cell, Cmd, Msg, State, update};
use cox_tui::view::{buffer_to_string, view};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

const WIDTH: u16 = 60;

fn replay() -> State {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let jsonl = include_str!("../../../fixtures/events/transcript.jsonl");
    // The bash call is still running: a few ticks give it an elapsed time.
    for _ in 0..3 {
        update(&mut state, Msg::Tick);
    }
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let ev: Event = serde_json::from_str(line).expect("fixture line is an Event");
        update(&mut state, Msg::Event(ev));
    }
    for _ in 0..14 {
        update(&mut state, Msg::Tick);
    }
    state
}

fn cell(state: &State, pick: impl Fn(&Cell) -> bool) -> &Cell {
    state
        .transcript
        .iter()
        .find(|c| pick(c))
        .expect("fixture has the cell")
}

fn text(state: &State, cell: &Cell) -> String {
    cell_lines(cell, &state.look(WIDTH))
        .iter()
        .map(|l| l.to_string().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ascii_glyphs_leave_no_unicode_in_any_cell() {
    let mut s = replay();
    s.glyphs = cox_tui::glyph::ASCII;
    s.show_thinking = true;
    let rendered = s
        .transcript
        .iter()
        .map(|c| text(&s, c))
        .collect::<Vec<_>>()
        .join("\n")
        // The fixture's own tool output contains a `…`, which the TUI must
        // print as the tool wrote it; only cox's own glyphs are under test.
        .replace('…', "");
    assert!(rendered.is_ascii(), "non-ASCII in ascii mode:\n{rendered}");
}

#[test]
fn a_read_of_a_rust_file_is_highlighted_by_its_extension() {
    let s = replay();
    let c = cell(
        &s,
        |c| matches!(c, Cell::Tool { call, .. } if call.name == "read"),
    );
    let lines = cell_lines(c, &s.look(WIDTH));
    // The header is styled as a whole; a highlighted body line is split into
    // spans of its own, so more than one span means syntect ran.
    let painted = lines.iter().skip(1).any(|l| l.spans.len() > 2);
    assert!(
        painted,
        "read of src/main.rs was not highlighted: {lines:?}"
    );
}

#[test]
fn cell_user_lists_attachments() {
    let s = replay();
    insta::assert_snapshot!(text(&s, cell(&s, |c| matches!(c, Cell::User { .. }))));
}

#[test]
fn cell_assistant_renders_markdown_wrapped() {
    let s = replay();
    insta::assert_snapshot!(text(&s, cell(&s, |c| matches!(c, Cell::Assistant { .. }))));
}

#[test]
fn cell_thinking_collapses_until_ctrl_t() {
    let mut s = replay();
    let collapsed = text(&s, cell(&s, |c| matches!(c, Cell::Thinking { .. })));
    update(
        &mut s,
        Msg::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
    );
    let expanded = text(&s, cell(&s, |c| matches!(c, Cell::Thinking { .. })));
    insta::assert_snapshot!(format!("{collapsed}\n---\n{expanded}"));
}

#[test]
fn cell_tool_folds_output_and_hints_expand() {
    let s = replay();
    insta::assert_snapshot!(text(
        &s,
        cell(
            &s,
            |c| matches!(c, Cell::Tool { call, .. } if call.name == "read")
        )
    ));
}

#[test]
fn cell_tool_running_shows_spinner_and_elapsed() {
    let s = replay();
    insta::assert_snapshot!(text(
        &s,
        cell(
            &s,
            |c| matches!(c, Cell::Tool { call, .. } if call.name == "bash")
        )
    ));
}

/// T24.7: under `tui.motion = reduced` a running card is the same frame
/// whatever the tick — one still rail glyph, `running` for the clock.
#[test]
fn reduced_motion_spinner_is_static() {
    let mut s = replay();
    s.still = true;
    let bash = |s: &State| {
        text(
            s,
            cell(
                s,
                |c| matches!(c, Cell::Tool { call, .. } if call.name == "bash"),
            ),
        )
    };
    let before = bash(&s);
    for _ in 0..7 {
        update(&mut s, Msg::Tick);
    }
    assert_eq!(bash(&s), before);
    insta::assert_snapshot!(before);
}

/// T24.7: a table wider than the viewport reads as `Header: value`
/// records; one that fits stays a table.
#[test]
fn table_falls_back_to_records_at_50_columns() {
    let md = "| crate | owns | depends on |\n| --- | --- | --- |\n\
              | cox-core | the agent loop as a state machine | cox-protocol |\n\
              | cox-tui | the ratatui app in TEA form | cox-core, cox-protocol |\n";
    let s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let render = |width| {
        cox_tui::markdown::render(md, &s.look(width))
            .iter()
            .map(|l| l.to_string().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(render(120).starts_with("crate "), "{}", render(120));
    insta::assert_snapshot!(render(50));
}

#[test]
fn cell_notice_error_and_summary() {
    let s = replay();
    let rest: Vec<String> = s
        .transcript
        .iter()
        .filter(|c| {
            matches!(
                c,
                Cell::Notice { .. } | Cell::Error { .. } | Cell::Summary { .. }
            )
        })
        .map(|c| text(&s, c))
        .collect();
    insta::assert_snapshot!(rest.join("\n"));
}

/// T34.7/SM§6: `Event::TaskMessage` renders one transcript line, labelled
/// like a relayed `ApprovalRequired`'s "X asks:" (`Approval::from_agent`,
/// `modal.rs`) — here the task's own registered label, resolved by
/// `TaskCreated`. `from == task` (a child speaking to the parent, `SM§3`'s
/// `message_parent`) reads "label says: text"; anything else (a message
/// delivered to the task) reads "→ label: text".
#[test]
fn cell_task_message_labels_the_speaker_and_direction() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let task = TaskId::new();
    update(
        &mut s,
        Msg::Event(Event::TaskCreated {
            task,
            label: "explore: find the flaky test".into(),
            tier: Tier::Cheap,
        }),
    );
    update(
        &mut s,
        Msg::Event(Event::TaskMessage {
            task,
            from: Some(task),
            hop: 1,
            text: "found it: flaky_sleep in tests/shell.rs".into(),
        }),
    );
    let says = text(
        &s,
        cell(&s, |c| {
            matches!(
                c,
                Cell::TaskMessage {
                    from_task: true,
                    ..
                }
            )
        }),
    );
    update(
        &mut s,
        Msg::Event(Event::TaskMessage {
            task,
            from: None,
            hop: 0,
            text: "keep looking".into(),
        }),
    );
    let delivered = text(
        &s,
        cell(&s, |c| {
            matches!(
                c,
                Cell::TaskMessage {
                    from_task: false,
                    ..
                }
            )
        }),
    );
    insta::assert_snapshot!(format!("{says}\n---\n{delivered}"));
}

// T24.4 tool cards: a dedicated session per test (rather than the fixture
// above) so each one starts from a single, isolated tool call.

fn requested(state: &mut State, name: &str, subject: &str, risk: Risk) -> CallId {
    let id = CallId::new();
    update(
        state,
        Msg::Event(Event::ToolCallRequested {
            call: ToolCall {
                id,
                name: name.into(),
                input: serde_json::json!({}),
                risk,
                subject: subject.into(),
                segments: None,
            },
        }),
    );
    id
}

fn output(state: &mut State, id: CallId, delta: &str) {
    update(
        state,
        Msg::Event(Event::ToolCallOutput {
            call_id: id,
            delta: delta.into(),
        }),
    );
}

fn done(state: &mut State, id: CallId, ok: bool) {
    update(
        state,
        Msg::Event(Event::ToolCallDone {
            call_id: id,
            result: ToolResult {
                ok,
                visible: String::new(),
                archive: None,
                bytes: 0,
                duration_ms: 8,
                diff: None,
                structured: None,
            },
        }),
    );
}

/// Twenty numbered lines: past `HEAD + TAIL + 1` (12), so long enough to
/// fold, with a bash exit trailer so the header's `exit N` has something to
/// parse.
fn long_output(exit: u32) -> String {
    let body = (1..=20)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}\n[exit {exit} in 8ms]")
}

fn tool_cell(state: &State) -> &Cell {
    cell(state, |c| matches!(c, Cell::Tool { .. }))
}

#[test]
fn card_pending() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    requested(&mut s, "bash", "sleep 5", Risk::Exec);
    for _ in 0..7 {
        update(&mut s, Msg::Tick);
    }
    insta::assert_snapshot!(text(&s, tool_cell(&s)));
}

#[test]
fn card_ok_folded() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "bash", "seq 20", Risk::Exec);
    output(&mut s, id, &long_output(0));
    done(&mut s, id, true);
    let rendered = text(&s, tool_cell(&s));
    assert!(
        rendered.contains("more lines"),
        "20 lines should still fold:\n{rendered}"
    );
    insta::assert_snapshot!(rendered);
}

#[test]
fn card_error_unfolded() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "bash", "seq 20 && exit 1", Risk::Exec);
    output(&mut s, id, &long_output(1));
    done(&mut s, id, false);
    let rendered = text(&s, tool_cell(&s));
    assert!(
        !rendered.contains("more lines"),
        "a failed call must show its output whole:\n{rendered}"
    );
    assert!(
        rendered.contains("20"),
        "the tail of the output must still be there:\n{rendered}"
    );
    insta::assert_snapshot!(rendered);
}

/// `view()` is the only place that knows which tool cell is last, so this
/// drives the real render loop rather than `cell_lines` directly, proving
/// `Ctrl+E` reaches through `state.rs` and `view.rs` into the fold.
#[test]
fn ctrl_e_expands_last_card() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "bash", "seq 20", Risk::Exec);
    output(&mut s, id, &long_output(0));
    done(&mut s, id, true);

    let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, 60));
    view(&s, Rect::new(0, 0, WIDTH, 60), &mut buf);
    let folded = buffer_to_string(&buf);
    assert!(
        folded.contains("Ctrl+E"),
        "the last card should hint at Ctrl+E while folded:\n{folded}"
    );

    update(
        &mut s,
        Msg::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
    );
    let mut buf = Buffer::empty(Rect::new(0, 0, WIDTH, 60));
    view(&s, Rect::new(0, 0, WIDTH, 60), &mut buf);
    let expanded = buffer_to_string(&buf);
    assert!(
        !expanded.contains("more lines"),
        "Ctrl+E should unfold the last card in place:\n{expanded}"
    );
    assert!(
        expanded.contains("20"),
        "the previously hidden tail should now be visible:\n{expanded}"
    );
}

/// T25.2 step 3: plan mode refusing an `edit` is one dim `planned` line with
/// the plan glyph, not a red card with the denial as its body.
#[test]
fn plan_mode_denial_renders_as_planned_line() {
    let mut s = State::new(PermissionMode::Plan, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "edit", "src/lib.rs", Risk::Write);
    update(
        &mut s,
        Msg::Event(Event::ToolCallDone {
            call_id: id,
            result: ToolResult {
                ok: false,
                visible: "permission denied: plan mode: only read-only tools run; \
                          describe the change instead"
                    .into(),
                archive: None,
                bytes: 0,
                duration_ms: 0,
                diff: None,
                structured: None,
            },
        }),
    );
    let lines = cell_lines(tool_cell(&s), &s.look(WIDTH));
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|sp| sp.style.fg == Some(s.theme.dim))
    );
    insta::assert_snapshot!(text(&s, tool_cell(&s)), @"▷ planned: edit src/lib.rs");
}

/// T25.3: a `!!` line submits `UserShell`, its `bash` card reads `$ cmd`
/// and keeps the TUI busy until it is done; a line typed meanwhile waits
/// behind it and then goes out as a turn.
#[test]
fn bang_line_card_reads_as_a_shell_prompt() {
    let enter = || Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    s.composer.insert("!!ls -1");
    assert_eq!(
        update(&mut s, enter()),
        vec![Cmd::Submit(Submission::UserShell {
            command: "ls -1".into(),
            share: true,
        })]
    );
    let id = requested(&mut s, "bash", "ls -1", Risk::Exec);
    assert!(s.status.busy);
    insta::assert_snapshot!(text(&s, tool_cell(&s)).lines().next().unwrap_or(""), @"$ ls -1");
    s.composer.insert("next");
    assert!(update(&mut s, enter()).is_empty());
    let cmds = update(
        &mut s,
        Msg::Event(Event::ToolCallDone {
            call_id: id,
            result: ToolResult {
                ok: true,
                visible: String::new(),
                archive: None,
                bytes: 0,
                duration_ms: 3,
                diff: None,
                structured: None,
            },
        }),
    );
    assert!(!s.status.busy);
    assert!(
        matches!(&cmds[..], [Cmd::Submit(Submission::UserTurn { text, .. })] if text == "next")
    );
}

#[test]
fn tsx_read_is_highlighted() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "read", "example.tsx", Risk::ReadOnly);
    let tsx_content = "export const Button = () => {\n  return <button>Click me</button>;\n};\n";
    output(&mut s, id, tsx_content);
    done(&mut s, id, true);

    let lines = cell_lines(tool_cell(&s), &s.look(WIDTH));
    let painted = lines.iter().skip(1).any(|l| l.spans.len() > 2);
    assert!(
        painted,
        "read of example.tsx was not highlighted: {lines:?}"
    );
    insta::assert_snapshot!(text(&s, tool_cell(&s)));
}

#[test]
fn dockerfile_read_is_highlighted() {
    let mut s = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let id = requested(&mut s, "read", "Dockerfile", Risk::ReadOnly);
    let dockerfile_content =
        "FROM rust:latest\nRUN apt-get update\nCOPY . /app\nWORKDIR /app\nRUN cargo build\n";
    output(&mut s, id, dockerfile_content);
    done(&mut s, id, true);

    let lines = cell_lines(tool_cell(&s), &s.look(WIDTH));
    let painted = lines.iter().skip(1).any(|l| l.spans.len() > 2);
    assert!(painted, "read of Dockerfile was not highlighted: {lines:?}");
    insta::assert_snapshot!(text(&s, tool_cell(&s)));
}
