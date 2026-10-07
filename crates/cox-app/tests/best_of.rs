// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Best of n end to end (T52.9, T52.10, DT§3.3.1): `App::best_of`,
//! `compare` and `pick` over a fake git side that makes plain directories,
//! counts their files' lines as the diffstat, treats a `DIRTY` file as
//! uncommitted work and remembers what it was asked for,
//! with every cox candidate on the Scripted provider in a scratch
//! `COX_HOME`. Never the real `~/.cox`, never a keychain (A49); nextest runs
//! each test in its own process, so each sets its own environment.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cox_app::app::{App, AppError, Host};
use cox_app::live::LiveSession;
use cox_app::{
    BestOfError, BestOfMerge, BestOfRequest, BlockKind, Candidate, CandidateState, InboxItem,
    Launch, TimelinePatch,
};
use cox_protocol::Config;
use cox_protocol::errors::WorktreeError;
use cox_protocol::traits::{FileStat, Store as _, Worktree, Worktrees};

const THEME: &str = "base16-ocean.dark";
const PROMPT: &str = "Split CheckoutForm into three components.";

/// One reply per session.
const REPLY: &str = r#"
[[turn]]
text = "Done."
"#;

struct FakeKey;

impl Host for FakeKey {
    fn notify(&self, _: InboxItem, _: u32) {}
    fn badge(&self, _: u32) {}
    fn open_url(&self, _: &str) {}
    /// Only the fake agent's key, as the app's Keychain would hold it.
    fn secret(&self, section: &str) -> Option<String> {
        (section == "fake").then(|| String::from("test-key"))
    }
}

/// Makes `<root>/<name>` for each worktree asked for and remembers the
/// name and owner.
struct Trees {
    root: PathBuf,
    asked: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl Worktrees for Trees {
    async fn add(&self, from: &Path, name: &str, owner: &str) -> Result<Worktree, WorktreeError> {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).map_err(|e| WorktreeError::Git {
            args: "mkdir".into(),
            stderr: e.to_string(),
        })?;
        self.asked
            .lock()
            .expect("asked")
            .push((name.into(), owner.into()));
        Ok(Worktree {
            path,
            branch: name.into(),
            main: from.to_path_buf(),
        })
    }

    async fn diffstat(&self, path: &Path) -> Result<Vec<FileStat>, WorktreeError> {
        let mut stats = Vec::new();
        for entry in std::fs::read_dir(path).expect("tree").flatten() {
            if entry.path().is_file() {
                let text = std::fs::read_to_string(entry.path()).expect("file");
                stats.push(FileStat {
                    path: entry.file_name().into(),
                    added: u32::try_from(text.lines().count()).expect("lines"),
                    removed: 0,
                });
            }
        }
        stats.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(stats)
    }

    async fn remove(&self, path: &Path, owner: &str, discard: bool) -> Result<(), WorktreeError> {
        assert!(owner.starts_with("cox /"), "{owner}");
        if !discard && path.join("DIRTY").exists() {
            return Err(WorktreeError::Dirty {
                path: path.to_path_buf(),
            });
        }
        std::fs::remove_dir_all(path).map_err(|e| WorktreeError::Git {
            args: "rm".into(),
            stderr: e.to_string(),
        })
    }
}

/// A scratch home on the Scripted provider, a project, and the fake trees.
fn scratch() -> (tempfile::TempDir, Arc<Trees>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("user");
    std::fs::create_dir_all(dir.path().join("project")).expect("project");
    let scenario = dir.path().join("scenario.toml");
    std::fs::write(&scenario, REPLY).expect("scenario");
    // SAFETY: first thing in this test's own process (nextest).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("COX_HOME", home.join(".cox"));
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("COX_PROVIDER", "scripted");
        std::env::set_var("COX_SCENARIO", scenario);
    }
    let trees = Arc::new(Trees {
        root: dir.path().join("_worktrees"),
        asked: Mutex::default(),
    });
    (dir, trees)
}

fn app(dir: &Path, trees: &Arc<Trees>) -> Arc<App> {
    let worktrees: Arc<dyn Worktrees> = trees.clone();
    App::with_worktrees(Some(dir.join("user/.cox")), Arc::new(FakeKey), worktrees).expect("app")
}

