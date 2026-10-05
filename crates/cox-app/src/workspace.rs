// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The workspace (DT§4.3): what the sidebar lists before any session is
//! open — projects (git roots) with their sessions, the sidebar's sections
//! (T58.4.4), full-text search over every past session, and each project's
//! worktrees with their disk size, and the best-of-n groups launched here
//! (T52.9). Read from `cox.db` through `cox-store`; separate from the
//! controller because it spans every session and drives none.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_protocol::traits::{FileStat, Store as _, Worktree, WorktreeInfo, Worktrees};
use cox_protocol::{SessionId, StoreError, WorktreeError};
use cox_store::Store;
use cox_store::fts::SessionInfo;
use cox_store::lock::Holder;
use serde::{Deserialize, Serialize};

use crate::app::{App, AppError};
use crate::best_of::{BestOf, BestOfError, BestOfId, Groups};
use crate::inbox::{Activity, InboxItem, InboxStatus};

/// How many prompts, across every session, [`Workspace::prompts`] reads:
/// as many as the TUI's `Ctrl+R` search.
const PROMPT_SCAN: i64 = 5000;

/// What a workspace query can fail with.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    BestOf(#[from] BestOfError),
}

/// One sidebar project: a git root (or a bare cwd outside git). The count
/// is a `u64` so `cox-ffi` exports the type as it is (D11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub root: PathBuf,
    /// The root's last path component.
    pub name: String,
    pub sessions: u64,
    pub cost_usd: f64,
    /// RFC 3339, the newest session's last write.
    pub updated_at: String,
}

/// One session row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub info: SessionInfo,
    /// Another process drives it (T37.34): open it read-only or fork it.
    pub held_by: Option<Holder>,
    /// The external ACP agent that drove it (T52.6); `None` for cox.
    #[serde(default)]
    pub agent: Option<String>,
    /// The best-of-n group it was launched in (T52.9), which the sidebar
    /// shows as one group; `None` for every other session.
    #[serde(default)]
    pub best_of: Option<String>,
}

impl SessionEntry {
    /// What a sidebar row calls it until the core has stored a title.
    pub fn name(&self) -> &str {
        self.info.title.as_deref().unwrap_or("Untitled session")
    }
}

/// One full-text hit, with the session it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub session: SessionInfo,
    pub turn: i64,
    pub snippet: String,
}

/// How many projects, and sessions of each, the sidebar reads: the same
/// caps the Swift store used, so a move does not change what is listed.
const SIDEBAR_LIMIT: i64 = 20;

/// The status glyph, named as the clients' status dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarStatus {
    Running,
    Waiting,
    Idle,
    Error,
}

/// One subtitle piece. The filter matches text; `age` holds `updated_at`
/// so each client localizes it and the filter never sees that wording (A129).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubtitlePart {
    Text { text: String },
    Age { updated_at: String },
}

/// A status section or a project, as the clients' sidebar group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidebarKind {
    Section { count: Option<String> },
    Project { is_expanded: bool },
}

/// One inbox or session row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarRow {
    pub id: String,
    pub session: SessionId,
    pub status: SidebarStatus,
    pub title: String,
    pub subtitle: Vec<SubtitlePart>,
    /// Set only when the session has spent something.
    pub cost: Option<f64>,
    pub is_read_only: bool,
}

/// "Needs you", "Running", or one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarSection {
    pub id: String,
    pub title: String,
    pub kind: SidebarKind,
    pub rows: Vec<SidebarRow>,
}

/// One per process. `worktrees` is the git side (`cox_tools::git::GitWorktrees`
/// in the app), injected so this crate never runs a process itself.
pub struct Workspace {
    store: Store,
    worktrees: Arc<dyn Worktrees>,
    groups: Groups,
}

impl Workspace {
    /// The workspace over `COX_HOME` = `home`.
    pub fn open(home: &Path, worktrees: Arc<dyn Worktrees>) -> Result<Self, WorkspaceError> {
        Ok(Self {
            store: Store::open(home)?,
            worktrees,
            groups: Groups::default(),
        })
    }

