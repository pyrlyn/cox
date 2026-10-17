//! Refine history (T66.4, P66): the `refine_events` table over Diesel's typed
//! DSL. Every refine of the harness entries — applied, rejected or rolled
//! back — is one row, so a later undo and `cox refine list` read the same
//! record. The enums are stored as the CHECKed text of the migration; this
//! module is the only place that maps them, and a stored word it does not
//! know is a store error rather than a silent default.

use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use cox_protocol::{SessionId, StoreError};

use crate::models::RefineEventDbRow;
use crate::schema::refine_events::dsl as r;
use crate::{Store, now_rfc3339};

/// Where a refine writes: the project's entries or the user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineScope {
    Local,
    Global,
}

/// Who started the refine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineTrigger {
    Manual,
    Auto,
}

/// What became of the refine. A rollback is its own event (`rolled_back`,
/// with `rollback_of` naming the one it reverses) because history is
/// append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineOutcome {
    Applied,
    Rejected,
    RolledBack,
}

impl RefineScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Global => "global",
        }
    }

    fn parse(word: &str) -> Result<Self, StoreError> {
        match word {
            "local" => Ok(Self::Local),
            "global" => Ok(Self::Global),
            _ => Err(StoreError::Io),
        }
    }
}

impl RefineTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }

    fn parse(word: &str) -> Result<Self, StoreError> {
        match word {
            "manual" => Ok(Self::Manual),
            "auto" => Ok(Self::Auto),
            _ => Err(StoreError::Io),
        }
    }
}

impl RefineOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::RolledBack => "rolled_back",
        }
    }

    fn parse(word: &str) -> Result<Self, StoreError> {
        match word {
            "applied" => Ok(Self::Applied),
            "rejected" => Ok(Self::Rejected),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(StoreError::Io),
        }
    }
}

/// One edit of a refine. The hashes are those of the entry's file content:
/// `before_sha` is absent when the edit creates the entry and `after_sha`
/// when it removes it, so an undo knows which backup to restore or delete.
/// `action` and `kind` stay words for the caller (T66.5) to define; this
/// crate only carries them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefineEdit {
    pub action: String,
    pub kind: String,
    pub id: String,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}

/// What the caller knows when it records a refine; the store stamps
/// `created_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRefineEvent {
    pub id: String,
    pub session_id: SessionId,
    pub scope: RefineScope,
    pub trigger: RefineTrigger,
    pub evidence: String,
    pub edits: Vec<RefineEdit>,
    pub outcome: RefineOutcome,
    pub rollback_of: Option<String>,
    pub fingerprint: String,
}

/// A stored refine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefineEvent {
    pub id: String,
    pub session_id: SessionId,
    pub scope: RefineScope,
    pub trigger: RefineTrigger,
    pub evidence: String,
    pub edits: Vec<RefineEdit>,
    pub outcome: RefineOutcome,
    pub rollback_of: Option<String>,
    pub fingerprint: String,
    pub created_at: String,
}

impl RefineEvent {
    fn from_row(row: RefineEventDbRow) -> Result<Self, StoreError> {
        Ok(Self {
            session_id: row.session_id.parse().map_err(|_| StoreError::Io)?,
            scope: RefineScope::parse(&row.scope)?,
            trigger: RefineTrigger::parse(&row.trigger)?,
            edits: serde_json::from_str(&row.edits).map_err(|_| StoreError::Io)?,
            outcome: RefineOutcome::parse(&row.outcome)?,
            id: row.id,
            evidence: row.evidence,
            rollback_of: row.rollback_of,
            fingerprint: row.fingerprint,
            created_at: row.created_at,
        })
    }
}

