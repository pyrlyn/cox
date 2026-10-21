// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Runs a slash command's `Action`: `/rewind`, `/agents`, `/mode`,
//! `/plugin`, `/loop`, `/theme` and the rest. Separate from `update` so the
//! one `match` over every `commands::Action` and the helpers only it needs
//! sit together, apart from key and event handling.

use cox_protocol::ids::{SessionId, TaskId};
use cox_protocol::types::{Level, Presence, Submission, Tier};

use super::{
    Ask, Cell, Cmd, Loop, Modal, PluginMgmtRequest, PluginNewRequest, PluginRequest, State,
    apply_theme_choice, set_mode,
};
use crate::commands::{self, Action};
use crate::modal::RemoveConfirm;
use crate::picker::{self, Kind, Picker};
use crate::tasks;

pub(super) fn notice(state: &mut State, level: Level, text: String) {
    state.transcript.push(Cell::Notice { level, text });
}

/// `/rewind` and `Esc Esc`: the timeline, newest first.
pub(super) fn open_rewind(state: &mut State) -> Vec<Cmd> {
    if state.status.busy {
        notice(
            state,
            Level::Warn,
            "rewind: interrupt the turn first".into(),
        );
        return Vec::new();
    }
    if state.turns.is_empty() {
        notice(state, Level::Info, "nothing to rewind yet".into());
        return Vec::new();
    }
    let rows = state
        .turns
        .iter()
        .rev()
        .map(|t| picker::turn_entry(t.seq, t.files, &t.text))
        .collect();
    state.modal = Some(Modal::Picker(Picker::open(Kind::Rewind, rows)));
    Vec::new()
}

/// `/agents` (T27.2, one row per T27.5): one row per live agent — a
/// sibling cox session (T16.1 presence) or a subagent/background task this
/// session started (`state.tasks`, fed by `TaskCreated`/`TaskCompleted`).
/// The narrow row the creator chose over a new `Event::AgentProgress`:
/// name, preset, tier, cost, elapsed, state. Presence carries none of
/// preset/tier/cost/elapsed, so those show `-`; a task's cost shows `-`
/// too while it runs — it is only known once `TaskCompleted` retires it
/// from `state.tasks`. One line each (T27.5: a `Modal::Agents` row cannot
/// span lines, unlike T27.2's two-line `Notice` card), paired with the
/// `SessionId` `Enter` fetches a rollout for — `None` for a task, which has
/// no resumable id on the wire yet (plan.md §3 P27).
fn agents_rows(
    agents: &[Presence],
    tasks: &[(TaskId, String, Tier, u64, Option<String>)],
    tick: u64,
    worktree_glyph: &str,
) -> Vec<(String, Option<SessionId>)> {
    let mut rows: Vec<(String, Option<SessionId>)> = agents
        .iter()
        .map(|a| {
            let mut text = format!(
                "{} · preset - · tier - · cost - · elapsed - · {}",
                a.session,
                a.status.name()
            );
            // T44.3: which worktree the session holds, by its directory name
            // (the full path is the project's `_worktrees/` prefix again),
            // behind the status line's worktree glyph.
            if let Some(name) = a.worktree.as_deref().and_then(std::path::Path::file_name) {
                text.push_str(&format!(" · {worktree_glyph} {}", name.to_string_lossy()));
            }
            (crate::text::sanitize(&text), Some(a.session))
        })
        .collect();
    rows.extend(tasks.iter().map(|(_, label, tier, started, last)| {
        let preset = label.split_once(": ").map_or("-", |(p, _)| p);
        let elapsed = tick.saturating_sub(*started);
        let mut text = format!(
            "{label} · preset {preset} · tier {} · cost - · elapsed {}.{}s · running",
            format!("{tier:?}").to_lowercase(),
            elapsed / 10,
            elapsed % 10
        );
        // T34.7/SM§6: the narrow card grows a last-message line instead of
        // a new progress event stream — already sanitized in `update`, but
        // re-run through `sanitize` with the rest of the row, same as the
        // label above (`crate::text::sanitize` is idempotent).
        if let Some(last) = last {
            text.push_str(&format!(" · last: {last}"));
        }
        (crate::text::sanitize(&text), None)
    }));
    rows
}

