// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Which sessions `cox sessions` stops listing (T66.14). The sweep writes
//! `sessions.archived_at` and deletes nothing: the row, the rollout and the
//! archive stay, so `cox expand` and `cox sessions <id>` keep working.
//! A resident session (its lock is held) is skipped. A session with a
//! scheduled job would be skipped too; no such table exists until T66.13,
//! so the predicate is wired and tested and matches nothing today.

use std::time::{SystemTime, UNIX_EPOCH};

use diesel::prelude::*;

use cox_protocol::{SessionId, StoreError};

use crate::Store;
use crate::schema;

/// How many of the newest sessions stay even when the table is long.
pub(crate) const ARCHIVE_KEEP_NEWEST: usize = 200;

/// A session older than this is archived even when it is inside that window.
pub(crate) const ARCHIVE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// One not-yet-archived session the sweep is allowed to mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveCandidate {
    pub id: String,
    pub updated_at: String,
    /// Its process lock is held (`Store::session_holder`).
    pub resident: bool,
    /// A scheduled job names it. Nothing sets this until T66.13.
    pub scheduled: bool,
}

/// Ids to mark, newest-first by `updated_at` then id, matching the list order.
/// Unparseable timestamps stay listed: a corrupt clock must not hide a session.
pub(crate) fn ids_to_archive(mut rows: Vec<ArchiveCandidate>, now_unix: u64) -> Vec<String> {
    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    rows.into_iter()
        .enumerate()
        .filter_map(|(rank, row)| {
            if row.resident || row.scheduled {
                return None;
            }
            let then = unix_of_rfc3339(&row.updated_at)?;
            let old = now_unix.saturating_sub(then) > ARCHIVE_MAX_AGE_SECS;
            let outside = rank >= ARCHIVE_KEEP_NEWEST;
            (old || outside).then_some(row.id)
        })
        .collect()
}

impl Store {
    /// Marks sessions that are older than 30 days or outside the newest 200.
    /// Already-archived rows are left alone, including their `archived_at`.
    pub fn archive_stale_sessions(&self) -> Result<usize, StoreError> {
        let loaded: Vec<(String, String)> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            schema::sessions::table
                .filter(schema::sessions::archived_at.is_null())
                .select((schema::sessions::id, schema::sessions::updated_at))
                .load(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
        };
        let mut candidates = Vec::with_capacity(loaded.len());
        for (id, updated_at) in loaded {
            let resident = match id.parse::<SessionId>() {
                Ok(session) => crate::lock::resident(&self.sessions_dir(), &session)?,
                Err(_) => false,
            };
            candidates.push(ArchiveCandidate {
                id,
                updated_at,
                resident,
                // T66.13 has no table yet; a later card fills this from it.
                scheduled: false,
            });
        }
        let ids = ids_to_archive(candidates, now_unix());
        if ids.is_empty() {
            return Ok(0);
        }
        let now = crate::now_rfc3339();
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let mut marked = 0usize;
        for chunk in ids.chunks(400) {
            let n = diesel::update(
                schema::sessions::table.filter(schema::sessions::id.eq_any(chunk.to_vec())),
            )
            .set(schema::sessions::archived_at.eq(&now))
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            marked += n;
        }
        Ok(marked)
    }