impl Store {
    /// Records a refine. A second insert of the same id is an error rather
    /// than an overwrite: history is append-only.
    pub fn refine_insert(&self, event: &NewRefineEvent) -> Result<(), StoreError> {
        let row = RefineEventDbRow {
            id: event.id.clone(),
            session_id: event.session_id.to_string(),
            scope: event.scope.as_str().to_string(),
            trigger: event.trigger.as_str().to_string(),
            evidence: event.evidence.clone(),
            edits: serde_json::to_string(&event.edits).map_err(|_| StoreError::Io)?,
            outcome: event.outcome.as_str().to_string(),
            rollback_of: event.rollback_of.clone(),
            fingerprint: event.fingerprint.clone(),
            created_at: now_rfc3339(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(r::refine_events)
            .values(&row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    /// One refine, or `None` when the id is unknown.
    pub fn refine_get(&self, id: &str) -> Result<Option<RefineEvent>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let row: Option<RefineEventDbRow> = r::refine_events
            .filter(r::id.eq(id))
            .select(RefineEventDbRow::as_select())
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        row.map(RefineEvent::from_row).transpose()
    }

    /// A session's refines, newest first, at most `limit`. `created_at` has
    /// millisecond resolution, so ties fall back to the id: the caller's ids
    /// are ULIDs, which sort by creation time.
    pub fn refine_list(
        &self,
        session: &SessionId,
        limit: usize,
    ) -> Result<Vec<RefineEvent>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<RefineEventDbRow> = r::refine_events
            .filter(r::session_id.eq(session.to_string()))
            .order((r::created_at.desc(), r::id.desc()))
            .limit(i64::try_from(limit).unwrap_or(i64::MAX))
            .select(RefineEventDbRow::as_select())
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        rows.into_iter().map(RefineEvent::from_row).collect()
    }
}

#[cfg(test)]
mod tests {
    use diesel_migrations::MigrationHarness;

    use cox_protocol::Store as _;

    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        (home, store)
    }

    fn event(id: &str, session: SessionId) -> NewRefineEvent {
        NewRefineEvent {
            id: id.into(),
            session_id: session,
            scope: RefineScope::Local,
            trigger: RefineTrigger::Manual,
            evidence: "the user corrected the same lint rule twice".into(),
            edits: vec![
                RefineEdit {
                    action: "add".into(),
                    kind: "rule".into(),
                    id: "lint-first".into(),
                    before_sha: None,
                    after_sha: Some("a1".into()),
                },
                RefineEdit {
                    action: "remove".into(),
                    kind: "skill".into(),
                    id: "stale".into(),
                    before_sha: Some("b2".into()),
                    after_sha: None,
                },
            ],
            outcome: RefineOutcome::Applied,
            rollback_of: None,
            fingerprint: "fp-1".into(),
        }
    }

    #[test]
    fn refine_event_round_trips_every_field() {
        let (_home, store) = store();
        let session = SessionId::new();
        let mut new = event("01A", session);
        new.scope = RefineScope::Global;
        new.trigger = RefineTrigger::Auto;
        new.outcome = RefineOutcome::RolledBack;
        new.rollback_of = Some("00Z".into());
        store.refine_insert(&new).expect("insert");

        let got = store.refine_get("01A").expect("get").expect("row");
        assert_eq!(got.session_id, session);
        assert_eq!(got.scope, RefineScope::Global);
        assert_eq!(got.trigger, RefineTrigger::Auto);
        assert_eq!(got.evidence, new.evidence);
        assert_eq!(got.edits, new.edits);
        assert_eq!(got.outcome, RefineOutcome::RolledBack);
        assert_eq!(got.rollback_of.as_deref(), Some("00Z"));
        assert_eq!(got.fingerprint, "fp-1");
        assert!(!got.created_at.is_empty());

        assert_eq!(store.refine_get("nope").expect("get"), None);
        assert!(
            store.refine_insert(&new).is_err(),
            "an event id is inserted once"
        );
    }

    #[test]
    fn refine_list_is_newest_first_and_honours_limit() {
        let (_home, store) = store();
        let mine = SessionId::new();
        for id in ["01A", "01B", "01C"] {
            store.refine_insert(&event(id, mine)).expect("insert");
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        store
            .refine_insert(&event("01D", SessionId::new()))
            .expect("insert");

        let ids = |limit| -> Vec<String> {
            store
                .refine_list(&mine, limit)
                .expect("list")
                .into_iter()
                .map(|e| e.id)
                .collect()
        };
        assert_eq!(ids(10), ["01C", "01B", "01A"]);
        assert_eq!(ids(2), ["01C", "01B"]);
        assert!(ids(0).is_empty());
    }

    #[test]
    fn down_migration_drops_the_refine_events_table() {
        let (_home, store) = store();
        let session = SessionId::new();
        store.refine_insert(&event("01A", session)).expect("insert");

        let mut conn = store.conn.lock().expect("lock");
        let migrations = diesel::migration::MigrationSource::<diesel::sqlite::Sqlite>::migrations(
            &crate::MIGRATIONS,
        )
        .expect("migrations");
        let ours = migrations
            .iter()
            .find(|m| m.name().to_string().ends_with("_refine_events"))
            .expect("the refine_events migration is embedded");
        conn.revert_migration(ours.as_ref()).expect("down");

        let gone = r::refine_events
            .select(r::id)
            .load::<String>(&mut *conn)
            .is_err();
        assert!(gone, "the table is dropped");
    }
}
