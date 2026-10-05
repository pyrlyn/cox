// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Ledger aggregations for `cox stats` (T8.4): usage grouped by period,
//! tier and job, plus top tools by archived bytes. Raw SQL lives here —
//! `cox-store` is the only crate that contains SQL (D9); callers group
//! nothing themselves. Also the session tree `/sessions` and `cox sessions`
//! nest forks and handoffs by (T26.3), one session's children
//! (T37.29.6) and a project's spend since a time (T37.29.3.3), over
//! Diesel's typed DSL.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Double, Nullable, Text};

use cox_protocol::{SessionId, StoreError, UsageRow};

use super::Store;
use crate::fts::SessionInfo;
use crate::schema::{sessions, usage};

/// One project's ledger totals: the single `GROUP BY` row
/// [`Store::project_totals`] returns. `tokens` is the `context_tokens`
/// sum (what the models saw, §1.9), `turns` the usage-row count.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTotals {
    /// Distinct sessions with at least one usage row.
    pub sessions: i64,
    /// Usage rows aggregated.
    pub turns: i64,
    /// Summed cost in USD.
    pub cost_usd: f64,
    /// Summed `context_tokens`.
    pub tokens: i64,
}

/// One `sql_query` row for [`Store::project_totals`]: the single `GROUP BY`
/// row over `usage` joined to `sessions`. Same `sql_query` +
/// `QueryableByName` shape as [`TierJobRow`] below (D9: SQL stays in
/// `cox-store`).
#[derive(Debug, Clone, PartialEq, diesel::QueryableByName)]
struct ProjectTotalsRow {
    /// Distinct sessions with at least one usage row.
    #[diesel(sql_type = BigInt)]
    sessions: i64,
    /// Usage rows aggregated.
    #[diesel(sql_type = BigInt)]
    turns: i64,
    /// Summed cost in USD.
    #[diesel(sql_type = Nullable<Double>)]
    cost_usd: Option<f64>,
    /// Summed `context_tokens`.
    #[diesel(sql_type = Nullable<BigInt>)]
    tokens: Option<i64>,
}

/// One `usage` row as [`Store::usage_ledger`] returns it: the call and
/// when it was written (RFC 3339 UTC, `2026-09-02T10:11:12.345Z`).
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRow {
    pub usage: UsageRow,
    pub created_at: String,
}

/// One [`Store::sessions_tree`] row: a session and how deep it nests.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    /// The session's ledger columns.
    pub info: SessionInfo,
    /// `0` for a root; a child sits one deeper than its parent.
    pub depth: usize,
}

/// `(id, title, cwd, created_at, updated_at, turns, cost_usd, parent_id)`.
type SessionCols = (
    String,
    Option<String>,
    String,
    String,
    String,
    i32,
    f64,
    Option<String>,
);

/// One `(period, tier, job)` aggregate over the `usage` ledger. `period` is
/// a day (`2026-09-03`), a month (`2026-09`) or `all`, depending on the
/// [`Period`] asked for.
#[derive(Debug, Clone, PartialEq, QueryableByName)]
pub struct TierJobRow {
    /// The time bucket (day, month or `all`).
    #[diesel(sql_type = Text)]
    pub period: String,
    /// Tier tag (`cheap`, `code`, `think`).
    #[diesel(sql_type = Text)]
    pub tier: String,
    /// Job tag (`main`, `compact`, …).
    #[diesel(sql_type = Text)]
    pub job: String,
    /// Provider calls in the bucket.
    #[diesel(sql_type = BigInt)]
    pub calls: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = BigInt)]
    pub input_tokens: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = BigInt)]
    pub output_tokens: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = BigInt)]
    pub cache_read_tokens: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = BigInt)]
    pub cache_write_tokens: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = BigInt)]
    pub context_tokens: i64,
    /// Summed tokens and cost.
    #[diesel(sql_type = Double)]
    pub cost_usd: f64,
}

/// One tool's archived-byte total over the `archive` table.
#[derive(Debug, Clone, PartialEq, QueryableByName)]
pub struct ToolBytesRow {
    /// Tool name (`read`, `bash`, `mcp__srv__tool`, …).
    #[diesel(sql_type = Text)]
    pub tool: String,
    /// Total archived bytes.
    #[diesel(sql_type = BigInt)]
    pub bytes: i64,
    /// Archived calls.
    #[diesel(sql_type = BigInt)]
    pub calls: i64,
}

/// Which time bucket [`Store::usage_by_period`] groups by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    /// One row set per day (`YYYY-MM-DD`).
    Day,
    /// One row set per month (`YYYY-MM`).
    Month,
    /// A single `all` bucket over the whole ledger.
    All,
}

