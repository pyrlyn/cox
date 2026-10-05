// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The cross-process change feed (T37.35, DT§11 item 6): the app
//! and a TUI share one `cox.db`, and each must notice when the other commits
//! a session or a ledger row, so the sidebar and the cost totals refresh.
//! Separate from `lib.rs` because it is a read-only view on the connection,
//! not a store operation, and it carries the cross-process tests.
//!
//! The feed polls SQLite's `PRAGMA data_version`, which changes when another
//! connection — in practice another process — commits, and stays put for this
//! connection's own commits. It runs on the store's one connection behind its
//! `Mutex` rather than a second read connection: the version is per
//! connection, and "commits not made through this `Store`" is exactly what a
//! caller wants, since it already knows about its own writes. Pull-based: no
//! thread, no timer; the caller polls as often as it refreshes.

use diesel::prelude::*;
use diesel::sql_types::BigInt;

use cox_protocol::StoreError;

use crate::Store;

/// A position in [`Store::changes`]: the `data_version` a consumer last saw.
/// Meaningful only for the `Store` that issued it, because the version is
/// per connection. Each consumer holds its own token, so two pollers never
/// consume each other's notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeToken(i64);

#[derive(QueryableByName)]
struct DataVersion {
    #[diesel(sql_type = BigInt)]
    data_version: i64,
}

impl Store {
    /// The feed's current position; the next [`Store::changes`] reports
    /// only commits after this call.
    pub fn change_token(&self) -> Result<ChangeToken, StoreError> {
        self.data_version().map(ChangeToken)
    }

    /// Whether another process committed to `cox.db` since `since` was
    /// taken or last advanced, and advances it. `true` means sessions, the
    /// ledger or both may have changed — `data_version` does not say which
    /// table — so the caller re-reads what it shows. This `Store`'s own
    /// writes never report `true`.
    pub fn changes(&self, since: &mut ChangeToken) -> Result<bool, StoreError> {
        let now = self.data_version()?;
        let changed = now != since.0;
        since.0 = now;
        Ok(changed)
    }

    fn data_version(&self) -> Result<i64, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        // Raw SQL because Diesel cannot model a PRAGMA (D9).
        diesel::sql_query("PRAGMA data_version")
            .get_result::<DataVersion>(&mut *conn)
            .map(|row| row.data_version)
            .map_err(|_| StoreError::Sqlite)
    }
}

