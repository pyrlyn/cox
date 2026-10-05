// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Rebuild history from a store rollout for `cox run --resume` (T2.4).
//! `--continue` (latest session for this cwd) waits on a store listing API.

use std::path::{Path, PathBuf};

use cox_core::History;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use cox_store::Store;

use crate::cli::{Cli, RunArgs};
use crate::config_load;

/// Reconstructs history for `--resume <id>` or the current directory's latest
/// session for `--continue`.
pub fn run(cli: &Cli, args: &RunArgs) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let id = if args.r#continue {
        let cwd = cli.cwd.clone().unwrap_or(std::env::current_dir()?);
        Store::open(&home)?
            .latest_session_for_cwd(&cwd)?
            .to_string()
    } else {
        let Some(id) = args.resume.as_deref() else {
            anyhow::bail!("cox run requires -p <prompt>, --resume <id>, or --continue");
        };
        id.to_string()
    };
    let history = from_home(&home, &id)?;
    println!("{} messages", history.messages.len());
    Ok(())
}

/// T44.4: the directory session `id` ran in, as the store recorded it at
/// `session_create`; `None` when the id does not parse or the store has no
/// row for it (the resume itself then reports that).
pub fn recorded_cwd(home: &Path, id: &str) -> Option<PathBuf> {
    let id: SessionId = id.parse().ok()?;
    let info = Store::open(home).ok()?.session_info(&id).ok()?;
    Some(PathBuf::from(info.cwd))
}

/// Reads a session's rollout from `home` and rebuilds [`History`].
pub fn from_home(home: &Path, id: &str) -> anyhow::Result<History> {
    let id: SessionId = id.parse()?;
    Ok(cox_session::resume(home, id)?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cox_protocol::Event;
    use cox_protocol::ids::{SessionId, TurnId};
    use cox_protocol::types::StopReason;

    use super::*;

    #[test]
    fn resume_truncated_tail_keeps_events_and_warns() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        let id = SessionId::new();
        store
            .rollout_append(
                &id,
                &Event::TurnDone {
                    turn: TurnId::new(),
                    stop: StopReason::EndTurn,
                },
            )
            .expect("append");
        let path = home.path().join("sessions").join(format!("{id}.jsonl"));
        fs::write(
            &path,
            [fs::read(&path).expect("read"), b"{\"event\":".to_vec()].concat(),
        )
        .expect("truncate tail");

        let history = from_home(home.path(), &id.to_string()).expect("resume");
        assert!(history.truncated);
        assert!(history.truncated_notice().is_some());
    }
}
