// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The core's `Event` stream folded into `State` (`Msg::Event`): cells, the
//! status line, modals, tasks and the rewind timeline, plus what a finished
//! turn releases from the queue and the architect think-price question. Apart
//! from `update` because this is the one place the session's events land; the
//! keys and slash commands live beside it.

use cox_protocol::ids::CallId;
use cox_protocol::types::{
    Event, ItemKind, Level, Mode as SessionMode, PermissionMode, SlashCommand, StopReason,
    Submission,
};

use super::{Cell, Cmd, Modal, Notify, State, TurnRow, notice, user_turn};
use crate::banner::Banner;
use crate::item_render::{CellRef, ItemRender};
use crate::modal::{Approval, Question, QuestionAnswer};
use crate::tasks;

/// `Cmd::Notify` for `body` when `tui.notify` says to ring now (T23.5).
fn notify(state: &State, body: String) -> Vec<Cmd> {
    let ring = match state.notify {
        Notify::Always => true,
        Notify::Auto => !state.focused,
        Notify::Off => false,
    };
    if !ring {
        return Vec::new();
    }
    vec![Cmd::Notify {
        title: "cox".to_string(),
        body,
    }]
}

pub(super) fn on_event(state: &mut State, ev: Event) -> Vec<Cmd> {
    if let Some(banner) = Banner::from_event(&ev) {
        state.banner = Some(banner);
        return Vec::new();
    }
    let mut cmds = Vec::new();
    match ev {
        Event::ItemStarted { item, kind } => match kind {
            ItemKind::UserMessage { text, attachments } => {
                state.turns.push(TurnRow {
                    seq: state.current_seq,
                    text: text.clone(),
                    files: 0,
                    cell_at: state.transcript.len(),
                });
                state.transcript.push(Cell::User {
                    text,
                    attachments: attachments.into_iter().map(|a| a.name).collect(),
                });
            }
            ItemKind::AssistantMessage { text } => state.transcript.push(Cell::Assistant {
                item,
                text,
                done: false,
                render: ItemRender::Builtin,
            }),
            // T39.2 keeps a tool call's signature as an empty signed item
            // for the provider's history: a replay token, not something the
            // model said. A streamed thought also starts empty, but unsigned.
            ItemKind::Thinking {
                text,
                signature: Some(_),
            } if text.is_empty() => {}
            ItemKind::Thinking { text, .. } => state.transcript.push(Cell::Thinking {
                item,
                text,
                done: false,
            }),
            ItemKind::Summary { text } => state.transcript.push(Cell::Summary { text }),
            ItemKind::Notice { level, text } => state.transcript.push(Cell::Notice { level, text }),
            // Tool items arrive as `ToolCallRequested`/`ToolCallDone` too;
            // those carry the streamed output, so they own the cell.
            ItemKind::ToolCall { .. } | ItemKind::ToolResult { .. } => {}
        },
        Event::TextDelta { item, text } | Event::ThinkingDelta { item, text } => {
            if let Some(Cell::Assistant { text: t, .. } | Cell::Thinking { text: t, .. }) =
                state.item_mut(item)
            {
                t.push_str(&text);
            }
        }
        // The desktop's fold header shows the duration (A91); the TUI's
        // thinking cell has no header to put it in.
        Event::ThinkingDone { .. } => {}
        Event::ItemDone { item } => {
            if let Some(Cell::Assistant { done, .. } | Cell::Thinking { done, .. }) =
                state.item_mut(item)
            {
                *done = true;
            }
            cmds = crate::item_render::ask(state, CellRef::Item(item));
        }
        Event::ToolCallRequested { call } => {
            let user = call.name == "bash" && state.shell.take().is_some();
            if user {
                state.shell_call = Some(call.id);
                state.status.busy = true;
            }
            state.transcript.push(Cell::Tool {
                call: Box::new(call),
                output: String::new(),
                result: None,
                started: state.tick,
                user,
                render: ItemRender::Builtin,
            });
        }
        Event::ToolCallOutput { call_id, delta } => {
            if let Some(Cell::Tool { output, .. }) = state.tool_mut(call_id) {
                output.push_str(&delta);
            }
        }
        Event::ToolCallDone { call_id, result } => {
            let mut todo = None;
            if let Some(Cell::Tool {
                call, result: r, ..
            }) = state.tool_mut(call_id)
            {
                if call.name == "todo" && result.ok {
                    todo = result.todo_list();
                }
                *r = Some(result);
            }
            if let Some(todo) = todo {
                state.todo = todo;
            }
            cmds = crate::item_render::ask(state, CellRef::Call(call_id));
            // A `!` line has no `TurnDone`; its card closing ends it, and
            // a message queued behind it goes out as a turn would release it.
            if state.shell_call == Some(call_id) {
                state.shell_call = None;
                state.status.busy = false;
                if let Some(text) = state.queue.pop_front() {
                    cmds.push(Cmd::Submit(user_turn(state, text)));
                }
            }
        }
        Event::ApprovalRequired { call, why, source } => {
            cmds = notify(state, format!("approval: {} {}", call.name, call.subject));
            let agent = source.and_then(|s| s.agent);
            state.modal = Some(Modal::Approval(Approval::new(call, why).from_agent(agent)));
        }
        Event::ApprovalDecided { .. } => state.modal = None,
        Event::TurnStarted {
            seq, tier, model, ..
        } => {
            state.current_seq = seq;
            state.status.busy = true;
            // A `!` the core refused never got its card.
            state.shell = None;
            state.status.tier = Some(tier);
            state.status.model = model.to_string();
        }
        Event::TurnDone { stop, .. } => {
            state.status.busy = false;
            // An interrupt is the user's own doing; they are already here.
            if stop != StopReason::Interrupted {
                cmds = notify(state, "turn done".to_string());
            }
            cmds.extend(turn_done_cmds(state, stop));
        }
        Event::Usage { usage, .. } => {
            state.status.cost_usd += usage.cost_usd;
            state.status.context_tokens = usage.context_tokens();
            state.status.cache_ratio = cox_core::cache_diag::ratio_of(&usage);
        }
        Event::ModelSwitched { tier, to, .. } => {
            if state.status.tier == Some(tier) {
                state.status.model = to.to_string();
            }
        }
        // T22.1, DT G4: `ask_user` (or a subagent's, T34.3) waits in a modal.
        Event::QuestionAsked {
            call_id,
            question,
            options,
            source,
        } => {
            cmds = notify(state, format!("question: {question}"));
            state.modal = Some(Modal::Question(
                Question::new(call_id, question, options).from_agent(source.and_then(|s| s.agent)),
            ));
        }
        // The core's word on mode and effort replaces what a key or a
        // slash command set ahead of it (DT G5).
        Event::StateChanged { mode, effort } => {
            state.mode = mode;
            state.status.effort = effort;
        }
        // A113: the status line shows it; the sessions picker reads the store.
        Event::TitleSet { title, .. } => state.title = Some(title),
        // A98: the status line's share and the `/context` overlay.
        Event::ContextBreakdown { breakdown, .. } => {
            if let Some(window) = breakdown.window.filter(|w| *w > 0) {
                state.status.context_window = window;
            }
            state.status.context = Some(breakdown);
        }
        // T33.20: the `TurnStarted` that follows already carries the
        // advised tier and model, so the status line needs nothing more.
        Event::Advised { .. } => {}
        Event::TaskCreated { task, label, tier } => {
            state.tasks.push((task, label, tier, state.tick, None));
        }
        Event::TaskCompleted {
            task,
            exit_code,
            archive,
            ..
        } => {
            if let Some(i) = state.tasks.iter().position(|(t, ..)| *t == task) {
                let (_, label, ..) = state.tasks.remove(i);
                let line = tasks::finished_line(task, &label, exit_code, archive);
                state.finished_tasks.push(line);
                let over = state
                    .finished_tasks
                    .len()
                    .saturating_sub(tasks::FINISHED_KEPT);
                state.finished_tasks.drain(..over);
            }
        }
        // T34.7/SM§6: `task` is always the task this message concerns —
        // the addressee delivered to (`deliver`), or, when a child speaks
        // to the parent (`message_parent`), its own id doubling as `from`
        // (`tasks.rs`: "the parent has no id"). Resolved once here to the
        // task's registered label, since a finished task is already gone
        // from `state.tasks` by the time an in-flight reply renders.
        Event::TaskMessage {
            task, from, text, ..
        } => {
            let label = state
                .tasks
                .iter()
                .find(|(t, ..)| *t == task)
                .map_or_else(|| task.to_string(), |(_, label, ..)| label.clone());
            let label = crate::text::sanitize(&label);
            let text = crate::text::sanitize(&text);
            if let Some(entry) = state.tasks.iter_mut().find(|(t, ..)| *t == task) {
                entry.4 = Some(text.lines().next().unwrap_or("").to_string());
            }
            state.transcript.push(Cell::TaskMessage {
                label,
                from_task: from == Some(task),
                text,
            });
        }
        Event::Notice { level, text } => state.transcript.push(Cell::Notice { level, text }),
        Event::Error { error, fatal } => state.transcript.push(Cell::Error {
            text: error.to_string(),
            fatal,
        }),
        Event::Checkpoint { files, .. } => {
            if let Some(turn) = state.turns.last_mut() {
                turn.files += files.len();
            }
        }
        Event::Rewound {
            to_turn,
            conversation,
            ..
        } => {
            if conversation && let Some(at) = state.turns.iter().position(|t| t.seq >= to_turn) {
                let cut = state.turns[at].cell_at;
                state.transcript.truncate(cut);
                state.turns.truncate(at);
            }
        }
        Event::ModeChanged {
            mode,
            permission_mode,
        } => mode_changed(state, mode, permission_mode),
        Event::SessionStarted { .. }
        | Event::Compacted { .. }
        | Event::GrantRevoked { .. }
        | Event::RepoMapBuilt { .. } => {}
    }
    cmds
}

