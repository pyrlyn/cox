// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Refine in the session layer (T66.7, P66): a proposal from the core
//! (T66.6) becomes files through `cox_ext::harness` (T66.5) and one row in
//! `refine_events` (T66.4), and an undo reverses it. It lives here, not in
//! `cox-core`, because the core reads and writes no files (D2); this crate
//! is the one that already knows `COX_HOME`, the memory directory and the
//! store. Prompt notes reach the model as a tail block of the volatile
//! `system[3]` (`Session::set_harness_notes`), so a refine never moves the
//! cache breakpoint; skills and subagents are read once at open, so they
//! take effect in the next session.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cox_core::Session;
use cox_core::refine::{Action as ProposedAction, Kind as ProposedKind, Proposal};
use cox_ext::harness::{Action, Edit, EditRecord, HarnessError, Kind, Loaded, Scope, sha256_hex};
use cox_protocol::StoreError;
use cox_protocol::ids::SessionId;
use cox_store::Store;
use cox_store::refine::{
    NewRefineEvent, RefineEdit, RefineEvent, RefineOutcome, RefineScope, RefineTrigger,
};

/// A prompt note per file is capped at the proposal's 4 KiB, but there is
/// no cap on how many accumulate; this bounds what they add to every
/// request.
const MAX_NOTES_BYTES: usize = 16 * 1024;
const NOTES_OPEN: &str = "<harness_notes>";
const NOTES_CLOSE: &str = "</harness_notes>";
const DIGEST_LINE_CHARS: usize = 120;

#[derive(Debug, thiserror::Error)]
pub enum RefineError {
    #[error(transparent)]
    Harness(#[from] HarnessError),
    #[error("refine history: {0}")]
    Store(#[from] StoreError),
    #[error("no refine event {0:?}")]
    UnknownEvent(String),
    #[error("refine event {0:?} did not apply any edit, so there is nothing to undo")]
    NotApplied(String),
    #[error("refine event {0:?} was already undone")]
    AlreadyUndone(String),
    #[error("{kind} {id:?} changed after the refine; undo the later change first")]
    ChangedSince { kind: String, id: String },
    /// Global entries reach every session, so undoing them is as explicit
    /// as making them was.
    #[error("refine event {0:?} changed the global entries; pass --global to undo it")]
    GlobalNeedsFlag(String),
    #[error("this session has no applied refine to undo")]
    NothingToUndo,
}

/// What one `refine` call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refined {
    /// The edits are on disk and recorded under `event_id`.
    Applied {
        event_id: String,
        evidence: String,
        edits: Vec<EditRecord>,
    },
    /// The model found nothing to change, or every edit rewrites an entry
    /// to the bytes it already has. No backup and no row are written.
    Unchanged,
    /// The proposal was refused; `reason` is written by cox and is also the
    /// recorded `rejected` row's evidence.
    Rejected { event_id: String, reason: String },
}

/// What one `undo` did: the new `rolled_back` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    pub event_id: String,
    pub undid: String,
}

/// A proposal that passed every check and waits for the user's yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub scope: RefineScope,
    pub evidence: String,
    /// What applying would change, as a unified diff the user reviews.
    pub diff: String,
    edits: Vec<Edit>,
}

/// What `propose` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reviewed {
    Ready(Pending),
    /// Same meaning as [`Refined::Unchanged`].
    Unchanged,
    /// Same meaning as [`Refined::Rejected`]; the `rejected` row is written.
    Rejected {
        event_id: String,
        reason: String,
    },
}

/// Applies, records and undoes refines for one session.
pub struct Refiner {
    session: Session,
    store: Arc<Store>,
    home: PathBuf,
    memory_dir: PathBuf,
    /// Fingerprint of the entries the installed notes were rendered from, so
    /// an unchanged store does not re-render (or re-install) the same text.
    notes_for: Mutex<Option<String>>,
}

impl Refiner {
    pub fn new(session: Session, store: Arc<Store>, home: PathBuf, memory_dir: PathBuf) -> Self {
        Self {
            session,
            store,
            home,
            memory_dir,
            notes_for: Mutex::new(None),
        }
    }

