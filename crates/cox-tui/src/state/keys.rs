// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A key press (`Msg::Key`): the keymap's actions (T25.5) first, then the open
//! modal's own keys, then the composer, plus the copy and fold toggles the
//! actions share. Apart from `update` because `on_key` is the TUI's largest
//! single rule and the keys own their own timing (`ESC_ESC_TICKS`) and paging
//! (`DIFF_PAGE`).

use cox_protocol::types::{Level, Submission};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::cell::cell_text;
use super::event::think_consent_answered;
use super::{
    Ask, Cell, Cmd, GrantDecision, Modal, PluginMgmtRequest, PluginNewRequest, PluginRequest,
    State, apply_theme_choice, compose, notice, open_rewind, open_theme_editor, preview_theme,
    restore_theme, set_mode,
};
use crate::commands::{self, Context};
use crate::keymap;
use crate::modal::QuestionAnswer;
use crate::picker::{self, Kind, Pick, Picker};
use crate::theme_editor::EditorOutcome;
use crate::vim::Mode;

/// Lines a `PageUp`/`PageDown` moves the diff view (the inline viewport is
/// 15 rows, so a page is a little less).
const DIFF_PAGE: usize = 10;

/// Ticks (100 ms each) two `Esc`s may be apart to count as `Esc Esc`.
pub const ESC_ESC_TICKS: u64 = 5;