/// A slash command's effect; anything the core owns becomes a `Submit`.
pub(super) fn act(state: &mut State, action: Action) -> Vec<Cmd> {
    match action {
        Action::Submit(sub) => {
            if let Submission::SetEffort { effort } = &sub {
                state.status.effort = *effort;
            }
            if let Submission::Command { command } = &sub
                && command.name == "clear"
            {
                state.queue.clear();
                // T27.4: a fresh session should not keep firing an old
                // loop's prompt into it.
                state.active_loop = None;
                return vec![Cmd::Clear];
            }
            return vec![Cmd::Submit(sub)];
        }
        Action::Quit => return vec![Cmd::Quit],
        Action::Mode(mode) => return set_mode(state, mode),
        Action::Help => {
            let text = commands::help(&state.keymap);
            notice(state, Level::Info, text);
        }
        Action::Context => state.modal = Some(Modal::Context),
        // The proposal is a model call, so say so before the review opens.
        Action::Refine(request) => {
            if matches!(request, crate::refine::Request::Propose { .. }) {
                notice(
                    state,
                    Level::Info,
                    "refine: asking the cheap tier for a proposal".into(),
                );
            }
            return vec![Cmd::Ask(Ask::Refine(request))];
        }
        Action::Cost => {
            let s = &state.status;
            let text = format!(
                "${:.2} this session · {} tokens in context",
                s.cost_usd, s.context_tokens
            );
            notice(state, Level::Info, text);
        }
        Action::Todo => state.show_todo = !state.show_todo,
        Action::Tasks => {
            let running: Vec<(TaskId, String)> = state
                .tasks
                .iter()
                .map(|(id, label, ..)| (*id, label.clone()))
                .collect();
            notice(
                state,
                Level::Info,
                tasks::list(&running, &state.finished_tasks),
            )
        }
        Action::Vim => {
            let on = state.composer.vim_mode().is_none();
            state.composer.set_vim(on);
        }
        // T27.5: an empty list stays the T27.2 `Notice` (nothing to
        // navigate); otherwise `/agents` opens the navigable overlay.
        Action::Agents => {
            let entries = agents_rows(
                &state.agents,
                &state.tasks,
                state.tick,
                state.glyphs.worktree,
            );
            if entries.is_empty() {
                notice(state, Level::Info, "no live agents".to_string());
            } else {
                let (rows, ids) = entries.into_iter().unzip();
                state.modal = Some(Modal::Agents {
                    rows,
                    ids,
                    selected: 0,
                });
            }
        }
        Action::Sessions => {
            let text = if state.sessions.is_empty() {
                "no sessions for this project yet".to_string()
            } else {
                state
                    .sessions
                    .iter()
                    .map(|(id, row)| format!("{id} · {row}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            notice(state, Level::Info, text);
        }
        Action::Resume => {
            let rows = state.sessions.iter().map(|(_, row)| row.clone()).collect();
            state.modal = Some(Modal::Picker(Picker::open(Kind::Sessions, rows)));
        }
        Action::Notice(text) => notice(state, Level::Warn, text),
        // The core refuses one mid-turn anyway; saying so here keeps the
        // pending card from latching onto the turn's next `bash` call.
        Action::Shell { .. } if state.status.busy => {
            notice(
                state,
                Level::Warn,
                "a turn is running; `!` waits until it ends".into(),
            );
        }
        Action::Shell { cmd, share } => {
            state.shell = Some(cmd.clone());
            return vec![Cmd::Submit(Submission::UserShell {
                command: cmd,
                share,
            })];
        }
        Action::Rewind => return open_rewind(state),
        Action::Undo => match state.turns.last() {
            Some(t) => {
                return vec![Cmd::Submit(Submission::Rewind {
                    to_turn: t.seq,
                    code: true,
                    conversation: false,
                })];
            }
            None => notice(state, Level::Warn, "undo: no turn yet".into()),
        },
        Action::Redo => return vec![Cmd::Submit(Submission::Redo)],
        // A running turn would be cut mid-write, so both wait for it.
        Action::Fork(_) | Action::Handoff(_) if state.status.busy => {
            notice(state, Level::Warn, "interrupt the turn first".into());
        }
        Action::Fork(Some(turn)) if !state.turns.iter().any(|t| t.seq == turn) => {
            let text = format!("fork: no turn T{turn}; /rewind lists them");
            notice(state, Level::Warn, text);
        }
        Action::Fork(turn) => {
            state.queue.clear();
            return vec![Cmd::Fork(turn)];
        }
        Action::Handoff(objective) => {
            state.queue.clear();
            return vec![Cmd::Handoff(objective)];
        }
        Action::Theme(Some(name)) => {
            if state.theme_rows.contains(&name) {
                return apply_theme_choice(state, &name);
            }
            notice(
                state,
                Level::Warn,
                format!("unknown theme {name:?}; /theme lists them"),
            );
        }
        Action::Theme(None) => {
            state.theme_prev = Some((state.dark, state.theme, state.syntax_theme));
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::Themes,
                state.theme_rows.clone(),
            )));
        }
        // T27.4: `interval` is whole seconds (`parse_interval`), so
        // `* 10` (100 ms ticks) never loses precision; a bare `--budget`
        // defaults to the session cap the status line already shows.
        Action::LoopStart {
            interval,
            prompt,
            budget_usd,
        } => {
            let interval_ticks = (interval.as_secs() * 10).max(1);
            let budget_usd = budget_usd.unwrap_or(state.status.budget_cap_usd);
            state.active_loop = Some(Loop {
                prompt,
                interval_ticks,
                next_at: state.tick + interval_ticks,
                budget_usd,
                started_cost_usd: state.status.cost_usd,
                iterations: 0,
            });
            let text = format!(
                "loop started: every {}s, budget ${budget_usd:.2}",
                interval.as_secs()
            );
            notice(state, Level::Info, text);
        }
        Action::LoopStop => match state.active_loop.take() {
            Some(_) => notice(state, Level::Info, "loop stopped".into()),
            None => notice(state, Level::Warn, "no loop running".into()),
        },
        // T33.25, PL§8: `crates/cox`'s `plugin_ui::answer` runs
        // `cox_command` and the answer comes back as
        // `Msg::Plugin(PluginUiMsg::Command)`, applied in `plugin_command_out`.
        Action::PluginCommand { plugin, name, args } => {
            return vec![Cmd::Plugin(PluginRequest::Command { plugin, name, args })];
        }
        // T33.30: `--lang` already named a language, so there is nothing
        // to pick — the request goes straight to the runtime, which is the
        // one place that validates it (`plugin_new::scaffold`'s errors).
        Action::PluginNew {
            name,
            lang: Some(lang),
            with,
        } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::New(PluginNewRequest {
                name,
                lang,
                with,
            }))];
        }
        // No `--lang`: stash `name`/`with` and ask the picker, the same
        // two-step shape `Action::Rewind` uses for its turn-then-what picks.
        Action::PluginNew {
            name,
            lang: None,
            with,
        } => {
            state.pending_plugin_new = Some((name, with));
            state.modal = Some(Modal::Picker(Picker::open(
                Kind::PluginLang,
                picker::PLUGIN_LANGS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )));
        }
        // T33.33: `list`/`update` need no confirmation — the runtime
        // already pre-approves them the way `--yes` would, since the TUI's
        // raw mode has no stdin to prompt a widened capability list on.
        Action::PluginList { json } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::List { json })];
        }
        Action::PluginUpdate {
            ids,
            all,
            check,
            rollback,
        } => {
            return vec![Cmd::PluginMgmt(PluginMgmtRequest::Update {
                ids,
                all,
                check,
                rollback,
            })];
        }
        // `remove` is the one irreversible plugin action, so it asks
        // through `Modal::PluginRemove` first — `on_key`'s handler for it
        // sends `Cmd::PluginMgmt(PluginMgmtRequest::Remove)` only on `y`.
        Action::PluginRemove { id, keep_data } => {
            state.modal = Some(Modal::PluginRemove(RemoveConfirm { id, keep_data }));
        }
        // `/plugin reload` means `/clear`: the plugin host only reloads a
        // manifest at session open, so restarting the cache prefix is the
        // only way to pick up a changed one — mirrors the `command.name ==
        // "clear"` handling below, plus a notice explaining why.
        Action::PluginReload => {
            state.queue.clear();
            state.active_loop = None;
            notice(
                state,
                Level::Info,
                "plugin reload: restarting session to reload plugins".into(),
            );
            return vec![Cmd::Clear];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use cox_protocol::types::{PermissionMode, SandboxMode};
    use crossterm::event::{KeyCode, KeyEvent};

    use super::super::{Cmd, Msg, State, update};

    #[test]
    fn clear_command_emits_cmd_clear() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        // `/` at column 0 opens the palette; Esc leaves `/` in the composer.
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        for c in "clear".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(cmds, vec![Cmd::Clear]);
    }
}