    /// One `/refine` without a review step: propose, apply under `scope`,
    /// record.
    pub async fn refine(&self, text: &str, scope: RefineScope) -> Result<Refined, RefineError> {
        Ok(match self.propose(text, scope).await? {
            Reviewed::Ready(pending) => return self.apply(&pending),
            Reviewed::Unchanged => Refined::Unchanged,
            Reviewed::Rejected { event_id, reason } => Refined::Rejected { event_id, reason },
        })
    }

    /// The model's proposal for `text`, checked against what is on disk and
    /// not written. A refusal is recorded here, since nothing will follow.
    pub async fn propose(&self, text: &str, scope: RefineScope) -> Result<Reviewed, RefineError> {
        let session_id = self.session.id();
        let target = scope_for(&self.home, &self.memory_dir, scope, &session_id);
        let current = target.load()?;
        let proposed = match self.session.propose_refine(text, &digest(&current)).await {
            Ok(proposed) => proposed,
            Err(rejected) => {
                let reason = rejected.reason;
                let event_id = self.reject(&target, scope, session_id, &reason)?;
                return Ok(Reviewed::Rejected { event_id, reason });
            }
        };
        let edits = harness_edits(&proposed);
        if edits.is_empty() || is_noop(&current, &edits) {
            return Ok(Reviewed::Unchanged);
        }
        Ok(Reviewed::Ready(Pending {
            scope,
            evidence: proposed.evidence,
            diff: cox_sanitize::sanitize(&unified(&current, &edits)),
            edits,
        }))
    }

    /// Writes a reviewed proposal and records it.
    pub fn apply(&self, pending: &Pending) -> Result<Refined, RefineError> {
        let (scope, edits) = (pending.scope, &pending.edits);
        let session_id = self.session.id();
        let target = scope_for(&self.home, &self.memory_dir, scope, &session_id);
        let event_id = new_event_id();
        let applied = match target.apply(&event_id, edits) {
            Ok(applied) => applied,
            // The entry changed since the review, or the model named one the
            // disk disagrees with: a bad proposal, not a broken store.
            Err(
                e @ (HarnessError::Refused { .. }
                | HarnessError::Duplicate { .. }
                | HarnessError::InvalidId(_)),
            ) => {
                let reason = e.to_string();
                let event_id = self.reject(&target, scope, session_id, &reason)?;
                return Ok(Refined::Rejected { event_id, reason });
            }
            Err(e) => return Err(e.into()),
        };
        let row = NewRefineEvent {
            id: event_id.clone(),
            session_id,
            scope,
            trigger: RefineTrigger::Manual,
            evidence: pending.evidence.clone(),
            edits: applied.edits.iter().map(refine_edit).collect(),
            outcome: RefineOutcome::Applied,
            rollback_of: None,
            fingerprint: applied.after.clone(),
        };
        if let Err(e) = self.store.refine_insert(&row) {
            // Files without a row cannot be undone later, so put them back.
            let _ = target.rollback(&event_id);
            return Err(e.into());
        }
        let _ = self.refresh_notes();
        Ok(Refined::Applied {
            event_id,
            evidence: pending.evidence.clone(),
            edits: applied.edits,
        })
    }

    /// This session's refines, newest first.
    pub fn history(&self, limit: usize) -> Result<Vec<RefineEvent>, RefineError> {
        Ok(self.store.refine_list(&self.session.id(), limit)?)
    }

    /// [`Self::undo`] of the newest applied refine nobody undid yet.
    pub fn undo_latest(&self, allow_global: bool) -> Result<Undone, RefineError> {
        let id = latest_undoable(&self.store, &self.session.id())?;
        self.undo(&id, allow_global)
    }

    /// [`undo_event`], then the prompt notes catch up with the files.
    pub fn undo(&self, event_id: &str, allow_global: bool) -> Result<Undone, RefineError> {
        let undone = undo_event(
            &self.store,
            &self.home,
            &self.memory_dir,
            event_id,
            allow_global,
        )?;
        let _ = self.refresh_notes();
        Ok(undone)
    }