pub(super) fn on_key(state: &mut State, key: KeyEvent) -> Vec<Cmd> {
    // T54.6: a recording owns `Esc` and the voice key's release and repeats
    // before anything else sees them.
    if let Some(cmds) = crate::voice::on_key(state, key) {
        return cmds;
    }
    // A held `Enter`/`Esc` must not repeat-submit or repeat-dismiss under the
    // Kitty keyboard protocol (T23.1); `app.rs` forwards a `Release` only
    // while push-to-talk records, and `voice::on_key` takes it above.
    if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
        return Vec::new();
    }
    // T33.25, PL§8: the key right after `plugin.leader` never reaches the
    // composer, a modal or another binding, matched or not — `<leader>
    // <key>` is the only way a plugin key fires.
    if state.plugin_leader_armed {
        state.plugin_leader_armed = false;
        return match state.keymap.resolve_plugin_key(key) {
            Some((plugin, name)) => vec![Cmd::Plugin(PluginRequest::Key {
                plugin: plugin.to_string(),
                name: name.to_string(),
            })],
            None => Vec::new(),
        };
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // `Ctrl+C` interrupts a running turn; when idle it must be pressed twice.
    if ctrl && key.code == KeyCode::Char('c') {
        if state.status.busy {
            return vec![Cmd::Submit(Submission::Interrupt)];
        }
        if state.ctrl_c_armed {
            return vec![Cmd::Quit];
        }
        state.ctrl_c_armed = true;
        return Vec::new();
    }
    state.ctrl_c_armed = false;
    // A `git` line completes on `Tab` (T15.4) before `Tab` means anything else.
    if key.code == KeyCode::Tab && state.modal.is_none() {
        let line = state.composer.text();
        let found = picker::candidates(&line, state);
        if !found.is_empty() {
            let picker = Picker::open(Kind::Shell, found).with_query(picker::last_word(&line));
            state.modal = Some(Modal::Picker(picker));
            return Vec::new();
        }
        // T25.2: `Tab` completes the `@`/`/` token under the cursor with the
        // picker typing the sigil would have opened, its query pre-filled.
        if let Some((sigil, query)) = state.composer.take_token() {
            let picker = match sigil {
                '@' => Picker::open(
                    Kind::Files,
                    picker::at_candidates(&state.agent_names, &state.files),
                ),
                _ => Picker::open(
                    Kind::Commands,
                    state.commands.iter().map(|(n, ..)| n.clone()).collect(),
                ),
            };
            state.modal = Some(Modal::Picker(picker.with_query(&query)));
            return Vec::new();
        }
    }
    // T46.7: `Ctrl+E` is `expand` everywhere else, a global the keymap
    // below would run over the picker; on `/theme` it opens the editor.
    if key.code == KeyCode::Char('e')
        && key.modifiers == KeyModifiers::CONTROL
        && matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::Themes)
    {
        open_theme_editor(state);
        return Vec::new();
    }
    // T25.5: every other key the TUI owns goes through the keymap; a key it
    // does not claim falls through to the modal or the composer.
    let base = if state.status.busy {
        Context::Running
    } else {
        Context::Idle
    };
    if let Some(action) = state.keymap.resolve(key, base)
        && let Some(cmds) = run(state, action, key)
    {
        return cmds;
    }
    match state.modal.take() {
        Some(Modal::Approval(mut approval)) => match approval.key(key) {
            Some(decision) => vec![Cmd::Submit(Submission::Approve {
                call_id: approval.call.id,
                decision,
            })],
            None => {
                state.modal = Some(Modal::Approval(approval));
                Vec::new()
            }
        },
        Some(Modal::Question(mut question)) => match question.key(key) {
            Some(answer) if state.think_consent == Some(question.call) => {
                think_consent_answered(state, answer)
            }
            // `None` (Esc) dismisses, so the tool call fails instead of
            // succeeding with empty text.
            Some(answer) => vec![Cmd::Submit(Submission::Answer {
                call_id: question.call,
                text: match answer {
                    QuestionAnswer::Text(text) => Some(text),
                    QuestionAnswer::Dismissed => None,
                },
            })],
            None => {
                state.modal = Some(Modal::Question(question));
                Vec::new()
            }
        },
        // T33.8, PL§3: `y`/`n` decide one dialog and `pending_grants`
        // supplies the next, so the queue drains one key at a time without
        // this crate ever writing the grant itself.
        Some(Modal::PluginGrant(grant)) => match grant.key(key) {
            Some(granted) => {
                let text = if granted {
                    format!("granted {}; it will load next session", grant.plugin_id)
                } else {
                    format!("skipped {} for this session", grant.plugin_id)
                };
                state.transcript.push(Cell::Notice {
                    level: Level::Info,
                    text,
                });
                state.modal = state.pending_grants.pop_front().map(Modal::PluginGrant);
                if granted {
                    vec![Cmd::PluginGrant(GrantDecision {
                        plugin_id: grant.plugin_id,
                        digest: grant.digest,
                        scope: grant.scope,
                        capabilities: grant.capabilities,
                    })]
                } else {
                    Vec::new()
                }
            }
            None => {
                state.modal = Some(Modal::PluginGrant(grant));
                Vec::new()
            }
        },
        // T33.33, PL§1c: `y` reaches the runtime only from here — `act()`
        // just opens the modal, never the request itself, so a stray
        // `/plugin remove` can never fire without this confirmation.
        Some(Modal::PluginRemove(confirm)) => match confirm.key(key) {
            Some(true) => {
                vec![Cmd::PluginMgmt(PluginMgmtRequest::Remove {
                    id: confirm.id,
                    keep_data: confirm.keep_data,
                })]
            }
            Some(false) => {
                notice(
                    state,
                    Level::Info,
                    format!("cancelled removing {}", confirm.id),
                );
                Vec::new()
            }
            None => {
                state.modal = Some(Modal::PluginRemove(confirm));
                Vec::new()
            }
        },
        // T24.2: every key that changes the selection previews the row
        // immediately, not only `Enter` — the same live-apply the picker's
        // other kinds do not need, since none of them redraws the screen
        // they came from.
        // T46.7: every accepted keystroke is drawn at once; `Esc` puts
        // back what was drawn before `/theme` opened, like the picker's.
        Some(Modal::ThemeEditor(mut editor)) => match editor.key(key) {
            None => {
                state.theme = editor.theme();
                state.modal = Some(Modal::ThemeEditor(editor));
                Vec::new()
            }
            Some(EditorOutcome::Revert) => {
                restore_theme(state);
                Vec::new()
            }
            Some(EditorOutcome::Save) => {
                state.theme_prev = None;
                let stem = if editor.builtin {
                    format!("{}-custom", editor.name)
                } else {
                    editor.name
                };
                let tokens = editor
                    .edits
                    .iter()
                    .map(|(t, c)| ((*t).to_string(), crate::theme::format_color(*c)))
                    .collect();
                vec![Cmd::Ask(Ask::SaveTheme {
                    stem,
                    dark: editor.dark,
                    tokens,
                })]
            }
        },
        Some(Modal::Picker(mut picker)) if picker.kind == Kind::Themes => {
            match picker.key(key) {
                Pick::Closed => restore_theme(state),
                Pick::Chosen(row) => {
                    state.theme_prev = None;
                    return apply_theme_choice(state, &row);
                }
                Pick::Nothing => {
                    preview_theme(state, &picker);
                    state.modal = Some(Modal::Picker(picker));
                }
            }
            Vec::new()
        }
        Some(Modal::Picker(mut picker)) => {
            match picker.key(key) {
                Pick::Nothing => state.modal = Some(Modal::Picker(picker)),
                // Backspacing out of the picker also removes the `@`/`/`
                // that opened it, as the user meant.
                Pick::Closed if key.code == KeyCode::Backspace && picker.kind != Kind::Shell => {
                    state.composer.key(key, state.status.busy);
                }
                // `Esc` gives back what was typed after the sigil, so a
                // `Tab` that found nothing costs no text (T25.2).
                Pick::Closed if matches!(picker.kind, Kind::Files | Kind::Commands) => {
                    state.composer.insert(&picker.query);
                }
                Pick::Closed => {}
                Pick::Chosen(choice) if picker.kind == Kind::Rewind => {
                    state.rewind_to = picker::turn_of_entry(&choice);
                    state.modal = Some(Modal::Picker(Picker::open(
                        Kind::RewindWhat,
                        picker::REWIND_WHAT.map(String::from).to_vec(),
                    )));
                }
                Pick::Chosen(choice) if picker.kind == Kind::RewindWhat => {
                    if let Some(to_turn) = state.rewind_to.take() {
                        let what = choice.split(' ').next().unwrap_or("");
                        return vec![Cmd::Submit(Submission::Rewind {
                            to_turn,
                            code: what != "talk",
                            conversation: what != "code",
                        })];
                    }
                }
                // T33.30: the second (and only) step of `/plugin new`
                // without `--lang` — `name`/`with` were stashed when the
                // picker opened, the same way `Kind::Rewind` stashes
                // `rewind_to` for `Kind::RewindWhat` above.
                Pick::Chosen(choice) if picker.kind == Kind::PluginLang => {
                    if let Some((name, with)) = state.pending_plugin_new.take() {
                        return vec![Cmd::PluginMgmt(PluginMgmtRequest::New(PluginNewRequest {
                            name,
                            lang: choice,
                            with,
                        }))];
                    }
                }
                Pick::Chosen(choice) => match picker.kind {
                    Kind::Files | Kind::Commands => {
                        let choice = picker::untag(&choice);
                        state.composer.insert(&format!("{choice} "));
                    }
                    // Resuming in place needs `app::run` to return a request;
                    // until then the command is the answer (T16.5).
                    Kind::Sessions => {
                        let id = state
                            .sessions
                            .iter()
                            .find(|(_, row)| *row == choice)
                            .map_or(choice.as_str(), |(id, _)| id.as_str());
                        let text = format!("to resume: cox --resume {id}");
                        notice(state, Level::Info, text);
                    }
                    Kind::History => {
                        let text = state
                            .past_prompts
                            .iter()
                            .find(|(row, _)| *row == choice)
                            .map_or(choice.as_str(), |(_, text)| text.as_str())
                            .to_string();
                        state.composer.set_text(&text);
                    }
                    // `Themes` and `PluginLang` are each intercepted by
                    // their own guarded arm above and never reach this
                    // generic one.
                    Kind::Rewind | Kind::RewindWhat | Kind::Themes | Kind::PluginLang => {}
                    Kind::Shell => {
                        let mut line = state.composer.text();
                        let keep = line.len() - picker::last_word(&line).len();
                        line.truncate(keep);
                        line.push_str(&choice);
                        line.push(' ');
                        state.composer.set_text(&line);
                    }
                },
            }
            Vec::new()
        }
        Some(Modal::Refine(review)) => {
            let (review, cmds) = review.key(key);
            state.modal = review.map(Modal::Refine);
            cmds
        }
        Some(Modal::Diff { text, scroll }) => {
            let scroll = match key.code {
                KeyCode::Esc | KeyCode::Char('?') => return Vec::new(),
                KeyCode::Char('g') if ctrl => return Vec::new(),
                KeyCode::PageDown => {
                    (scroll + DIFF_PAGE).min(text.lines().count().saturating_sub(1))
                }
                KeyCode::PageUp => scroll.saturating_sub(DIFF_PAGE),
                _ => scroll,
            };
            state.modal = Some(Modal::Diff { text, scroll });
            Vec::new()
        }
        Some(modal @ (Modal::Help | Modal::Context)) => {
            if !matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                state.modal = Some(modal);
            }
            Vec::new()
        }
        Some(Modal::Agents {
            rows,
            ids,
            mut selected,
        }) => {
            let mut cmds = Vec::new();
            match key.code {
                KeyCode::Esc => {}
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                }
                KeyCode::Down => {
                    if selected + 1 < rows.len() {
                        selected += 1;
                    }
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                }
                // A task row's id is `None` (no `SessionId` on the wire
                // yet, plan.md §3 P27): `Enter` on it just keeps the list.
                KeyCode::Enter => {
                    let ask = ids.get(selected).copied().flatten();
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    });
                    if let Some(id) = ask {
                        cmds.push(Cmd::Ask(Ask::Rollout(id)));
                    }
                }
                _ => {
                    state.modal = Some(Modal::Agents {
                        rows,
                        ids,
                        selected,
                    })
                }
            }
            cmds
        }
        Some(Modal::Transcript { cells, scroll }) => {
            // No upper clamp here (unlike `Diff`'s raw line count): `cells`
            // wrap at render width, which this pure update has no access
            // to, and `view.rs` already clamps the offset it actually uses.
            let scroll = match key.code {
                KeyCode::Esc | KeyCode::Char('?') => return Vec::new(),
                KeyCode::PageDown => scroll + DIFF_PAGE,
                KeyCode::PageUp => scroll.saturating_sub(DIFF_PAGE),
                _ => scroll,
            };
            state.modal = Some(Modal::Transcript { cells, scroll });
            Vec::new()
        }
        // T33.24, PL§8: `overlay`'s only key — full screen, no scroll of
        // its own, so anything but `Esc` just keeps it open.
        Some(Modal::Plugin { id }) => {
            if key.code != KeyCode::Esc {
                state.modal = Some(Modal::Plugin { id });
            }
            Vec::new()
        }
        None => {
            let esc_idle_empty =
                key.code == KeyCode::Esc && !state.status.busy && state.composer.is_empty();
            // `/loop`'s stop key (T27.4): the same idle-empty `Esc`, ahead of
            // Esc-Esc's rewind-timeline role below, so a running loop is
            // always one `Esc` away.
            if esc_idle_empty && state.active_loop.take().is_some() {
                notice(state, Level::Info, "loop stopped".into());
                return Vec::new();
            }
            // `Esc Esc` on an idle empty composer opens the rewind timeline
            // (T26.2); a lone Esc still reaches the composer for vim.
            if esc_idle_empty {
                let armed = state.esc_armed.take();
                if armed.is_some_and(|t| state.tick.saturating_sub(t) <= ESC_ESC_TICKS) {
                    return open_rewind(state);
                }
                state.esc_armed = Some(state.tick);
            } else {
                state.esc_armed = None;
            }
            // An `Enter` no binding claims (`send` moved elsewhere) is a
            // newline rather than the composer's own submit.
            if key.code == KeyCode::Enter {
                return compose(state, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
            }
            compose(state, key)
        }
    }
}

