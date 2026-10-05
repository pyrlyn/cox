// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Background task list (T9.2): `/tasks` renders the running tasks the
//! core reports via `TaskCreated`/`TaskCompleted` (which `state` already
//! tracks for the status count), then the last few that finished with
//! their exit code and where their output went (T27.1).

use cox_protocol::ids::{ArchiveId, TaskId};

/// How many finished tasks `/tasks` still shows.
pub const FINISHED_KEPT: usize = 10;

/// One line per running task, then the recently finished ones, or the
/// empty-state line when there is nothing to show.
pub fn list(tasks: &[(TaskId, String)], finished: &[String]) -> String {
    let mut out = if tasks.is_empty() {
        "no background tasks running".to_string()
    } else {
        format!(
            "{} running background task{}",
            tasks.len(),
            if tasks.len() == 1 { "" } else { "s" }
        )
    };
    for (id, label) in tasks {
        out.push_str(&format!("\n- {id}: {label}"));
    }
    if !finished.is_empty() {
        out.push_str("\nfinished:");
        for line in finished {
            out.push_str(&format!("\n- {line}"));
        }
    }
    out
}

/// A finished task's `/tasks` line: a shell task's exit code and the
/// `/expand` id of its archived output; a subagent reports neither.
pub fn finished_line(
    task: TaskId,
    label: &str,
    exit_code: Option<i32>,
    archive: Option<ArchiveId>,
) -> String {
    let mut line = format!("{task}: {label}");
    if let Some(code) = exit_code {
        line.push_str(&format!(" · exit {code}"));
    }
    if let Some(id) = archive {
        line.push_str(&format!(" · /expand {id}"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_shell_task_shows_exit_code_and_expand_hint() {
        let (task, archive) = (TaskId::new(), ArchiveId::new());
        let line = finished_line(task, "bash: sleep 1", Some(0), Some(archive));
        assert_eq!(
            line,
            format!("{task}: bash: sleep 1 · exit 0 · /expand {archive}")
        );
        let text = list(&[], &[line]);
        assert!(
            text.starts_with("no background tasks running\nfinished:"),
            "{text}"
        );
        assert!(text.contains(&format!("/expand {archive}")), "{text}");
    }
}