async fn launch(app: &Arc<App>, dir: &Path, candidates: Vec<Candidate>) -> Launch {
    let request = BestOfRequest {
        project: dir.join("project"),
        prompt: PROMPT.into(),
        candidates,
    };
    app.best_of(request, THEME.into()).await.expect("best of")
}

fn cox() -> Candidate {
    Candidate::Cox { model: None }
}

/// Pulls until the running turn ends.
async fn finish(session: &LiveSession) {
    while let Some(batch) = session.next_patches().await {
        let ended = batch.iter().any(|p| {
            matches!(p, TimelinePatch::Upsert { block, .. }
                if matches!(block.kind, BlockKind::TurnMeta { stop: Some(_), .. }))
        });
        if ended {
            return;
        }
    }
    panic!("the stream closed before the turn ended");
}

#[tokio::test]
async fn best_of_opens_one_worktree_per_candidate() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    let asked = trees.asked.lock().expect("asked").clone();
    let id = &launch.group.id;
    assert_eq!(
        asked
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [format!("best-{id}-1"), format!("best-{id}-2")]
    );
    assert!(asked.iter().all(|(_, owner)| owner.starts_with("cox /")));
    let cwds: Vec<PathBuf> = launch
        .group
        .candidates
        .iter()
        .filter_map(|c| c.worktree.as_ref().map(|t| t.path.clone()))
        .collect();
    assert_eq!(cwds.len(), 2);
    assert_ne!(cwds[0], cwds[1]);
    assert_eq!(launch.sessions.len(), 2);
    for session in &launch.sessions {
        finish(session).await;
    }
}

#[tokio::test]
async fn best_of_sends_the_same_prompt() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
        let prompts: Vec<String> = session
            .snapshot()
            .into_iter()
            .filter_map(|b| match b.kind {
                BlockKind::User { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(prompts, [PROMPT]);
    }
}

#[tokio::test]
async fn best_of_one_failure_leaves_the_others_running() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let missing = Candidate::Agent {
        name: "missing".into(),
    };
    let launch = launch(&app, dir.path(), vec![cox(), missing, cox()]).await;
    let failed: Vec<bool> = launch
        .group
        .candidates
        .iter()
        .map(|c| c.failed.is_some())
        .collect();
    assert_eq!(failed, [false, true, false]);
    assert!(launch.group.candidates[1].session.is_none());
    assert_eq!(launch.sessions.len(), 2);
    for session in &launch.sessions {
        finish(session).await;
    }
    // The sidebar lists the two that ran under the group.
    let listed = app
        .workspace()
        .sessions(
            &dir.path()
                .join("_worktrees")
                .join(format!("best-{}-1", launch.group.id)),
            50,
        )
        .expect("sessions");
    assert_eq!(
        listed.iter().map(|s| s.best_of.clone()).collect::<Vec<_>>(),
        [Some(launch.group.id.0.clone())]
    );
}

#[tokio::test]
async fn best_of_failed_candidate_shows_the_provider_error() {
    let (dir, trees) = scratch();
    // The scenario is read when each session opens, so this reaches both.
    std::fs::write(
        dir.path().join("scenario.toml"),
        "[[turn]]\nerror = \"auth failed\"\n",
    )
    .expect("scenario");
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    let views = app.compare(&launch.group.id).await.expect("compare");
    for view in &views {
        assert_eq!(
            view.state,
            CandidateState::Failed {
                why: "provider error: bad request: auth failed".into()
            }
        );
        // A turn that failed before any usage row: the sum of nothing must not be -0.0.
        assert!(view.cost_usd.is_sign_positive(), "{}", view.cost_usd);
    }
}

#[tokio::test]
async fn best_of_total_is_the_sum_of_ledger_rows() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let mut expected = 0.0;
    for session in &launch.sessions {
        let rows = store.usage_ledger(&session.id()).expect("ledger");
        assert!(!rows.is_empty(), "each candidate has its own usage rows");
        expected += rows.iter().map(|r| r.usage.usage.cost_usd).sum::<f64>();
    }
    let total = app
        .workspace()
        .best_of_cost(&launch.group.id)
        .expect("total");
    assert!((total - expected).abs() < 1e-9, "{total} != {expected}");
}

