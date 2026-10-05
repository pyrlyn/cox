// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What a Tasks-tab click opens (T37.29.6, DT§5.1): a subagent's own
//! session or a background shell's archived output, and which of the two a
//! task is. The events name neither the child session nor, until the task
//! ends, its kind, so both are read back here — from the parent's rollout
//! and its children in the store — rather than added to the protocol.
//! Separate from the fold, which never sees another session.

use cox_protocol::ids::{ArchiveId, SessionId, TaskId};
use cox_protocol::types::{Event, ItemKind};
use serde::{Deserialize, Serialize};

/// What a task runs, as the Tasks tab labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// A subagent: its transcript opens.
    Agent,
    /// A detached shell call: its archived output opens.
    Shell,
}

impl TaskKind {
    /// The core labels a detached call `<tool>: …` and a subagent
    /// `<preset>: …`; which tool is a shell is its rule, not ours.
    pub fn of(label: &str) -> Self {
        let head = label.split_once(": ").map_or(label, |(head, _)| head);
        match cox_core::tasks::TaskKind::of(head) {
            cox_core::tasks::TaskKind::Shell => Self::Shell,
            cox_core::tasks::TaskKind::Agent => Self::Agent,
        }
    }

    /// Settled once the task ends: only a shell reports an exit code or
    /// archives its output.
    pub fn ended(self, exit_code: Option<i32>, archive: Option<ArchiveId>) -> Self {
        if exit_code.is_some() || archive.is_some() {
            Self::Shell
        } else {
            self
        }
    }
}

/// What opening a task shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskTarget {
    /// A subagent's own session.
    Transcript { session: SessionId },
    /// A finished shell's full output, as `cox expand <id>` prints it.
    Output { archive: ArchiveId },
}

/// What opening `task` shows, from the parent's rollout `events` and its
/// `children` oldest first, `prompt` reading a child's first prompt.
/// `None` for an unknown task, a shell still running or a subagent whose
/// session is not found.
///
/// The n-th subagent is the n-th child whose first prompt its label quotes:
/// a fork or handoff is a child too, and its first prompt is not the task.
pub fn open(
    events: &[Event],
    task: TaskId,
    children: &[SessionId],
    mut prompt: impl FnMut(&SessionId) -> Option<String>,
) -> Option<TaskTarget> {
    // In the order each task was first created; a woken subagent is created again.
    let mut tasks: Vec<(TaskId, &str, TaskKind, Option<ArchiveId>)> = Vec::new();
    for event in events {
        match event {
            Event::TaskCreated { task, label, .. } if tasks.iter().all(|t| t.0 != *task) => {
                tasks.push((*task, label, TaskKind::of(label), None));
            }
            Event::TaskCompleted {
                task,
                exit_code,
                archive,
                ..
            } => {
                if let Some(t) = tasks.iter_mut().find(|t| t.0 == *task) {
                    (t.2, t.3) = (t.2.ended(*exit_code, *archive), archive.or(t.3));
                }
            }
            _ => {}
        }
    }
    let &(_, _, kind, archive) = tasks.iter().find(|t| t.0 == task)?;
    if kind == TaskKind::Shell {
        return archive.map(|archive| TaskTarget::Output { archive });
    }
    let mut prompts: Vec<Option<Option<String>>> = vec![None; children.len()];
    let mut next = 0;
    for (id, label, ..) in tasks.iter().filter(|t| t.2 == TaskKind::Agent) {
        let quoted = label.split_once(": ").map_or("", |(_, quoted)| quoted);
        let found = (next..children.len()).find(|&i| {
            prompts[i]
                .get_or_insert_with(|| prompt(&children[i]))
                .as_deref()
                .is_some_and(|p| p.trim_start().starts_with(quoted))
        });
        match found {
            Some(i) if *id == task => {
                return Some(TaskTarget::Transcript {
                    session: children[i],
                });
            }
            Some(i) => next = i + 1,
            None if *id == task => return None,
            None => {}
        }
    }
    None
}

/// A session's first prompt: its first user message.
pub fn first_prompt(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| match event {
        Event::ItemStarted {
            kind: ItemKind::UserMessage { text, .. },
            ..
        } => Some(text.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::ItemId;
    use cox_protocol::types::Tier;

    use super::*;

    fn created(task: TaskId, label: &str) -> Event {
        Event::TaskCreated {
            task,
            label: label.into(),
            tier: Tier::Cheap,
        }
    }

    fn completed(task: TaskId, exit_code: Option<i32>, archive: Option<ArchiveId>) -> Event {
        Event::TaskCompleted {
            task,
            result_item: ItemId::new(),
            cost_usd: 0.0,
            exit_code,
            archive,
        }
    }

    #[test]
    fn a_shell_label_or_an_exit_code_makes_a_shell_task() {
        assert_eq!(TaskKind::of("bash: cargo test"), TaskKind::Shell);
        assert_eq!(TaskKind::of("explore: find x"), TaskKind::Agent);
        assert_eq!(TaskKind::Agent.ended(None, None), TaskKind::Agent);
        assert_eq!(TaskKind::Agent.ended(Some(0), None), TaskKind::Shell);
    }

    /// A fork sits between the two subagents' sessions and a shell between
    /// their tasks; each subagent still gets its own child.
    #[test]
    fn subagents_pair_with_the_children_their_labels_quote() {
        let (a, shell, b) = (TaskId::new(), TaskId::new(), TaskId::new());
        let archive = ArchiveId::new();
        let events = [
            created(a, "explore: find x"),
            created(shell, "bash: ls"),
            created(b, "explore: find y"),
            completed(shell, Some(0), Some(archive)),
            created(a, "explore: find x"),
        ];
        let (child_a, fork, child_b) = (SessionId::new(), SessionId::new(), SessionId::new());
        let prompt = |id: &SessionId| {
            Some(
                if *id == child_a {
                    "find x"
                } else if *id == fork {
                    "hello"
                } else {
                    "find y\nmore"
                }
                .into(),
            )
        };
        let children = [child_a, fork, child_b];
        let target = |task| open(&events, task, &children, prompt);
        assert_eq!(target(a), Some(TaskTarget::Transcript { session: child_a }));
        assert_eq!(target(b), Some(TaskTarget::Transcript { session: child_b }));
        assert_eq!(target(shell), Some(TaskTarget::Output { archive }));
        assert_eq!(target(TaskId::new()), None);
    }

    #[test]
    fn a_running_shell_opens_nothing() {
        let shell = TaskId::new();
        let events = [created(shell, "bash: sleep 9")];
        assert_eq!(open(&events, shell, &[], |_| None), None);
    }
}