/// `Event::TurnDone` (T25.1): a natural finish drains the queue's head as
/// the next turn; an interrupt from `send_now` instead joins everything
/// queued (composer text included, folded in by `send_now`) into the one
/// turn `Ctrl+Enter`/`Alt+Enter` asked for. A plain `Ctrl+C` interrupt
/// (`send_now` unset) leaves the queue untouched — the user cancelled, they
/// did not ask to send it.
fn turn_done_cmds(state: &mut State, stop: StopReason) -> Vec<Cmd> {
    let text = match stop {
        StopReason::Interrupted if state.send_now => {
            state.send_now = false;
            let joined = state.queue.drain(..).collect::<Vec<_>>().join("\n\n");
            (!joined.is_empty()).then_some(joined)
        }
        StopReason::Interrupted => None,
        _ => state.queue.pop_front(),
    };
    match text {
        Some(text) => vec![Cmd::Submit(user_turn(state, text))],
        None => Vec::new(),
    }
}

/// `Event::ModeChanged` (P42): the core's word on the mode. Entering
/// architect asks the think price once per stretch, reusing the `ask_user`
/// modal; leaving it forgets the answer, so the next stretch asks again.
fn mode_changed(state: &mut State, mode: SessionMode, permission_mode: PermissionMode) {
    state.mode = permission_mode;
    state.session_mode = mode;
    if mode != SessionMode::Architect {
        state.think_confirmed = false;
        return;
    }
    let asking = matches!((&state.modal, state.think_consent),
        (Some(Modal::Question(q)), Some(id)) if q.call == id);
    if state.think_confirmed || asking {
        return;
    }
    let question = format!(
        "architect mode runs main turns on the think tier ({}); use it?",
        cox_core::router::THINK_PRICE
    );
    if state.modal.is_none() {
        let id = CallId::new();
        state.think_consent = Some(id);
        state.modal = Some(Modal::Question(Question::new(
            id,
            question,
            vec!["yes".into(), "no".into()],
        )));
    } else {
        // The one modal slot is taken: the first turn's refusal names the
        // price instead, and `/mode architect` asks again.
        notice(
            state,
            Level::Warn,
            "architect: think price not confirmed yet; run /mode architect again".into(),
        );
    }
}

