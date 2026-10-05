// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Queryable`/`Insertable` row types for `schema.rs`'s non-virtual tables
//! (plan.md §1.7/D9). Every field is a plain SQL-shaped type (`String`,
//! `i64`, ...); the `Store` impl in `lib.rs` converts to/from the
//! `cox_protocol` types at the boundary.

use diesel::prelude::*;

use crate::schema::{
    archive, checkpoints, cloud_runs, memory, plugin_grants, plugin_kv, sessions, usage,
};

#[derive(Insertable)]
#[diesel(table_name = sessions)]
pub(crate) struct NewSession {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub cwd: String,
    pub project_slug: String,
    pub title: Option<String>,
    pub parent_id: Option<String>,
    pub rollout_path: String,
    pub turns: i32,
    pub cost_usd: f64,
    pub state: String,
}

/// Both directions of the `usage` table: written by `usage_insert`, read back
/// by `usage_for_session` (`cox stats`). Field order matches the table's
/// column order after `id`, which is what `Selectable` checks.
#[derive(Insertable, Queryable, Selectable)]
#[diesel(table_name = usage)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct UsageDbRow {
    pub session_id: String,
    pub turn: i32,
    pub job: String,
    pub tier: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub estimated: bool,
    pub cost_usd: f64,
    pub latency_ms: i64,
    pub context_tokens: i64,
    pub created_at: String,
    /// `NULL` on rows written before `00000000000002_usage_effort`.
    pub effort: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = archive)]
pub(crate) struct NewArchive {
    pub id: String,
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
    pub subject: Option<String>,
    pub bytes: i64,
    pub sha256: String,
    pub inline: Option<Vec<u8>>,
    pub path: Option<String>,
    pub created_at: String,
}

/// The external agent behind a session (T52.6): `sessions.agent` and
/// `sessions.agent_session`, both `NULL` for a cox session.
#[derive(Queryable)]
pub(crate) struct SessionAgentDb {
    pub agent: Option<String>,
    pub agent_session: Option<String>,
}

/// The columns `Store::archive_get` needs to resolve and verify a payload;
/// selected explicitly rather than the whole row (nothing reads the rest).
#[derive(Queryable)]
pub(crate) struct ArchiveBytes {
    pub inline: Option<Vec<u8>>,
    pub path: Option<String>,
    pub sha256: String,
}

/// One `memory` row for `Store::memory_upsert` (T10.1); `id` autoincrements
/// and the matching `memory_fts` row is written with the same rowid.
#[derive(Insertable)]
#[diesel(table_name = memory)]
pub(crate) struct NewMemory {
    pub project_slug: String,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub updated_at: String,
}

/// Both directions of the `checkpoints` table (T26.1): written by
/// `checkpoint_insert`, read back by `checkpoint_list`. Field order matches
/// the table after `id`.
#[derive(Insertable, Queryable, Selectable)]
#[diesel(table_name = checkpoints)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct CheckpointDbRow {
    pub session_id: String,
    pub turn: i32,
    pub call_id: Option<String>,
    pub path: String,
    pub kind: String,
    pub archive_id: Option<String>,
    pub created_at: String,
}

/// Both directions of the `plugin_grants` table (T33.5, PL§3): the grant
/// approved for one plugin id at one package digest, in one scope. Field
/// order matches the table; `Store` converts `scope`/`capabilities`/`source`
/// to and from `GrantScope`/JSON at the boundary.
#[derive(Insertable, Queryable, Selectable)]
#[diesel(table_name = plugin_grants)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct PluginGrantDbRow {
    pub plugin_id: String,
    pub scope: String,
    pub digest: String,
    pub capabilities: String,
    pub enabled: bool,
    pub source: String,
    pub decided_at: String,
}

/// Both directions of the `plugin_kv` table (T33.5, PL§3): one value under
/// one key for one plugin id.
#[derive(Insertable, Queryable, Selectable)]
#[diesel(table_name = plugin_kv)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct PluginKvDbRow {
    pub plugin_id: String,
    pub key: String,
    pub value: Vec<u8>,
    pub updated_at: String,
}

/// Both directions of the `cloud_runs` table (T56.5, P56).
#[derive(Insertable, Queryable, Selectable)]
#[diesel(table_name = cloud_runs)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct CloudRunDbRow {
    pub backend: String,
    pub run_id: String,
    pub session_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub repository: String,
    pub model: Option<String>,
    pub status: String,
    pub terminal: bool,
    pub usage_recorded: bool,
    pub created_at: String,
    pub updated_at: String,
}
