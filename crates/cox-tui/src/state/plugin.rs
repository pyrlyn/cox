// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Commands that are not built in: markdown command files (T22.2) and plugin
//! commands (T33.25, PL§8) — finding a line's command, declaring a plugin's
//! commands into the `/` palette, and applying a `CommandOut` answer. Apart
//! from `act` because these names come from files and plugins, so the guards
//! that keep a built-in winning over them live in one place.

use cox_protocol::plugin::{CommandDecl, CommandOut, NoticeLevel};
use cox_protocol::types::{Level, SlashCommand, Submission};

use super::submit::user_turn;
use super::{Cmd, Modal, State, notice};
use crate::commands::{Action, COMMANDS};

/// T22.2: a `/name args` line naming a file command — something
/// `State.commands` carries beyond the built-in `COMMANDS`, which the T5.5
/// parser owns — submits `Submission::Command` for the core, the same shape
/// the parser already produces for built-ins without a dedicated arm. A
/// colon-bearing name is a plugin command (T33.25) instead, never one of
/// these: no built-in or file command name has ever contained one.
pub(super) fn file_command(commands: &[(String, String, String)], line: &str) -> Option<Action> {
    let mut words = line.strip_prefix('/')?.split_whitespace();
    let name = words.next()?;
    if COMMANDS.iter().any(|(n, ..)| *n == name) || name.contains(':') {
        return None;
    }
    commands.iter().any(|(n, ..)| n == name).then(|| {
        Action::Submit(Submission::Command {
            command: SlashCommand {
                name: name.to_string(),
                args: words.map(str::to_string).collect(),
            },
        })
    })
}

/// T33.25, PL§8: a `/<id>:<name>` line naming a plugin command — the colon
/// is what tells it apart from `file_command`'s names. Guarded the same
/// way against `COMMANDS` so a built-in still wins even over a
/// mis-registered plugin command that dropped its `<id>:` prefix.
pub(super) fn plugin_command(commands: &[(String, String, String)], line: &str) -> Option<Action> {
    let mut words = line.strip_prefix('/')?.split_whitespace();
    let full = words.next()?;
    if COMMANDS.iter().any(|(n, ..)| *n == full) {
        return None;
    }
    let (plugin, name) = full.split_once(':')?;
    commands
        .iter()
        .any(|(n, ..)| n == full)
        .then(|| Action::PluginCommand {
            plugin: plugin.to_string(),
            name: name.to_string(),
            args: words.collect::<Vec<_>>().join(" "),
        })
}

/// T33.25, PL§8: a plugin's declared commands join `State.commands` after
/// the built-ins and any file commands, as `/<id>:<name>` — appended once,
/// a re-`Declare` never duplicates a name it already added.
pub(super) fn declare_plugin_commands(state: &mut State, plugin: &str, commands: &[CommandDecl]) {
    for c in commands {
        let full = format!("{plugin}:{}", crate::text::sanitize(&c.name));
        if state.commands.iter().any(|(n, ..)| *n == full) {
            continue;
        }
        state.commands.push((
            full.clone(),
            format!("/{full}"),
            crate::text::sanitize(&c.description),
        ));
    }
}

