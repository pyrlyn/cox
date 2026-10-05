// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A session's children and its reopening (T26.3, T2.4): `/fork` and
//! `/handoff` seed a new session whose own rollout rebuilds its history,
//! and `resume` rebuilds any session's history from its rollout;
//! [`Follow`] tails the rollout of a session another process drives
//! (T37.34). Separate from `open`, which takes the history these return.

use std::path::{Path, PathBuf};

use cox_core::History;
use cox_protocol::ids::{ItemId, SessionId};
use cox_protocol::traits::{SessionRow, Store as _};
use cox_protocol::types::{Event, ItemKind, Job};
use cox_store::Store;

use crate::SessionError;

/// `/fork` and `/handoff` (T26.3): a new session with `parent_id` whose own
/// rollout opens with `events`, so a later `--resume` of the child rebuilds
/// the same history from its file. The parent's rollout is only read.
fn seed_child(
    store: &Store,
    cwd: &Path,
    parent: SessionId,
    events: &[Event],
) -> Result<(SessionId, History), SessionError> {
    let id = SessionId::new();
    store.session_create(&SessionRow {
        id,
        created_at: String::new(),
        cwd: cwd.to_path_buf(),
        project_slug: String::new(),
        title: None,
        parent_id: Some(parent),
        rollout_path: PathBuf::new(),
    })?;
    let started = Event::SessionStarted {
        session: id,
        config_digest: String::new(),
        cwd: cwd.to_path_buf(),
    };
    for ev in std::iter::once(&started).chain(events) {
        store.rollout_append(&id, ev)?;
    }
    Ok((id, History::from_events(events)))
}

/// `/fork [turn]`: the parent's events up to the end of main turn `turn`
/// (all of them for `None`), minus its `SessionStarted`, which names the
/// parent. A `Rewound` or `Compacted` inside the kept span replays as-is.
pub fn fork(
    home: &Path,
    cwd: &Path,
    parent: SessionId,
    turn: Option<u32>,
) -> Result<(SessionId, History), SessionError> {
    let store = Store::open(home)?;
    let (events, _) = store.rollout_read_with_truncation(&parent)?;
    let kept: Vec<Event> = events
        .into_iter()
        .take_while(|ev| match (ev, turn) {
            (
                Event::TurnStarted {
                    seq,
                    job: Job::Main,
                    ..
                },
                Some(turn),
            ) => *seq <= turn,
            _ => true,
        })
        .filter(|ev| !matches!(ev, Event::SessionStarted { .. }))
        .collect();
    seed_child(&store, cwd, parent, &kept)
}

/// `/handoff <objective>`: the child's first history item is one `Summary`
/// (compaction's item kind) carrying the parent's summary and the
/// objective. A summariser that returned nothing still hands the objective
/// over, and says so in the item.
pub fn handoff(
    home: &Path,
    cwd: &Path,
    parent: SessionId,
    objective: &str,
    summary: Option<&str>,
) -> Result<(SessionId, History), SessionError> {
    let summary = summary.unwrap_or("(no summary: the summariser returned nothing)");
    let item = ItemId::new();
    let events = [
        Event::ItemStarted {
            item,
            kind: ItemKind::Summary {
                text: format!(
                    "[Handoff from session {parent}]\n\n{summary}\n\nObjective: {objective}"
                ),
            },
        },
        Event::ItemDone { item },
    ];
    seed_child(&Store::open(home)?, cwd, parent, &events)
}

/// `--resume`/`--continue` (T2.4): session `id`'s history rebuilt from its
/// rollout under `home`, truncation noted.
pub fn resume(home: &Path, id: SessionId) -> Result<History, SessionError> {
    let store = Store::open(home)?;
    let (events, truncated) = store.rollout_read_with_truncation(&id)?;
    Ok(History::from_rollout(&events, truncated))
}

/// T37.34: a read-only view of a session another process drives (its
/// `SessionBusy`): re-reads the holder's rollout, the D2 replay path, and
/// hands back what it appended since the last look, for the caller to fold
/// (`History::from_events`, a surface's own fold). It never writes: no
/// claim, no rollout writer.
pub struct Follow {
    store: Store,
    id: SessionId,
    seen: usize,
}

impl Follow {
    pub fn new(home: &Path, id: SessionId) -> Result<Self, SessionError> {
        Ok(Self {
            store: Store::open(home)?,
            id,
            seen: 0,
        })
    }

