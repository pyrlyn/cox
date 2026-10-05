// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One SQLite file (`~/.cox/cox.db`): sessions, rollouts (JSONL), the
//! tool-output archive, memory, the cost ledger, and plugin grants/kv
//! (PL§3, A52). Separate so `cox-core` never opens a file directly; it only
//! calls the `Store`/`Archive`/`PluginStore` traits this crate implements
//! (plan.md §1.7/D9). The only crate that contains SQL — a workspace test
//! asserts no other crate depends on `diesel`.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod cloud_runs;
pub mod fts;
pub mod lock;
mod models;
pub mod queries;
mod rollout;
pub mod schema;
mod watch;

pub use watch::ChangeToken;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use sha2::{Digest, Sha256};

use cox_protocol::{
    Archive, ArchiveId, ArchivePut, CheckpointRow, Event, GrantScope, MemoryHit, ModelId,
    PluginGrant, PluginStore as PluginStoreTrait, SessionId, SessionRow, Store as StoreTrait,
    StoreError, Usage, UsageRow,
};
// The `plugin_kv` quotas (PL§3) live beside `PluginStore` so the plugin
// host names the same limit in its refusal (T33.9).
use cox_protocol::traits::{KV_PLUGIN_LIMIT, KV_VALUE_LIMIT};

use models::{
    CheckpointDbRow, NewArchive, NewMemory, NewSession, PluginGrantDbRow, PluginKvDbRow,
    SessionAgentDb, UsageDbRow,
};
use queries::LedgerRow;
use rollout::RolloutWriter;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Inline archive payloads up to this size live in `archive.inline`; larger
/// ones spill to `archive/<id>` under `home` (plan.md §1.7).
const INLINE_ARCHIVE_LIMIT: usize = 16 * 1024;

/// Fsync a rollout file at least this often (plan.md T0.4 step 3): a crash
/// loses at most this many buffered lines.
const ROLLOUT_FSYNC_EVERY: u32 = 16;

/// The concrete `cox_protocol::Store`/`Archive` implementation: one SQLite
/// connection behind a `Mutex` (D9: sync, single-process, no pool) plus a
/// small per-session rollout writer cache.
pub struct Store {
    home: PathBuf,
    conn: Mutex<SqliteConnection>,
    rollouts: Mutex<HashMap<SessionId, RolloutWriter>>,
    /// The sessions this store drives (T37.34), held while it lives: the
    /// session that writes through it keeps it alive.
    locks: Mutex<Vec<lock::SessionLock>>,
}

impl Store {
    /// `COX_HOME`'s default when unset: `~/.cox` (plan.md §1.7). Falls back
    /// to a relative `.cox` if the OS reports no home directory at all
    /// (headless containers without `$HOME`), so this never panics.
    pub fn default_home() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".cox"))
            .unwrap_or_else(|| PathBuf::from(".cox"))
    }

    /// T37.34: makes this store the one writer of session `id`'s rollout
    /// for as long as it lives, or names the process that already is.
    /// `None` means claimed.
    pub fn claim_session(
        &self,
        id: &SessionId,
        surface: &str,
    ) -> Result<Option<lock::Holder>, StoreError> {
        match lock::claim(&self.sessions_dir(), id, surface)? {
            Ok(held) => {
                self.locks.lock().map_err(|_| StoreError::Io)?.push(held);
                Ok(None)
            }
            Err(holder) => Ok(Some(holder)),
        }
    }

    /// T37.10: the process driving session `id`, when it is another one;
    /// probes without claiming (`lock::holder`).
    pub fn session_holder(&self, id: &SessionId) -> Result<Option<lock::Holder>, StoreError> {
        lock::holder(&self.sessions_dir(), id)
    }

    fn sessions_dir(&self) -> PathBuf {
        self.home.join("sessions")
    }

    /// Where session `id`'s JSONL rollout lives, whether or not it exists
    /// yet; the desktop inspector shows it (T37.29.5).
    pub fn rollout_path(&self, id: &SessionId) -> PathBuf {
        self.sessions_dir().join(format!("{id}.jsonl"))
    }

    fn archive_dir(&self) -> PathBuf {
        self.home.join("archive")
    }
}

/// Formats the current time as RFC 3339 UTC with millisecond precision
/// (`"2026-09-02T10:11:12.345Z"`), matching the rollout line format
/// (plan.md §1.7). No date/time crate for this: the calendar math is
/// Howard Hinnant's public-domain `civil_from_days` algorithm. `pub` so
/// `cox plugin enable`/`install` (T33.7, `crates/cox/src/plugin_cmd.rs`)
/// stamp a grant's `decided_at` the same way every other row's timestamp
/// is stamped, instead of a second formatter.
pub fn now_rfc3339() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let millis_total = now.as_millis();
    let secs = (millis_total / 1000) as i64;
    let millis = (millis_total % 1000) as u32;

    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = sod / 3600;
    let mm = (sod % 3600) / 60;
    let ss = sod % 60;

    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Days-since-epoch to a proleptic-Gregorian `(year, month, day)`; public
/// domain (<https://howardhinnant.github.io/date_algorithms.html>).
/// ponytail: assumes `z >= 0` (any real wall-clock "now" since the epoch);
/// upgrade to floor division throughout if this ever needs pre-1970 dates.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Serializes a fieldless/transparent `cox_protocol` type (`Job`, `Tier`,
/// `ProviderId`) to the bare snake_case string its `serde` derive already
/// produces, so the ledger's text columns stay in lock-step with the wire
/// format instead of a hand-maintained second mapping. `pub` so the
/// desktop's cost history (T37.29.3.2) names a tier or job as the ledger
/// stores it.
pub fn to_tag<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Inverse of `to_tag`: a stored tag is the bare string form of a unit-variant
/// enum, so it deserializes straight from a JSON string. `None` means the tag
/// no longer names a variant — a corrupt row, not a defaultable one.
fn from_tag<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// `GrantScope` to the `plugin_grants.scope` text PL§3 names: `user` or
/// `project:<root>`.
fn scope_to_text(scope: &GrantScope) -> String {
    match scope {
        GrantScope::User => "user".to_string(),
        GrantScope::Project(root) => format!("project:{}", root.to_string_lossy()),
    }
}

/// Inverse of `scope_to_text`. `None` means the stored tag matches neither
/// shape — a corrupt row, not a defaultable one.
fn scope_from_text(s: &str) -> Option<GrantScope> {
    if s == "user" {
        Some(GrantScope::User)
    } else {
        s.strip_prefix("project:")
            .map(|root| GrantScope::Project(PathBuf::from(root)))
    }
}

