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

    /// One `/refine`: propose, apply under `scope`, record.
    pub async fn refine(&self, text: &str, scope: RefineScope) -> Result<Refined, RefineError> {
        let session_id = self.session.id();
        let target = scope_for(&self.home, &self.memory_dir, scope, &session_id);
        let current = target.load()?;
        let proposed = match self.session.propose_refine(text, &digest(&current)).await {
            Ok(proposed) => proposed,
            Err(rejected) => {
                return self.reject(&target, scope, session_id, rejected.reason);
            }
        };
        let edits = harness_edits(&proposed);
        if edits.is_empty() || is_noop(&current, &edits) {
            return Ok(Refined::Unchanged);
        }
        let event_id = new_event_id();
        let applied = match target.apply(&event_id, &edits) {
            Ok(applied) => applied,
            // The model named an entry the disk disagrees with; that is a bad
            // proposal, not a broken store.
            Err(
                e @ (HarnessError::Refused { .. }
                | HarnessError::Duplicate { .. }
                | HarnessError::InvalidId(_)),
            ) => return self.reject(&target, scope, session_id, e.to_string()),
            Err(e) => return Err(e.into()),
        };
        let row = NewRefineEvent {
            id: event_id.clone(),
            session_id,
            scope,
            trigger: RefineTrigger::Manual,
            evidence: proposed.evidence.clone(),
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
            evidence: proposed.evidence,
            edits: applied.edits,
        })
    }

    /// Reverses an applied refine and records a `rolled_back` row that
    /// names it. Refuses when a later change touched one of its entries,
    /// because restoring the backup would erase that work.
    pub fn undo(&self, event_id: &str) -> Result<Undone, RefineError> {
        let event = self
            .store
            .refine_get(event_id)?
            .ok_or_else(|| RefineError::UnknownEvent(event_id.to_string()))?;
        if event.outcome != RefineOutcome::Applied {
            return Err(RefineError::NotApplied(event_id.to_string()));
        }
        let history = self.store.refine_list(&event.session_id, usize::MAX)?;
        if history
            .iter()
            .any(|e| e.rollback_of.as_deref() == Some(event_id))
        {
            return Err(RefineError::AlreadyUndone(event_id.to_string()));
        }
        let target = scope_for(&self.home, &self.memory_dir, event.scope, &event.session_id);
        untouched_since(&target.load()?, &event)?;
        target.rollback(event_id)?;
        let undo_id = new_event_id();
        self.store.refine_insert(&NewRefineEvent {
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
        let _ = self.refresh_notes();
        Ok(Undone {
            event_id: undo_id,
            undid: event_id.to_string(),
        })
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
        reason: String,
    ) -> Result<Refined, RefineError> {
        let event_id = new_event_id();
        self.store.refine_insert(&NewRefineEvent {
            id: event_id.clone(),
            session_id,
            scope,
            trigger: RefineTrigger::Manual,
            evidence: reason.clone(),
            edits: Vec::new(),
            outcome: RefineOutcome::Rejected,
            rollback_of: None,
            fingerprint: target.fingerprint()?,
        })?;
        Ok(Refined::Rejected { event_id, reason })
    }
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

        let stale = f.refiner.undo(&first);
        assert!(
            matches!(stale, Err(RefineError::ChangedSince { .. })),
            "{stale:?}"
        );
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("v2"),
            "v2"
        );

        let undone = f.refiner.undo(&second).expect("undo");
        assert_eq!(
            fs::read_to_string(f.note_path("tests-first")).expect("v1"),
            "v1"
        );
        let newest = &f.rows()[0];
        assert_eq!(newest.id, undone.event_id);
        assert_eq!(newest.outcome, RefineOutcome::RolledBack);
        assert_eq!(newest.rollback_of.as_deref(), Some(second.as_str()));
        assert!(matches!(
            f.refiner.undo(&second),
            Err(RefineError::AlreadyUndone(_))
        ));

        f.refiner
            .undo(&first)
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
}
