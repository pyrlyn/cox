// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/refine` and `cox refine` (T66.8, P66): the TUI's answer to a
//! `Ask::Refine` and the two CLI subcommands, over the one `cox_session::refine`
//! implementation. Here and not in `cox-session` because this is where
//! output is worded for a person and where the TUI's channel ends; the
//! rules (what a global undo needs, what a stale entry does) stay in
//! `cox-session`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cox_core::Session;
use cox_protocol::Config;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use cox_session::refine::{Pending, RefineError, Refined, Refiner, Reviewed, describe, undo_event};
use cox_store::Store;
use cox_store::refine::RefineScope;
use cox_tui::refine::{Msg, Request};

/// How many rows `/refine list` shows.
const LIST_LIMIT: usize = 20;

/// One live session's refine state: the proposal the user is reviewing is
/// held here between the review and the `y`, so the TUI carries no entries.
pub struct TuiRefine {
    refiner: Refiner,
    pending: Mutex<Option<Pending>>,
}

impl TuiRefine {
    /// `None` when the store cannot be opened: refine is an extension, so
    /// the session runs without it and the command says why.
    pub fn open(session: Session, home: &Path, config: &Config, cwd: &Path) -> Option<Arc<Self>> {
        let store = Arc::new(Store::open(home).ok()?);
        let memory_dir = cox_session::memory_dir_for(config, home, cwd);
        Some(Arc::new(Self {
            refiner: Refiner::new(session, store, home.to_path_buf(), memory_dir),
            pending: Mutex::new(None),
        }))
    }

    pub async fn answer(&self, request: Request) -> Msg {
        let notice = |result: Result<String, RefineError>| {
            Msg::Notice(result.map_err(|e| format!("refine: {e}")))
        };
        match request {
            Request::Propose { text, global } => {
                match self.refiner.propose(&text, scope(global)).await {
                    Ok(Reviewed::Ready(pending)) => {
                        let review = Msg::Review {
                            evidence: pending.evidence.clone(),
                            diff: pending.diff.clone(),
                        };
                        *self.slot() = Some(pending);
                        review
                    }
                    Ok(Reviewed::Unchanged) => Msg::Notice(Ok("refine: nothing to change".into())),
                    Ok(Reviewed::Rejected { event_id, reason }) => {
                        Msg::Notice(Err(format!("refine rejected ({event_id}): {reason}")))
                    }
                    Err(e) => notice(Err(e)),
                }
            }
            Request::Apply => {
                let Some(pending) = self.slot().take() else {
                    return Msg::Notice(Err("refine: no proposal to apply".into()));
                };
                notice(self.refiner.apply(&pending).map(|done| match done {
                    Refined::Applied {
                        event_id, edits, ..
                    } => applied_line(&event_id, &edits),
                    Refined::Unchanged => "refine: nothing to change".into(),
                    Refined::Rejected { event_id, reason } => {
                        format!("refine rejected ({event_id}): {reason}")
                    }
                }))
            }
            Request::List => {
                notice(
                    self.refiner
                        .history(LIST_LIMIT)
                        .map(|rows| match rows.is_empty() {
                            true => "no refines in this session".to_string(),
                            false => rows.iter().map(describe).collect::<Vec<_>>().join("\n"),
                        }),
                )
            }
            Request::Undo { id, global } => {
                let undone = match id {
                    Some(id) => self.refiner.undo(&id, global),
                    None => self.refiner.undo_latest(global),
                };
                notice(undone.map(|u| format!("refine: undid {} ({})", u.undid, u.event_id)))
            }
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn scope(global: bool) -> RefineScope {
    match global {
        true => RefineScope::Global,
        false => RefineScope::Local,
    }
}

/// Skills and subagents are read once at session open, so say when the
/// change will show up.
fn applied_line(event_id: &str, edits: &[cox_ext::harness::EditRecord]) -> String {
    let list: Vec<String> = edits
        .iter()
        .map(|e| format!("{} {}/{}", e.action.as_str(), e.kind.as_str(), e.id))
        .collect();
    let later = edits.iter().any(|e| {
        matches!(
            e.kind,
            cox_ext::harness::Kind::Skill | cox_ext::harness::Kind::Subagent
        )
    });
    let when = if later {
        "; skills and subagents load in the next session"
    } else {
        ""
    };
    format!(
        "refine applied {event_id}: {}{when}; /refine undo reverses it",
        list.join(", ")
    )
}

/// The session a CLI call is about: `--session`, else the newest one in
/// this directory.
fn session_of(store: &Store, cwd: &Path, given: Option<&str>) -> anyhow::Result<SessionId> {
    match given {
        Some(id) => Ok(id.parse()?),
        None => store
            .latest_session_for_cwd(cwd)
            .map_err(|_| anyhow::anyhow!("no session in {}; pass --session", cwd.display())),
    }
}

/// `cox refine list [--session ID]`.
pub fn list(home: &Path, cwd: &Path, session: Option<&str>) -> anyhow::Result<()> {
    let store = Store::open(home)?;
    let id = session_of(&store, cwd, session)?;
    let rows = store.refine_list(&id, usize::MAX)?;
    if rows.is_empty() {
        println!("no refines in session {id}");
    }
    for row in &rows {
        println!("{}", describe(row));
    }
    Ok(())
}

/// `cox refine undo <id> [--global]`.
pub fn undo(
    home: &Path,
    config: &Config,
    cwd: &Path,
    event: &str,
    global: bool,
) -> anyhow::Result<()> {
    let store = Store::open(home)?;
    let memory_dir = cox_session::memory_dir_for(config, home, cwd);
    let undone = undo_event(&store, home, &memory_dir, event, global)?;
    println!("undid {} ({})", undone.undid, undone.event_id);
    Ok(())
}