/// The error a write transaction's closure hands back to Diesel's
/// transaction manager. `immediate_transaction` needs
/// `E: From<diesel::result::Error>` (for a failing `BEGIN`/`COMMIT`), which
/// `StoreError` cannot implement: `cox-protocol` has no Diesel dependency.
struct TxError(StoreError);

impl From<diesel::result::Error> for TxError {
    fn from(_: diesel::result::Error) -> Self {
        Self(StoreError::Sqlite)
    }
}

/// Runs a write that reads first, or spans several statements, under
/// `BEGIN IMMEDIATE` (T37.35). The write lock is taken before the first
/// read, so a commit by another process (the app beside the TUI) makes
/// `BEGIN` wait up to `busy_timeout` instead of the read going stale or a
/// deferred transaction's upgrade failing with `SQLITE_BUSY_SNAPSHOT`; and
/// the statements commit together or not at all. A single `INSERT`,
/// `UPDATE` or `DELETE` is atomic on its own and stays in autocommit.
fn write_tx<T>(
    conn: &mut SqliteConnection,
    body: impl FnOnce(&mut SqliteConnection) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    conn.immediate_transaction(|c| body(c).map_err(TxError))
        .map_err(|TxError(e)| e)
}

/// Fails with `SchemaNewer` if `cox.db` records a migration this binary
/// does not embed (T37.36): a newer `cox` migrated it, and running this
/// one's queries or pending migrations against that schema could corrupt
/// it. Runs before any migration, inside the same write transaction.
fn refuse_newer_schema(conn: &mut SqliteConnection) -> Result<(), StoreError> {
    let applied = conn
        .applied_migrations()
        .map_err(|_| StoreError::Migrate { from: 0, to: 1 })?;
    let embedded: Vec<String> =
        diesel::migration::MigrationSource::<diesel::sqlite::Sqlite>::migrations(&MIGRATIONS)
            .map_err(|_| StoreError::Migrate { from: 0, to: 1 })?
            .iter()
            .map(|m| m.name().version().to_string())
            .collect();
    let unknown = applied
        .iter()
        .map(ToString::to_string)
        .filter(|v| !embedded.contains(v))
        .max();
    match unknown {
        Some(db) => Err(StoreError::SchemaNewer {
            db,
            binary: embedded.into_iter().max().unwrap_or_default(),
        }),
        None => Ok(()),
    }
}