#[tokio::test]
async fn best_of_compare_lists_diffstat_and_cost() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    let first = launch.group.candidates[0].worktree.clone().expect("tree");
    std::fs::write(first.path.join("AddressFields.tsx"), "a\nb\nc\n").expect("write");
    let views = app.compare(&launch.group.id).await.expect("compare");
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].state, CandidateState::Done);
    assert_eq!(views[0].label, "cox");
    assert_eq!(
        views[0].files,
        [FileStat {
            path: "AddressFields.tsx".into(),
            added: 3,
            removed: 0,
        }]
    );
    assert_eq!((views[0].added, views[0].removed), (3, 0));
    assert!(views[1].files.is_empty());
    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    for (view, session) in views.iter().zip(&launch.sessions) {
        let ledger: f64 = store
            .usage_ledger(&session.id())
            .expect("ledger")
            .iter()
            .map(|r| r.usage.usage.cost_usd)
            .sum();
        assert!((view.cost_usd - ledger).abs() < 1e-9);
        assert_eq!(view.session, Some(session.id()));
    }
}

#[tokio::test]
async fn best_of_pick_prunes_the_others() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    let trees: Vec<PathBuf> = launch
        .group
        .candidates
        .iter()
        .map(|c| c.worktree.clone().expect("tree").path)
        .collect();
    let picked = app.pick(&launch.group.id, 1, false).await.expect("pick");
    assert_eq!(picked.pruned, [trees[0].clone(), trees[2].clone()]);
    assert!(picked.dirty.is_empty() && picked.refused.is_empty());
    assert!(trees[1].exists());
    assert!(!trees[0].exists() && !trees[2].exists());
    let states: Vec<CandidateState> = app
        .compare(&launch.group.id)
        .await
        .expect("compare")
        .into_iter()
        .map(|v| v.state)
        .collect();
    assert_eq!(
        states,
        [
            CandidateState::Pruned,
            CandidateState::Kept,
            CandidateState::Pruned
        ]
    );
}

#[tokio::test]
async fn best_of_pick_refuses_dirty_without_second_confirmation() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox(), cox()]).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    let other = launch.group.candidates[1]
        .worktree
        .clone()
        .expect("tree")
        .path;
    std::fs::write(other.join("DIRTY"), "work in progress\n").expect("write");
    let first = app.pick(&launch.group.id, 0, false).await.expect("pick");
    assert_eq!(first.dirty, std::slice::from_ref(&other));
    assert!(first.pruned.is_empty());
    assert!(
        other.exists(),
        "a tree with changes stays without a second yes"
    );
    let second = app.pick(&launch.group.id, 0, true).await.expect("pick");
    assert_eq!(second.pruned, std::slice::from_ref(&other));
    assert!(!other.exists());
}

#[tokio::test]
async fn best_of_refuses_a_cox_candidate_without_a_usable_provider_before_its_worktree() {
    let (dir, trees) = scratch();
    // SAFETY: this test's own process (nextest).
    unsafe { std::env::remove_var("COX_PROVIDER") };
    let app = app(dir.path(), &trees);
    let launch = launch(&app, dir.path(), vec![cox()]).await;
    let candidate = &launch.group.candidates[0];
    let why = candidate.failed.as_deref().expect("refused");
    assert!(why.contains("anthropic") && why.contains("key"), "{why}");
    assert!(candidate.worktree.is_none() && candidate.session.is_none());
    assert!(trees.asked.lock().expect("asked").is_empty());
    assert!(launch.sessions.is_empty());
}

// T52.24 (A142): merging chosen candidates with a model or an agent.

/// A launch of `candidates` whose sessions have all finished.
async fn finished(app: &Arc<App>, dir: &Path, candidates: Vec<Candidate>) -> Launch {
    let launch = launch(app, dir, candidates).await;
    for session in &launch.sessions {
        finish(session).await;
    }
    launch
}

async fn merge(
    app: &Arc<App>,
    launch: &Launch,
    from: &[u32],
    by: Candidate,
) -> Result<Launch, AppError> {
    let merge = BestOfMerge {
        id: launch.group.id.clone(),
        from: from.to_vec(),
        by,
    };
    app.best_of_merge(merge, THEME.into()).await
}

