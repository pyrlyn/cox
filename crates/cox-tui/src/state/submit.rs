// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What leaves the composer: a line becomes a turn, a slash command, an
//! `@agent` dispatch (T45.6) or a queued message (T25.1); `Ctrl+Enter` sends
//! now, and `/loop` (T27.4) fires its prompt on the tick clock. Apart from the
//! keys because every TUI source of a user turn (`Enter`, the queue, `/loop`,
//! a plugin prompt) ends in `user_turn` here.

use cox_protocol::types::{Level, Mode as SessionMode, PermissionMode, Submission, Tier};
use crossterm::event::KeyEvent;

use super::{Cmd, Modal, State, act, file_command, notice, plugin_command};
use crate::commands;
use crate::composer::Edit;
use crate::picker::{self, Kind, Picker};

/// `/loop`'s running state (T27.4). `interval_ticks`/`next_at` are
/// `State::tick` units (100 ms each — the same clock `cells.rs` already
/// drives elapsed time and spinners from) rather than the wall clock, so a
/// replayed `Msg::Tick` stream behaves identically in a test and for real.
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    pub prompt: String,
    pub interval_ticks: u64,
    pub next_at: u64,
    /// This loop's own spend cap (`/loop`'s `--budget`, default: the
    /// session cap `status.budget_cap_usd`); `budget.session_usd` still
    /// applies underneath, enforced by the core as always.
    pub budget_usd: f64,
    /// `status.cost_usd` when the loop started, so its own cap tracks only
    /// what the loop itself has spent, not the whole session.
    pub started_cost_usd: f64,
    pub iterations: u32,
}

/// A key for the composer, and what its `Edit` means for the session.
pub(crate) fn compose(state: &mut State, key: KeyEvent) -> Vec<Cmd> {
    match state.composer.key(key, state.status.busy) {
        Edit::Submit(text) => {
            if let Some((name, task)) = agent_line(&state.agent_names, &text) {
                return user_agent(state, name, task);
            }
            let tier = state.status.tier.unwrap_or(Tier::Code);
            // T22.2: a file command's name reaches the core as
            // `Submission::Command`; the T5.5 parser owns the
            // built-ins and would answer these with a notice.
            // T33.25, PL§8: a `/<id>:<name>` line calls the plugin instead.
            match file_command(&state.commands, &text)
                .or_else(|| plugin_command(&state.commands, &text))
                .or_else(|| commands::parse(&text, tier))
            {
                Some(action) => act(state, action),
                // A turn is running: queue instead of submitting
                // (T25.1). A slash command still runs immediately
                // above — `/clear` in particular must reach the
                // queue it is about to empty.
                None if state.status.busy => {
                    state.queue.push_back(text);
                    Vec::new()
                }
                None => vec![Cmd::Submit(user_turn(state, text))],
            }
        }
        Edit::SendNow(text) => send_now(state, text),
        Edit::OpenFiles => {
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::Files,
                picker::at_candidates(&state.agent_names, &state.files),
            )));
            Vec::new()
        }
        Edit::OpenCommands => {
            let names = state.commands.iter().map(|(n, ..)| n.clone()).collect();
            state.modal = Some(Modal::Picker(Picker::open(Kind::Commands, names)));
            Vec::new()
        }
        Edit::OpenHistory => {
            // Newest first: the entry wanted is usually the last one.
            let mut history = state.composer.history().to_vec();
            history.reverse();
            // T25.8: this project's other sessions after this one's own.
            history.extend(state.past_prompts.iter().map(|(row, _)| row.clone()));
            state.modal = Some(Modal::Picker(Picker::open(Kind::History, history)));
            Vec::new()
        }
        Edit::Nothing => Vec::new(),
    }
}

pub(super) fn set_mode(state: &mut State, mode: PermissionMode) -> Vec<Cmd> {
    state.mode = mode;
    vec![Cmd::Submit(Submission::SetPermissionMode { mode })]
}

