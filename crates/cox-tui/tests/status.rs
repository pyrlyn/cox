// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Status line, todo panel and slash commands (T5.5): the §1.13 row after
//! real usage events, and what each command line turns into.

use cox_protocol::ids::{CallId, TurnId};
use cox_protocol::types::Effort;
use cox_protocol::types::{
    Event, Job, ModelId, PermissionMode, Risk, SandboxMode, StopReason, Submission, Tier, ToolCall,
    ToolResult, Usage,
};
use cox_tui::commands::{self, Action, COMMANDS};
use cox_tui::state::{Cell, Cmd, GitStatus, Loop, Msg, State, update};
use cox_tui::view::{buffer_to_string, render};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn turn(state: &mut State, model: &str, cost: f64, input: u32) {
    let turn = TurnId::new();
    for ev in [
        Event::TurnStarted {
            seq: 1,
            turn,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId(model.into()),
        },
        Event::Usage {
            turn,
            usage: Usage {
                input_tokens: input,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                estimated: false,
                cost_usd: cost,
                latency_ms: 900,
            },
        },
        Event::TurnDone {
            turn,
            stop: StopReason::EndTurn,
        },
    ] {
        update(state, Msg::Event(ev));
    }
}

fn submit(state: &mut State, line: &str) -> Vec<Cmd> {
    let mut chars = line.chars();
    if let Some(c) = chars.next() {
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        // A `/` at column 0 opened the palette; Esc closes it, keeps the `/`
        // and lets the rest be typed as text.
        if c == '/' {
            update(state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        }
    }
    for c in chars {
        update(state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    update(state, Msg::Key(KeyEvent::from(KeyCode::Enter)))
}

#[test]
fn status_line_after_two_turns() {
    let mut state = State::new(PermissionMode::Plan, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.41, 60_000);
    turn(&mut state, "claude-sonnet-5", 0.42, 82_000);
    insta::assert_snapshot!(cox_tui::status::line(&state).to_string());
}

/// T22.1: while `ask_user`'s modal is open, the mode slot names it instead
/// of the permission mode — there is nothing to permission-check meanwhile.
#[test]
fn status_line_shows_question_in_the_mode_slot_while_the_modal_is_open() {
    let mut state = State::new(PermissionMode::Plan, SandboxMode::WorkspaceWrite);
    update(
        &mut state,
        Msg::Event(Event::QuestionAsked {
            call_id: CallId::new(),
            question: "which environment?".into(),
            options: vec![],
            source: None,
        }),
    );
    let line = cox_tui::status::line(&state).to_string();
    assert!(line.contains("[question]"), "{line}");
}

#[test]
fn command_slash_model_opus_emits_switch_model() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.1, 10);
    assert_eq!(
        submit(&mut state, "/model opus"),
        vec![Cmd::Submit(Submission::SwitchModel {
            tier: Tier::Code,
            model: Some(ModelId("opus".into())),
        })]
    );
    assert_eq!(
        commands::parse("/model think claude-opus-5", Tier::Code),
        Some(Action::Submit(Submission::SwitchModel {
            tier: Tier::Think,
            model: Some(ModelId("claude-opus-5".into())),
        }))
    );
}

#[test]
fn command_lines_map_to_their_submissions() {
    let parse = |l: &str| commands::parse(l, Tier::Code);
    assert_eq!(
        parse("/compact the auth work"),
        Some(Action::Submit(Submission::Compact {
            focus: Some("the auth work".into())
        }))
    );
    assert_eq!(
        parse("/permissions auto"),
        Some(Action::Mode(PermissionMode::Auto))
    );
    assert_eq!(parse("/quit"), Some(Action::Quit));
    assert!(matches!(parse("/think"), Some(Action::Notice(_))));
    assert!(matches!(parse("/nope"), Some(Action::Notice(_))));
    assert_eq!(parse("not a command"), None);
    assert!(matches!(
        parse("/expand 01ARZ3NDEKTSV4RRFFQ69G5FAA"),
        Some(Action::Submit(Submission::Command { command }))
            if command.name == "expand" && command.args == ["01ARZ3NDEKTSV4RRFFQ69G5FAA"]
    ));
}

#[test]
fn command_help_lists_every_command_and_shift_tab_cycles_the_mode() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::ReadOnly);
    assert!(submit(&mut state, "/help").is_empty());
    let Some(Cell::Notice { text, .. }) = state.transcript.last() else {
        panic!("help is a notice cell");
    };
    for (name, ..) in COMMANDS {
        assert!(
            text.contains(&format!("/{name}")),
            "{name} missing from /help"
        );
    }
    assert_eq!(
        update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        ),
        vec![Cmd::Submit(Submission::SetPermissionMode {
            mode: PermissionMode::Plan
        })]
    );
    assert_eq!(state.mode, PermissionMode::Plan);
}