    /// The store the workspace reads; a live session reads its checkpoint
    /// rows from it too (T37.29.1).
    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    /// Projects of the `limit` most recent sessions, most recently active
    /// first, as the TUI's `/resume` groups them (`find_git_root`).
    pub fn projects(&self, limit: i64) -> Result<Vec<Project>, WorkspaceError> {
        let mut rows: Vec<Project> = Vec::new();
        for s in self.store.list_sessions(limit)? {
            let root = project_of(Path::new(&s.cwd));
            match rows.iter_mut().find(|r| r.root == root) {
                Some(row) => {
                    row.sessions += 1;
                    row.cost_usd += s.cost_usd;
                }
                None => rows.push(Project {
                    name: root.file_name().map_or_else(
                        || root.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    ),
                    root,
                    sessions: 1,
                    cost_usd: s.cost_usd,
                    updated_at: s.updated_at,
                }),
            }
        }
        Ok(rows)
    }

    /// `project`'s sessions among the `limit` most recent, newest first,
    /// each with the process that drives it when that is another one.
    pub fn sessions(
        &self,
        project: &Path,
        limit: i64,
    ) -> Result<Vec<SessionEntry>, WorkspaceError> {
        let mut out = Vec::new();
        for info in self.store.list_sessions(limit)? {
            if project_of(Path::new(&info.cwd)) != project {
                continue;
            }
            let (held_by, agent) = match info.id.parse::<SessionId>() {
                Ok(id) => (
                    self.store.session_holder(&id)?,
                    self.store.session_agent(&id)?.map(|a| a.agent),
                ),
                Err(_) => (None, None),
            };
            let best_of = info
                .id
                .parse::<SessionId>()
                .ok()
                .and_then(|id| self.groups.group_of(&id))
                .map(|g| g.0);
            out.push(SessionEntry {
                info,
                held_by,
                agent,
                best_of,
            });
        }
        Ok(out)
    }

