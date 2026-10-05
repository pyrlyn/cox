//! Hosted cloud-agent runs (T56.5, P56): the `cloud_runs` table over
//! Diesel's typed DSL. Kept out of `queries.rs` because it is a write path
//! with its own row type, not a ledger aggregation. The table lets a run
//! outlive the session that started it: a resumed session lists the runs
//! still going, and `mark_usage_recorded` decides which resumer writes the
//! run's ledger row, exactly once.

use diesel::prelude::*;

use cox_protocol::{SessionId, StoreError, TaskId};

use crate::models::CloudRunDbRow;
use crate::schema::cloud_runs::dsl as c;
use crate::{Store, now_rfc3339, write_tx};

/// One hosted run. `status` is the backend's own word; `terminal` is the
/// host's verdict on it, so this crate knows no backend's status list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRun {
    pub backend: String,
    pub run_id: String,
    pub session_id: SessionId,
    pub task_id: TaskId,
    pub agent_id: String,
    pub repository: String,
    pub model: Option<String>,
    pub status: String,
    pub terminal: bool,
    pub usage_recorded: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// What the driver knows when it creates a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCloudRun {
    pub backend: String,
    pub run_id: String,
    pub session_id: SessionId,
    pub task_id: TaskId,
    pub agent_id: String,
    pub repository: String,
    pub model: Option<String>,
    pub status: String,
    pub terminal: bool,
}