    /// The events appended since the last call; all of them the first
    /// time. A half-written last line is left for the next call.
    pub fn poll(&mut self) -> Result<Vec<Event>, SessionError> {
        let (events, _) = self.store.rollout_read_with_truncation(&self.id)?;
        let new: Vec<Event> = events.into_iter().skip(self.seen).collect();
        self.seen += new.len();
        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{scripted_session, user_turn};

    fn texts(history: &History) -> Vec<String> {
        history
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|c| match c {
                cox_protocol::types::Content::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn depth_of(store: &Store, id: SessionId) -> Option<usize> {
        let tree = store.sessions_tree(50).expect("tree");
        tree.iter()
            .find(|r| r.info.id == id.to_string())
            .map(|r| r.depth)
    }

    /// T26.3: `/fork T1` after two turns starts a child of the session with
    /// only the first turn; the child's own rollout rebuilds the same
    /// history (a later `--resume`), and a bare `/fork` keeps every turn.
    #[tokio::test]
    async fn fork_creates_child_with_truncated_history() {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let (session, store) = scripted_session(
            home.path(),
            work.path(),
            "[[turn]]\ntext = \"a1\"\n[[turn]]\ntext = \"a2\"\n",
        );
        user_turn(&session, "one").await;
        user_turn(&session, "two").await;
        let parent = session.id();

        let (child, history) = fork(home.path(), work.path(), parent, Some(1)).expect("fork");
        assert_eq!(texts(&history), ["one", "a1"]);
        assert_eq!(history.turns, 1, "the child keeps counting from T1");
        let resumed = resume(home.path(), child).expect("resume");
        assert_eq!(resumed.messages, history.messages);
        assert_eq!(depth_of(&store, parent), Some(0));
        assert_eq!(depth_of(&store, child), Some(1));

        let (_, all) = fork(home.path(), work.path(), parent, None).expect("fork all");
        assert_eq!(texts(&all), ["one", "a1", "two", "a2"]);
    }

    /// T37.34: a follower sees the turns the driving session appends, only
    /// the new ones on each look, and folds them into the same history.
    #[tokio::test]
    async fn follow_sees_events_the_holder_appends() {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let (session, _store) = scripted_session(
            home.path(),
            work.path(),
            "[[turn]]\ntext = \"a1\"\n[[turn]]\ntext = \"a2\"\n",
        );
        user_turn(&session, "one").await;
        let mut follow = Follow::new(home.path(), session.id()).expect("follow");
        let mut seen = follow.poll().expect("first look");
        assert_eq!(texts(&History::from_events(&seen)), ["one", "a1"]);
        assert!(follow.poll().expect("idle look").is_empty());

        user_turn(&session, "two").await;
        let new = follow.poll().expect("second look");
        assert!(
            !new.is_empty()
                && !new
                    .iter()
                    .any(|e| matches!(e, Event::SessionStarted { .. }))
        );
        seen.extend(new);
        assert_eq!(
            texts(&History::from_events(&seen)),
            ["one", "a1", "two", "a2"]
        );
    }

    /// T26.3: `/handoff` asks the parent's `compact` job (cheap tier, in the
    /// ledger) for a summary; the child's first and only history item is
    /// that summary plus the objective, as a `Summary` item, not a turn.
    #[tokio::test]
    async fn handoff_seeds_summary() {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let (session, store) = scripted_session(
            home.path(),
            work.path(),
            "[[turn]]\ntext = \"a1\"\n[[turn]]\ntext = \"we said hello\"\n",
        );
        user_turn(&session, "hello").await;
        let parent = session.id();

        let summary = session.handoff_summary("ship it").await;
        assert_eq!(summary.as_deref(), Some("we said hello"));
        let usage = store.usage_for_session(&parent).expect("usage");
        assert!(
            usage
                .iter()
                .any(|u| u.job == Job::Compact && u.tier == cox_protocol::types::Tier::Cheap),
            "the summary is a cheap-tier compact call: {usage:?}"
        );
        assert_eq!(session.history().await.len(), 2, "the parent is untouched");

        let (child, history) = handoff(
            home.path(),
            work.path(),
            parent,
            "ship it",
            summary.as_deref(),
        )
        .expect("handoff");
        let [text] = texts(&history).try_into().expect("one seed item");
        assert!(text.contains("we said hello") && text.ends_with("Objective: ship it"));
        assert!(history.turn_marks.is_empty(), "the seed is not a user turn");
        let (events, _) = store.rollout_read_with_truncation(&child).expect("rollout");
        assert!(matches!(
            events.get(1),
            Some(Event::ItemStarted {
                kind: ItemKind::Summary { .. },
                ..
            })
        ));
        assert_eq!(depth_of(&store, child), Some(1));
    }
}