/// A merge whose session has finished.
async fn merged(app: &Arc<App>, launch: &Launch, from: &[u32], by: Candidate) -> Launch {
    let merged = merge(app, launch, from, by).await.expect("merge");
    for session in &merged.sessions {
        finish(session).await;
    }
    merged
}

fn worktree(launch: &Launch, n: usize) -> PathBuf {
    launch.group.candidates[n]
        .worktree
        .clone()
        .expect("tree")
        .path
}

fn user_texts(session: &LiveSession) -> Vec<String> {
    session
        .snapshot()
        .into_iter()
        .filter_map(|b| match b.kind {
            BlockKind::User { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn ledger(dir: &Path, session: &LiveSession) -> Vec<f64> {
    let store = cox_store::Store::open(&dir.join("user/.cox")).expect("store");
    let rows = store.usage_ledger(&session.id()).expect("ledger");
    rows.iter().map(|r| r.usage.usage.cost_usd).collect()
}

/// Names `tests/fixtures/fake_acp.sh` as the agent `fake` in the scratch
/// user config; false where this host has no argv sandbox backend, which
/// T35.2's own test covers.
fn fake_agent(dir: &Path) -> bool {
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp.sh");
    let home = dir.join("user/.cox");
    std::fs::create_dir_all(&home).expect("home");
    let config = format!(
        "[external_agents.fake]\ncommand = {program:?}\nargs = []\nkey_env = \"COX_FAKE_ACP_KEY\"\n"
    );
    std::fs::write(home.join("config.toml"), config).expect("config");
    let roots = [dir.join("project")];
    cox_session::agent_argv(&program, &[], &Config::default(), &roots).is_ok()
}

fn fake() -> Candidate {
    Candidate::Agent {
        name: "fake".into(),
    }
}

#[tokio::test]
async fn best_of_merge_refuses_fewer_than_two() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    for from in [&[0][..], &[1, 1]] {
        let refused = merge(&app, &launch, from, cox()).await.err();
        assert!(
            matches!(refused, Some(AppError::BestOf(BestOfError::TooFewToMerge))),
            "{refused:?}"
        );
    }
    let asked = trees.asked.lock().expect("asked").len();
    assert_eq!(asked, 2, "no worktree for a refused merge");
}

#[tokio::test]
async fn best_of_merge_refuses_a_candidate_not_done() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let missing = Candidate::Agent {
        name: "missing".into(),
    };
    let launch = finished(&app, dir.path(), vec![cox(), missing, cox()]).await;
    let refused = merge(&app, &launch, &[0, 1], cox()).await.err();
    assert!(
        matches!(
            refused,
            Some(AppError::BestOf(BestOfError::NotDone { index: 1, .. }))
        ),
        "{refused:?}"
    );
    // The missing agent's worktree was made before it failed to start.
    assert_eq!(trees.asked.lock().expect("asked").len(), 3);
}

#[tokio::test]
async fn best_of_merge_refuses_after_a_pick() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox(), cox()]).await;
    std::fs::write(worktree(&launch, 2).join("DIRTY"), "kept\n").expect("write");
    app.pick(&launch.group.id, 0, false).await.expect("pick");
    let refused = merge(&app, &launch, &[0, 2], cox()).await.err();
    assert!(
        matches!(refused, Some(AppError::BestOf(BestOfError::Picked(_)))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn best_of_merge_prompt_holds_each_chosen_answer_and_only_those() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox(), cox()]).await;
    let merged = merged(&app, &launch, &[2, 0], cox()).await;
    let last = trees.asked.lock().expect("asked").last().cloned();
    let name = format!("best-{}-4", launch.group.id);
    assert_eq!(last.map(|(n, _)| n), Some(name));
    let prompts = user_texts(&merged.sessions[0]);
    assert_eq!(prompts.len(), 1);
    let prompt = &prompts[0];
    assert!(prompt.contains(PROMPT), "{prompt}");
    assert_eq!(prompt.matches("\nCandidate ").count(), 2, "{prompt}");
    // Each candidate's final answer, fenced as data.
    assert_eq!(prompt.matches("```text\nDone.\n```").count(), 2, "{prompt}");
}