    /// Re-renders the prompt notes into `system[3]` when the entries under
    /// either scope changed since the last call. Returns whether it did.
    pub fn refresh_notes(&self) -> Result<bool, HarnessError> {
        let id = self.session.id();
        let scopes = notes_scopes(&self.home, &self.memory_dir, &id);
        let stamp = scopes_fingerprint(&scopes)?;
        let mut shown = self
            .notes_for
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if shown.as_deref() == Some(stamp.as_str()) {
            return Ok(false);
        }
        self.session.set_harness_notes(render_notes(&scopes)?);
        *shown = Some(stamp);
        Ok(true)
    }

    fn reject(
        &self,
        target: &Scope,
        scope: RefineScope,
        session_id: SessionId,
        reason: &str,
    ) -> Result<String, RefineError> {
        let event_id = new_event_id();
        self.store.refine_insert(&NewRefineEvent {
            id: event_id.clone(),
            session_id,
            scope,
            trigger: RefineTrigger::Manual,
            evidence: reason.to_string(),
            edits: Vec::new(),
            outcome: RefineOutcome::Rejected,
            rollback_of: None,
            fingerprint: target.fingerprint()?,
        })?;
        Ok(event_id)
    }
}

/// Reverses an applied refine and records a `rolled_back` row that names
/// it. Refuses when a later change touched one of its entries, because
/// restoring the backup would erase that work. It needs no `Session`, so
/// `cox refine undo` runs without a provider.
pub fn undo_event(
    store: &Store,
    home: &Path,
    memory_dir: &Path,
    event_id: &str,
    allow_global: bool,
) -> Result<Undone, RefineError> {
    let event = store
        .refine_get(event_id)?
        .ok_or_else(|| RefineError::UnknownEvent(event_id.to_string()))?;
    if event.outcome != RefineOutcome::Applied {
        return Err(RefineError::NotApplied(event_id.to_string()));
    }
    if event.scope == RefineScope::Global && !allow_global {
        return Err(RefineError::GlobalNeedsFlag(event_id.to_string()));
    }
    let history = store.refine_list(&event.session_id, usize::MAX)?;
    if history
        .iter()
        .any(|e| e.rollback_of.as_deref() == Some(event_id))
    {
        return Err(RefineError::AlreadyUndone(event_id.to_string()));
    }
    let target = scope_for(home, memory_dir, event.scope, &event.session_id);
    untouched_since(&target.load()?, &event)?;
    target.rollback(event_id)?;
    let undo_id = new_event_id();
    store.refine_insert(&NewRefineEvent {
        id: undo_id.clone(),
        session_id: event.session_id,
        scope: event.scope,
        trigger: RefineTrigger::Manual,
        evidence: format!("undo of {event_id}"),
        edits: event.edits.iter().map(reverse).collect(),
        outcome: RefineOutcome::RolledBack,
        rollback_of: Some(event_id.to_string()),
        fingerprint: target.fingerprint()?,
    })?;
    Ok(Undone {
        event_id: undo_id,
        undid: event_id.to_string(),
    })
}

/// The newest applied refine of `session` that no later row undid: what a
/// bare `/refine undo` reverses.
pub fn latest_undoable(store: &Store, session: &SessionId) -> Result<String, RefineError> {
    let history = store.refine_list(session, usize::MAX)?;
    history
        .iter()
        .find(|e| {
            e.outcome == RefineOutcome::Applied
                && !history
                    .iter()
                    .any(|u| u.rollback_of.as_deref() == Some(e.id.as_str()))
        })
        .map(|e| e.id.clone())
        .ok_or(RefineError::NothingToUndo)
}

/// One line per refine for `/refine list` and `cox refine list`. The
/// evidence is the model's text, so it is sanitized here, once.
pub fn describe(event: &RefineEvent) -> String {
    let scope = match event.scope {
        RefineScope::Local => "local",
        RefineScope::Global => "global",
    };
    let outcome = match event.outcome {
        RefineOutcome::Applied => "applied".to_string(),
        RefineOutcome::Rejected => "rejected".to_string(),
        RefineOutcome::RolledBack => format!(
            "rolled back {}",
            event.rollback_of.as_deref().unwrap_or("?")
        ),
    };
    let edits: Vec<String> = event
        .edits
        .iter()
        .map(|e| format!("{} {}/{}", e.action, e.kind, e.id))
        .collect();
    let first = event.evidence.lines().next().unwrap_or("");
    let why: String = first.chars().take(DIGEST_LINE_CHARS).collect();
    let mut line = format!("{} {scope} {outcome}", event.id);
    if !edits.is_empty() {
        line.push_str(&format!(" [{}]", edits.join(", ")));
    }
    line.push_str(&format!(": {}", cox_sanitize::sanitize(why.trim())));
    line
}

