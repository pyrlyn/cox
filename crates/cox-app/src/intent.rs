// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The one enum the app sends (DT§4.3) and what each intent means to the
//! core: a `Submission`, a turn to spawn or hold, or a lineage call. Pure,
//! so a test checks an intent without a session, and separate from the
//! controller that executes the `Dispatch` (it owns the session and the
//! runtime). What a composer draft becomes — shell line, `/` command line
//! or turn, now or queued — is decided here too (T58.4.17), so every client
//! sends a draft alike.

use cox_protocol::CallId;
use cox_protocol::types::{
    Attachment, Decision, Effort, ModelId, PermissionMode, SlashCommand, Submission, Tier,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    /// `confirm_think`: this one turn goes to the think tier, as `/think`
    /// sends it (A103).
    Send {
        text: String,
        attachments: Vec<Attachment>,
        #[serde(default)]
        confirm_think: bool,
    },
    Approve {
        call: CallId,
        decision: Decision,
    },
    /// `None` dismisses the question unanswered.
    Answer {
        question: CallId,
        text: Option<String>,
    },
    Interrupt,
    /// Sent while a turn runs: becomes the next turn when it ends, with
    /// the `confirm_think` it was sent with.
    Queue {
        text: String,
        attachments: Vec<Attachment>,
        #[serde(default)]
        confirm_think: bool,
    },
    Compact {
        focus: Option<String>,
    },
    SetMode {
        mode: PermissionMode,
    },
    SwitchModel {
        tier: Tier,
        model: Option<ModelId>,
    },
    SetEffort {
        effort: Option<Effort>,
    },
    Rewind {
        to_turn: u32,
        code: bool,
        conversation: bool,
    },
    Redo,
    /// Restore one file to before `to_turn` (DT§5.4); `path` as the
    /// Changes tab lists it.
    RevertFile {
        path: String,
        to_turn: u32,
    },
    /// Put hunk `hunk` of Review's diff of `path` back (T51.20, DT§5.4):
    /// `to_turn` as for `RevertFile`, `now_digest` the `DiffModel`'s
    /// digest, so the core refuses bytes Review did not show.
    RevertHunk {
        path: String,
        to_turn: u32,
        hunk: u32,
        now_digest: String,
    },
    Fork {
        turn: Option<u32>,
    },
    Handoff {
        objective: String,
    },
    Background {
        call: CallId,
    },
    Shell {
        command: String,
        share: bool,
    },
    /// A composer line: `!cmd`/`!!cmd`, or `/name args`.
    Command {
        line: String,
    },
    /// The user's title for the session (A113): the toolbar's or a
    /// sidebar row's rename, as `/rename` sends it.
    Rename {
        title: String,
    },
}