impl Store {
    /// Every project slug with at least one usage row, oldest spend first —
    /// the scope list for `cox stats --project` with no slug.
    pub fn project_slugs(&self) -> Result<Vec<String>, StoreError> {
        #[derive(diesel::QueryableByName)]
        struct SlugRow {
            #[diesel(sql_type = Text)]
            slug: String,
        }
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let rows: Vec<SlugRow> = diesel::sql_query(
            "SELECT sessions.project_slug AS slug FROM usage \
             INNER JOIN sessions ON usage.session_id = sessions.id \
             GROUP BY sessions.project_slug ORDER BY MIN(usage.created_at)",
        )
        .load(&mut *conn)
        .map_err(|_| StoreError::Sqlite)?;
        Ok(rows.into_iter().map(|r| r.slug).collect())
    }

    /// One project's ledger totals in a single query: `usage` joined to
    /// `sessions` on the slug, grouped once. Sessions without a usage row
    /// contribute nothing (they cost nothing).
    pub fn project_totals(&self, slug: &str) -> Result<ProjectTotals, StoreError> {
        // One `sql_query` like `usage_by_period` below: `SUM()` over the
        // integer columns widens to `Numeric` on SQLite, whose typed-DSL
        // mapping needs the `numeric` feature, while the `sql_query` path
        // reads the sums back as `BigInt` exactly as `usage_by_period` does.
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let row: Option<ProjectTotalsRow> = diesel::sql_query(
            "SELECT COUNT(DISTINCT sessions.id) AS sessions, COUNT(usage.id) AS turns, \
             SUM(usage.cost_usd) AS cost_usd, SUM(usage.context_tokens) AS tokens \
             FROM usage INNER JOIN sessions ON usage.session_id = sessions.id \
             WHERE sessions.project_slug = ? GROUP BY sessions.project_slug",
        )
        .bind::<Text, _>(slug)
        .get_result(&mut *conn)
        .optional()
        .map_err(|_| StoreError::Sqlite)?;
        Ok(match row {
            Some(row) => ProjectTotals {
                sessions: row.sessions,
                turns: row.turns,
                cost_usd: row.cost_usd.unwrap_or(0.0),
                tokens: row.tokens.unwrap_or(0),
            },
            None => ProjectTotals {
                sessions: 0,
                turns: 0,
                cost_usd: 0.0,
                tokens: 0,
            },
        })
    }