    /// Full-text search over every session (`rollout_fts`), best first; a
    /// hit whose session row is gone is dropped.
    pub fn search(&self, query: &str, limit: i64) -> Result<Vec<SearchHit>, WorkspaceError> {
        let mut out = Vec::new();
        for hit in self.store.rollout_search(query, limit)? {
            let Ok(id) = hit.session_id.parse::<SessionId>() else {
                continue;
            };
            match self.store.session_info(&id) {
                Ok(session) => out.push(SearchHit {
                    session,
                    turn: hit.turn,
                    snippet: hit.snippet,
                }),
                Err(StoreError::NotFound) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }

    /// `session`'s own prompts, newest first, at most `limit` (T37.24.6):
    /// what ↑ in an empty composer walks. The TUI's `Ctrl+R` query
    /// (`user_prompts`) kept to one session; that query spans every
    /// session, so this scans as far back as the TUI's search does.
    pub fn prompts(&self, session: SessionId, limit: usize) -> Result<Vec<String>, WorkspaceError> {
        let session = session.to_string();
        Ok(self
            .store
            .user_prompts(PROMPT_SCAN)?
            .into_iter()
            .filter(|p| p.session_id == session)
            .map(|p| p.text)
            .take(limit)
            .collect())
    }

    /// `project`'s checkouts with branch, lock, merged/stale state and disk
    /// size; the size walk runs off the async runtime (`worktree_list`).
    pub async fn worktrees(&self, project: &Path) -> Result<Vec<WorktreeInfo>, WorkspaceError> {
        Ok(self.worktrees.list(project).await?)
    }

    /// The worktree `name` of `project`'s repository, locked for `owner`
    /// (T52.9): made through the injected git side, as every worktree is.
    pub(crate) async fn add_worktree(
        &self,
        project: &Path,
        name: &str,
        owner: &str,
    ) -> Result<Worktree, WorkspaceError> {
        Ok(self.worktrees.add(project, name, owner).await?)
    }

    /// What the worktree at `path` changed against its base (T52.10).
    pub(crate) async fn diffstat(&self, path: &Path) -> Result<Vec<FileStat>, WorkspaceError> {
        Ok(self.worktrees.diffstat(path).await?)
    }

    /// Removes the worktree at `path` locked for `owner`; `discard` only
    /// after the person confirmed a second time (T52.10).
    pub(crate) async fn remove_worktree(
        &self,
        path: &Path,
        owner: &str,
        discard: bool,
    ) -> Result<(), WorkspaceError> {
        Ok(self.worktrees.remove(path, owner, discard).await?)
    }

    pub(crate) fn groups(&self) -> &Groups {
        &self.groups
    }

    /// The best-of-n group `id` as launched (T52.9).
    pub fn best_of(&self, id: &BestOfId) -> Result<BestOf, WorkspaceError> {
        self.groups
            .get(id)
            .ok_or_else(|| BestOfError::Unknown(id.clone()).into())
    }

    /// What group `id` cost: the sum of its candidates' own ledger rows
    /// (T52.9). An external agent's session has none; its billing is the
    /// agent's.
    pub fn best_of_cost(&self, id: &BestOfId) -> Result<f64, WorkspaceError> {
        let mut total = 0.0;
        for session in self
            .best_of(id)?
            .candidates
            .iter()
            .filter_map(|c| c.session)
        {
            total += self.session_cost(&session)?;
        }
        Ok(total)
    }

    /// One session's cost from its ledger rows.
    pub(crate) fn session_cost(&self, session: &SessionId) -> Result<f64, WorkspaceError> {
        Ok(self
            .store
            .usage_ledger(session)?
            .iter()
            .map(|row| row.usage.usage.cost_usd)
            .sum())
    }
}

impl App {
    /// "Needs you" from the inbox, "Running" lifted out of the projects,
    /// then each project. A project with no match is hidden while filtering,
    /// and a folded one opens for the match.
    pub fn sidebar(
        &self,
        filter: &str,
        folded: &[PathBuf],
    ) -> Result<Vec<SidebarSection>, AppError> {
        let inbox = self.inbox();
        let mut groups = Vec::new();
        for project in self.workspace().projects(SIDEBAR_LIMIT)? {
            let sessions = self.workspace().sessions(&project.root, SIDEBAR_LIMIT)?;
            groups.push((project, sessions));
        }
        Ok(sections(
            &inbox,
            &groups,
            |id| self.activity(id),
            filter,
            folded,
        ))
    }
}

fn sections(
    inbox: &[InboxItem],
    projects: &[(Project, Vec<SessionEntry>)],
    activity: impl Fn(SessionId) -> Activity,
    filter: &str,
    folded: &[PathBuf],
) -> Vec<SidebarSection> {
    let needs: Vec<SidebarRow> = inbox
        .iter()
        .map(inbox_row)
        .filter(|row| matches_filter(row, filter))
        .collect();
    let mut running = Vec::new();
    let mut groups = Vec::new();
    for (project, sessions) in projects {
        let mut rows = Vec::new();
        for entry in sessions {
            let Ok(id) = entry.info.id.parse() else {
                continue;
            };
            let row = session_row(entry, id, project, activity(id));
            if !matches_filter(&row, filter) {
                continue;
            }
            if row.status == SidebarStatus::Running {
                running.push(row);
            } else {
                rows.push(row);
            }
        }
        if rows.is_empty() && !filter.is_empty() {
            continue;
        }
        let is_expanded = !folded.iter().any(|p| p == &project.root) || !filter.is_empty();
        groups.push(SidebarSection {
            id: project.root.to_string_lossy().into_owned(),
            title: project.name.clone(),
            kind: SidebarKind::Project { is_expanded },
            rows,
        });
    }
    let mut out = Vec::new();
    if !needs.is_empty() {
        out.push(SidebarSection {
            id: "needs-you".into(),
            title: "Needs you".into(),
            kind: SidebarKind::Section {
                count: Some(needs.len().to_string()),
            },
            rows: needs,
        });
    }
    if !running.is_empty() {
        out.push(SidebarSection {
            id: "running".into(),
            title: "Running".into(),
            kind: SidebarKind::Section { count: None },
            rows: running,
        });
    }
    out.extend(groups);
    out
}

fn inbox_row(item: &InboxItem) -> SidebarRow {
    SidebarRow {
        id: format!("{}#{}", item.session, item.seq),
        session: item.session,
        status: match item.status {
            InboxStatus::Waiting => SidebarStatus::Waiting,
            InboxStatus::Idle => SidebarStatus::Idle,
            InboxStatus::Error => SidebarStatus::Error,
        },
        title: item.title.clone(),
        subtitle: vec![SubtitlePart::Text {
            text: item.subtitle.clone(),
        }],
        cost: None,
        is_read_only: item.expired,
    }
}

fn session_row(
    entry: &SessionEntry,
    session: SessionId,
    project: &Project,
    activity: Activity,
) -> SidebarRow {
    let mut subtitle = Vec::new();
    if let Some(agent) = &entry.agent {
        subtitle.push(SubtitlePart::Text {
            text: agent.clone(),
        });
    }
    let status = match activity {
        Activity::Running => {
            subtitle.push(SubtitlePart::Text {
                text: project.name.clone(),
            });
            subtitle.push(SubtitlePart::Text {
                text: "running".into(),
            });
            SidebarStatus::Running
        }
        Activity::WaitingOnYou => {
            subtitle.push(SubtitlePart::Text {
                text: project.name.clone(),
            });
            subtitle.push(SubtitlePart::Text {
                text: "waiting for you".into(),
            });
            SidebarStatus::Waiting
        }
        Activity::Failed => {
            subtitle.push(SubtitlePart::Age {
                updated_at: entry.info.updated_at.clone(),
            });
            subtitle.push(SubtitlePart::Text {
                text: "failed".into(),
            });
            SidebarStatus::Error
        }
        Activity::Idle => {
            subtitle.push(SubtitlePart::Age {
                updated_at: entry.info.updated_at.clone(),
            });
            if entry.info.turns > 0 {
                subtitle.push(SubtitlePart::Text {
                    text: "done".into(),
                });
            }
            SidebarStatus::Idle
        }
    };
    SidebarRow {
        id: entry.info.id.clone(),
        session,
        status,
        title: entry.name().to_owned(),
        subtitle,
        cost: (entry.info.cost_usd > 0.0).then_some(entry.info.cost_usd),
        is_read_only: false,
    }
}

fn matches_filter(row: &SidebarRow, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    // A129: no workspace crate folds diacritics; case-insensitive only, and
    // never the `age` part, which each client localizes.
    let needle = filter.to_lowercase();
    if row.title.to_lowercase().contains(&needle) {
        return true;
    }
    row.subtitle.iter().any(|part| match part {
        SubtitlePart::Text { text } => text.to_lowercase().contains(&needle),
        SubtitlePart::Age { .. } => false,
    })
}

/// The project a session belongs to: its cwd's git root, else the cwd.
fn project_of(cwd: &Path) -> PathBuf {
    cox_config::load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn project(root: &str, name: &str) -> Project {
        Project {
            root: PathBuf::from(root),
            name: name.into(),
            sessions: 0,
            cost_usd: 0.0,
            updated_at: String::new(),
        }
    }

    fn entry(
        id: SessionId,
        title: Option<&str>,
        cwd: &str,
        updated_at: &str,
        turns: i64,
        cost_usd: f64,
    ) -> SessionEntry {
        SessionEntry {
            info: SessionInfo {
                id: id.to_string(),
                title: title.map(str::to_owned),
                cwd: cwd.into(),
                created_at: updated_at.into(),
                updated_at: updated_at.into(),
                turns,
                cost_usd,
            },
            held_by: None,
            agent: None,
            best_of: None,
        }
    }

    // Clippy rejects this tuple inline (`type_complexity`). The alias is
    // the fixture the sidebar tests share.
    type Listed = (
        Vec<(Project, Vec<SessionEntry>)>,
        HashMap<SessionId, Activity>,
        [SessionId; 4],
    );

    fn listed() -> Listed {
        let ids = [
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        ];
        let [jitter, bench, sitemap, fresh] = ids;
        let groups = vec![
            (
                project("/src/cox", "cox"),
                vec![
                    entry(
                        jitter,
                        Some("Add retry jitter"),
                        "/src/cox",
                        "2026-09-29T12:00:00Z",
                        3,
                        0.42,
                    ),
                    entry(
                        bench,
                        Some("Bench plugin cold start"),
                        "/src/cox",
                        "2026-09-29T10:00:00Z",
                        5,
                        1.18,
                    ),
                ],
            ),
            (
                project("/src/acme-web", "acme-web"),
                vec![
                    entry(
                        sitemap,
                        Some("Sitemap generator"),
                        "/src/acme-web",
                        "2026-09-29T11:00:00Z",
                        1,
                        0.0,
                    ),
                    entry(fresh, None, "/src/acme-web", "2026-09-29T12:01:00Z", 0, 0.0),
                ],
            ),
        ];
        let activity = HashMap::from([(jitter, Activity::Running), (sitemap, Activity::Failed)]);
        (groups, activity, ids)
    }

    fn draw(
        groups: &[(Project, Vec<SessionEntry>)],
        activity: &HashMap<SessionId, Activity>,
        filter: &str,
        folded: &[PathBuf],
    ) -> Vec<SidebarSection> {
        sections(
            &[],
            groups,
            |id| activity.get(&id).copied().unwrap_or_default(),
            filter,
            folded,
        )
    }

    fn texts(row: &SidebarRow) -> Vec<&str> {
        row.subtitle
            .iter()
            .filter_map(|part| match part {
                SubtitlePart::Text { text } => Some(text.as_str()),
                SubtitlePart::Age { .. } => None,
            })
            .collect()
    }

    #[test]
    fn running_sessions_leave_their_project() {
        let (groups, activity, [jitter, bench, sitemap, fresh]) = listed();
        let sections = draw(&groups, &activity, "", &[]);
        assert_eq!(
            sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["running", "/src/cox", "/src/acme-web"]
        );
        assert_eq!(sections[0].kind, SidebarKind::Section { count: None });
        assert_eq!(sections[0].rows.len(), 1);
        let run = &sections[0].rows[0];
        assert_eq!(run.session, jitter);
        assert_eq!(run.status, SidebarStatus::Running);
        assert_eq!(run.title, "Add retry jitter");
        assert_eq!(texts(run), ["cox", "running"]);
        assert_eq!(run.cost, Some(0.42));
        assert_eq!(sections[1].kind, SidebarKind::Project { is_expanded: true });
        assert_eq!(
            sections[1]
                .rows
                .iter()
                .map(|r| r.session)
                .collect::<Vec<_>>(),
            [bench]
        );
        assert_eq!(texts(&sections[1].rows[0]), ["done"]);
        assert_eq!(sections[1].rows[0].cost, Some(1.18));
        let acme = &sections[2].rows;
        assert_eq!(
            acme.iter().map(|r| r.status).collect::<Vec<_>>(),
            [SidebarStatus::Error, SidebarStatus::Idle]
        );
        assert_eq!(acme[0].session, sitemap);
        assert_eq!(texts(&acme[0]), ["failed"]);
        assert_eq!(acme[1].session, fresh);
        assert_eq!(acme[1].title, "Untitled session");
        assert_eq!(acme[1].cost, None);
        assert!(texts(&acme[1]).is_empty());
    }

    #[test]
    fn a_filter_opens_a_folded_project() {
        let (groups, activity, [_, _, sitemap, _]) = listed();
        let acme = PathBuf::from("/src/acme-web");
        let folded = draw(&groups, &activity, "", std::slice::from_ref(&acme));
        assert_eq!(
            folded.last().map(|s| &s.kind),
            Some(&SidebarKind::Project { is_expanded: false })
        );
        let sections = draw(&groups, &activity, "sitemap", std::slice::from_ref(&acme));
        assert_eq!(
            sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["/src/acme-web"]
        );
        assert_eq!(sections[0].kind, SidebarKind::Project { is_expanded: true });
        assert_eq!(
            sections[0]
                .rows
                .iter()
                .map(|r| r.session)
                .collect::<Vec<_>>(),
            [sitemap]
        );
    }

    #[test]
    fn an_idle_session_says_done_only_after_a_turn() {
        let id = SessionId::new();
        let groups = vec![(
            project("/src/cox", "cox"),
            vec![entry(
                id,
                Some("Fresh"),
                "/src/cox",
                "2026-09-29T12:00:00Z",
                0,
                0.0,
            )],
        )];
        let idle = HashMap::new();
        let before = draw(&groups, &idle, "", &[]);
        assert!(texts(&before[0].rows[0]).is_empty());
        let groups = vec![(
            project("/src/cox", "cox"),
            vec![entry(
                id,
                Some("Fresh"),
                "/src/cox",
                "2026-09-29T12:00:00Z",
                1,
                0.0,
            )],
        )];
        let after = draw(&groups, &idle, "", &[]);
        assert_eq!(texts(&after[0].rows[0]), ["done"]);
    }

    #[test]
    fn an_untitled_session_is_named_untitled() {
        let id = SessionId::new();
        let groups = vec![(
            project("/src/cox", "cox"),
            vec![entry(id, None, "/src/cox", "2026-09-29T12:00:00Z", 0, 0.0)],
        )];
        let sections = draw(&groups, &HashMap::new(), "", &[]);
        assert_eq!(sections[0].rows[0].title, "Untitled session");
        let groups = vec![(
            project("/src/cox", "cox"),
            vec![entry(
                id,
                Some("Fix the ledger"),
                "/src/cox",
                "2026-09-29T12:00:00Z",
                0,
                0.0,
            )],
        )];
        let sections = draw(&groups, &HashMap::new(), "", &[]);
        assert_eq!(sections[0].rows[0].title, "Fix the ledger");
    }
}