/// `Ctrl+Enter`/`Alt+Enter` while a turn runs (T25.1): the composer already
/// cleared itself (`Edit::SendNow`); its text joins the queue's tail so the
/// interrupt this triggers, once `TurnDone{Interrupted}` acknowledges it,
/// flushes everything typed so far as one turn instead of leaving it queued
/// for the next natural finish.
/// `@name task` at line start with a dispatchable `name` and a task
/// (T45.6); anything else — an `@file` mention included — is a normal turn.
fn agent_line(names: &[String], text: &str) -> Option<(String, String)> {
    let (name, task) = text.strip_prefix('@')?.split_once(' ')?;
    let task = task.trim();
    (names.iter().any(|n| n == name) && !task.is_empty())
        .then(|| (name.to_string(), task.to_string()))
}

/// Submits `@name task`; mid-turn it is refused here, as `!` is, since the
/// core would refuse it anyway.
fn user_agent(state: &mut State, name: String, task: String) -> Vec<Cmd> {
    if state.status.busy {
        let text = format!("a turn is running; `@{name}` waits until it ends");
        notice(state, Level::Warn, text);
        return Vec::new();
    }
    vec![Cmd::Submit(Submission::UserAgent { name, task })]
}

fn send_now(state: &mut State, text: String) -> Vec<Cmd> {
    if !text.trim().is_empty() {
        state.queue.push_back(text);
    }
    state.send_now = true;
    vec![Cmd::Submit(Submission::Interrupt)]
}

/// `Msg::Tick` (T27.4): `/loop`'s timer. A due loop fires through the same
/// path an idle `Enter` uses — a direct `Submit`, not the T25.1 queue,
/// which only defers while busy — so a turn still running just waits for a
/// later tick instead of piling up. The loop's own budget is checked first,
/// so a due-but-over-budget tick stops it instead of firing once more.
pub(super) fn loop_tick(state: &mut State) -> Vec<Cmd> {
    let Some(lp) = state.active_loop.as_ref() else {
        return Vec::new();
    };
    if state.status.cost_usd - lp.started_cost_usd >= lp.budget_usd {
        state.active_loop = None;
        notice(state, Level::Warn, "loop stopped: budget reached".into());
        return Vec::new();
    }
    if state.status.busy || state.tick < lp.next_at {
        return Vec::new();
    }
    let prompt = lp.prompt.clone();
    let interval_ticks = lp.interval_ticks;
    if let Some(lp) = state.active_loop.as_mut() {
        lp.next_at = state.tick + interval_ticks;
        lp.iterations += 1;
    }
    vec![Cmd::Submit(user_turn(state, prompt))]
}