/// What the controller does with an intent.
#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    /// Submit to the session. `spawn`: a turn, spawned and never awaited
    /// (R9.4.3), so `send` returns while it runs.
    Submit { submission: Submission, spawn: bool },
    /// Hold until the running turn ends, then spawn it.
    Queue(Submission),
    /// `cox_session::fork` the session up to `turn` and open the child.
    Fork { turn: Option<u32> },
    /// `cox_session::handoff` with a summary, then open the child.
    Handoff { objective: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntentError {
    #[error("nothing to send")]
    Empty,
    #[error("`{0}` is neither a `/command` nor a `!` shell line")]
    NotACommand(String),
}

/// Maps `intent` to what the core should do with it.
pub fn dispatch(intent: Intent) -> Result<Dispatch, IntentError> {
    let now = |submission| {
        Ok(Dispatch::Submit {
            submission,
            spawn: false,
        })
    };
    match intent {
        Intent::Send {
            text,
            attachments,
            confirm_think,
        } => Ok(Dispatch::Submit {
            submission: turn(text, attachments, confirm_think)?,
            spawn: true,
        }),
        Intent::Queue {
            text,
            attachments,
            confirm_think,
        } => Ok(Dispatch::Queue(turn(text, attachments, confirm_think)?)),
        Intent::Approve { call, decision } => now(Submission::Approve {
            call_id: call,
            decision,
        }),
        Intent::Answer { question, text } => now(Submission::Answer {
            call_id: question,
            text,
        }),
        Intent::Interrupt => now(Submission::Interrupt),
        Intent::Compact { focus } => now(Submission::Compact { focus }),
        Intent::SetMode { mode } => now(Submission::SetPermissionMode { mode }),
        Intent::SwitchModel { tier, model } => now(Submission::SwitchModel { tier, model }),
        Intent::SetEffort { effort } => now(Submission::SetEffort { effort }),
        Intent::Rewind {
            to_turn,
            code,
            conversation,
        } => now(Submission::Rewind {
            to_turn,
            code,
            conversation,
        }),
        Intent::Redo => now(Submission::Redo),
        Intent::RevertFile { path, to_turn } => now(Submission::RevertFile { path, to_turn }),
        Intent::RevertHunk {
            path,
            to_turn,
            hunk,
            now_digest,
        } => now(Submission::RevertHunk {
            path,
            to_turn,
            hunk,
            now_digest,
        }),
        Intent::Fork { turn } => Ok(Dispatch::Fork { turn }),
        Intent::Handoff { objective } if objective.trim().is_empty() => Err(IntentError::Empty),
        Intent::Handoff { objective } => Ok(Dispatch::Handoff { objective }),
        Intent::Background { call } => now(Submission::Background { call_id: call }),
        Intent::Shell { command, share } => shell(command, share),
        Intent::Command { line } => command(&line),
        Intent::Rename { title } => rename(title),
    }
}

/// What a session driven by an external ACP agent does with an intent
/// (T52.4, DT§3.3.1): the agent owns the model, mode, history and files, so
/// only a prompt, a cancel, an answer to its own permission request (T52.5)
/// and a rename mean anything; the rest is refused by name.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentDispatch {
    /// `session/prompt`, after the one in flight.
    Prompt(String),
    /// `session/cancel`.
    Cancel,
    /// The user's answer to one of the agent's `session/request_permission`
    /// asks, waiting in the inbox (T52.5).
    Approve { call: CallId, decision: Decision },
    /// The session's title in `cox.db`, as for any session.
    Rename(String),
    /// Not available in an agent's session; the intent's name.
    Refused(&'static str),
}

/// Maps `intent` for an external agent's session. A `/` line goes to the
/// agent verbatim (its commands apply, not cox's); a `!` line needs cox's
/// shell and is refused.
pub fn agent_dispatch(intent: Intent) -> Result<AgentDispatch, IntentError> {
    Ok(match intent {
        Intent::Send {
            text, attachments, ..
        }
        | Intent::Queue {
            text, attachments, ..
        } => {
            if !attachments.is_empty() {
                AgentDispatch::Refused("Attachments")
            } else if text.trim().is_empty() {
                return Err(IntentError::Empty);
            } else {
                AgentDispatch::Prompt(text)
            }
        }
        Intent::Command { line } if line.trim_start().starts_with('!') => {
            AgentDispatch::Refused("Shell")
        }
        Intent::Command { line } if line.trim().is_empty() => return Err(IntentError::Empty),
        Intent::Command { line } => AgentDispatch::Prompt(line.trim().to_string()),
        Intent::Interrupt => AgentDispatch::Cancel,
        Intent::Rename { title } if title.trim().is_empty() => return Err(IntentError::Empty),
        Intent::Rename { title } => AgentDispatch::Rename(title),
        Intent::Approve { call, decision } => AgentDispatch::Approve { call, decision },
        Intent::Answer { .. } => AgentDispatch::Refused("Answer"),
        Intent::Compact { .. } => AgentDispatch::Refused("Compact"),
        Intent::SetMode { .. } => AgentDispatch::Refused("SetMode"),
        Intent::SwitchModel { .. } => AgentDispatch::Refused("SwitchModel"),
        Intent::SetEffort { .. } => AgentDispatch::Refused("SetEffort"),
        Intent::Rewind { .. } => AgentDispatch::Refused("Rewind"),
        Intent::Redo => AgentDispatch::Refused("Redo"),
        Intent::RevertFile { .. } => AgentDispatch::Refused("RevertFile"),
        Intent::RevertHunk { .. } => AgentDispatch::Refused("RevertHunk"),
        Intent::Fork { .. } => AgentDispatch::Refused("Fork"),
        Intent::Handoff { .. } => AgentDispatch::Refused("Handoff"),
        Intent::Background { .. } => AgentDispatch::Refused("Background"),
        Intent::Shell { .. } => AgentDispatch::Refused("Shell"),
    })
}

/// When a turn goes while another runs (`[desktop.review] send`, A108).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendWhen {
    /// Behind the running turn, as the composer queues a prompt.
    #[default]
    Queue,
    /// At once, even while a turn runs.
    Now,
}