#[test]
fn command_todo_shows_the_panel_from_the_tool_output() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    let call_id = CallId::new();
    update(
        &mut state,
        Msg::Event(Event::ToolCallRequested {
            call: ToolCall {
                id: call_id,
                name: "todo".into(),
                input: serde_json::json!({}),
                risk: Risk::ReadOnly,
                subject: "3 items".into(),
                segments: None,
            },
        }),
    );
    update(
        &mut state,
        Msg::Event(Event::ToolCallDone {
            call_id,
            result: ToolResult {
                ok: true,
                visible: "[x] 1: read the loop\n[~] 2: write cells\n[ ] 3: snapshots".into(),
                archive: None,
                bytes: 0,
                duration_ms: 1,
                diff: None,
                structured: Some(Box::new(serde_json::json!([
                    {"id": "1", "text": "read the loop", "state": "done"},
                    {"id": "2", "text": "write cells", "state": "in_progress"},
                    {"id": "3", "text": "snapshots", "state": "pending"},
                ]))),
            },
        }),
    );
    assert!(submit(&mut state, "/todo").is_empty());
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 8)));
}

#[test]
fn command_slash_effort_sets_or_clears_the_session_effort() {
    assert_eq!(
        commands::parse("/effort xhigh", Tier::Code),
        Some(Action::Submit(Submission::SetEffort {
            effort: Some(Effort::Xhigh)
        }))
    );
    assert_eq!(
        commands::parse("/effort medium", Tier::Code),
        Some(Action::Submit(Submission::SetEffort {
            effort: Some(Effort::Medium)
        }))
    );
    assert_eq!(
        commands::parse("/effort", Tier::Code),
        Some(Action::Submit(Submission::SetEffort { effort: None }))
    );
    assert!(matches!(
        commands::parse("/effort max", Tier::Code),
        Some(Action::Notice(text)) if text.contains("medium") && text.contains("xhigh")
    ));
}

#[test]
fn status_at_60_100_160_columns() {
    use cox_tui::state::GitStatus;
    use cox_tui::status::line_at;
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 4.2, 82_000);
    state.status.cache_ratio = 0.6;
    state.git = Some(GitStatus {
        branch: "main".into(),
        added: 12,
        removed: 3,
    });
    // Full width: every segment, in display order.
    let wide = line_at(&state, 160).to_string();
    assert!(wide.contains("sonnet-5"), "{wide}");
    assert!(wide.contains("ctx "), "{wide}");
    assert!(wide.contains("$4.20/5"), "{wide}");
    assert!(wide.contains("workspace-write"), "{wide}");
    assert!(wide.contains("0 tasks"), "{wide}");
    assert!(wide.contains("cache 60%"), "{wide}");
    assert!(wide.contains("⎇ main +12 −3"), "{wide}");
    insta::assert_snapshot!("status_wide_160", wide);
    // Medium: the droppable cache, tasks, effort and sandbox segments fall
    // off, in that order.
    let mid = line_at(&state, 90).to_string();
    assert!(mid.contains("ctx "), "{mid}");
    assert!(!mid.contains("cache 60%"), "{mid}");
    assert!(mid.contains("⎇ main"), "{mid}");
    insta::assert_snapshot!("status_mid_100", mid);
    // Narrow: only model, ctx, cost and the mode slot stay.
    let narrow = line_at(&state, 60).to_string();
    assert!(narrow.contains("ctx "), "{narrow}");
    assert!(!narrow.contains("workspace-write"), "{narrow}");
    assert!(narrow.contains("$4.20/5"), "{narrow}");
    insta::assert_snapshot!("status_narrow_60", narrow);
}

/// T28.1: `$` names the spend over the session cap and warns past `warn_at`;
/// the effort badge shows only when `/effort` overrode the tier default.
#[test]
fn status_cost_warns_past_warn_at_and_badges_effort() {
    use cox_protocol::types::Effort;
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.1, 10);
    let calm = cox_tui::status::line(&state).to_string();
    assert!(calm.contains("$0.10/5"), "{calm}");
    assert!(!calm.contains("effort:"), "{calm}");
    state.status.cost_usd = 4.5;
    let over = cox_tui::status::line(&state);
    assert!(over.to_string().contains("$4.50/5"), "{}", over.to_string());
    let cost = over
        .spans
        .iter()
        .find(|s| s.content.contains('$'))
        .expect("cost span");
    assert_eq!(
        cost.style.fg,
        Some(state.theme.warn),
        "past warn_at the cost takes theme.warn"
    );
    state.status.effort = Some(Effort::Xhigh);
    let badged = cox_tui::status::line(&state).to_string();
    assert!(badged.contains("effort:xhigh"), "{badged}");
}

