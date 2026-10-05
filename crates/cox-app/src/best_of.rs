// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Best of n (T52.9, DT§3.3.1, mockup 27): one prompt sent to n candidates
//! — cox on a model, or an external ACP agent — each in a worktree of its
//! own and a session of its own, grouped under one [`BestOfId`] the sidebar
//! shows as one group. The person chose the candidates in the UI, which is
//! A75's consent to the worktrees. A candidate that fails to start is
//! recorded with why and never stops the others (fail open). Then the
//! comparison (T52.10): each candidate's state, the files its worktree
//! changed with `+n −m`, its cost and how long it ran; and the pick, which
//! keeps one worktree and prunes the others. Separate from `app.rs` because
//! it spans several sessions and owns none of them.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use cox_protocol::errors::WorktreeError;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{FileStat, Worktree};
use cox_protocol::types::{ModelId, Tier};
use serde::{Deserialize, Serialize};

use crate::Activity;
use crate::Intent;
use crate::app::{App, AppError};
use crate::live::LiveSession;
use crate::workspace::WorkspaceError;

/// One best-of-n group: the ULID of its launch, lowercased so it can name
/// the candidates' worktrees and branches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BestOfId(pub String);

impl fmt::Display for BestOfId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who answers the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Candidate {
    /// Cox itself; `model` is switched to on the code tier before the
    /// prompt, `None` keeps the config's.
    Cox { model: Option<String> },
    /// An external ACP agent by its `[external_agents.<name>]` or plugin
    /// name.
    Agent { name: String },
}

/// What [`App::best_of`] launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestOfRequest {
    /// The repository every worktree is cut from.
    pub project: PathBuf,
    pub prompt: String,
    pub candidates: Vec<Candidate>,
}

/// One candidate as it was launched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launched {
    pub candidate: Candidate,
    /// `None` when the worktree could not be made.
    pub worktree: Option<Worktree>,
    /// `None` when no session opened.
    pub session: Option<SessionId>,
    /// Why it did not start; the others ran anyway.
    pub failed: Option<String>,
    /// When its launch began, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    /// Its worktree was removed by a pick (T52.10).
    #[serde(default)]
    pub pruned: bool,
}

impl Candidate {
    /// What the compare view heads its column with: `cox`, `cox · <model>`
    /// or the agent's name.
    pub fn label(&self) -> String {
        match self {
            Self::Cox { model: None } => "cox".into(),
            Self::Cox { model: Some(model) } => format!("cox · {model}"),
            Self::Agent { name } => name.clone(),
        }
    }
}

/// A launched group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestOf {
    pub id: BestOfId,
    pub project: PathBuf,
    pub prompt: String,
    pub candidates: Vec<Launched>,
    /// The candidate a pick kept (T52.10).
    #[serde(default)]
    pub kept: Option<u32>,
}

/// Where a candidate is (T52.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CandidateState {
    Running,
    /// An approval or question waits on the person.
    WaitingOnYou,
    Done,
    /// It did not start, or its turn failed.
    Failed {
        why: String,
    },
    /// A pick kept it.
    Kept,
    /// A pick removed its worktree.
    Pruned,
}

/// One column of the compare view (T52.10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateView {
    pub candidate: Candidate,
    pub label: String,
    pub state: CandidateState,
    pub session: Option<SessionId>,
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    /// What its worktree changed against the commit it was cut from.
    pub files: Vec<FileStat>,
    pub added: u32,
    pub removed: u32,
    /// Its own ledger rows; 0 for an external agent, which bills itself.
    pub cost_usd: f64,
    /// From its launch to its session's last write, or to now while it runs.
    pub duration_ms: u64,
}

/// What a pick did (T52.10).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picked {
    /// The worktrees removed.
    pub pruned: Vec<PathBuf>,
    /// The worktrees left because they hold uncommitted or untracked
    /// changes; picking again with `discard` removes them, which the person
    /// confirms a second time.
    pub dirty: Vec<PathBuf>,
    /// Why the rest were left: still running, or git refused.
    pub refused: Vec<String>,
}

/// What [`App::best_of`] returns: the group, and the live sessions of the
/// candidates that started, in candidate order, for the windows to show.
pub struct Launch {
    pub group: BestOf,
    pub sessions: Vec<Arc<LiveSession>>,
}

/// What a launch can fail with before any candidate runs.
#[derive(Debug, thiserror::Error)]
pub enum BestOfError {
    #[error("best of n needs at least one candidate")]
    NoCandidates,
    #[error("no best-of-n group {0}")]
    Unknown(BestOfId),
    #[error("best-of-n group {id} has no candidate {index}")]
    NoCandidate { id: BestOfId, index: u32 },
}