/// Which intent a draft is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftKind {
    /// [`Intent::Shell`].
    Shell,
    /// [`Intent::Command`], for the core's command table.
    Command,
    /// [`Intent::Send`], or [`Intent::Queue`] when `queued`.
    Turn,
}

/// What a composer draft becomes (T58.4.17, DT§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftIntent {
    pub kind: DraftKind,
    /// A turn held behind the running one ([`Intent::Queue`]).
    pub queued: bool,
    /// There is something to send.
    pub can_send: bool,
    /// The attachments and the think toggle stay after the send: a shell or
    /// command line cannot carry them.
    pub keeps_attachments: bool,
    /// The draft is a lone `!`: shell mode instead of text.
    pub enters_shell: bool,
}

/// What the draft `text` becomes: in `shell` mode a shell line, else a `/`
/// line for the command table, else a turn with its `attachments` — queued
/// while a turn is `running` unless `when` is [`SendWhen::Now`].
pub fn draft_intent(
    text: &str,
    shell: bool,
    attachments: u32,
    running: bool,
    when: SendWhen,
) -> DraftIntent {
    let kind = if shell {
        DraftKind::Shell
    } else if text.starts_with('/') {
        DraftKind::Command
    } else {
        DraftKind::Turn
    };
    let turn = kind == DraftKind::Turn;
    DraftIntent {
        kind,
        queued: turn && running && when == SendWhen::Queue,
        can_send: !text.trim().is_empty() || (!shell && attachments > 0),
        keeps_attachments: !turn,
        enters_shell: !shell && text == "!",
    }
}

/// A turn needs text or an attachment.
fn turn(
    text: String,
    attachments: Vec<Attachment>,
    confirm_think: bool,
) -> Result<Submission, IntentError> {
    if text.trim().is_empty() && attachments.is_empty() {
        return Err(IntentError::Empty);
    }
    Ok(Submission::UserTurn {
        text,
        attachments,
        confirm_think,
    })
}

/// A rename needs a title; the core keeps its first line, sanitized.
fn rename(title: String) -> Result<Dispatch, IntentError> {
    if title.trim().is_empty() {
        return Err(IntentError::Empty);
    }
    Ok(Dispatch::Submit {
        submission: Submission::Rename { title },
        spawn: false,
    })
}

fn shell(command: String, share: bool) -> Result<Dispatch, IntentError> {
    if command.trim().is_empty() {
        return Err(IntentError::Empty);
    }
    Ok(Dispatch::Submit {
        submission: Submission::UserShell { command, share },
        spawn: true,
    })
}