/// T28.1: the `ctx` bar fills its cached share in the accent colour.
#[test]
fn status_ctx_bar_marks_the_cached_share_in_accent() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.1, 10);
    state.status.cache_ratio = 0.6;
    let line = cox_tui::status::line(&state);
    let bar: Vec<_> = line
        .spans
        .iter()
        .filter(|s| s.content.contains('▰') || s.content.contains('▱'))
        .collect();
    assert_eq!(bar.len(), 2, "one span per share: {line:?}");
    assert_eq!(bar[0].content, "▰▰▰", "60% of five cells: {line:?}");
    assert_eq!(bar[0].style.fg, Some(state.theme.accent));
    assert_eq!(bar[1].style.fg, Some(state.theme.text));
}

#[test]
fn status_line_shows_the_git_segment_only_inside_a_repository() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.1, 10);
    let plain = cox_tui::status::line(&state).to_string();
    update(
        &mut state,
        Msg::Git(Some(GitStatus {
            branch: "main".into(),
            added: 12,
            removed: 3,
        })),
    );
    let with = cox_tui::status::line(&state).to_string();
    let g = state.glyphs;
    // The git counts ride the mode slot at the end; the rest of the row is
    // the same row as without a repository.
    let tail = format!(
        "{} {} main +12 {}3 {} [default]",
        g.sep, g.branch, g.minus, g.sep
    );
    assert!(with.ends_with(&tail), "{with}");
    let head = with[..with.len() - tail.len()].trim_end().to_string();
    let without_mode = plain
        .trim_start()
        .to_string()
        .replace(&format!(" {} [default]", g.sep), "");
    assert_eq!(head, without_mode, "{with} vs {plain}");
    update(&mut state, Msg::Git(None));
    assert_eq!(cox_tui::status::line(&state).to_string(), plain);
}

/// T27.7: an active `/loop` shows the time left until its next turn, dropped
/// before `cache` when the line narrows; nothing shows without one.
#[test]
fn status_shows_loop_countdown() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.1, 10);
    state.status.cache_ratio = 0.6;
    let without = cox_tui::status::line(&state).to_string();
    assert!(!without.contains('↻'), "{without}");
    state.active_loop = Some(Loop {
        prompt: "go".into(),
        interval_ticks: 3_000,
        next_at: state.tick + 2_520, // 252.0s
        budget_usd: 5.0,
        started_cost_usd: 0.0,
        iterations: 0,
    });
    let with = cox_tui::status::line(&state).to_string();
    assert!(with.contains("↻ 4m12s"), "{with}");
    // Narrower than the wide line but wide enough to keep `cache`: the
    // countdown, being the right-most droppable segment, is gone first.
    let width = (with.len() - 1) as u16;
    let mid = cox_tui::status::line_at(&state, width).to_string();
    assert!(!mid.contains('↻'), "{mid}");
    assert!(mid.contains("cache 60%"), "{mid}");
}

/// T27.3: a worktree session shows `⧉ <name>` right after the branch
/// segment; a session on the main tree shows nothing extra.
#[test]
fn status_line_names_the_worktree_after_the_branch() {
    let mut state = State::new(PermissionMode::Plan, SandboxMode::WorkspaceWrite);
    let plain = cox_tui::status::line(&state).to_string();
    assert!(!plain.contains('⧉'));
    state.git = Some(cox_tui::state::GitStatus {
        branch: "t42".into(),
        added: 1,
        removed: 0,
    });
    state.worktree = Some("t42".into());
    let line = cox_tui::status::line(&state).to_string();
    assert!(line.ends_with("· ⎇ t42 +1 −0 · ⧉ t42 · [plan]"), "{line}");
    state.glyphs = cox_tui::glyph::ASCII;
    assert!(
        cox_tui::status::line(&state)
            .to_string()
            .ends_with("| # t42 +1 -0 | wt t42 | [plan]"),
        "{}",
        cox_tui::status::line(&state)
    );
}

#[test]
fn state_changed_from_the_core_sets_mode_and_effort() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    state.status.effort = Some(Effort::High);
    update(
        &mut state,
        Msg::Event(Event::StateChanged {
            mode: PermissionMode::Plan,
            effort: None,
        }),
    );
    assert_eq!(state.mode, PermissionMode::Plan);
    assert_eq!(state.status.effort, None);
}

/// A113 (T37.22.9): the title a `TitleSet` carries shows before the mode
/// badge, and `/rename` submits the user's title to the core.
#[test]
fn status_line_shows_the_session_title_and_rename_submits_it() {
    let mut state = State::new(PermissionMode::Plan, SandboxMode::WorkspaceWrite);
    turn(&mut state, "claude-sonnet-5", 0.41, 60_000);
    update(
        &mut state,
        Msg::Event(Event::TitleSet {
            title: "Fix the ledger".into(),
            by_user: false,
        }),
    );
    insta::assert_snapshot!(buffer_to_string(&render(&state, 120, 3)));
    let rename = Submission::Rename {
        title: "Ledger fix".into(),
    };
    assert!(submit(&mut state, "/rename Ledger fix").contains(&Cmd::Submit(rename)));
    assert!(matches!(
        commands::parse("/rename", Tier::Code),
        Some(Action::Notice(_))
    ));
}