/// A keymap action (T25.5); `None` leaves the key to the modal or the
/// composer. With a modal open only the view toggles and quit act, and a
/// binding on a plain character acts only on an empty composer.
fn run(state: &mut State, action: keymap::Action, key: KeyEvent) -> Option<Vec<Cmd>> {
    use keymap::Action as A;
    let plain = matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let global = matches!(action, A::Quit | A::Thinking | A::Transcript | A::Expand);
    if (plain && !state.composer.is_empty()) || (state.modal.is_some() && !global) {
        return None;
    }
    let enter = |modifiers| KeyEvent::new(KeyCode::Enter, modifiers);
    Some(match action {
        A::Quit => vec![Cmd::Quit],
        // T33.25, PL§8: arms `on_key`'s leader check for the very next key.
        A::PluginLeader => {
            state.plugin_leader_armed = true;
            Vec::new()
        }
        A::Thinking => toggle(&mut state.show_thinking),
        A::Transcript => toggle(&mut state.show_diffs),
        A::Expand => {
            let last_tool = state
                .transcript
                .iter()
                .rposition(|c| matches!(c, Cell::Tool { .. }));
            match last_tool {
                Some(i) => toggle_fold(state, i),
                None => Vec::new(),
            }
        }
        A::Diff => vec![Cmd::Ask(Ask::GitDiff)],
        // `Ctrl+B` (T27.1): the newest pending `bash`/`agent` card becomes a
        // background task; the turn goes on without waiting for it.
        A::Background => {
            let call_id = state.detachable_call()?;
            vec![Cmd::Submit(Submission::Background { call_id })]
        }
        // The queue's tail back for editing (T25.1); a non-empty composer
        // keeps its usual line-kill.
        A::Unqueue => {
            if !state.composer.is_empty() {
                return None;
            }
            if let Some(text) = state.queue.pop_back() {
                state.composer.set_text(&text);
            }
            Vec::new()
        }
        A::Help => {
            state.modal = Some(Modal::Help);
            Vec::new()
        }
        A::ModeCycle => set_mode(state, commands::next_mode(state.mode)),
        // In vim's normal and visual modes `Esc` never interrupts (T25.4):
        // there it only cancels a pending command; `Ctrl+C` still does.
        A::Interrupt => {
            let vim_owns_esc = state.composer.vim_mode().is_some_and(|m| m != Mode::Insert);
            if key.code == KeyCode::Esc && vim_owns_esc {
                return None;
            }
            vec![Cmd::Submit(Submission::Interrupt)]
        }
        // The composer decides what an `Enter` does; these hand it the one
        // each action means, whatever key was bound.
        A::Voice => crate::voice::toggle(state),
        A::Send => compose(state, enter(KeyModifiers::NONE)),
        A::Newline => compose(state, enter(KeyModifiers::SHIFT)),
        A::SendNow => compose(state, enter(KeyModifiers::ALT)),
        // Nothing to copy falls through to the composer, same as
        // `A::Background` with no pending call.
        A::Copy => {
            let text = cell_text(state.transcript.last()?).to_string();
            copy_text(state, text)
        }
        A::CopyAll => {
            if state.transcript.is_empty() {
                return None;
            }
            let text: Vec<&str> = state.transcript.iter().map(cell_text).collect();
            copy_text(state, text.join("\n\n"))
        }
    })
}