/// A user turn from any TUI source. In architect it carries the think
/// consent the user gave once for the stretch (P42, invariant 9); anywhere
/// else `confirm_think` would move the turn to think, so it stays off.
pub(super) fn user_turn(state: &State, text: String) -> Submission {
    Submission::UserTurn {
        text,
        attachments: Vec::new(),
        confirm_think: state.session_mode == SessionMode::Architect && state.think_confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testkit::{type_after_sigil, type_command, type_line};
    use crate::state::{Msg, update};
    use cox_protocol::ids::TurnId;
    use cox_protocol::types::{Event, SandboxMode, StopReason};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::collections::VecDeque;

    /// T45.6: `@<agent> task` at line start runs that agent directly.
    #[test]
    fn at_agent_name_submits_user_agent() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.agent_names = vec!["explore".into(), "reviewer".into()];
        let cmds = type_after_sigil(&mut state, '@', "@explore find the router");
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserAgent {
                name: "explore".into(),
                task: "find the router".into(),
            })]
        );
    }

    /// T45.6: an `@file` mention, an unknown name and a bare agent name
    /// with no task all stay ordinary user turns.
    #[test]
    fn at_file_path_stays_a_user_turn() {
        for line in ["@src/main.rs explain it", "@nope do it", "@explore"] {
            let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
            state.agent_names = vec!["explore".into()];
            state.files = vec!["src/main.rs".into()];
            let cmds = type_after_sigil(&mut state, '@', line);
            assert_eq!(
                cmds,
                vec![Cmd::Submit(Submission::UserTurn {
                    text: line.into(),
                    attachments: Vec::new(),
                    confirm_think: false,
                })],
                "{line}"
            );
        }
    }

    /// T25.1 step 1/3: `Enter` while a turn runs queues instead of
    /// submitting, and each natural `TurnDone` drains the queue's head in
    /// the order the messages were typed.
    #[test]
    fn queued_messages_drain_in_order() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        type_line(&mut state, "second");
        assert_eq!(
            state.queue,
            VecDeque::from(["first".to_string(), "second".to_string()])
        );
        assert!(state.composer.is_empty());

        let done = |stop| {
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop,
            })
        };
        let cmds = update(&mut state, done(StopReason::EndTurn));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "first".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert_eq!(state.queue, VecDeque::from(["second".to_string()]));

        let cmds = update(&mut state, done(StopReason::EndTurn));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "second".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert!(state.queue.is_empty());
    }

    /// T25.1 step 4: `Ctrl+Enter` while a turn runs interrupts it and, once
    /// the interrupt lands, joins the queue with whatever was still in the
    /// composer into one turn.
    #[test]
    fn send_now_interrupts_and_flushes() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        for c in "second".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }

        let cmds = update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
        );
        assert_eq!(cmds, vec![Cmd::Submit(Submission::Interrupt)]);
        assert!(state.composer.is_empty());
        assert_eq!(
            state.queue,
            VecDeque::from(["first".to_string(), "second".to_string()])
        );

        let cmds = update(
            &mut state,
            Msg::Event(Event::TurnDone {
                turn: TurnId::new(),
                stop: StopReason::Interrupted,
            }),
        );
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "first\n\nsecond".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
        assert!(state.queue.is_empty());
    }

    /// T25.1 step 1: `Ctrl+U` on an empty composer pops the queue's tail
    /// back for editing.
    #[test]
    fn ctrl_u_unqueues_last() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.status.busy = true;
        type_line(&mut state, "first");
        type_line(&mut state, "second");

        let cmds = update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        );
        assert!(cmds.is_empty());
        assert_eq!(state.composer.text(), "second");
        assert_eq!(state.queue, VecDeque::from(["first".to_string()]));
    }

    /// T27.4: `/loop`'s timer fires a direct `Submit` (not the T25.1 queue)
    /// once the session is idle and the tick it scheduled arrives; earlier
    /// ticks are silent.
    #[test]
    fn loop_enqueues_when_due_and_idle() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        // The shortest interval `parse_interval` accepts, 1s = 10 ticks.
        type_command(&mut state, "/loop 1s go");
        assert!(state.active_loop.is_some());
        for _ in 0..9 {
            assert!(update(&mut state, Msg::Tick).is_empty());
        }
        assert_eq!(
            update(&mut state, Msg::Tick),
            vec![Cmd::Submit(Submission::UserTurn {
                text: "go".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
    }

    /// T27.4: `/loop stop` and an idle empty-composer `Esc` both end a
    /// running loop; `Esc` takes it before Esc-Esc's rewind-timeline role.
    #[test]
    fn loop_stop_and_esc_both_end_a_running_loop() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/loop 1h go");
        assert!(state.active_loop.is_some());
        type_command(&mut state, "/loop stop");
        assert!(state.active_loop.is_none());

        type_command(&mut state, "/loop 1h go");
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(cmds.is_empty());
        assert!(state.active_loop.is_none());
        assert!(
            state.esc_armed.is_none(),
            "the stop consumed the Esc, not the Esc-Esc rewind timer"
        );
    }

    /// T27.4: once the loop's own spend equals its budget, the next tick
    /// stops it instead of firing another turn.
    #[test]
    fn loop_stops_itself_when_its_own_budget_is_spent() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/loop 1s go --budget 1.00");
        state.status.cost_usd = 1.00;
        assert_eq!(update(&mut state, Msg::Tick), Vec::new());
        assert!(state.active_loop.is_none());
    }
}