/// The think-price question's answer (P42). Yes confirms think for the
/// rest of this architect stretch; anything else leaves architect.
pub(super) fn think_consent_answered(state: &mut State, answer: QuestionAnswer) -> Vec<Cmd> {
    state.think_consent = None;
    let yes = matches!(&answer, QuestionAnswer::Text(t)
        if matches!(t.trim().to_ascii_lowercase().as_str(), "y" | "yes"));
    if yes {
        state.think_confirmed = true;
        return Vec::new();
    }
    vec![Cmd::Submit(Submission::Command {
        command: SlashCommand {
            name: "mode".into(),
            args: vec!["editor".into()],
        },
    })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testkit::type_command;
    use crate::state::{Msg, update};
    use cox_protocol::ids::{ItemId, TurnId};
    use cox_protocol::types::{ContextBreakdown, SandboxMode, ToolCall};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// T23.5: `auto` rings on a finished turn, an approval and a question
    /// only while the terminal is unfocused; `always` ignores focus, `off`
    /// never rings, and an interrupt the user made never does.
    #[test]
    fn update_emits_notify_only_when_unfocused() {
        let turn_done = |stop| {
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop,
            })
        };
        let rings = |cmds: &[Cmd]| cmds.iter().any(|c| matches!(c, Cmd::Notify { .. }));
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert!(!rings(&update(&mut state, turn_done(StopReason::EndTurn))));

        update(&mut state, Msg::Focus(false));
        assert_eq!(
            update(&mut state, turn_done(StopReason::EndTurn)),
            vec![Cmd::Notify {
                title: "cox".into(),
                body: "turn done".into(),
            }]
        );
        assert!(!rings(&update(
            &mut state,
            turn_done(StopReason::Interrupted)
        )));
        let call = ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: serde_json::Value::Null,
            risk: cox_protocol::types::Risk::Exec,
            subject: "cargo test".into(),
            segments: None,
        };
        let why = cox_protocol::types::Why::Risk {
            risk: cox_protocol::types::Risk::Exec,
        };
        let cmds = update(
            &mut state,
            Msg::Event(Event::ApprovalRequired {
                call,
                why,
                source: None,
            }),
        );
        assert!(
            matches!(&cmds[..], [Cmd::Notify { body, .. }] if body == "approval: bash cargo test")
        );
        let question = Msg::Event(Event::QuestionAsked {
            call_id: CallId::new(),
            question: "which?".into(),
            options: Vec::new(),
            source: None,
        });
        assert!(rings(&update(&mut state, question)));

        update(&mut state, Msg::Focus(true));
        state.notify = Notify::Always;
        assert!(rings(&update(&mut state, turn_done(StopReason::EndTurn))));
        state.notify = Notify::Off;
        update(&mut state, Msg::Focus(false));
        assert!(!rings(&update(&mut state, turn_done(StopReason::EndTurn))));
    }

    /// A98: `Event::ContextBreakdown` sets the window `ctx N%` divides by
    /// and keeps the split; one with no window leaves the last known one,
    /// and `/context` opens the overlay without asking the core.
    #[test]
    fn context_breakdown_sets_the_window_and_the_split() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let event = |window| {
            Msg::Event(Event::ContextBreakdown {
                turn: TurnId::new(),
                breakdown: ContextBreakdown {
                    window,
                    total: 50_000,
                    system: 5_000,
                    tools: 20_000,
                    instructions: 1_000,
                    history: 24_000,
                    cached: 0,
                },
            })
        };
        update(&mut state, event(Some(1_000_000)));
        assert_eq!(state.status.context_window, 1_000_000);
        assert_eq!(state.status.context_used(), 50_000, "estimate before usage");
        update(&mut state, event(None));
        assert_eq!(state.status.context_window, 1_000_000);
        assert_eq!(state.status.context.and_then(|b| b.window), None);
        assert_eq!(type_command(&mut state, "/context"), Vec::new());
        assert_eq!(state.modal, Some(Modal::Context));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(state.modal, None);
    }

    #[test]
    fn empty_signed_thinking_draws_no_cell() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let before = state.transcript.len();
        let signed = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: Some("sig".into()),
        };
        update(
            &mut state,
            Msg::Event(Event::ItemStarted { item: signed, kind }),
        );
        update(&mut state, Msg::Event(Event::ItemDone { item: signed }));
        assert_eq!(state.transcript.len(), before);

        // A streamed thought starts empty too, but unsigned: it keeps its cell.
        let streamed = ItemId::new();
        let kind = ItemKind::Thinking {
            text: String::new(),
            signature: None,
        };
        update(
            &mut state,
            Msg::Event(Event::ItemStarted {
                item: streamed,
                kind,
            }),
        );
        assert!(matches!(
            state.transcript.last(),
            Some(Cell::Thinking { item, .. }) if *item == streamed
        ));
    }

    /// P42: entering architect asks the think price once; a yes rides on
    /// every architect turn, a repeat `/mode architect` does not ask again,
    /// leaving forgets it, and a no sends `/mode editor`.
    #[test]
    fn architect_confirmation_is_asked_once() {
        let key = |c| Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        let architect = Event::ModeChanged {
            mode: SessionMode::Architect,
            permission_mode: PermissionMode::Plan,
        };
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        update(&mut state, Msg::Event(architect.clone()));
        assert!(matches!(state.modal, Some(Modal::Question(_))));
        assert_eq!(state.mode, PermissionMode::Plan);
        // `1` picks "yes"; the answer never reaches the core.
        assert_eq!(update(&mut state, key('1')), Vec::new());
        assert!(state.modal.is_none());
        let confirmed = |state: &State| {
            matches!(
                user_turn(state, "go".into()),
                Submission::UserTurn {
                    confirm_think: true,
                    ..
                }
            )
        };
        assert!(confirmed(&state));

        update(&mut state, Msg::Event(architect.clone()));
        assert!(state.modal.is_none(), "asked once per stretch");

        update(
            &mut state,
            Msg::Event(Event::ModeChanged {
                mode: SessionMode::Editor,
                permission_mode: PermissionMode::Default,
            }),
        );
        assert!(!confirmed(&state), "editor turns never confirm think");

        update(&mut state, Msg::Event(architect));
        assert!(matches!(state.modal, Some(Modal::Question(_))));
        // `2` picks "no": leave architect.
        assert_eq!(
            update(&mut state, key('2')),
            vec![Cmd::Submit(Submission::Command {
                command: SlashCommand {
                    name: "mode".into(),
                    args: vec!["editor".into()],
                },
            })]
        );
        assert!(!confirmed(&state));
    }
}
