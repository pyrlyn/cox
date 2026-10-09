// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Memory recall ranking (T59.10): the FTS query behind `memory_search`, the
//! file-link lookup, and the reciprocal-rank fusion that lifts a fact linked
//! to a touched file above an equally text-matching one. Separate from
//! `lib.rs` because the fusion is pure and tested without a database.

use std::collections::HashSet;
use std::path::PathBuf;

use cox_protocol::{MemoryHit, StoreError};
use diesel::prelude::*;

use crate::schema::memory_files;

/// Standard reciprocal-rank-fusion constant: damps the lead of rank 1 so a
/// second list can move an entry past a near neighbour, not past a far one.
const RRF_K: f64 = 60.0;

/// FTS candidates fetched when links can reorder them: a linked fact sitting
/// below `limit` in text order must still get the chance to rise into it.
pub(crate) const CANDIDATES: usize = 50;

/// One FTS5 hit with the `memory` row id the link table keys on.
#[derive(diesel::QueryableByName)]
pub(crate) struct FtsRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub id: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub path: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub snippet: String,
}

pub(crate) fn into_hit(row: FtsRow) -> MemoryHit {
    MemoryHit {
        name: row.name,
        path: PathBuf::from(row.path),
        snippet: row.snippet,
    }
}

/// The FTS5 match, in the order `memory_search` has always returned it. Raw
/// SQL because Diesel cannot model an FTS5 virtual table.
pub(crate) fn fts_rows(
    conn: &mut SqliteConnection,
    q: &str,
    limit: usize,
) -> Result<Vec<FtsRow>, StoreError> {
    // Both tables are written together by `memory_upsert` with a shared
    // rowid, which is what the join lines up on.
    diesel::sql_query(
        "SELECT m.id AS id, m.name AS name, m.path AS path, \
         snippet(memory_fts, 1, '', '', '...', 8) AS snippet \
         FROM memory_fts JOIN memory m ON m.rowid = memory_fts.rowid \
         WHERE memory_fts MATCH ? LIMIT ?",
    )
    .bind::<diesel::sql_types::Text, _>(q)
    .bind::<diesel::sql_types::BigInt, _>(limit as i64)
    .load(conn)
    .map_err(|_| StoreError::Sqlite)
}

/// The ids of facts that name at least one of `touched`.
pub(crate) fn linked_ids(
    conn: &mut SqliteConnection,
    touched: &[PathBuf],
) -> Result<HashSet<i32>, StoreError> {
    let paths: Vec<String> = touched
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    Ok(memory_files::table
        .filter(memory_files::path.eq_any(paths))
        .select(memory_files::memory_id)
        .load::<i32>(conn)
        .map_err(|_| StoreError::Sqlite)?
        .into_iter()
        .collect())
}

/// Fuses the text list (`rows`, in FTS order) with the linked list (the same
/// rows whose id is in `linked`, in the same relative order) by reciprocal
/// rank, ties by id, and keeps `limit`. The linked list is a subset of the
/// text list on purpose: a link alone never makes an unrelated fact a hit.
pub(crate) fn fuse(rows: Vec<FtsRow>, linked: &HashSet<i32>, limit: usize) -> Vec<FtsRow> {
    let mut linked_rank = 0usize;
    let mut scored: Vec<(f64, FtsRow)> = rows
        .into_iter()
        .enumerate()
        .map(|(text_rank, row)| {
            let mut score = 1.0 / (RRF_K + text_rank as f64 + 1.0);
            if linked.contains(&row.id) {
                score += 1.0 / (RRF_K + linked_rank as f64 + 1.0);
                linked_rank += 1;
            }
            (score, row)
        })
        .collect();
    scored.sort_by(|(a, ra), (b, rb)| b.total_cmp(a).then(ra.id.cmp(&rb.id)));
    scored.into_iter().map(|(_, r)| r).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i32) -> FtsRow {
        FtsRow {
            id,
            name: format!("m{id}"),
            path: String::new(),
            snippet: String::new(),
        }
    }

    fn ids(rows: &[FtsRow]) -> Vec<i32> {
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn fuse_without_links_keeps_text_order() {
        let out = fuse(vec![row(3), row(1), row(2)], &HashSet::new(), 10);
        assert_eq!(ids(&out), [3, 1, 2]);
    }

    #[test]
    fn fuse_lifts_a_linked_row_above_a_better_text_match() {
        let out = fuse(vec![row(1), row(2), row(3)], &HashSet::from([3]), 10);
        assert_eq!(ids(&out), [3, 1, 2]);
    }

    #[test]
    fn fuse_cuts_to_limit_after_the_lift() {
        let out = fuse(vec![row(1), row(2), row(3)], &HashSet::from([3]), 1);
        assert_eq!(ids(&out), [3]);
    }
}