    /// Usage grouped by period, tier and job, oldest bucket first.
    pub fn usage_by_period(&self, period: Period) -> Result<Vec<TierJobRow>, StoreError> {
        // Fixed strings only — no user input reaches the format.
        let bucket = match period {
            Period::Day => "date(created_at)",
            Period::Month => "strftime('%Y-%m', created_at)",
            Period::All => "'all'",
        };
        let sql = format!(
            "SELECT {bucket} AS period, tier, job, COUNT(*) AS calls, \
             SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens, \
             SUM(cache_read_tokens) AS cache_read_tokens, \
             SUM(cache_write_tokens) AS cache_write_tokens, \
             SUM(context_tokens) AS context_tokens, SUM(cost_usd) AS cost_usd \
             FROM usage GROUP BY period, tier, job ORDER BY period, tier, job"
        );
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        diesel::sql_query(sql)
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite)
    }

    /// Tools ordered by archived bytes, most first. `session` scopes the
    /// totals to one session; `None` totals the whole archive.
    pub fn top_tools(
        &self,
        session: Option<&SessionId>,
        limit: i64,
    ) -> Result<Vec<ToolBytesRow>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        match session {
            Some(id) => diesel::sql_query(
                "SELECT tool, SUM(bytes) AS bytes, COUNT(*) AS calls FROM archive \
                 WHERE session_id = ? GROUP BY tool ORDER BY bytes DESC LIMIT ?",
            )
            .bind::<Text, _>(id.to_string())
            .bind::<BigInt, _>(limit)
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite),
            None => diesel::sql_query(
                "SELECT tool, SUM(bytes) AS bytes, COUNT(*) AS calls FROM archive \
                 GROUP BY tool ORDER BY bytes DESC LIMIT ?",
            )
            .bind::<BigInt, _>(limit)
            .load(&mut *conn)
            .map_err(|_| StoreError::Sqlite),
        }
    }

    /// The newest `limit` sessions with every child listed under its parent
    /// (newest first at each level). A child whose parent is not in the
    /// page is shown as a root, so a limit never hides a session.
    pub fn sessions_tree(&self, limit: i64) -> Result<Vec<TreeRow>, StoreError> {
        let rows: Vec<SessionCols> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            sessions::table
                .select((
                    sessions::id,
                    sessions::title,
                    sessions::cwd,
                    sessions::created_at,
                    sessions::updated_at,
                    sessions::turns,
                    sessions::cost_usd,
                    sessions::parent_id,
                ))
                .order_by((sessions::updated_at.desc(), sessions::id.desc()))
                .limit(limit)
                .load(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
        };
        let present: HashSet<String> = rows.iter().map(|r| r.0.clone()).collect();
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        let mut roots = Vec::new();
        for (at, row) in rows.iter().enumerate() {
            match row.7.as_ref().filter(|p| present.contains(*p)) {
                Some(parent) => children.entry(parent.clone()).or_default().push(at),
                None => roots.push(at),
            }
        }
        let mut out = Vec::with_capacity(rows.len());
        let mut seen = HashSet::new();
        // Depth-first, children pushed in reverse so they pop newest first.
        let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|at| (at, 0)).collect();
        while let Some((at, depth)) = stack.pop() {
            if !seen.insert(at) {
                continue;
            }
            let (id, title, cwd, created_at, updated_at, turns, cost_usd, _) = rows[at].clone();
            for &child in children.get(&id).into_iter().flatten().rev() {
                stack.push((child, depth + 1));
            }
            out.push(TreeRow {
                info: SessionInfo {
                    id,
                    title,
                    cwd,
                    created_at,
                    updated_at,
                    turns: i64::from(turns),
                    cost_usd,
                },
                depth,
            });
        }
        Ok(out)
    }

    /// What the sessions under `root` spent since `since` (RFC 3339 UTC,
    /// as `usage.created_at` is written): the Context tab's project
    /// footnote (T37.29.3.3). A session belongs to the project its cwd is
    /// in, as `/sessions` lists them; the join is grouped by cwd, so the
    /// folder test runs once per folder, on the cwd as stored or
    /// canonicalized.
    pub fn project_spend(&self, root: &Path, since: &str) -> Result<f64, StoreError> {
        let rows: Vec<(String, Option<f64>)> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            usage::table
                .inner_join(sessions::table.on(sessions::id.eq(usage::session_id)))
                .filter(usage::created_at.ge(since))
                .group_by(sessions::cwd)
                .select((sessions::cwd, diesel::dsl::sum(usage::cost_usd)))
                .load(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
        };
        let under = |cwd: &Path| {
            cwd.starts_with(root) || cwd.canonicalize().is_ok_and(|c| c.starts_with(root))
        };
        Ok(rows
            .into_iter()
            .filter(|(cwd, _)| under(Path::new(cwd)))
            .filter_map(|(_, cost)| cost)
            .sum())
    }

    /// What the whole ledger spent since `since` (RFC 3339 UTC, as
    /// `usage.created_at` is written) and how many sessions were written
    /// since then: the menu bar's "Today" footer (T51.12). A session counts
    /// once however many rows it wrote, subagents included, as the sidebar
    /// lists them.
    pub fn activity_since(&self, since: &str) -> Result<(f64, i64), StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let cost: Option<f64> = usage::table
            .filter(usage::created_at.ge(since))
            .select(diesel::dsl::sum(usage::cost_usd))
            .get_result(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        let active: i64 = sessions::table
            .filter(sessions::updated_at.ge(since))
            .count()
            .get_result(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
        Ok((cost.unwrap_or(0.0), active))
    }

    /// Every session whose parent is `parent`, oldest first: its forks,
    /// handoffs and subagents (T37.29.6).
    pub fn children(&self, parent: &SessionId) -> Result<Vec<SessionId>, StoreError> {
        let ids: Vec<String> = {
            let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
            sessions::table
                .filter(sessions::parent_id.eq(parent.to_string()))
                .select(sessions::id)
                .order_by((sessions::created_at.asc(), sessions::id.asc()))
                .load(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?
        };
        ids.iter()
            .map(|id| {
                id.parse().map_err(|_| StoreError::Corrupt {
                    path: self.home.join("cox.db"),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::{Job, ModelId, ProviderId, Tier, Usage};
    use cox_protocol::{SessionRow, Store as _, UsageRow};

    use super::*;

    fn usage_row(session: SessionId, turn: u32, context: u32, cost: f64) -> UsageRow {
        UsageRow {
            session_id: session,
            turn,
            job: Job::Main,
            tier: Tier::Code,
            provider: ProviderId::Anthropic,
            model: ModelId("claude-sonnet-5".into()),
            effort: None,
            usage: Usage {
                input_tokens: context,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                estimated: false,
                cost_usd: cost,
                latency_ms: 1,
            },
        }
    }

    fn create(store: &Store, parent: Option<SessionId>) -> SessionId {
        // `updated_at` has millisecond resolution; keep the order strict.
        std::thread::sleep(std::time::Duration::from_millis(3));
        let id = SessionId::new();
        store
            .session_create(&SessionRow {
                id,
                created_at: String::new(),
                cwd: "/tmp/work".into(),
                project_slug: "work".into(),
                title: None,
                parent_id: parent,
                rollout_path: std::path::PathBuf::new(),
            })
            .expect("session_create");
        id
    }

    /// T26.3: a fork sits under its parent and a fork of the fork one level
    /// deeper; an unrelated session stays a root; a child whose parent fell
    /// outside the page is a root rather than hidden.
    #[test]
    fn sessions_tree_nests_children() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        let root = create(&store, None);
        let fork = create(&store, Some(root));
        let grandchild = create(&store, Some(fork));
        let other = create(&store, None);
        let sibling = create(&store, Some(root));

        let tree = store.sessions_tree(10).expect("tree");
        let shape: Vec<(String, usize)> =
            tree.iter().map(|r| (r.info.id.clone(), r.depth)).collect();
        assert_eq!(
            shape,
            vec![
                (other.to_string(), 0),
                (root.to_string(), 0),
                (sibling.to_string(), 1),
                (fork.to_string(), 1),
                (grandchild.to_string(), 2),
            ]
        );

        let page = store.sessions_tree(2).expect("page");
        assert!(
            page.iter().all(|r| r.depth == 0),
            "a parent outside the page makes its child a root: {page:?}"
        );
    }

    /// T37.29.6: a session's direct children only, oldest first.
    #[test]
    fn children_lists_direct_children_oldest_first() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        let root = create(&store, None);
        let first = create(&store, Some(root));
        create(&store, Some(first));
        create(&store, None);
        let second = create(&store, Some(root));

        assert_eq!(store.children(&root).expect("children"), [first, second]);
    }

    /// T28.2: the project aggregate is one `GROUP BY` row whose totals equal
    /// the sum of the sessions' rows; an unknown slug totals zero.
    #[test]
    fn project_totals_match_sum_of_sessions() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        let first = create(&store, None);
        let second = create(&store, None);
        store
            .usage_insert(&usage_row(first, 1, 100, 0.01))
            .expect("insert 1");
        store
            .usage_insert(&usage_row(first, 2, 200, 0.02))
            .expect("insert 2");
        store
            .usage_insert(&usage_row(second, 1, 50, 0.005))
            .expect("insert 3");

        let totals = store.project_totals("work").expect("totals");
        assert_eq!(
            totals,
            ProjectTotals {
                sessions: 2,
                turns: 3,
                cost_usd: 0.035,
                tokens: 350,
            },
            "one GROUP BY row equals the sum of the sessions' rows: {totals:?}"
        );

        let empty = store.project_totals("no-such-project").expect("empty");
        assert_eq!(
            empty,
            ProjectTotals {
                sessions: 0,
                turns: 0,
                cost_usd: 0.0,
                tokens: 0,
            }
        );
    }

    #[test]
    fn project_spend_sums_the_sessions_under_the_folder_since_the_cutoff() {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        let at = |cwd: &str, cost: f64| {
            let id = SessionId::new();
            let row = SessionRow {
                id,
                created_at: String::new(),
                cwd: cwd.into(),
                project_slug: String::new(),
                title: None,
                parent_id: None,
                rollout_path: std::path::PathBuf::new(),
            };
            store.session_create(&row).expect("session_create");
            store
                .usage_insert(&usage_row(id, 1, 10, cost))
                .expect("usage_insert");
        };
        at("/work/cox", 1.25);
        at("/work/cox/crates/app", 0.5);
        at("/work/cox-other", 7.0);
        at("/elsewhere", 9.0);

        let root = Path::new("/work/cox");
        let spent = store.project_spend(root, "2000-01-01T00:00:00.000Z");
        assert_eq!(
            spent.expect("spend"),
            1.75,
            "a sibling folder is not the project"
        );
        let later = store.project_spend(root, "2999-01-01T00:00:00.000Z");
        assert_eq!(
            later.expect("spend"),
            0.0,
            "rows before the cutoff are left out"
        );
    }
}