/// The groups launched by this process, by id. Kept in memory: a group
/// outlives no process, its sessions and worktrees do.
#[derive(Default)]
pub(crate) struct Groups(Mutex<HashMap<BestOfId, BestOf>>);

impl Groups {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<BestOfId, BestOf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn insert(&self, group: BestOf) {
        self.lock().insert(group.id.clone(), group);
    }

    pub(crate) fn get(&self, id: &BestOfId) -> Option<BestOf> {
        self.lock().get(id).cloned()
    }

    /// The group `session` was launched in, if any.
    pub(crate) fn group_of(&self, session: &SessionId) -> Option<BestOfId> {
        self.lock()
            .values()
            .find(|g| {
                g.candidates
                    .iter()
                    .any(|c| c.session.as_ref() == Some(session))
            })
            .map(|g| g.id.clone())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Launches every candidate in turn: its worktree, its session, the model
/// switch for a cox candidate, then the prompt, which starts a turn and
/// returns at once.
pub(crate) async fn launch(
    app: &Arc<App>,
    request: BestOfRequest,
    theme: String,
) -> Result<Launch, AppError> {
    if request.candidates.is_empty() {
        return Err(BestOfError::NoCandidates.into());
    }
    let id = BestOfId(SessionId::new().to_string().to_ascii_lowercase());
    let owner = owner(&id);
    let mut group = BestOf {
        id: id.clone(),
        project: request.project.clone(),
        prompt: request.prompt.clone(),
        candidates: Vec::new(),
        kept: None,
    };
    let mut sessions = Vec::new();
    for (n, candidate) in request.candidates.into_iter().enumerate() {
        let mut launched = Launched {
            candidate,
            worktree: None,
            session: None,
            failed: None,
            started_ms: now_ms(),
            pruned: false,
        };
        let name = format!("best-{id}-{}", n + 1);
        match app
            .workspace()
            .add_worktree(&request.project, &name, &owner)
            .await
        {
            Ok(tree) => {
                let cwd = tree.path.clone();
                launched.worktree = Some(tree);
                match start(app, &launched.candidate, cwd, &request.prompt, &theme).await {
                    Ok(live) => {
                        launched.session = Some(live.id());
                        sessions.push(live);
                    }
                    Err((session, why)) => {
                        launched.session = session;
                        launched.failed = Some(why);
                    }
                }
            }
            Err(e) => launched.failed = Some(e.to_string()),
        }
        group.candidates.push(launched);
    }
    app.workspace().groups().insert(group.clone());
    Ok(Launch { group, sessions })
}

/// The lock owner of group `id`'s worktrees: a cox owner, so cox may reuse
/// and prune them.
fn owner(id: &BestOfId) -> String {
    format!("{} best-of-{id}", cox_tools::git::OWNER_PREFIX)
}

/// Every candidate of group `id` as the compare view shows it. A worktree
/// git cannot read shows no files rather than failing the view.
pub(crate) async fn compare(app: &App, id: &BestOfId) -> Result<Vec<CandidateView>, AppError> {
    let workspace = app.workspace();
    let group = workspace.best_of(id)?;
    let mut views = Vec::new();
    for (n, launched) in group.candidates.iter().enumerate() {
        let state = state_of(app, &group, n, launched);
        let files = match &launched.worktree {
            Some(tree) if !launched.pruned => {
                workspace.diffstat(&tree.path).await.unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let cost_usd = match &launched.session {
            Some(session) => workspace.session_cost(session)?,
            None => 0.0,
        };
        let end = match (&state, &launched.session) {
            (CandidateState::Running | CandidateState::WaitingOnYou, _) | (_, None) => None,
            (_, Some(session)) => last_write_ms(app, session),
        };
        views.push(CandidateView {
            label: launched.candidate.label(),
            candidate: launched.candidate.clone(),
            session: launched.session,
            worktree: launched.worktree.as_ref().map(|t| t.path.clone()),
            branch: launched.worktree.as_ref().map(|t| t.branch.clone()),
            added: files.iter().map(|f| f.added).sum(),
            removed: files.iter().map(|f| f.removed).sum(),
            files,
            cost_usd,
            duration_ms: end
                .unwrap_or_else(now_ms)
                .saturating_sub(launched.started_ms),
            state,
        });
    }
    Ok(views)
}

fn state_of(app: &App, group: &BestOf, n: usize, launched: &Launched) -> CandidateState {
    if let Some(why) = &launched.failed {
        return CandidateState::Failed { why: why.clone() };
    }
    if group.kept.is_some_and(|kept| kept as usize == n) {
        return CandidateState::Kept;
    }
    if launched.pruned {
        return CandidateState::Pruned;
    }
    match launched.session.map(|s| app.activity(s)) {
        Some(Activity::Running) => CandidateState::Running,
        Some(Activity::WaitingOnYou) => CandidateState::WaitingOnYou,
        Some(Activity::Failed) => CandidateState::Failed {
            why: "its turn failed".into(),
        },
        Some(Activity::Idle) | None => CandidateState::Done,
    }
}

/// When `session` last wrote to `cox.db`, in milliseconds since the epoch.
fn last_write_ms(app: &App, session: &SessionId) -> Option<u64> {
    let info = app.workspace().store().session_info(session).ok()?;
    let at = chrono::DateTime::parse_from_rfc3339(&info.updated_at).ok()?;
    u64::try_from(at.timestamp_millis()).ok()
}

/// Keeps candidate `keep` of group `id` and removes the other worktrees.
/// A worktree with changes is left and listed unless `discard`; one whose
/// session still runs is left and says so, so a pick never pulls a tree
/// out from under a running turn.
pub(crate) async fn pick(
    app: &App,
    id: &BestOfId,
    keep: u32,
    discard: bool,
) -> Result<Picked, AppError> {
    let workspace = app.workspace();
    let mut group = workspace.best_of(id)?;
    if keep as usize >= group.candidates.len() {
        return Err(BestOfError::NoCandidate {
            id: id.clone(),
            index: keep,
        }
        .into());
    }
    let owner = owner(id);
    let mut picked = Picked::default();
    for (n, launched) in group.candidates.iter_mut().enumerate() {
        if n == keep as usize || launched.pruned {
            continue;
        }
        let Some(tree) = &launched.worktree else {
            continue;
        };
        let busy = launched
            .session
            .map(|s| app.activity(s))
            .is_some_and(|a| matches!(a, Activity::Running | Activity::WaitingOnYou));
        if busy {
            picked.refused.push(format!(
                "{} is still running; stop it first",
                launched.candidate.label()
            ));
            continue;
        }
        match workspace.remove_worktree(&tree.path, &owner, discard).await {
            Ok(()) => {
                picked.pruned.push(tree.path.clone());
                launched.pruned = true;
            }
            Err(WorkspaceError::Worktree(WorktreeError::Dirty { .. })) => {
                picked.dirty.push(tree.path.clone());
            }
            Err(e) => picked.refused.push(e.to_string()),
        }
    }
    group.kept = Some(keep);
    workspace.groups().insert(group);
    Ok(picked)
}

/// Opens one candidate's session in `cwd` and sends it the prompt. A
/// session that opened but refused the model or the prompt is reported
/// with its id, so the group still lists it.
async fn start(
    app: &Arc<App>,
    candidate: &Candidate,
    cwd: PathBuf,
    prompt: &str,
    theme: &str,
) -> Result<Arc<LiveSession>, (Option<SessionId>, String)> {
    let opened = match candidate {
        Candidate::Cox { .. } => app.open(cwd, None, theme.to_string()).await,
        Candidate::Agent { name } => app.open_agent(cwd, name, theme.to_string()).await,
    };
    let live = opened.map_err(|e| (None, e.to_string()))?;
    let failed = |e: AppError| (Some(live.id()), e.to_string());
    if let Candidate::Cox { model: Some(model) } = candidate {
        live.send(Intent::SwitchModel {
            tier: Tier::Code,
            model: Some(ModelId(model.clone())),
        })
        .await
        .map_err(failed)?;
    }
    live.send(Intent::Send {
        text: prompt.to_string(),
        attachments: Vec::new(),
        confirm_think: false,
    })
    .await
    .map_err(failed)?;
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launched(session: Option<SessionId>) -> Launched {
        Launched {
            candidate: Candidate::Cox { model: None },
            worktree: None,
            session,
            failed: None,
            started_ms: 0,
            pruned: false,
        }
    }

    #[test]
    fn best_of_group_of_finds_the_group_a_session_was_launched_in() {
        let (one, other) = (SessionId::new(), SessionId::new());
        let groups = Groups::default();
        groups.insert(BestOf {
            id: BestOfId("g".into()),
            project: PathBuf::from("/p"),
            prompt: "x".into(),
            candidates: vec![launched(None), launched(Some(one))],
            kept: None,
        });
        assert_eq!(groups.group_of(&one), Some(BestOfId("g".into())));
        assert_eq!(groups.group_of(&other), None);
    }
}