/// T33.25, PL§8: `CommandOut`'s closed effects for a `cox_command`/
/// `cox_key` answer. `None` — a timeout, an error or a missing export —
/// fails open like a missed render: nothing happens, nothing is shown.
pub(super) fn plugin_command_out(
    state: &mut State,
    plugin: &str,
    out: Option<CommandOut>,
) -> Vec<Cmd> {
    match out {
        Some(CommandOut::Prompt { text }) => {
            vec![Cmd::Submit(user_turn(state, crate::text::sanitize(&text)))]
        }
        Some(CommandOut::Compact { focus }) => vec![Cmd::Submit(Submission::Compact {
            focus: focus.map(|f| crate::text::sanitize(&f)),
        })],
        // T33.24, PL§8: opening either slot is a "slot became visible"
        // moment, one of the three times a plugin renders — `render_requests`
        // itself decides whether `plugin` actually has that slot to render.
        Some(CommandOut::TogglePanel) => {
            state.plugin_panel_open = if state.plugin_panel_open.as_deref() == Some(plugin) {
                None
            } else {
                Some(plugin.to_string())
            };
            crate::status::render_requests(state, Some(plugin))
        }
        Some(CommandOut::OpenOverlay) => {
            state.modal = Some(Modal::Plugin {
                id: plugin.to_string(),
            });
            crate::status::render_requests(state, Some(plugin))
        }
        Some(CommandOut::Notice(n)) => {
            let level = match n.level {
                NoticeLevel::Warn => Level::Warn,
                NoticeLevel::Info => Level::Info,
            };
            notice(state, level, crate::text::sanitize(&n.text));
            Vec::new()
        }
        // PL§15.6 case 5: the inspector only exists in the desktop app.
        Some(CommandOut::OpenInspector) => {
            notice(
                state,
                Level::Info,
                format!("plugin {plugin}: the inspector is desktop-only"),
            );
            Vec::new()
        }
        Some(CommandOut::Nothing) | None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::Kind;
    use crate::state::testkit::type_command;
    use crate::state::{
        Msg, PluginMgmtRequest, PluginNewRequest, PluginRequest, PluginUiMsg, update,
    };
    use cox_protocol::plugin::KeyDecl;
    use cox_protocol::types::{PermissionMode, SandboxMode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// T33.30's Done-when: `/plugin new <name>` with no `--lang` opens the
    /// language picker, and choosing a row reaches
    /// `Cmd::PluginMgmt(PluginMgmtRequest::New)` with that language — the
    /// one `Cmd` `crates/cox`'s executor turns into a
    /// `plugin_new::scaffold`/`write` call, so there is no second
    /// implementation of the mapping. `fake_executor` stands in for that
    /// runtime executor (`cox-tui` cannot call `plugin_new::scaffold`
    /// itself — it does not depend on `crates/cox`) and records what it
    /// would have been called with.
    #[test]
    fn tui_plugin_new_calls_shared_scaffold() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        type_command(&mut state, "/plugin new demo");
        assert!(
            matches!(&state.modal, Some(Modal::Picker(p)) if p.kind == Kind::PluginLang),
            "/plugin new with no --lang opens the language picker"
        );

        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));

        let mut calls: Vec<(String, String, Vec<String>)> = Vec::new();
        let fake_executor =
            |req: &PluginNewRequest| (req.name.clone(), req.lang.clone(), req.with.clone());
        for cmd in &cmds {
            if let Cmd::PluginMgmt(PluginMgmtRequest::New(req)) = cmd {
                calls.push(fake_executor(req));
            }
        }
        assert_eq!(
            calls,
            vec![("demo".to_string(), "rust".to_string(), Vec::new())],
            "exactly one call, with the picker's chosen language"
        );
        assert!(state.modal.is_none(), "the picker closes after a pick");
    }

    /// T22.2: a markdown file command joins the `/` palette after the
    /// built-ins, a chosen row inserts `/name `, and `Enter` submits it as
    /// `Submission::Command { name, args }` — args tokenized like any line.
    #[test]
    fn palette_lists_file_commands() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.commands.push((
            "review".into(),
            "/review [pr]".into(),
            "review a pull request".into(),
        ));
        assert_eq!(
            state.commands.first().map(|(n, ..)| n.as_str()),
            Some("model"),
            "built-in COMMANDS come first"
        );
        assert_eq!(
            state.commands.last().map(|(n, ..)| n.as_str()),
            Some("review"),
            "file commands are appended after them"
        );

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('/'))));
        for c in "rev".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let rows = match &state.modal {
            Some(Modal::Picker(p)) => p.matches.clone(),
            other => panic!("the palette is open, got {other:?}"),
        };
        assert!(rows.contains(&"review".to_string()), "{rows:?}");

        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(state.composer.text(), "/review ");
        for c in "pr-1".chars() {
            update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::Command {
                command: SlashCommand {
                    name: "review".into(),
                    args: vec!["pr-1".into()],
                },
            })]
        );
    }

    /// T33.25, PL§8: even a plugin command mis-registered without its
    /// `<id>:` prefix (bypassing `declare_plugin_commands`'s own naming)
    /// never shadows a built-in — the same guard `file_command` already
    /// gives a markdown command — and the real built-in still dispatches
    /// with plugin commands present in the table.
    #[test]
    fn builtin_command_wins_over_plugin() {
        let commands = vec![("quit".to_string(), String::new(), String::new())];
        assert_eq!(plugin_command(&commands, "/quit"), None);

        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state
            .commands
            .push(("acme:quit".into(), "/acme:quit".into(), String::new()));
        assert_eq!(type_command(&mut state, "/quit"), vec![Cmd::Quit]);
    }

    /// T33.25, PL§8: `/<id>:<name>` calls the plugin (`Cmd::Plugin`, not
    /// the core), and its `CommandOut::Prompt` answer submits a `UserTurn`,
    /// like a markdown command's own text would.
    #[test]
    fn plugin_command_prompt_submits_user_turn() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state
            .commands
            .push(("acme:go".into(), "/acme:go".into(), "go".into()));

        let cmds = type_command(&mut state, "/acme:go do it");
        assert_eq!(
            cmds,
            vec![Cmd::Plugin(PluginRequest::Command {
                plugin: "acme".into(),
                name: "go".into(),
                args: "do it".into(),
            })]
        );

        let cmds = update(
            &mut state,
            Msg::Plugin(PluginUiMsg::Command {
                plugin: "acme".into(),
                out: Some(CommandOut::Prompt {
                    text: "go go go".into(),
                }),
            }),
        );
        assert_eq!(
            cmds,
            vec![Cmd::Submit(Submission::UserTurn {
                text: "go go go".into(),
                attachments: Vec::new(),
                confirm_think: false,
            })]
        );
    }

    /// T33.25, PL§8: a plugin key fires only as `<leader> <key>` — the bare
    /// key on an empty composer does nothing plugin-related, the same as
    /// any other unbound letter.
    #[test]
    fn plugin_key_only_under_leader() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.keymap.declare_plugin_keys(
            "acme",
            &[KeyDecl {
                key: "r".into(),
                name: "reset".into(),
                description: String::new(),
            }],
        );
        let plugin_key = Cmd::Plugin(PluginRequest::Key {
            plugin: "acme".into(),
            name: "reset".into(),
        });

        let r = || Msg::Key(KeyEvent::from(KeyCode::Char('r')));
        assert!(!update(&mut state, r()).contains(&plugin_key));

        let leader = Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(update(&mut state, leader), Vec::new());
        assert!(state.plugin_leader_armed);
        assert_eq!(update(&mut state, r()), vec![plugin_key]);
        assert!(!state.plugin_leader_armed, "one key disarms it");
    }

    /// T33.24, PL§8: `TogglePanel` flips `plugin_panel_open` for the
    /// plugin that sent it (twice returns to closed); `OpenOverlay` opens
    /// `Modal::Plugin`. Neither needs a declared slot to be safe — with
    /// none, `render_requests` just has nothing to ask for.
    #[test]
    fn plugin_command_toggles_panel_and_opens_overlay() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let toggle = |state: &mut State| {
            update(
                state,
                Msg::Plugin(PluginUiMsg::Command {
                    plugin: "acme".into(),
                    out: Some(CommandOut::TogglePanel),
                }),
            )
        };
        assert!(toggle(&mut state).is_empty());
        assert_eq!(state.plugin_panel_open.as_deref(), Some("acme"));
        assert!(toggle(&mut state).is_empty());
        assert_eq!(state.plugin_panel_open, None);

        update(
            &mut state,
            Msg::Plugin(PluginUiMsg::Command {
                plugin: "acme".into(),
                out: Some(CommandOut::OpenOverlay),
            }),
        );
        assert_eq!(state.modal, Some(Modal::Plugin { id: "acme".into() }));
    }

    /// T33.24, PL§8: `overlay`'s only key is `Esc`; it closes with no other
    /// side effect, the same close `Diff`/`Transcript` already have.
    #[test]
    fn esc_closes_plugin_overlay() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.modal = Some(Modal::Plugin { id: "acme".into() });
        let cmds = update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert_eq!(cmds, Vec::new());
        assert_eq!(state.modal, None);
    }
}