/// The prompt-note block for a session's `system[3]`: global notes, then the
/// session's own, empty when there are none. `open` installs it so a new
/// session sees earlier global refines without a `Refiner`.
pub fn prompt_notes(
    home: &Path,
    memory_dir: &Path,
    session: &SessionId,
) -> Result<String, HarnessError> {
    render_notes(&notes_scopes(home, memory_dir, session))
}

/// A session's local entries live in a directory of its own beside its
/// rollout, so another session's load cannot see them.
pub fn scope_for(home: &Path, memory_dir: &Path, scope: RefineScope, session: &SessionId) -> Scope {
    match scope {
        RefineScope::Local => Scope::Local {
            root: home.join("sessions").join(session.to_string()),
        },
        RefineScope::Global => Scope::Global {
            home: home.to_path_buf(),
            memory_dir: memory_dir.to_path_buf(),
        },
    }
}

fn notes_scopes(home: &Path, memory_dir: &Path, session: &SessionId) -> [Scope; 2] {
    [RefineScope::Global, RefineScope::Local].map(|s| scope_for(home, memory_dir, s, session))
}

fn scopes_fingerprint(scopes: &[Scope]) -> Result<String, HarnessError> {
    let prints: Result<Vec<_>, _> = scopes.iter().map(Scope::fingerprint).collect();
    Ok(prints?.join(":"))
}

fn render_notes(scopes: &[Scope]) -> Result<String, HarnessError> {
    let mut body = String::new();
    for scope in scopes {
        for entry in scope
            .load()?
            .into_iter()
            .filter(|e| e.kind == Kind::PromptNote)
        {
            // The file is untrusted text: it must not close the block early.
            let content = String::from_utf8_lossy(&entry.content).replace(NOTES_CLOSE, "");
            let part = format!("## {}\n{}\n", entry.id, content.trim());
            if body.len() + part.len() > MAX_NOTES_BYTES {
                break;
            }
            body.push_str(&part);
        }
    }
    Ok(if body.is_empty() {
        String::new()
    } else {
        format!("{NOTES_OPEN}\nStanding notes from earlier /refine runs:\n{body}{NOTES_CLOSE}\n")
    })
}