    /// `archived_at` for a test that the second sweep does not rewrite it.
    #[cfg(test)]
    pub(crate) fn archived_at_of(&self, id: &str) -> Result<Option<String>, StoreError> {
        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            archived_at: Option<String>,
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let row: Option<Row> = diesel::sql_query("SELECT archived_at FROM sessions WHERE id = ?")
            .bind::<diesel::sql_types::Text, _>(id)
            .get_result(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        Ok(row.and_then(|r| r.archived_at))
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SS` prefix to unix seconds. Same cut the CLI uses for
/// age, so a session the list calls "30d" is the one the sweep measures.
fn unix_of_rfc3339(text: &str) -> Option<u64> {
    let date = text.get(..10)?;
    let time = text.get(11..19)?;
    let number = |s: &str| s.parse::<i64>().ok();
    let days = days_from_civil(
        number(date.get(..4)?)?,
        number(date.get(5..7)?)?,
        number(date.get(8..10)?)?,
    );
    let secs =
        number(time.get(..2)?)? * 3600 + number(time.get(3..5)?)? * 60 + number(time.get(6..8)?)?;
    u64::try_from(days * 86400 + secs).ok()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cox_protocol::{ArchivePut, CallId, SessionId, SessionRow, Store as _};
    use diesel::sql_types::Text;

    use super::*;
    use crate::Store;

    fn candidate(id: &str, updated_at: &str) -> ArchiveCandidate {
        ArchiveCandidate {
            id: id.into(),
            updated_at: updated_at.into(),
            resident: false,
            scheduled: false,
        }
    }

    fn now_of(stamp: &str) -> u64 {
        unix_of_rfc3339(stamp).expect("stamp")
    }

    #[test]
    fn archive_keeps_the_newest_two_hundred() {
        let stamp = "2026-10-01T00:00:00Z";
        let mut rows = Vec::new();
        for n in 0..=200 {
            rows.push(candidate(&format!("s{n:04}"), stamp));
        }
        let marked = ids_to_archive(rows, now_of(stamp));
        assert_eq!(marked, vec!["s0000".to_string()]);
    }

    #[test]
    fn archive_marks_a_session_older_than_thirty_days() {
        let marked = ids_to_archive(
            vec![candidate("old", "2026-08-01T00:00:00Z")],
            now_of("2026-10-01T00:00:00Z"),
        );
        assert_eq!(marked, vec!["old".to_string()]);
    }

    #[test]
    fn archive_keeps_a_session_at_thirty_days() {
        let now = now_of("2026-10-31T00:00:00Z");
        let then = now - ARCHIVE_MAX_AGE_SECS;
        let days = then.div_euclid(86_400);
        let sod = then.rem_euclid(86_400);
        let (y, m, d) = crate::civil_from_days(days as i64);
        let stamp = format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            sod / 3600,
            (sod % 3600) / 60,
            sod % 60
        );
        assert_eq!(now.saturating_sub(now_of(&stamp)), ARCHIVE_MAX_AGE_SECS);
        let marked = ids_to_archive(vec![candidate("edge", &stamp)], now);
        assert!(marked.is_empty());
    }

    #[test]
    fn archive_skips_a_resident_or_scheduled_session() {
        let now = now_of("2026-10-01T00:00:00Z");
        let mut resident = candidate("held", "2020-01-01T00:00:00Z");
        resident.resident = true;
        let mut scheduled = candidate("cron", "2020-01-01T00:00:00Z");
        scheduled.scheduled = true;
        let marked = ids_to_archive(vec![resident, scheduled], now);
        assert!(marked.is_empty());
    }

    #[test]
    fn archive_leaves_an_unparseable_timestamp_listed() {
        let marked = ids_to_archive(
            vec![candidate("bad", "not-a-time")],
            now_of("2026-10-01T00:00:00Z"),
        );
        assert!(marked.is_empty());
    }

    fn open_session(store: &Store) -> SessionId {
        let id = SessionId::new();
        store
            .session_create(&SessionRow {
                id,
                created_at: String::new(),
                cwd: PathBuf::from("/tmp/work"),
                project_slug: "work".into(),
                title: None,
                parent_id: None,
                rollout_path: PathBuf::new(),
            })
            .expect("session");
        id
    }

    fn backdate(store: &Store, id: &SessionId, stamp: &str) {
        let mut conn = store.conn.lock().expect("lock");
        diesel::sql_query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind::<Text, _>(stamp)
            .bind::<Text, _>(id.to_string())
            .execute(&mut *conn)
            .expect("backdate");
    }

    #[test]
    fn archive_sweep_archives_only_past_the_newest_two_hundred() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");
        for _ in 0..=ARCHIVE_KEEP_NEWEST {
            open_session(&store);
        }
        let marked = store.archive_stale_sessions().expect("sweep");
        assert_eq!(marked, 1);
        assert_eq!(store.list_archived_sessions(10).expect("archived").len(), 1);
        assert_eq!(
            store.list_sessions(500).expect("live").len(),
            ARCHIVE_KEEP_NEWEST
        );
    }

    #[test]
    fn archive_sweep_keeps_rollout_and_archive_and_hides_the_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");
        let id = open_session(&store);
        let rollout = store.rollout_path(&id);
        std::fs::write(&rollout, b"{\"seq\":1}\n").expect("rollout");
        let archive_id = store
            .archive_put(&ArchivePut {
                session: id,
                call: CallId::new(),
                tool: "read".into(),
                subject: Some("a.rs".into()),
                bytes: b"kept".to_vec(),
            })
            .expect("archive");
        backdate(&store, &id, "2000-01-01T00:00:00Z");

        assert_eq!(store.archive_stale_sessions().expect("sweep"), 1);
        let stamped = store.archived_at_of(&id.to_string()).expect("stamp");
        assert!(stamped.is_some());
        assert_eq!(store.archive_stale_sessions().expect("second"), 0);
        assert_eq!(
            store.archived_at_of(&id.to_string()).expect("stamp"),
            stamped
        );

        let live = store.list_sessions(10).expect("live");
        assert!(live.iter().all(|row| row.id != id.to_string()));
        let archived = store.list_archived_sessions(10).expect("archived");
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, id.to_string());
        assert_eq!(store.session_info(&id).expect("row").id, id.to_string());
        assert_eq!(
            store.archive_get(&archive_id).expect("expand"),
            b"kept".to_vec()
        );
        assert_eq!(std::fs::read(&rollout).expect("rollout"), b"{\"seq\":1}\n");
    }

    #[test]
    fn archive_sweep_skips_a_held_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");
        let id = open_session(&store);
        assert!(store.claim_session(&id, "tui").expect("claim").is_none());
        backdate(&store, &id, "2000-01-01T00:00:00Z");
        assert_eq!(store.archive_stale_sessions().expect("sweep"), 0);
        assert!(
            store
                .archived_at_of(&id.to_string())
                .expect("null")
                .is_none()
        );
        assert_eq!(store.list_sessions(10).expect("live").len(), 1);

        drop(store);
        let store = Store::open(dir.path()).expect("reopen");
        assert_eq!(store.archive_stale_sessions().expect("sweep"), 1);
        assert_eq!(store.list_sessions(10).expect("live").len(), 0);
        assert_eq!(store.list_archived_sessions(10).expect("archived").len(), 1);
    }
}