/// `!!cmd` shares its output with the model, `!cmd` does not (T25.3);
/// `/name args` goes to the core as `Submission::Command`, the shape the
/// TUI submits for file commands and built-ins without a dedicated arm.
/// Built-ins that have their own intent (`/fork`, `/effort`, …) are sent as
/// that intent by the palette.
fn command(line: &str) -> Result<Dispatch, IntentError> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('!') {
        return match rest.strip_prefix('!') {
            Some(cmd) => shell(cmd.trim().to_string(), true),
            None => shell(rest.trim().to_string(), false),
        };
    }
    let mut words = line
        .strip_prefix('/')
        .ok_or_else(|| IntentError::NotACommand(line.to_string()))?
        .split_whitespace();
    let name = words.next().ok_or(IntentError::Empty)?.to_string();
    if name == "rename" {
        return rename(words.collect::<Vec<_>>().join(" "));
    }
    let args = words.map(str::to_string).collect();
    Ok(Dispatch::Submit {
        submission: Submission::Command {
            command: SlashCommand { name, args },
        },
        spawn: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revert_hunk_intent_maps_to_the_submission() {
        let intent = Intent::RevertHunk {
            path: "src/a.rs".into(),
            to_turn: 2,
            hunk: 1,
            now_digest: "00ff".into(),
        };
        assert_eq!(
            dispatch(intent),
            Ok(Dispatch::Submit {
                submission: Submission::RevertHunk {
                    path: "src/a.rs".into(),
                    to_turn: 2,
                    hunk: 1,
                    now_digest: "00ff".into(),
                },
                spawn: false,
            })
        );
    }

    /// T52.4: an agent's session prompts, cancels and renames; a `/` line
    /// goes verbatim, and everything that needs cox's own state is refused.
    #[test]
    fn agent_dispatch_refuses_what_the_agent_owns() {
        let line = |l: &str| Intent::Command { line: l.into() };
        assert_eq!(
            agent_dispatch(line("/review now")),
            Ok(AgentDispatch::Prompt("/review now".into()))
        );
        assert_eq!(
            agent_dispatch(line("!ls")),
            Ok(AgentDispatch::Refused("Shell"))
        );
        assert_eq!(agent_dispatch(Intent::Interrupt), Ok(AgentDispatch::Cancel));
        let rewind = Intent::Rewind {
            to_turn: 1,
            code: true,
            conversation: true,
        };
        assert_eq!(agent_dispatch(rewind), Ok(AgentDispatch::Refused("Rewind")));
        assert_eq!(
            agent_dispatch(Intent::Fork { turn: None }),
            Ok(AgentDispatch::Refused("Fork"))
        );
    }

    fn draft(text: &str, shell: bool, attachments: u32, running: bool) -> DraftIntent {
        draft_intent(text, shell, attachments, running, SendWhen::Queue)
    }

    #[test]
    fn a_draft_is_queued_while_a_turn_runs() {
        let idle = draft("fix it", false, 0, false);
        assert_eq!((idle.kind, idle.queued), (DraftKind::Turn, false));
        let busy = draft("fix it", false, 0, true);
        assert_eq!((busy.kind, busy.queued), (DraftKind::Turn, true));
        let line = draft("/compact", false, 0, true);
        assert_eq!((line.kind, line.queued), (DraftKind::Command, false));
        let shell = draft("ls", true, 0, true);
        assert_eq!((shell.kind, shell.queued), (DraftKind::Shell, false));
    }

    #[test]
    fn a_bang_enters_shell_mode() {
        assert!(draft("!", false, 0, false).enters_shell);
        assert!(!draft("!", true, 0, false).enters_shell, "already in it");
        assert!(!draft("!ls", false, 0, false).enters_shell);
    }

    #[test]
    fn attachments_clear_only_after_a_send_or_queue() {
        assert!(!draft("fix", false, 1, false).keeps_attachments);
        assert!(!draft("fix", false, 1, true).keeps_attachments);
        assert!(draft("/compact", false, 1, false).keeps_attachments);
        assert!(draft("ls", true, 1, false).keeps_attachments);
        assert!(draft("  ", false, 1, false).can_send, "attachments alone");
        assert!(
            !draft("  ", true, 1, false).can_send,
            "not for a shell line"
        );
        assert!(!draft("\n", false, 0, false).can_send);
    }

    #[test]
    fn review_send_now_skips_the_queue() {
        let now = draft_intent("Review comments", false, 0, true, SendWhen::Now);
        assert_eq!((now.kind, now.queued), (DraftKind::Turn, false));
        let queue = draft_intent("Review comments", false, 0, true, SendWhen::Queue);
        assert!(queue.queued);
    }
}