/// One line per entry for the proposal call: enough to tell the model what
/// exists, without sending every file's body.
fn digest(entries: &[Loaded]) -> String {
    entries
        .iter()
        .map(|e| {
            let text = String::from_utf8_lossy(&e.content);
            let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let first: String = first.chars().take(DIGEST_LINE_CHARS).collect();
            format!(
                "- {} {}: {}",
                e.kind.as_str(),
                e.id,
                cox_sanitize::sanitize(first.trim())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn harness_edits(proposed: &Proposal) -> Vec<Edit> {
    proposed
        .edits
        .iter()
        .map(|e| Edit {
            action: match e.action {
                ProposedAction::Create => Action::Create,
                ProposedAction::Update => Action::Update,
                ProposedAction::Delete => Action::Delete,
            },
            kind: match e.kind {
                ProposedKind::PromptNote => Kind::PromptNote,
                ProposedKind::Memory => Kind::Memory,
                ProposedKind::Skill => Kind::Skill,
                ProposedKind::Subagent => Kind::Subagent,
            },
            id: e.id.clone(),
            content: e.content.clone(),
        })
        .collect()
}

/// Every edit rewrites an existing entry to the bytes it already has.
fn is_noop(current: &[Loaded], edits: &[Edit]) -> bool {
    edits.iter().all(|edit| {
        edit.action == Action::Update
            && current.iter().any(|c| {
                c.kind == edit.kind
                    && c.id == edit.id
                    && Some(c.content.as_slice()) == edit.content.as_deref().map(str::as_bytes)
            })
    })
}

/// The edits as one git-style patch against what is on disk, for the review.
/// A created entry diffs from `/dev/null` and a deleted one to it.
fn unified(current: &[Loaded], edits: &[Edit]) -> String {
    let eol = |s: String| {
        if s.is_empty() || s.ends_with('\n') {
            s
        } else {
            s + "\n"
        }
    };
    let mut out = String::new();
    for edit in edits {
        let name = format!("{}/{}", edit.kind.as_str(), edit.id);
        let old = current
            .iter()
            .find(|c| c.kind == edit.kind && c.id == edit.id)
            .map(|c| eol(String::from_utf8_lossy(&c.content).into_owned()));
        let new = edit.content.clone().map(eol);
        let (old_label, new_label) = (
            old.as_ref()
                .map_or("/dev/null".to_string(), |_| name.clone()),
            new.as_ref()
                .map_or("/dev/null".to_string(), |_| name.clone()),
        );
        let (old, new) = (old.unwrap_or_default(), new.unwrap_or_default());
        let diff = similar::TextDiff::from_lines(&old, &new);
        let mut unified = diff.unified_diff();
        unified.context_radius(3).header(&old_label, &new_label);
        // The git header lets the TUI split a multi-entry patch per entry.
        out.push_str(&format!("diff --git a/{name} b/{name}\n{unified}"));
    }
    out
}

fn refine_edit(record: &EditRecord) -> RefineEdit {
    RefineEdit {
        action: record.action.as_str().to_string(),
        kind: record.kind.as_str().to_string(),
        id: record.id.clone(),
        before_sha: record.before_sha.clone(),
        after_sha: record.after_sha.clone(),
    }
}

/// The edit as an undo sees it: the hashes swap, and a create becomes a
/// delete and back.
fn reverse(edit: &RefineEdit) -> RefineEdit {
    let action = match edit.action.as_str() {
        "create" => "delete",
        "delete" => "create",
        other => other,
    };
    RefineEdit {
        action: action.to_string(),
        kind: edit.kind.clone(),
        id: edit.id.clone(),
        before_sha: edit.after_sha.clone(),
        after_sha: edit.before_sha.clone(),
    }
}

/// Each entry the event wrote must still hold the bytes it wrote.
fn untouched_since(current: &[Loaded], event: &RefineEvent) -> Result<(), RefineError> {
    for edit in &event.edits {
        let now = current
            .iter()
            .find(|c| c.kind.as_str() == edit.kind && c.id == edit.id)
            .map(|c| sha256_hex(&c.content));
        if now != edit.after_sha {
            return Err(RefineError::ChangedSince {
                kind: edit.kind.clone(),
                id: edit.id.clone(),
            });
        }
    }
    Ok(())
}

/// `harness::apply` wants an id of `[a-z0-9-]`, so the ULID is lowercased;
/// it still sorts by creation time, which `refine_list` relies on.
fn new_event_id() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cox_protocol::Config;
    use cox_protocol::traits::Store as _;
    use cox_protocol::types::{Job, Request};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::testing::{Recorder, user_turn};

    struct Fixture {
        dir: TempDir,
        refiner: Refiner,
        store: Arc<Store>,
        sent: Arc<Recorder>,
    }

    impl Fixture {
        fn home(&self) -> &Path {
            self.dir.path()
        }

        fn memory_dir(&self) -> PathBuf {
            self.dir.path().join("memory")
        }

        fn id(&self) -> SessionId {
            self.refiner.session.id()
        }

        fn main_requests(&self) -> Vec<Request> {
            let sent = self.sent.sent.lock().expect("sent");
            sent.iter()
                .filter(|r| r.job == Job::Main)
                .cloned()
                .collect()
        }

        fn rows(&self) -> Vec<RefineEvent> {
            self.store.refine_list(&self.id(), 50).expect("rows")
        }

        fn note_path(&self, id: &str) -> PathBuf {
            self.home()
                .join("sessions")
                .join(self.id().to_string())
                .join("harness/prompt-note")
                .join(format!("{id}.md"))
        }
    }

    fn answer(action: &str, kind: &str, id: &str, content: Option<&str>) -> String {
        json!({"evidence": "seen twice", "edits": [{
            "action": action, "kind": kind, "id": id, "content": content, "reason": "r",
        }]})
        .to_string()
    }

    /// A main turn, one memory-job reply per refine, then a last main turn
    /// whose request shows what `system[3]` carries by then.
    fn fixture(answers: &[String]) -> Fixture {
        fixture_in(TempDir::new().expect("tmp"), answers)
    }

    fn fixture_in(dir: TempDir, answers: &[String]) -> Fixture {
        let mut script = String::from("[[turn]]\ntext = \"ok\"\n");
        for a in answers {
            script.push_str(&format!("[[turn]]\njob = \"memory\"\ntext = '''{a}'''\n"));
        }
        script.push_str("[[turn]]\ntext = \"ok\"\n");
        let sent = Arc::new(Recorder {
            inner: cox_provider::scripted::Scripted::from_toml(&script, "").expect("scenario"),
            sent: Mutex::new(Vec::new()),
        });
        let store = Arc::new(Store::open(dir.path()).expect("store"));
        let session = Session::new(
            Config::default(),
            sent.clone(),
            vec![],
            store.clone(),
            store.clone(),
            dir.path().to_path_buf(),
        )
        .expect("session");
        let refiner = Refiner::new(
            session,
            store.clone(),
            dir.path().to_path_buf(),
            dir.path().join("memory"),
        );
        Fixture {
            dir,
            refiner,
            store,
            sent,
        }
    }

    #[tokio::test]
    async fn refine_writes_a_local_note_records_it_and_keeps_the_cached_prefix() {
        let f = fixture(&[answer(
            "create",
            "prompt-note",
            "tests-first",
            Some("Test first."),
        )]);
        user_turn(&f.refiner.session, "go").await;

        let outcome = f.refiner.refine("tests first", RefineScope::Local).await;
        let Ok(Refined::Applied { event_id, .. }) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("note"),
            "Test first."
        );
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, event_id);
        assert_eq!(rows[0].outcome, RefineOutcome::Applied);
        assert_eq!(rows[0].edits[0].action, "create");
        assert!(rows[0].edits[0].before_sha.is_none());

        user_turn(&f.refiner.session, "again").await;
        let requests = f.main_requests();
        assert_eq!(requests.len(), 2);
        for slot in 0..=2 {
            assert_eq!(requests[0].system[slot], requests[1].system[slot], "{slot}");
        }
        assert!(!requests[0].system[3].text.contains("Test first."));
        assert!(requests[1].system[3].text.contains("Test first."));
        assert!(!requests[1].system[3].cache, "after the last breakpoint");
    }

    #[tokio::test]
    async fn refining_twice_against_the_same_store_changes_no_bytes() {
        let same = answer("update", "prompt-note", "tests-first", Some("Test first."));
        let f = fixture(&[
            answer("create", "prompt-note", "tests-first", Some("Test first.")),
            same,
        ]);
        user_turn(&f.refiner.session, "go").await;
        f.refiner
            .refine("one", RefineScope::Local)
            .await
            .expect("first");
        let notes = prompt_notes(f.home(), &f.memory_dir(), &f.id()).expect("notes");
        let backups = || {
            fs::read_dir(
                f.home()
                    .join("sessions")
                    .join(f.id().to_string())
                    .join("harness-backup"),
            )
            .map_or(0, Iterator::count)
        };
        assert_eq!(backups(), 1);

        let second = f.refiner.refine("two", RefineScope::Local).await;

        assert!(matches!(second, Ok(Refined::Unchanged)), "{second:?}");
        assert_eq!(backups(), 1, "no new backup");
        assert_eq!(f.rows().len(), 1, "no new row");
        assert_eq!(
            prompt_notes(f.home(), &f.memory_dir(), &f.id()).expect("notes"),
            notes
        );
        assert!(!f.refiner.refresh_notes().expect("refresh"), "same print");
    }

    #[tokio::test]
    async fn undo_restores_the_files_and_writes_a_rolled_back_row() {
        let f = fixture(&[
            answer("create", "prompt-note", "tests-first", Some("v1")),
            answer("update", "prompt-note", "tests-first", Some("v2")),
        ]);
        user_turn(&f.refiner.session, "go").await;
        let Ok(Refined::Applied {
            event_id: first, ..
        }) = f.refiner.refine("a", RefineScope::Local).await
        else {
            panic!("first refine");
        };
        let Ok(Refined::Applied {
            event_id: second, ..
        }) = f.refiner.refine("b", RefineScope::Local).await
        else {
            panic!("second refine");
        };

        let stale = f.refiner.undo(&first, false);
        assert!(
            matches!(stale, Err(RefineError::ChangedSince { .. })),
            "{stale:?}"
        );
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("v2"),
            "v2"
        );

        let undone = f.refiner.undo(&second, false).expect("undo");
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("v1"),
            "v1"
        );
        let newest = &f.rows()[0];
        assert_eq!(newest.id, undone.event_id);
        assert_eq!(newest.outcome, RefineOutcome::RolledBack);
        assert_eq!(newest.rollback_of.as_deref(), Some(second.as_str()));
        assert!(matches!(
            f.refiner.undo(&second, false),
            Err(RefineError::AlreadyUndone(_))
        ));

        f.refiner
            .undo(&first, false)
            .expect("now the first is latest again");
        assert!(!f.note_path("tests-first").exists());
        assert_eq!(
            prompt_notes(f.home(), &f.memory_dir(), &f.id()).expect("notes"),
            ""
        );
    }

    #[tokio::test]
    async fn a_local_note_reaches_its_own_session_and_not_another() {
        let f = fixture(&[answer(
            "create",
            "prompt-note",
            "tests-first",
            Some("Test first."),
        )]);
        user_turn(&f.refiner.session, "go").await;
        f.refiner
            .refine("a", RefineScope::Local)
            .await
            .expect("refine");

        let other = SessionId::new();
        let mine = prompt_notes(f.home(), &f.memory_dir(), &f.id()).expect("mine");
        let theirs = prompt_notes(f.home(), &f.memory_dir(), &other).expect("theirs");

        assert!(mine.contains("Test first."), "{mine}");
        assert_eq!(theirs, "");
    }

    #[tokio::test]
    async fn a_proposal_aimed_at_the_base_prompt_is_recorded_as_rejected_and_writes_nothing() {
        let f = fixture(&[answer("update", "prompt-note", "system-prompt", Some("x"))]);
        user_turn(&f.refiner.session, "go").await;

        let outcome = f.refiner.refine("rewrite", RefineScope::Local).await;

        let Ok(Refined::Rejected { reason, .. }) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(reason, "edit 1: the base system prompt is not editable");
        let rows = f.rows();
        assert_eq!(rows[0].outcome, RefineOutcome::Rejected);
        assert_eq!(rows[0].evidence, reason);
        assert!(rows[0].edits.is_empty());
        assert!(
            !f.home()
                .join("sessions")
                .join(f.id().to_string())
                .join("harness")
                .exists()
        );
    }

    #[tokio::test]
    async fn a_disk_conflict_is_a_rejection_not_an_error() {
        let f = fixture(&[answer("update", "prompt-note", "missing", Some("x"))]);
        user_turn(&f.refiner.session, "go").await;

        let outcome = f.refiner.refine("x", RefineScope::Local).await;

        assert!(
            matches!(outcome, Ok(Refined::Rejected { .. })),
            "{outcome:?}"
        );
        assert_eq!(f.rows()[0].outcome, RefineOutcome::Rejected);
    }

    #[tokio::test]
    async fn a_global_note_reaches_every_session() {
        let f = fixture(&[]);
        fs::create_dir_all(f.home().join("notes")).expect("notes dir");
        fs::write(f.home().join("notes/concise.md"), "Be concise.").expect("note");

        let other = SessionId::new();
        let seen = prompt_notes(f.home(), &f.memory_dir(), &other).expect("notes");

        assert!(seen.contains("## concise\nBe concise."), "{seen}");
        assert!(seen.starts_with(NOTES_OPEN) && seen.trim_end().ends_with(NOTES_CLOSE));
    }

    #[tokio::test]
    async fn propose_shows_a_diff_and_writes_nothing_until_applied() {
        let f = fixture(&[answer(
            "create",
            "prompt-note",
            "tests-first",
            Some("Test first."),
        )]);
        user_turn(&f.refiner.session, "go").await;

        let Ok(Reviewed::Ready(pending)) = f.refiner.propose("tests", RefineScope::Local).await
        else {
            panic!("a proposal was expected");
        };

        assert_eq!(
            pending.diff,
            "diff --git a/prompt-note/tests-first b/prompt-note/tests-first\n\
             --- /dev/null\n+++ prompt-note/tests-first\n@@ -0,0 +1 @@\n+Test first.\n"
        );
        assert!(
            !f.note_path("tests-first").exists(),
            "review writes nothing"
        );
        assert!(f.rows().is_empty(), "and records nothing");

        let applied = f.refiner.apply(&pending);
        assert!(
            matches!(applied, Ok(Refined::Applied { .. })),
            "{applied:?}"
        );
        assert!(f.note_path("tests-first").exists());
        assert_eq!(f.rows().len(), 1);
    }

    #[tokio::test]
    async fn an_entry_changed_after_the_review_is_rejected_at_apply() {
        let f = fixture(&[
            answer("create", "prompt-note", "tests-first", Some("v1")),
            answer("create", "prompt-note", "tests-first", Some("v2")),
        ]);
        user_turn(&f.refiner.session, "go").await;
        let Ok(Reviewed::Ready(stale)) = f.refiner.propose("a", RefineScope::Local).await else {
            panic!("first proposal");
        };
        f.refiner
            .refine("b", RefineScope::Local)
            .await
            .expect("another refine lands first");

        let outcome = f.refiner.apply(&stale);

        assert!(
            matches!(outcome, Ok(Refined::Rejected { .. })),
            "{outcome:?}"
        );
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("note"),
            "v2"
        );
    }

    #[tokio::test]
    async fn undoing_a_global_refine_needs_the_flag_and_a_bare_undo_takes_the_latest_local() {
        let f = fixture(&[
            answer("create", "prompt-note", "everywhere", Some("Always.")),
            answer("create", "prompt-note", "here", Some("Only here.")),
        ]);
        user_turn(&f.refiner.session, "go").await;
        let Ok(Refined::Applied {
            event_id: global, ..
        }) = f.refiner.refine("g", RefineScope::Global).await
        else {
            panic!("global refine");
        };

        let refused = f.refiner.undo(&global, false);
        assert!(
            matches!(refused, Err(RefineError::GlobalNeedsFlag(_))),
            "{refused:?}"
        );
        assert!(f.home().join("notes/everywhere.md").exists(), "left alone");

        let Ok(Refined::Applied {
            event_id: local, ..
        }) = f.refiner.refine("l", RefineScope::Local).await
        else {
            panic!("local refine");
        };
        assert_eq!(latest_undoable(&f.store, &f.id()).expect("latest"), local);
        f.refiner.undo(&local, false).expect("undo local");
        assert_eq!(latest_undoable(&f.store, &f.id()).expect("latest"), global);

        f.refiner.undo(&global, true).expect("with the flag");
        assert!(!f.home().join("notes/everywhere.md").exists());
        assert!(matches!(
            latest_undoable(&f.store, &f.id()),
            Err(RefineError::NothingToUndo)
        ));
    }

    #[tokio::test]
    async fn describe_names_scope_outcome_edits_and_a_clean_first_line_of_evidence() {
        let f = fixture(&[
            json!({"evidence": "seen\u{1b}[31m twice\nsecond line", "edits": [{
                "action": "create", "kind": "prompt-note", "id": "tests-first",
                "content": "Test first.", "reason": "r",
            }]})
            .to_string(),
        ]);
        user_turn(&f.refiner.session, "go").await;
        let Ok(Refined::Applied { event_id, .. }) = f.refiner.refine("a", RefineScope::Local).await
        else {
            panic!("refine");
        };
        let row = f.store.refine_get(&event_id).expect("get").expect("row");

        assert_eq!(
            describe(&row),
            format!("{event_id} local applied [create prompt-note/tests-first]: seen twice")
        );
    }
}