impl CloudRun {
    fn from_row(row: CloudRunDbRow) -> Result<Self, StoreError> {
        Ok(Self {
            session_id: row.session_id.parse().map_err(|_| StoreError::Io)?,
            task_id: row.task_id.parse().map_err(|_| StoreError::Io)?,
            backend: row.backend,
            run_id: row.run_id,
            agent_id: row.agent_id,
            repository: row.repository,
            model: row.model,
            status: row.status,
            terminal: row.terminal,
            usage_recorded: row.usage_recorded,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl Store {
    /// Records a new run. A second insert of the same `(backend, run_id)` is
    /// an error rather than an overwrite: it would reset `usage_recorded`.
    pub fn cloud_run_insert(&self, run: &NewCloudRun) -> Result<(), StoreError> {
        let now = now_rfc3339();
        let row = CloudRunDbRow {
            backend: run.backend.clone(),
            run_id: run.run_id.clone(),
            session_id: run.session_id.to_string(),
            task_id: run.task_id.to_string(),
            agent_id: run.agent_id.clone(),
            repository: run.repository.clone(),
            model: run.model.clone(),
            status: run.status.clone(),
            terminal: run.terminal,
            usage_recorded: false,
            created_at: now.clone(),
            updated_at: now,
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(c::cloud_runs)
            .values(&row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    /// Sets a run's status; returns whether the run exists.
    pub fn cloud_run_set_status(
        &self,
        backend: &str,
        run_id: &str,
        status: &str,
        terminal: bool,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let changed = diesel::update(
            c::cloud_runs
                .filter(c::backend.eq(backend))
                .filter(c::run_id.eq(run_id)),
        )
        .set((
            c::status.eq(status),
            c::terminal.eq(terminal),
            c::updated_at.eq(now_rfc3339()),
        ))
        .execute(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(changed == 1)
    }

    /// One run, or `None` when the id is unknown.
    pub fn cloud_run(&self, backend: &str, run_id: &str) -> Result<Option<CloudRun>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let row: Option<CloudRunDbRow> = c::cloud_runs
            .filter(c::backend.eq(backend))
            .filter(c::run_id.eq(run_id))
            .select(CloudRunDbRow::as_select())
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        row.map(CloudRun::from_row).transpose()
    }

    /// The runs a session started that have not reached a terminal status,
    /// oldest first: what a resumed session has to pick up again.
    pub fn cloud_runs_non_terminal(
        &self,
        session: &SessionId,
    ) -> Result<Vec<CloudRun>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<CloudRunDbRow> = c::cloud_runs
            .filter(c::session_id.eq(session.to_string()))
            .filter(c::terminal.eq(false))
            .order((c::created_at.asc(), c::run_id.asc()))
            .select(CloudRunDbRow::as_select())
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        rows.into_iter().map(CloudRun::from_row).collect()
    }

    /// Claims the right to write this run's ledger row: true for exactly one
    /// caller, false for every later one and for an unknown run. The
    /// conditional UPDATE is the whole guard, so two resumed sessions cannot
    /// both record the usage.
    pub fn cloud_run_mark_usage_recorded(
        &self,
        backend: &str,
        run_id: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        write_tx(&mut conn, |conn| {
            let won = diesel::update(
                c::cloud_runs
                    .filter(c::backend.eq(backend))
                    .filter(c::run_id.eq(run_id))
                    .filter(c::usage_recorded.eq(false)),
            )
            .set((c::usage_recorded.eq(true), c::updated_at.eq(now_rfc3339())))
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            Ok(won == 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::Store as _;

    use super::*;

    fn new_run(session: SessionId, run_id: &str, terminal: bool) -> NewCloudRun {
        NewCloudRun {
            backend: "cursor".into(),
            run_id: run_id.into(),
            session_id: session,
            task_id: TaskId::new(),
            agent_id: "bc-1".into(),
            repository: "pyrlyn/cox".into(),
            model: Some("composer-2".into()),
            status: if terminal { "FINISHED" } else { "RUNNING" }.into(),
            terminal,
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        (home, store)
    }

    #[test]
    fn cloud_run_round_trips() {
        let (_home, store) = store();
        let session = SessionId::new();
        let new = new_run(session, "run-1", false);
        store.cloud_run_insert(&new).expect("insert");

        let run = store
            .cloud_run("cursor", "run-1")
            .expect("read")
            .expect("row");
        assert_eq!(run.session_id, session);
        assert_eq!(run.task_id, new.task_id);
        assert_eq!(run.agent_id, "bc-1");
        assert_eq!(run.repository, "pyrlyn/cox");
        assert_eq!(run.model.as_deref(), Some("composer-2"));
        assert_eq!(run.status, "RUNNING");
        assert!(!run.terminal && !run.usage_recorded);
        assert_eq!(run.created_at, run.updated_at);

        assert!(
            store
                .cloud_run_set_status("cursor", "run-1", "FINISHED", true)
                .expect("set")
        );
        let run = store
            .cloud_run("cursor", "run-1")
            .expect("read")
            .expect("row");
        assert_eq!((run.status.as_str(), run.terminal), ("FINISHED", true));
        assert!(
            !store
                .cloud_run_set_status("cursor", "nope", "ERROR", true)
                .expect("set")
        );
        assert_eq!(store.cloud_run("cursor", "nope").expect("read"), None);
        assert!(
            store.cloud_run_insert(&new).is_err(),
            "a run id is inserted once"
        );
    }

    #[test]
    fn cloud_runs_lists_only_non_terminal_runs() {
        let (_home, store) = store();
        let mine = SessionId::new();
        let other = SessionId::new();
        store
            .cloud_run_insert(&new_run(mine, "live-1", false))
            .expect("insert");
        store
            .cloud_run_insert(&new_run(mine, "done-1", true))
            .expect("insert");
        store
            .cloud_run_insert(&new_run(mine, "live-2", false))
            .expect("insert");
        store
            .cloud_run_insert(&new_run(other, "theirs", false))
            .expect("insert");

        let ids: Vec<String> = store
            .cloud_runs_non_terminal(&mine)
            .expect("list")
            .into_iter()
            .map(|run| run.run_id)
            .collect();
        assert_eq!(ids, ["live-1", "live-2"]);

        store
            .cloud_run_set_status("cursor", "live-1", "CANCELLED", true)
            .expect("set");
        let ids: Vec<String> = store
            .cloud_runs_non_terminal(&mine)
            .expect("list")
            .into_iter()
            .map(|run| run.run_id)
            .collect();
        assert_eq!(ids, ["live-2"]);
    }

    #[test]
    fn mark_usage_recorded_wins_once() {
        let (_home, store) = store();
        store
            .cloud_run_insert(&new_run(SessionId::new(), "run-1", true))
            .expect("insert");
        assert!(
            store
                .cloud_run_mark_usage_recorded("cursor", "run-1")
                .expect("first")
        );
        assert!(
            !store
                .cloud_run_mark_usage_recorded("cursor", "run-1")
                .expect("second")
        );
        assert!(
            !store
                .cloud_run_mark_usage_recorded("cursor", "unknown")
                .expect("unknown")
        );
        let run = store
            .cloud_run("cursor", "run-1")
            .expect("read")
            .expect("row");
        assert!(run.usage_recorded);
    }
}