/// Switches `cox.db` to WAL. The switch needs an exclusive lock, and SQLite
/// skips the busy handler when two connections both hold a shared lock and
/// wait to upgrade (it would deadlock), so a second process opening a fresh
/// file at the same moment gets "database is locked" at once despite
/// `busy_timeout`. Retry within the same 5 s budget; WAL persists in the
/// file, so only the very first opens can contend here.
fn enable_wal(conn: &mut SqliteConnection) -> Result<(), StoreError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match conn.batch_execute("PRAGMA journal_mode = WAL;") {
            Ok(()) => return Ok(()),
            Err(diesel::result::Error::DatabaseError(_, info))
                if info.message() == "database is locked"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return Err(StoreError::Open),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl StoreTrait for Store {
    fn open(home: &Path) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        for dir in [
            home.to_path_buf(),
            home.join("sessions"),
            home.join("archive"),
            home.join("logs"),
            home.join("projects"),
            home.join("cassettes"),
        ] {
            fs::create_dir_all(&dir).map_err(|_| StoreError::Open)?;
        }

        let db_path = home.join("cox.db");
        let mut conn = SqliteConnection::establish(&db_path.to_string_lossy())
            .map_err(|_| StoreError::Open)?;

        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .map_err(|_| StoreError::Open)?;
        enable_wal(&mut conn)?;

        // Reading the applied versions and then migrating is a read-then-write:
        // two processes opening an older `cox.db` at once would both find the
        // same migration pending. Diesel nests each migration's own
        // transaction as a savepoint inside this one.
        write_tx(&mut conn, |c| {
            refuse_newer_schema(c)?;
            c.run_pending_migrations(MIGRATIONS)
                .map(|_| ())
                .map_err(|_| StoreError::Migrate { from: 0, to: 1 })
        })?;

        Ok(Self {
            home: home.to_path_buf(),
            conn: Mutex::new(conn),
            rollouts: Mutex::new(HashMap::new()),
            locks: Mutex::new(Vec::new()),
        })
    }

    fn session_create(&self, s: &SessionRow) -> Result<(), StoreError> {
        let created = now_rfc3339();
        let new_row = NewSession {
            id: s.id.to_string(),
            created_at: created.clone(),
            updated_at: created,
            cwd: s.cwd.to_string_lossy().into_owned(),
            project_slug: s.project_slug.clone(),
            title: s.title.clone(),
            parent_id: s.parent_id.map(|p| p.to_string()),
            rollout_path: s.rollout_path.to_string_lossy().into_owned(),
            turns: 0,
            cost_usd: 0.0,
            state: "open".to_string(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(schema::sessions::table)
            .values(&new_row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    fn rollout_append(&self, id: &SessionId, ev: &Event) -> Result<u64, StoreError> {
        let mut writers = self.rollouts.lock().map_err(|_| StoreError::Io)?;
        let writer = match writers.entry(*id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                let path = self.rollout_path(id);
                let w = RolloutWriter::open(&path).map_err(|_| StoreError::Io)?;
                v.insert(w)
            }
        };
        let seq = writer
            .append(now_rfc3339(), ev)
            .map_err(|_| StoreError::Io)?;
        drop(writers);
        match ev {
            Event::TurnDone { .. } => self.finish_session_turn(id)?,
            // A113: the core's generated title, or a rename (T37.22.9).
            Event::TitleSet { title, by_user } => {
                let source = if *by_user {
                    TitleSource::User
                } else {
                    TitleSource::Auto
                };
                self.session_title_set(id, title, source)?;
            }
            _ => {}
        }
        Ok(seq)
    }

    fn rollout_read(&self, id: &SessionId) -> Result<Vec<Event>, StoreError> {
        Ok(self.rollout_read_with_truncation(id)?.0)
    }

    fn usage_insert(&self, row: &UsageRow) -> Result<(), StoreError> {
        let new_row = UsageDbRow {
            session_id: row.session_id.to_string(),
            turn: row.turn as i32,
            job: to_tag(&row.job),
            tier: to_tag(&row.tier),
            provider: to_tag(&row.provider),
            model: row.model.0.clone(),
            input_tokens: row.usage.input_tokens as i64,
            output_tokens: row.usage.output_tokens as i64,
            cache_read_tokens: row.usage.cache_read_tokens as i64,
            cache_write_tokens: row.usage.cache_write_tokens as i64,
            estimated: row.usage.estimated,
            cost_usd: row.usage.cost_usd,
            latency_ms: row.usage.latency_ms as i64,
            context_tokens: row.usage.context_tokens() as i64,
            created_at: now_rfc3339(),
            effort: row.effort.as_ref().map(to_tag),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(schema::usage::table)
            .values(&new_row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    fn archive_put(&self, a: &ArchivePut) -> Result<ArchiveId, StoreError> {
        let id = ArchiveId::new();
        let digest = sha256_hex(&a.bytes);

        let (inline, rel_path) = if a.bytes.len() <= INLINE_ARCHIVE_LIMIT {
            (Some(a.bytes.clone()), None)
        } else {
            fs::create_dir_all(self.archive_dir()).map_err(|_| StoreError::Io)?;
            let rel = format!("archive/{id}");
            fs::write(self.home.join(&rel), &a.bytes).map_err(|_| StoreError::Io)?;
            (None, Some(rel))
        };

        let new_row = NewArchive {
            id: id.to_string(),
            session_id: a.session.to_string(),
            call_id: a.call.to_string(),
            tool: a.tool.clone(),
            subject: a.subject.clone(),
            bytes: a.bytes.len() as i64,
            sha256: digest,
            inline,
            path: rel_path,
            created_at: now_rfc3339(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(schema::archive::table)
            .values(&new_row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(id)
    }

    fn archive_get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        let row: models::ArchiveBytes = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            schema::archive::table
                .filter(schema::archive::id.eq(id.to_string()))
                .select((
                    schema::archive::inline,
                    schema::archive::path,
                    schema::archive::sha256,
                ))
                .first(&mut *conn)
                .map_err(|e| match e {
                    diesel::result::Error::NotFound => StoreError::NotFound,
                    _ => StoreError::Sqlite,
                })?
        };

        let bytes = match (&row.inline, &row.path) {
            (Some(data), _) => data.clone(),
            (None, Some(p)) => fs::read(self.home.join(p)).map_err(|_| StoreError::Io)?,
            (None, None) => {
                return Err(StoreError::Corrupt {
                    path: self.archive_dir().join(id.to_string()),
                });
            }
        };

        if sha256_hex(&bytes) != row.sha256 {
            let bad_path = row
                .path
                .map(|p| self.home.join(p))
                .unwrap_or_else(|| PathBuf::from(format!("inline:{id}")));
            return Err(StoreError::Corrupt { path: bad_path });
        }
        Ok(bytes)
    }

    fn memory_search(&self, q: &str, limit: usize) -> Result<Vec<MemoryHit>, StoreError> {
        // Both tables are written together by `memory_upsert` with a shared
        // rowid, which is what the join below lines up on.
        let q = crate::fts::sanitize_match(q);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        #[derive(diesel::QueryableByName)]
        struct Hit {
            #[diesel(sql_type = diesel::sql_types::Text)]
            name: String,
            #[diesel(sql_type = diesel::sql_types::Text)]
            path: String,
            #[diesel(sql_type = diesel::sql_types::Text)]
            snippet: String,
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<Hit> = diesel::sql_query(
            "SELECT m.name AS name, m.path AS path, \
             snippet(memory_fts, 1, '', '', '...', 8) AS snippet \
             FROM memory_fts JOIN memory m ON m.rowid = memory_fts.rowid \
             WHERE memory_fts MATCH ? LIMIT ?",
        )
        .bind::<diesel::sql_types::Text, _>(q)
        .bind::<diesel::sql_types::BigInt, _>(limit as i64)
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;

        Ok(rows
            .into_iter()
            .map(|h| MemoryHit {
                name: h.name,
                path: PathBuf::from(h.path),
                snippet: h.snippet,
            })
            .collect())
    }

    fn memory_upsert(
        &self,
        project: &str,
        name: &str,
        path: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), StoreError> {
        // The FTS row carries the memory row's rowid explicitly, so the
        // `memory_search` join lines up on re-saves as well as first saves.
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        write_tx(&mut conn, |conn| {
            let existing: Option<i32> = schema::memory::table
                .filter(schema::memory::project_slug.eq(project))
                .filter(schema::memory::name.eq(name))
                .select(schema::memory::id)
                .first(&mut *conn)
                .optional()
                .map_err(|_| StoreError::Sqlite)?;
            let rowid = match existing {
                Some(id) => {
                    diesel::update(schema::memory::table.filter(schema::memory::id.eq(id)))
                        .set((
                            schema::memory::path.eq(path),
                            schema::memory::kind.eq(kind),
                            schema::memory::updated_at.eq(now_rfc3339()),
                        ))
                        .execute(&mut *conn)
                        .map_err(|_| StoreError::Sqlite)?;
                    diesel::sql_query("DELETE FROM memory_fts WHERE rowid = ?")
                        .bind::<diesel::sql_types::BigInt, _>(i64::from(id))
                        .execute(&mut *conn)
                        .map_err(|_| StoreError::Sqlite)?;
                    i64::from(id)
                }
                None => {
                    diesel::insert_into(schema::memory::table)
                        .values(&NewMemory {
                            project_slug: project.to_string(),
                            name: name.to_string(),
                            path: path.to_string(),
                            kind: kind.to_string(),
                            updated_at: now_rfc3339(),
                        })
                        .execute(&mut *conn)
                        .map_err(|_| StoreError::Sqlite)?;
                    diesel::select(diesel::dsl::sql::<diesel::sql_types::BigInt>(
                        "last_insert_rowid()",
                    ))
                    .get_result(&mut *conn)
                    .map_err(|_| StoreError::Sqlite)?
                }
            };
            diesel::sql_query(
                "INSERT INTO memory_fts(rowid, name, body, project_slug) VALUES(?,?,?,?)",
            )
            .bind::<diesel::sql_types::BigInt, _>(rowid)
            .bind::<diesel::sql_types::Text, _>(name)
            .bind::<diesel::sql_types::Text, _>(body)
            .bind::<diesel::sql_types::Text, _>(project)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            Ok(())
        })
    }

    fn rollout_index(&self, session: &SessionId, turn: u32, text: &str) -> Result<(), StoreError> {
        self.rollout_index_text(session, turn, text)
    }

    fn checkpoint_insert(&self, row: &CheckpointRow) -> Result<(), StoreError> {
        let new_row = CheckpointDbRow {
            session_id: row.session.to_string(),
            turn: row.turn as i32,
            call_id: row.call.map(|c| c.to_string()),
            path: row.path.to_string_lossy().into_owned(),
            kind: to_tag(&row.kind),
            archive_id: row.archive.map(|a| a.to_string()),
            created_at: now_rfc3339(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::insert_into(schema::checkpoints::table)
            .values(&new_row)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    fn checkpoint_list(&self, session: &SessionId) -> Result<Vec<CheckpointRow>, StoreError> {
        Ok(self
            .checkpoint_rows(session)?
            .into_iter()
            .map(|(row, _)| row)
            .collect())
    }
}

/// Narrower async view of the archive methods for `Tool::call` (D6a); wraps
/// the same sync store, no `spawn_blocking` needed behind a `Mutex` (T0.4).
#[async_trait]
impl Archive for Store {
    async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
        StoreTrait::archive_put(self, &put)
    }

    async fn get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        StoreTrait::archive_get(self, id)
    }
}

impl Store {
    /// Decodes one `plugin_grants` row. A tag or JSON blob that no longer
    /// parses is a corrupt row, same stance as `checkpoint_list`.
    fn grant_from_row(&self, r: PluginGrantDbRow) -> Result<PluginGrant, StoreError> {
        let corrupt = || StoreError::Corrupt {
            path: self.home.join("cox.db"),
        };
        Ok(PluginGrant {
            plugin_id: r.plugin_id,
            scope: scope_from_text(&r.scope).ok_or_else(corrupt)?,
            digest: r.digest,
            capabilities: serde_json::from_str(&r.capabilities).map_err(|_| corrupt())?,
            enabled: r.enabled,
            source: serde_json::from_str(&r.source).map_err(|_| corrupt())?,
            decided_at: r.decided_at,
        })
    }
}

/// Plugin grants and per-plugin kv (T33.5, PL§3).
impl PluginStoreTrait for Store {
    fn grant_get(
        &self,
        plugin_id: &str,
        scope: &GrantScope,
        digest: &str,
    ) -> Result<Option<PluginGrant>, StoreError> {
        let scope_text = scope_to_text(scope);
        let row: Option<PluginGrantDbRow> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            schema::plugin_grants::table
                .filter(schema::plugin_grants::plugin_id.eq(plugin_id))
                .filter(schema::plugin_grants::scope.eq(&scope_text))
                .filter(schema::plugin_grants::digest.eq(digest))
                .select(PluginGrantDbRow::as_select())
                .first(&mut *conn)
                .optional()
                .map_err(|_| StoreError::Sqlite)?
        };
        row.map(|r| self.grant_from_row(r)).transpose()
    }

    fn grant_put(&self, grant: &PluginGrant) -> Result<(), StoreError> {
        let row = PluginGrantDbRow {
            plugin_id: grant.plugin_id.clone(),
            scope: scope_to_text(&grant.scope),
            digest: grant.digest.clone(),
            capabilities: serde_json::to_string(&grant.capabilities).map_err(|_| StoreError::Io)?,
            enabled: grant.enabled,
            source: serde_json::to_string(&grant.source).map_err(|_| StoreError::Io)?,
            decided_at: grant.decided_at.clone(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        // Update-else-insert: two writers must not both see "no row".
        write_tx(&mut conn, |conn| {
            let updated = diesel::update(
                schema::plugin_grants::table
                    .filter(schema::plugin_grants::plugin_id.eq(&row.plugin_id))
                    .filter(schema::plugin_grants::scope.eq(&row.scope))
                    .filter(schema::plugin_grants::digest.eq(&row.digest)),
            )
            .set((
                schema::plugin_grants::capabilities.eq(&row.capabilities),
                schema::plugin_grants::enabled.eq(row.enabled),
                schema::plugin_grants::source.eq(&row.source),
                schema::plugin_grants::decided_at.eq(&row.decided_at),
            ))
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            if updated == 0 {
                diesel::insert_into(schema::plugin_grants::table)
                    .values(&row)
                    .execute(&mut *conn)
                    .map_err(|_| StoreError::Sqlite)?;
            }
            Ok(())
        })
    }

    fn grant_set_enabled(
        &self,
        plugin_id: &str,
        scope: &GrantScope,
        digest: &str,
        enabled: bool,
    ) -> Result<(), StoreError> {
        let scope_text = scope_to_text(scope);
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let updated = diesel::update(
            schema::plugin_grants::table
                .filter(schema::plugin_grants::plugin_id.eq(plugin_id))
                .filter(schema::plugin_grants::scope.eq(&scope_text))
                .filter(schema::plugin_grants::digest.eq(digest)),
        )
        .set(schema::plugin_grants::enabled.eq(enabled))
        .execute(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        if updated == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn grants_delete(&self, plugin_id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::delete(
            schema::plugin_grants::table.filter(schema::plugin_grants::plugin_id.eq(plugin_id)),
        )
        .execute(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    fn kv_get(&self, plugin_id: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        schema::plugin_kv::table
            .filter(schema::plugin_kv::plugin_id.eq(plugin_id))
            .filter(schema::plugin_kv::key.eq(key))
            .select(schema::plugin_kv::value)
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)
    }

    fn kv_put(&self, plugin_id: &str, key: &str, value: &[u8]) -> Result<(), StoreError> {
        if value.len() > KV_VALUE_LIMIT {
            return Err(StoreError::QuotaExceeded);
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        // The quota read and the write must see one snapshot, or two writers
        // each pass the check and together go over.
        write_tx(&mut conn, |conn| {
            // Sum every other key's bytes for this plugin so a re-saved key
            // does not double-count its own previous value against the quota.
            let other_bytes: usize = schema::plugin_kv::table
                .filter(schema::plugin_kv::plugin_id.eq(plugin_id))
                .filter(schema::plugin_kv::key.ne(key))
                .select(schema::plugin_kv::value)
                .load::<Vec<u8>>(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
                .iter()
                .map(Vec::len)
                .sum();
            if other_bytes + value.len() > KV_PLUGIN_LIMIT {
                return Err(StoreError::QuotaExceeded);
            }
            let row = PluginKvDbRow {
                plugin_id: plugin_id.to_string(),
                key: key.to_string(),
                value: value.to_vec(),
                updated_at: now_rfc3339(),
            };
            let updated = diesel::update(
                schema::plugin_kv::table
                    .filter(schema::plugin_kv::plugin_id.eq(plugin_id))
                    .filter(schema::plugin_kv::key.eq(key)),
            )
            .set((
                schema::plugin_kv::value.eq(&row.value),
                schema::plugin_kv::updated_at.eq(&row.updated_at),
            ))
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            if updated == 0 {
                diesel::insert_into(schema::plugin_kv::table)
                    .values(&row)
                    .execute(&mut *conn)
                    .map_err(|_| StoreError::Sqlite)?;
            }
            Ok(())
        })
    }

    fn kv_delete(&self, plugin_id: &str, key: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::delete(
            schema::plugin_kv::table
                .filter(schema::plugin_kv::plugin_id.eq(plugin_id))
                .filter(schema::plugin_kv::key.eq(key)),
        )
        .execute(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    fn kv_delete_all(&self, plugin_id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::delete(schema::plugin_kv::table.filter(schema::plugin_kv::plugin_id.eq(plugin_id)))
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }
}

/// Who set a session's title (A113), stored in `sessions.title_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    /// The `title` job's answer after the first turn.
    Auto,
    /// A rename; an automatic title never replaces it.
    User,
}

impl TitleSource {
    fn tag(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::User => "user",
        }
    }
}

/// The external ACP agent a session is driven by (T52.6, DT§3.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAgent {
    /// The `[external_agents.<name>]` or plugin entry's name.
    pub agent: String,
    /// The agent's own ACP `sessionId`, which `session/load` reopens.
    pub agent_session: Option<String>,
}

/// Public query methods for surfaces like `cox stats`.
impl Store {
    /// Marks session `id` as driven by `agent`, whose ACP session is
    /// `agent_session`.
    pub fn session_agent_set(
        &self,
        id: &SessionId,
        agent: &SessionAgent,
    ) -> Result<(), StoreError> {
        use schema::sessions::dsl as s;
        let row = s::sessions.filter(s::id.eq(id.to_string()));
        let values = (
            s::agent.eq(&agent.agent),
            s::agent_session.eq(agent.agent_session.as_deref()),
        );
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::update(row)
            .set(values)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    /// The agent behind session `id`; `None` for a cox session, and for an
    /// id with no row.
    pub fn session_agent(&self, id: &SessionId) -> Result<Option<SessionAgent>, StoreError> {
        use schema::sessions::dsl as s;
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let row: Option<SessionAgentDb> = s::sessions
            .filter(s::id.eq(id.to_string()))
            .select((s::agent, s::agent_session))
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        Ok(row.and_then(|row| {
            Some(SessionAgent {
                agent: row.agent?,
                agent_session: row.agent_session,
            })
        }))
    }

    /// Stores `title` as the session's title, set by `source`. An `Auto`
    /// title never replaces a `User` one (A113); returns whether the row
    /// changed.
    pub fn session_title_set(
        &self,
        id: &SessionId,
        title: &str,
        source: TitleSource,
    ) -> Result<bool, StoreError> {
        use schema::sessions::dsl as s;
        let row = s::sessions.filter(s::id.eq(id.to_string()));
        let values = (s::title.eq(title), s::title_source.eq(source.tag()));
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let changed = match source {
            TitleSource::User => diesel::update(row).set(values).execute(&mut *conn),
            TitleSource::Auto => diesel::update(
                row.filter(
                    s::title_source
                        .is_null()
                        .or(s::title_source.ne(TitleSource::User.tag())),
                ),
            )
            .set(values)
            .execute(&mut *conn),
        }
        .map_err(|_| StoreError::Sqlite)?;
        Ok(changed > 0)
    }

    /// Every checkpoint row of a session in insertion order, each with its
    /// RFC 3339 `created_at`: the desktop Changes tab shows when a turn it
    /// can rewind to started (T37.29.1); the trait's rows carry no time.
    pub fn checkpoint_rows(
        &self,
        session: &SessionId,
    ) -> Result<Vec<(CheckpointRow, String)>, StoreError> {
        let rows: Vec<CheckpointDbRow> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            schema::checkpoints::table
                .filter(schema::checkpoints::session_id.eq(session.to_string()))
                .order(schema::checkpoints::id.asc())
                .select(CheckpointDbRow::as_select())
                .load(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
        };
        // A tag or id that no longer parses is a corrupt row, not a
        // defaultable one — same stance as `usage_for_session`.
        let corrupt = || StoreError::Corrupt {
            path: self.home.join("cox.db"),
        };
        rows.into_iter()
            .map(|r| {
                let row = CheckpointRow {
                    session: *session,
                    turn: r.turn as u32,
                    call: r
                        .call_id
                        .as_deref()
                        .map(|c| c.parse().map_err(|_| corrupt()))
                        .transpose()?,
                    path: PathBuf::from(r.path),
                    kind: from_tag(&r.kind).ok_or_else(corrupt)?,
                    archive: r
                        .archive_id
                        .as_deref()
                        .map(|a| a.parse().map_err(|_| corrupt()))
                        .transpose()?,
                };
                Ok((row, r.created_at))
            })
            .collect()
    }

    /// Updates the denormalized session counters at a durable turn boundary.
    /// Usage is recorded before `TurnDone`, so the ledger is the source of
    /// truth for the stored cost rather than a second accumulator.
    fn finish_session_turn(&self, id: &SessionId) -> Result<(), StoreError> {
        let session_id = id.to_string();
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        // The ledger sum and the counter update commit as one, so a usage row
        // another process adds in between is not lost from `cost_usd`.
        write_tx(&mut conn, |conn| {
            let cost: Option<f64> = schema::usage::table
                .filter(schema::usage::session_id.eq(&session_id))
                .select(diesel::dsl::sum(schema::usage::cost_usd))
                .first(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?;
            diesel::update(schema::sessions::table.filter(schema::sessions::id.eq(session_id)))
                .set((
                    schema::sessions::turns.eq(schema::sessions::turns + 1),
                    schema::sessions::cost_usd.eq(cost.unwrap_or_default()),
                    schema::sessions::updated_at.eq(now_rfc3339()),
                ))
                .execute(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?;
            Ok(())
        })
    }

    /// Reads a rollout and reports whether a crash-truncated final line was
    /// discarded. Surfaces use this to warn without treating a recoverable
    /// tail as a corrupt session.
    pub fn rollout_read_with_truncation(
        &self,
        id: &SessionId,
    ) -> Result<(Vec<Event>, bool), StoreError> {
        let path = self.rollout_path(id);
        rollout::read_lines(&path).map_err(|_| StoreError::Io)
    }

    /// The most recently created session for `cwd`, used by `cox run --continue`.
    pub fn latest_session_for_cwd(&self, cwd: &Path) -> Result<SessionId, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let id: Option<String> = schema::sessions::table
            .filter(schema::sessions::cwd.eq(cwd.to_string_lossy().as_ref()))
            // ULIDs make the tie-breaker chronological too: two sessions can
            // share the millisecond-resolution `created_at` timestamp.
            .order_by((
                schema::sessions::created_at.desc(),
                schema::sessions::id.desc(),
            ))
            .select(schema::sessions::id)
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        let id = id.ok_or(StoreError::NotFound)?;
        id.parse().map_err(|_| StoreError::Corrupt {
            path: self.home.join("cox.db"),
        })
    }

    /// Every usage row for one session, in turn order — what
    /// `cox stats --session <id>` prints (T1.7).
    pub fn usage_for_session(&self, session_id: &SessionId) -> Result<Vec<UsageRow>, StoreError> {
        let mut rows: Vec<UsageRow> = self
            .usage_ledger(session_id)?
            .into_iter()
            .map(|r| r.usage)
            .collect();
        // Stable, so calls that share a number keep the order they were written in.
        rows.sort_by_key(|r| r.turn);
        Ok(rows)
    }

    /// Every usage row for one session in the order it was written, with
    /// its time: the inspector's cost history (T37.29.3.2) finds turns by
    /// it, because `turn` is a call's ordinal within its turn and restarts
    /// at 1 with each turn.
    pub fn usage_ledger(&self, session_id: &SessionId) -> Result<Vec<LedgerRow>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<UsageDbRow> = schema::usage::table
            .filter(schema::usage::session_id.eq(session_id.to_string()))
            .order_by(schema::usage::id.asc())
            .select(UsageDbRow::as_select())
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        drop(conn);

        rows.into_iter()
            .map(|r| {
                let corrupt = || StoreError::Corrupt {
                    path: self.home.join("cox.db"),
                };
                let usage = UsageRow {
                    session_id: r.session_id.parse().map_err(|_| corrupt())?,
                    turn: r.turn as u32,
                    job: from_tag(&r.job).ok_or_else(corrupt)?,
                    tier: from_tag(&r.tier).ok_or_else(corrupt)?,
                    provider: from_tag(&r.provider).ok_or_else(corrupt)?,
                    model: ModelId(r.model),
                    effort: r
                        .effort
                        .as_deref()
                        .map(|tag| from_tag(tag).ok_or_else(corrupt))
                        .transpose()?,
                    usage: Usage {
                        input_tokens: r.input_tokens as u32,
                        output_tokens: r.output_tokens as u32,
                        cache_read_tokens: r.cache_read_tokens as u32,
                        cache_write_tokens: r.cache_write_tokens as u32,
                        estimated: r.estimated,
                        cost_usd: r.cost_usd,
                        latency_ms: r.latency_ms as u64,
                    },
                };
                Ok(LedgerRow {
                    usage,
                    created_at: r.created_at,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::{CallId, Job, ModelId, ProviderId, Tier, Usage};

    use super::*;

    fn usage_row(session_id: SessionId, input_tokens: u32) -> UsageRow {
        UsageRow {
            session_id,
            turn: 1,
            job: Job::Main,
            tier: Tier::Code,
            provider: ProviderId::Anthropic,
            model: ModelId("claude-sonnet-5".into()),
            effort: Some(cox_protocol::types::Effort::High),
            usage: Usage {
                input_tokens,
                output_tokens: 10,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                estimated: false,
                cost_usd: 0.01,
                latency_ms: 100,
            },
        }
    }

    /// T37.36: a migration version this binary does not embed makes open
    /// fail with `SchemaNewer`, naming both versions, and runs no migration.
    #[test]
    fn older_binary_refuses_newer_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let store = Store::open(dir.path()).expect("first open");
            let mut conn = store.conn.lock().expect("lock");
            // A future binary's migration row; test-only raw SQL because
            // `__diesel_schema_migrations` has no public Diesel table.
            diesel::sql_query(
                "INSERT INTO __diesel_schema_migrations (version) VALUES ('99991231000000')",
            )
            .execute(&mut *conn)
            .expect("insert future version");
        }
        let err = Store::open(dir.path()).err().expect("newer schema refused");
        assert_eq!(
            err,
            StoreError::SchemaNewer {
                db: "99991231000000".into(),
                binary: "00000000000007".into(),
            }
        );
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        Store::open(dir.path()).expect("first open");
        Store::open(dir.path()).expect("second open (re-runs pending migrations, none pending)");
    }

    #[test]
    fn schema_snapshot_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");

        #[derive(diesel::QueryableByName)]
        struct Row {
            #[diesel(sql_type = diesel::sql_types::Text)]
            name: String,
            #[diesel(sql_type = diesel::sql_types::Text)]
            sql: String,
        }
        let rows: Vec<Row> = {
            let mut conn = store.conn.lock().expect("lock");
            diesel::sql_query(
                "SELECT name, sql FROM sqlite_master \
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
                 AND name != '__diesel_schema_migrations' ORDER BY name",
            )
            .load(&mut *conn)
            .expect("query sqlite_master")
        };

        let rendered: String = rows
            .iter()
            .map(|r| format!("-- {}\n{};\n\n", r.name, r.sql))
            .collect();
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn memory_upsert_and_search_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        store
            .memory_upsert(
                "proj",
                "auth-flow",
                "auth-flow.md",
                "decision",
                "Login goes through auth.rs with sessions.",
            )
            .expect("upsert");
        let hits = store.memory_search("sessions auth", 5).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "auth-flow");
        assert_eq!(hits[0].path, PathBuf::from("auth-flow.md"));
        // Re-saving replaces both rows: the join stays aligned, so the old
        // terms stop matching and the new ones start, with no ghost rows.
        store
            .memory_upsert(
                "proj",
                "auth-flow",
                "auth-flow.md",
                "fact",
                "Completely different words here.",
            )
            .expect("re-upsert");
        assert!(
            store
                .memory_search("sessions", 5)
                .expect("search")
                .is_empty()
        );
        let hits = store.memory_search("different words", 5).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "auth-flow");
    }

    #[test]
    fn archive_roundtrip_inline_and_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let session = SessionId::new();

        let small = ArchivePut {
            session,
            call: CallId::new(),
            tool: "read".into(),
            subject: Some("src/lib.rs".into()),
            bytes: b"hello".to_vec(),
        };
        let small_id = store.archive_put(&small).expect("put inline");
        let back = store.archive_get(&small_id).expect("get inline");
        assert_eq!(back, b"hello");
        assert!(
            !dir.path()
                .join("archive")
                .join(small_id.to_string())
                .exists()
        );

        let big = ArchivePut {
            session,
            call: CallId::new(),
            tool: "bash".into(),
            subject: None,
            bytes: vec![7u8; INLINE_ARCHIVE_LIMIT + 1],
        };
        let big_id = store.archive_put(&big).expect("put file");
        let back_big = store.archive_get(&big_id).expect("get file");
        assert_eq!(back_big, vec![7u8; INLINE_ARCHIVE_LIMIT + 1]);
        assert!(dir.path().join("archive").join(big_id.to_string()).exists());
    }

    #[test]
    fn archive_get_detects_corrupt_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let put = ArchivePut {
            session: SessionId::new(),
            call: CallId::new(),
            tool: "bash".into(),
            subject: None,
            bytes: vec![9u8; INLINE_ARCHIVE_LIMIT + 1],
        };
        let id = store.archive_put(&put).expect("put file");
        fs::write(dir.path().join("archive").join(id.to_string()), b"tampered")
            .expect("tamper with archived file");

        let err = store.archive_get(&id).expect_err("sha256 mismatch");
        assert!(matches!(err, StoreError::Corrupt { .. }));
    }

    #[test]
    fn rollout_survives_truncated_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let session = SessionId::new();
        let turn_done = Event::TurnDone {
            turn: cox_protocol::TurnId::new(),
            stop: cox_protocol::StopReason::EndTurn,
        };
        store.rollout_append(&session, &turn_done).expect("append");

        // Truncate the file mid-line to simulate a crash during the write.
        let path = dir.path().join("sessions").join(format!("{session}.jsonl"));
        let full = fs::read(&path).expect("read rollout");
        fs::write(&path, &full[..full.len() - 3]).expect("truncate");

        let events = store
            .rollout_read(&session)
            .expect("read tolerates truncation");
        assert!(events.is_empty());
    }

    #[test]
    fn latest_session_for_cwd_excludes_other_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let cwd = PathBuf::from("/workspace/cox");
        let older = SessionId::new();

        for (id, row_cwd) in [
            (older, cwd.clone()),
            (SessionId::new(), PathBuf::from("/elsewhere")),
        ] {
            store
                .session_create(&SessionRow {
                    id,
                    created_at: String::new(),
                    cwd: row_cwd,
                    project_slug: String::new(),
                    title: None,
                    parent_id: None,
                    rollout_path: PathBuf::new(),
                })
                .expect("session");
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        let newer = SessionId::new();
        store
            .session_create(&SessionRow {
                id: newer,
                created_at: String::new(),
                cwd: cwd.clone(),
                project_slug: String::new(),
                title: None,
                parent_id: None,
                rollout_path: PathBuf::new(),
            })
            .expect("session");

        assert_eq!(store.latest_session_for_cwd(&cwd).expect("latest"), newer);
    }

    /// T37.22.9: a rename reaches the store as a `TitleSet` marked
    /// `by_user`; the title generated after it (a first turn that was
    /// already running) leaves it in place.
    #[test]
    fn a_user_rename_survives_a_later_generated_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let id = SessionId::new();
        store
            .session_create(&SessionRow {
                id,
                created_at: String::new(),
                cwd: PathBuf::from("/tmp"),
                project_slug: String::new(),
                title: None,
                parent_id: None,
                rollout_path: PathBuf::new(),
            })
            .expect("session");
        for (title, by_user) in [("Mine", true), ("Fix the ledger", false)] {
            let set = Event::TitleSet {
                title: title.into(),
                by_user,
            };
            store.rollout_append(&id, &set).expect("append");
        }
        let info = store.session_info(&id).expect("info");
        assert_eq!(info.title.as_deref(), Some("Mine"));
    }

    /// T52.6: an external agent's name and ACP session id survive a
    /// reopen of the store; a cox session has neither.
    #[test]
    fn sessions_agent_column_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (agent, cox) = (SessionId::new(), SessionId::new());
        {
            let store = Store::open(dir.path()).expect("open store");
            for id in [agent, cox] {
                store
                    .session_create(&SessionRow {
                        id,
                        created_at: String::new(),
                        cwd: PathBuf::from("/tmp"),
                        project_slug: String::new(),
                        title: None,
                        parent_id: None,
                        rollout_path: PathBuf::new(),
                    })
                    .expect("session");
            }
            let row = SessionAgent {
                agent: "claude".into(),
                agent_session: Some("acp-7".into()),
            };
            store.session_agent_set(&agent, &row).expect("set");
        }
        let store = Store::open(dir.path()).expect("reopen store");
        let expected = SessionAgent {
            agent: "claude".into(),
            agent_session: Some("acp-7".into()),
        };
        assert_eq!(store.session_agent(&agent).expect("read"), Some(expected));
        assert_eq!(store.session_agent(&cox).expect("read"), None);
        assert_eq!(store.session_agent(&SessionId::new()).expect("read"), None);
    }

    /// A113: a `TitleSet` in the rollout lands in `sessions.title`, where
    /// the session list reads it, and never replaces a user's title.
    #[test]
    fn session_title_round_trips_and_keeps_a_user_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let (auto, renamed) = (SessionId::new(), SessionId::new());
        for id in [auto, renamed] {
            store
                .session_create(&SessionRow {
                    id,
                    created_at: String::new(),
                    cwd: PathBuf::from("/tmp"),
                    project_slug: String::new(),
                    title: None,
                    parent_id: None,
                    rollout_path: PathBuf::new(),
                })
                .expect("session");
        }
        let generated = Event::TitleSet {
            title: "Fix the ledger".into(),
            by_user: false,
        };
        store.rollout_append(&auto, &generated).expect("append");
        assert!(
            store
                .session_title_set(&renamed, "Mine", TitleSource::User)
                .expect("rename")
        );
        store.rollout_append(&renamed, &generated).expect("append");
        let titles: HashMap<_, _> = store
            .sessions_tree(10)
            .expect("list")
            .into_iter()
            .map(|row| (row.info.id, row.info.title))
            .collect();
        assert_eq!(titles[&auto.to_string()].as_deref(), Some("Fix the ledger"));
        assert_eq!(titles[&renamed.to_string()].as_deref(), Some("Mine"));
    }

    #[test]
    fn usage_insert_and_sum() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let session = SessionId::new();
        store
            .session_create(&SessionRow {
                id: session,
                created_at: now_rfc3339(),
                cwd: PathBuf::from("/tmp"),
                project_slug: "cox".into(),
                title: None,
                parent_id: None,
                rollout_path: dir.path().join("sessions").join(format!("{session}.jsonl")),
            })
            .expect("session_create (usage.session_id has a foreign key on sessions.id)");

        store
            .usage_insert(&usage_row(session, 100))
            .expect("insert 1");
        store
            .usage_insert(&usage_row(session, 250))
            .expect("insert 2");

        let mut conn = store.conn.lock().expect("lock");
        let input_tokens: Vec<i64> = schema::usage::table
            .filter(schema::usage::session_id.eq(session.to_string()))
            .select(schema::usage::input_tokens)
            .load(&mut *conn)
            .expect("load usage rows");
        let total: i64 = input_tokens.iter().sum();
        assert_eq!(total, 350);
        drop(conn);

        store
            .rollout_append(
                &session,
                &Event::TurnDone {
                    turn: cox_protocol::TurnId::new(),
                    stop: cox_protocol::StopReason::EndTurn,
                },
            )
            .expect("turn done");
        let mut conn = store.conn.lock().expect("lock");
        let (turns, cost): (i32, f64) = schema::sessions::table
            .filter(schema::sessions::id.eq(session.to_string()))
            .select((schema::sessions::turns, schema::sessions::cost_usd))
            .first(&mut *conn)
            .expect("session counters");
        assert_eq!(turns, 1);
        assert_eq!(cost, 0.02);
    }

    #[test]
    fn checkpoint_rows_round_trip_in_insertion_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let session = SessionId::new();
        let call = CallId::new();
        let archive = store
            .archive_put(&ArchivePut {
                session,
                call,
                tool: "checkpoint".into(),
                subject: Some("/w/a.rs".into()),
                bytes: b"before".to_vec(),
            })
            .expect("archive");
        let rows = vec![
            CheckpointRow {
                session,
                turn: 1,
                call: None,
                path: PathBuf::new(),
                kind: cox_protocol::CheckpointKind::Turn,
                archive: None,
            },
            CheckpointRow {
                session,
                turn: 1,
                call: Some(call),
                path: PathBuf::from("/w/a.rs"),
                kind: cox_protocol::CheckpointKind::Pre,
                archive: Some(archive),
            },
        ];
        for row in &rows {
            store.checkpoint_insert(row).expect("insert");
        }
        store
            .checkpoint_insert(&CheckpointRow {
                session: SessionId::new(),
                ..rows[0].clone()
            })
            .expect("another session's row");
        assert_eq!(store.checkpoint_list(&session).expect("list"), rows);
        let timed = store.checkpoint_rows(&session).expect("rows");
        assert!(
            timed.iter().all(|(_, at)| at.starts_with("20")),
            "{timed:?}"
        );
    }

    #[test]
    fn grant_rows_are_per_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let scope = GrantScope::User;
        let grant_v1 = PluginGrant {
            plugin_id: "git-glance".into(),
            scope: scope.clone(),
            digest: "digest-v1".into(),
            capabilities: serde_json::json!(["events:turn_started"]),
            enabled: true,
            source: serde_json::json!({"kind": "path", "path": "/plugins/git-glance"}),
            decided_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let grant_v2 = PluginGrant {
            digest: "digest-v2".into(),
            capabilities: serde_json::json!(["events:turn_started", "kv"]),
            ..grant_v1.clone()
        };
        store.grant_put(&grant_v1).expect("put v1");
        store.grant_put(&grant_v2).expect("put v2");

        // A new digest is a new row: the previous digest's grant is untouched.
        assert_eq!(
            store
                .grant_get("git-glance", &scope, "digest-v1")
                .expect("get v1"),
            Some(grant_v1)
        );
        assert_eq!(
            store
                .grant_get("git-glance", &scope, "digest-v2")
                .expect("get v2"),
            Some(grant_v2)
        );
        assert_eq!(
            store
                .grant_get("git-glance", &scope, "digest-v3")
                .expect("get missing digest"),
            None
        );

        store.grants_delete("git-glance").expect("delete all");
        assert_eq!(
            store
                .grant_get("git-glance", &scope, "digest-v1")
                .expect("get after delete"),
            None
        );
        assert_eq!(
            store
                .grant_get("git-glance", &scope, "digest-v2")
                .expect("get after delete"),
            None
        );
    }

    #[test]
    fn kv_quota_rejects_oversize_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");

        let too_big = vec![0u8; KV_VALUE_LIMIT + 1];
        assert_eq!(
            store.kv_put("git-glance", "big", &too_big),
            Err(StoreError::QuotaExceeded)
        );

        // Each value fits alone, but sixteen of them fill the 1 MiB
        // per-plugin quota; a seventeenth key of any size goes over it.
        let chunk = vec![0u8; KV_VALUE_LIMIT];
        for i in 0..(KV_PLUGIN_LIMIT / KV_VALUE_LIMIT) {
            store
                .kv_put("git-glance", &format!("k{i}"), &chunk)
                .expect("within the per-plugin quota so far");
        }
        assert_eq!(
            store.kv_put("git-glance", "one-more", &[0u8; 1]),
            Err(StoreError::QuotaExceeded)
        );

        // Re-saving an already-stored key must not double-count its own
        // previous bytes against the quota.
        store
            .kv_put("git-glance", "k0", &chunk)
            .expect("re-saving an existing key stays within quota");
    }

    #[test]
    fn kv_delete_all_removes_only_that_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        store
            .kv_put("git-glance", "a", b"1")
            .expect("put git-glance");
        store
            .kv_put("other-plugin", "a", b"2")
            .expect("put other-plugin");

        store
            .kv_delete_all("git-glance")
            .expect("delete git-glance's kv");

        assert_eq!(store.kv_get("git-glance", "a").expect("get"), None);
        assert_eq!(
            store.kv_get("other-plugin", "a").expect("get"),
            Some(b"2".to_vec())
        );
    }

    #[test]
    fn kv_delete_removes_only_that_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        store.kv_put("jev", "a", b"1").expect("put a");
        store.kv_put("jev", "b", b"2").expect("put b");

        store.kv_delete("jev", "a").expect("delete a");
        store
            .kv_delete("jev", "missing")
            .expect("absent key is fine");

        assert_eq!(store.kv_get("jev", "a").expect("get"), None);
        assert_eq!(store.kv_get("jev", "b").expect("get"), Some(b"2".to_vec()));
    }
}
