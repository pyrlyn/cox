// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Session full-text search (T10.3): `rollout_fts` writes and reads, plus
//! the session listing `cox sessions` prints. Raw SQL lives here —
//! `cox-store` is the only crate that contains SQL (D9).

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Double, Nullable, Text};

use cox_protocol::StoreError;

use cox_protocol::SessionId;

use super::Store;

/// One `rollout_fts` hit: which session and turn matched, with an excerpt.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RolloutHit {
    /// Matching session.
    pub session_id: String,
    /// Turn number recorded at index time.
    pub turn: i64,
    /// Excerpt around the match.
    pub snippet: String,
}

/// One row of `cox sessions`: the ledger columns plus the id.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    /// Session id.
    pub id: String,
    /// Generated title, if any.
    pub title: Option<String>,
    /// Working directory the session started in.
    pub cwd: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last-write timestamp (drives "age" and ordering).
    pub updated_at: String,
    /// Finished turns.
    pub turns: i64,
    /// Ledger cost in USD.
    pub cost_usd: f64,
}

#[derive(QueryableByName)]
struct HitRow {
    #[diesel(sql_type = Text)]
    session_id: String,
    #[diesel(sql_type = BigInt)]
    turn: i64,
    #[diesel(sql_type = Text)]
    snippet: String,
}

/// One prompt the user typed (T25.8): the session it was typed in, and the
/// text.
#[derive(Debug, Clone, PartialEq, QueryableByName)]
pub struct UserPrompt {
    #[diesel(sql_type = Text)]
    pub session_id: String,
    #[diesel(sql_type = Text)]
    pub text: String,
}

#[derive(QueryableByName)]
struct SessionRowLite {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Nullable<Text>)]
    title: Option<String>,
    #[diesel(sql_type = Text)]
    cwd: String,
    #[diesel(sql_type = Text)]
    created_at: String,
    #[diesel(sql_type = Text)]
    updated_at: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    turns: i32,
    #[diesel(sql_type = Double)]
    cost_usd: f64,
}

impl Store {
    /// Indexes one model-visible text under `(session, turn)`. Empty texts
    /// are skipped so markers and thinking-only turns leave no rows.
    pub fn rollout_index_text(
        &self,
        session: &SessionId,
        turn: u32,
        text: &str,
    ) -> Result<(), StoreError> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::sql_query("INSERT INTO rollout_fts(session_id, turn, text) VALUES(?,?,?)")
            .bind::<Text, _>(session.to_string())
            .bind::<BigInt, _>(i64::from(turn))
            .bind::<Text, _>(text)
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok(())
    }

    /// Full-text search over indexed session text, best match first.
    pub fn rollout_search(&self, q: &str, limit: i64) -> Result<Vec<RolloutHit>, StoreError> {
        let q = sanitize_match(q);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<HitRow> = diesel::sql_query(
            "SELECT session_id, turn, snippet(rollout_fts, 2, '', '', '...', 8) AS snippet \
             FROM rollout_fts WHERE rollout_fts MATCH ? LIMIT ?",
        )
        .bind::<Text, _>(q)
        .bind::<BigInt, _>(limit)
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(rows
            .into_iter()
            .map(|r| RolloutHit {
                session_id: r.session_id,
                turn: r.turn,
                snippet: r.snippet,
            })
            .collect())
    }

    /// The user's own prompts across every session, newest first (T25.8).
    /// The core indexes a turn's user text before anything else of that
    /// turn, so the first row of each `(session, turn)` is the prompt; turn
    /// 0 holds text indexed before any turn and is left out. A plain scan,
    /// not a `MATCH`, which is why it is SQL over the FTS5 table (D9).
    pub fn user_prompts(&self, limit: i64) -> Result<Vec<UserPrompt>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::sql_query(
            "SELECT session_id, text FROM rollout_fts WHERE rowid IN \
             (SELECT MIN(rowid) FROM rollout_fts WHERE turn > 0 GROUP BY session_id, turn) \
             ORDER BY rowid DESC LIMIT ?",
        )
        .bind::<BigInt, _>(limit)
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)
    }

    /// One session's ledger row, for `cox sessions <id>`.
    pub fn session_info(&self, id: &SessionId) -> Result<SessionInfo, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<SessionRowLite> = diesel::sql_query(
            "SELECT id, title, cwd, created_at, updated_at, turns, cost_usd \
             FROM sessions WHERE id = ?",
        )
        .bind::<Text, _>(id.to_string())
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        rows.into_iter()
            .next()
            .map(into_info)
            .ok_or(StoreError::NotFound)
    }

    /// Every session, most recently written first.
    pub fn list_sessions(&self, limit: i64) -> Result<Vec<SessionInfo>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<SessionRowLite> = diesel::sql_query(
            "SELECT id, title, cwd, created_at, updated_at, turns, cost_usd \
             FROM sessions ORDER BY updated_at DESC, id DESC LIMIT ?",
        )
        .bind::<BigInt, _>(limit)
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(rows.into_iter().map(into_info).collect())
    }
}

fn into_info(r: SessionRowLite) -> SessionInfo {
    SessionInfo {
        id: r.id,
        title: r.title,
        cwd: r.cwd,
        created_at: r.created_at,
        updated_at: r.updated_at,
        turns: i64::from(r.turns),
        cost_usd: r.cost_usd,
    }
}

/// Quotes every whitespace-separated term as an FTS5 phrase, so user input
/// (`no-such-term`, `column:term`, stray quotes) searches literally instead
/// of parsing as query syntax — or erroring the whole search.
pub(crate) fn sanitize_match(q: &str) -> String {
    q.split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::traits::Store as _;

    #[test]
    fn user_prompts_are_each_turns_first_text_newest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open store");
        let (a, b) = (SessionId::new(), SessionId::new());
        for (session, turn, text) in [
            (a, 0, "summary before any turn"),
            (a, 1, "add a cache"),
            (a, 1, "the assistant's reply"),
            (b, 1, "fix the login bug"),
            (a, 2, "now test it"),
        ] {
            store
                .rollout_index_text(&session, turn, text)
                .expect("index");
        }
        let prompts: Vec<_> = store
            .user_prompts(10)
            .expect("prompts")
            .into_iter()
            .map(|p| (p.session_id == a.to_string(), p.text))
            .collect();
        assert_eq!(
            prompts,
            [
                (true, "now test it".to_string()),
                (false, "fix the login bug".to_string()),
                (true, "add a cache".to_string()),
            ]
        );
    }
}