/// `Cmd::Copy` when the terminal draws OSC 52 (T23.0 `caps.osc52`); a
/// terminal without it would just print the escape as visible text, so it
/// gets a notice instead of bytes it cannot use.
fn copy_text(state: &mut State, text: String) -> Vec<Cmd> {
    if !state.caps.osc52 {
        notice(
            state,
            Level::Info,
            "clipboard: terminal does not support OSC 52".into(),
        );
        return Vec::new();
    }
    vec![Cmd::Copy(text)]
}

fn toggle(flag: &mut bool) -> Vec<Cmd> {
    *flag = !*flag;
    Vec::new()
}

/// Flips transcript index `i`'s membership in `state.expanded` (T22.9),
/// shared by `Ctrl+E` (always the last tool cell) and a click (whichever
/// cell `on_mouse` hit-tested).
pub(super) fn toggle_fold(state: &mut State, i: usize) -> Vec<Cmd> {
    if !state.expanded.remove(&i) {
        state.expanded.insert(i);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Msg, update};
    use cox_protocol::ids::CallId;
    use cox_protocol::types::{Event, PermissionMode, SandboxMode};

    /// T27.1: `Ctrl+B` backgrounds the newest pending `bash` card; with no
    /// such card it is not swallowed as a background request.
    #[test]
    fn ctrl_b_backgrounds_the_pending_bash_card() {
        use cox_protocol::types::{Risk, ToolCall};
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let ctrl_b = || Msg::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        // A pending call only exists inside a running turn, and `Ctrl+B` is a
        // running-turn key (T25.5).
        state.status.busy = true;
        assert_eq!(update(&mut state, ctrl_b()), Vec::new());
        let call_id = CallId::new();
        update(
            &mut state,
            Msg::Event(Event::ToolCallRequested {
                call: ToolCall {
                    id: call_id,
                    name: "bash".into(),
                    input: serde_json::json!({"command": "sleep 5"}),
                    risk: Risk::Exec,
                    subject: "sleep 5".into(),
                    segments: None,
                },
            }),
        );
        assert_eq!(
            update(&mut state, ctrl_b()),
            vec![Cmd::Submit(Submission::Background { call_id })]
        );
    }

    /// T23.4: `y` on an empty composer copies the last cell still held as
    /// plain text, only when the terminal draws OSC 52; without the
    /// capability it gets a notice instead of bytes it cannot use, and a
    /// non-empty composer keeps typing "y" rather than copying.
    #[test]
    fn overlay_y_emits_copy_of_cell() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc52 = true;
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "cell text".into(),
        });
        let y = || Msg::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(update(&mut state, y()), vec![Cmd::Copy("cell text".into())]);

        let mut unsupported = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        unsupported.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "cell text".into(),
        });
        assert_eq!(update(&mut unsupported, y()), Vec::new());
        assert!(matches!(
            unsupported.transcript.last(),
            Some(Cell::Notice { text, .. }) if text.contains("does not support OSC 52")
        ));

        let mut typing = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        typing.caps.osc52 = true;
        typing.composer.insert("yak");
        assert_eq!(update(&mut typing, y()), Vec::new());
        assert_eq!(typing.composer.text(), "yaky");
    }

    /// T23.4: `Shift+Y` joins every cell still held; nothing held is a
    /// no-op rather than a notice about an empty clipboard.
    #[test]
    fn shift_y_emits_copy_of_the_whole_transcript() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.caps.osc52 = true;
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "one".into(),
        });
        state.transcript.push(Cell::Notice {
            level: Level::Info,
            text: "two".into(),
        });
        let shift_y = Msg::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        assert_eq!(
            update(&mut state, shift_y),
            vec![Cmd::Copy("one\n\ntwo".into())]
        );

        let mut empty = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        empty.caps.osc52 = true;
        let shift_y = Msg::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        assert_eq!(update(&mut empty, shift_y), Vec::new());
    }
}