#[cfg(test)]
mod tests {
    //! Cross-process tests re-execute this test binary: the parent spawns
    //! `std::env::current_exe()` running only the ignored `writer_process`
    //! test, with `WRITER_HOME` naming the store to write. That is lighter
    //! than a test-only `[[bin]]` or example, which would be one more target
    //! to build and locate; this binary is already built and on disk.

    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};

    use cox_protocol::{
        Event, Job, ModelId, ProviderId, SessionId, SessionRow, StopReason, Store as _, Tier,
        TurnId, Usage, UsageRow,
    };

    use super::*;

    const WRITER_HOME: &str = "COX_STORE_TEST_WRITER_HOME";
    const WRITER_SESSIONS: &str = "COX_STORE_TEST_WRITER_SESSIONS";
    const WRITER_TURNS: &str = "COX_STORE_TEST_WRITER_TURNS";

    fn spawn_writer(home: &Path, sessions: u32, turns: u32) -> Child {
        let exe = std::env::current_exe().expect("current_exe");
        Command::new(exe)
            .args([
                "watch::tests::writer_process",
                "--exact",
                "--ignored",
                "--test-threads=1",
                "-q",
            ])
            .env(WRITER_HOME, home)
            .env(WRITER_SESSIONS, sessions.to_string())
            .env(WRITER_TURNS, turns.to_string())
            .spawn()
            .expect("spawn writer")
    }

    fn env_u32(name: &str) -> u32 {
        std::env::var(name)
            .expect("writer env")
            .parse()
            .expect("writer count")
    }

    /// The child side of the cross-process tests, run only by
    /// `spawn_writer`: opens the store and writes `sessions` sessions of
    /// `turns` ledger rows each, closing every turn with `TurnDone` so the
    /// read-then-write session counter runs too.
    #[test]
    #[ignore = "run by spawn_writer in a child process"]
    fn writer_process() {
        let home = PathBuf::from(std::env::var_os(WRITER_HOME).expect("writer home"));
        let store = Store::open(&home).expect("child open");
        for _ in 0..env_u32(WRITER_SESSIONS) {
            let id = SessionId::new();
            store
                .session_create(&SessionRow {
                    id,
                    created_at: String::new(),
                    cwd: "/tmp/work".into(),
                    project_slug: "work".into(),
                    title: None,
                    parent_id: None,
                    rollout_path: PathBuf::new(),
                })
                .expect("child session_create");
            for turn in 1..=env_u32(WRITER_TURNS) {
                store
                    .usage_insert(&UsageRow {
                        session_id: id,
                        turn,
                        job: Job::Main,
                        tier: Tier::Code,
                        provider: ProviderId::Anthropic,
                        model: ModelId("claude-sonnet-5".into()),
                        effort: None,
                        usage: Usage {
                            input_tokens: 10,
                            output_tokens: 1,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                            estimated: false,
                            cost_usd: 0.25,
                            latency_ms: 1,
                        },
                    })
                    .expect("child usage_insert");
                store
                    .rollout_append(
                        &id,
                        &Event::TurnDone {
                            turn: TurnId::new(),
                            stop: StopReason::EndTurn,
                        },
                    )
                    .expect("child turn done");
            }
        }
    }

    /// T37.35: two processes open a fresh `cox.db` at once and each writes
    /// 500 ledger rows across five sessions; every row lands, no call fails
    /// busy, and each session's counters match its own ledger rows.
    #[test]
    fn concurrent_writers_never_fail_busy() {
        let home = tempfile::tempdir().expect("home");
        let writers = [
            spawn_writer(home.path(), 5, 100),
            spawn_writer(home.path(), 5, 100),
        ];
        for writer in writers {
            let out = writer.wait_with_output().expect("writer exit");
            assert!(out.status.success(), "a writer failed: {out:?}");
        }

        let store = Store::open(home.path()).expect("open");
        let sessions = store.list_sessions(100).expect("sessions");
        assert_eq!(sessions.len(), 10, "every session landed");
        let mut rows = 0;
        for info in &sessions {
            let id: SessionId = info.id.parse().expect("session id");
            let usage = store.usage_for_session(&id).expect("usage");
            assert_eq!(usage.len(), 100, "every ledger row landed");
            assert_eq!(info.turns, 100, "every turn counted");
            assert!(
                (info.cost_usd - 25.0).abs() < 1e-9,
                "cost matches the ledger"
            );
            rows += usage.len();
        }
        assert_eq!(rows, 1000);
    }

    /// T37.35: a commit by another process moves the feed once; this
    /// store's own writes do not.
    #[test]
    fn change_feed_sees_other_process_commit() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("open");
        let mut token = store.change_token().expect("token");
        assert!(!store.changes(&mut token).expect("poll"), "nothing yet");

        let out = spawn_writer(home.path(), 1, 1)
            .wait_with_output()
            .expect("writer exit");
        assert!(out.status.success(), "the writer failed: {out:?}");
        assert!(store.changes(&mut token).expect("poll"), "sees the commit");
        assert!(!store.changes(&mut token).expect("poll"), "reported once");

        store
            .session_create(&SessionRow {
                id: SessionId::new(),
                created_at: String::new(),
                cwd: "/tmp/work".into(),
                project_slug: "work".into(),
                title: None,
                parent_id: None,
                rollout_path: PathBuf::new(),
            })
            .expect("own write");
        assert!(
            !store.changes(&mut token).expect("poll"),
            "own commits are not another process's"
        );
    }
}