#[tokio::test]
async fn best_of_merge_runs_on_the_chosen_model() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    let by = Candidate::Cox {
        model: Some("merge-model".into()),
    };
    let merged = merged(&app, &launch, &[0, 1], by.clone()).await;
    let row = merged.group.candidates.last().expect("merge row");
    assert_eq!(row.candidate, by);
    let models: Vec<String> = merged.sessions[0]
        .snapshot()
        .into_iter()
        .filter_map(|b| match b.kind {
            BlockKind::TurnMeta { model, .. } => Some(model.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(models, ["merge-model"]);
}

#[tokio::test]
async fn best_of_merge_runs_on_the_chosen_agent() {
    let (dir, trees) = scratch();
    if !fake_agent(dir.path()) {
        return;
    }
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    let merged = merged(&app, &launch, &[0, 1], fake()).await;
    let row = merged.group.candidates.last().expect("merge row");
    assert!(row.failed.is_none(), "{:?}", row.failed);
    assert_eq!(row.candidate, fake());
    let replies: Vec<String> = merged.sessions[0]
        .snapshot()
        .into_iter()
        .filter_map(|b| match b.kind {
            BlockKind::Assistant { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(replies, ["hello from the fake agent with a key"]);
}

#[tokio::test]
async fn best_of_merge_is_a_candidate_compare_and_pick_see() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    let merged = merged(&app, &launch, &[0, 1], cox()).await;
    assert_eq!(merged.group.candidates.len(), 3);
    assert_eq!(merged.group.candidates[2].merged_from, Some(vec![0, 1]));
    assert_eq!(merged.group.candidates[0].merged_from, None);
    let views = app.compare(&launch.group.id).await.expect("compare");
    assert_eq!(views.len(), 3);
    assert_eq!(views[2].state, CandidateState::Done);
    assert_eq!(views[2].session, Some(merged.sessions[0].id()));
    // Its sidebar row sits in the group.
    let tree = worktree(&merged, 2);
    let listed = app.workspace().sessions(&tree, 50).expect("sessions");
    assert_eq!(
        listed.iter().map(|s| s.best_of.clone()).collect::<Vec<_>>(),
        [Some(launch.group.id.0.clone())]
    );
    let picked = app.pick(&launch.group.id, 2, false).await.expect("pick");
    assert_eq!(picked.pruned, [worktree(&launch, 0), worktree(&launch, 1)]);
    assert!(tree.exists());
}

#[tokio::test]
async fn best_of_merge_usage_is_its_own_ledger_rows() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    let merged = merged(&app, &launch, &[0, 1], cox()).await;
    let own = ledger(dir.path(), &merged.sessions[0]);
    assert!(!own.is_empty(), "the merger has its own usage rows");
    let expected: f64 = launch
        .sessions
        .iter()
        .chain(&merged.sessions)
        .flat_map(|s| ledger(dir.path(), s))
        .sum();
    let total = app
        .workspace()
        .best_of_cost(&launch.group.id)
        .expect("total");
    assert!((total - expected).abs() < 1e-9, "{total} != {expected}");
}

#[tokio::test]
async fn best_of_merge_by_an_agent_writes_no_usage_row() {
    let (dir, trees) = scratch();
    if !fake_agent(dir.path()) {
        return;
    }
    let app = app(dir.path(), &trees);
    let launch = finished(&app, dir.path(), vec![cox(), cox()]).await;
    let workspace = app.workspace();
    let before = workspace.best_of_cost(&launch.group.id).expect("total");
    let merged = merged(&app, &launch, &[0, 1], fake()).await;
    assert!(ledger(dir.path(), &merged.sessions[0]).is_empty());
    let after = workspace.best_of_cost(&launch.group.id).expect("total");
    assert!((after - before).abs() < 1e-12, "{after} != {before}");
}

#[tokio::test]
async fn best_of_mergeable_lists_only_done_candidates() {
    let (dir, trees) = scratch();
    let app = app(dir.path(), &trees);
    let missing = Candidate::Agent {
        name: "missing".into(),
    };
    let launch = finished(&app, dir.path(), vec![cox(), missing, cox()]).await;
    let mergeable = app.mergeable(&launch.group.id).expect("mergeable");
    assert_eq!(mergeable, [0, 2]);
    app.pick(&launch.group.id, 0, false).await.expect("pick");
    assert!(
        app.mergeable(&launch.group.id)
            .expect("mergeable")
            .is_empty()
    );
}
