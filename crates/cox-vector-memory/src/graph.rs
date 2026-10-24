// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Graph memory: typed relations between memory entries in plain SQLite.
//! Nodes reuse the vector ids ([`crate::entry_id`]); traversal is a recursive
//! CTE. The `sqlite-graph` extension was evaluated and not used (alpha, no
//! variable-length paths, needs a loadable C extension); see the docs.

use std::path::Path;
use std::sync::Mutex;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};

use crate::{Entry, Error, Kind, Result, entry_id};

/// Relation type between two entries. Traversal ignores direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relation {
    DependsOn,
    RelatedTo,
    WorkedWith,
    Other(String),
}

impl Relation {
    pub fn as_str(&self) -> &str {
        match self {
            Relation::DependsOn => "depends_on",
            Relation::RelatedTo => "related_to",
            Relation::WorkedWith => "worked_with",
            Relation::Other(s) => s,
        }
    }
}

/// An entry reached by traversal and its hop distance from the nearest seed.
#[derive(Clone, Debug, PartialEq)]
pub struct Related {
    pub entry: Entry,
    pub depth: u32,
}

/// Relation storage scoped by project.
pub trait GraphStore: Send + Sync {
    /// Adds (or keeps) a node for `entry`; returns its id.
    fn add_node(&self, entry: &Entry) -> Result<u64>;
    fn add_edge(&self, project: &str, src: u64, dst: u64, rel: &Relation) -> Result<()>;
    /// Removes the node; its edges go with it (FK cascade).
    fn delete_node(&self, project: &str, id: u64) -> Result<()>;
    fn edge_count(&self, project: &str) -> Result<usize>;
    /// Entries within `max_depth` hops of `seeds`, seeds excluded.
    fn neighbors(&self, project: &str, seeds: &[u64], max_depth: u32) -> Result<Vec<Related>>;
}

pub struct SqliteGraphStore {
    conn: Mutex<SqliteConnection>,
}

fn err(e: impl std::fmt::Display) -> Error {
    Error(format!("graph: {e}"))
}

// SQLite integers are i64; ids keep their bits.
fn sql_id(id: u64) -> i64 {
    id as i64
}

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS mem_node (
  project TEXT NOT NULL, id INTEGER NOT NULL, kind TEXT NOT NULL, text TEXT NOT NULL,
  PRIMARY KEY (project, id));
CREATE TABLE IF NOT EXISTS mem_edge (
  project TEXT NOT NULL, src INTEGER NOT NULL, dst INTEGER NOT NULL, rel TEXT NOT NULL,
  PRIMARY KEY (project, src, dst, rel),
  FOREIGN KEY (project, src) REFERENCES mem_node(project, id) ON DELETE CASCADE,
  FOREIGN KEY (project, dst) REFERENCES mem_node(project, id) ON DELETE CASCADE);
CREATE INDEX IF NOT EXISTS mem_edge_src_rel ON mem_edge(project, src, rel);
CREATE INDEX IF NOT EXISTS mem_edge_dst_rel ON mem_edge(project, dst, rel);
";

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    project: String,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    text: String,
    #[diesel(sql_type = Integer)]
    depth: i32,
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

impl SqliteGraphStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = SqliteConnection::establish(&path.to_string_lossy()).map_err(err)?;
        conn.batch_execute(SCHEMA).map_err(err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut SqliteConnection) -> Result<T>) -> Result<T> {
        let mut c = self
            .conn
            .lock()
            .map_err(|_| Error("graph: poisoned lock".into()))?;
        f(&mut c)
    }
}

impl GraphStore for SqliteGraphStore {
    fn add_node(&self, entry: &Entry) -> Result<u64> {
        let id = entry_id(entry);
        self.with(|c| {
            diesel::sql_query(
                "INSERT OR IGNORE INTO mem_node (project, id, kind, text) VALUES (?, ?, ?, ?)",
            )
            .bind::<Text, _>(&entry.project)
            .bind::<BigInt, _>(sql_id(id))
            .bind::<Text, _>(entry.kind.as_str())
            .bind::<Text, _>(&entry.text)
            .execute(c)
            .map_err(err)?;
            Ok(id)
        })
    }

    fn add_edge(&self, project: &str, src: u64, dst: u64, rel: &Relation) -> Result<()> {
        self.with(|c| {
            diesel::sql_query(
                "INSERT OR IGNORE INTO mem_edge (project, src, dst, rel) VALUES (?, ?, ?, ?)",
            )
            .bind::<Text, _>(project)
            .bind::<BigInt, _>(sql_id(src))
            .bind::<BigInt, _>(sql_id(dst))
            .bind::<Text, _>(rel.as_str())
            .execute(c)
            .map(|_| ())
            .map_err(err)
        })
    }

    fn delete_node(&self, project: &str, id: u64) -> Result<()> {
        self.with(|c| {
            diesel::sql_query("DELETE FROM mem_node WHERE project = ? AND id = ?")
                .bind::<Text, _>(project)
                .bind::<BigInt, _>(sql_id(id))
                .execute(c)
                .map(|_| ())
                .map_err(err)
        })
    }

    fn edge_count(&self, project: &str) -> Result<usize> {
        self.with(|c| {
            let r: Count =
                diesel::sql_query("SELECT COUNT(*) AS n FROM mem_edge WHERE project = ?")
                    .bind::<Text, _>(project)
                    .get_result(c)
                    .map_err(err)?;
            Ok(r.n as usize)
        })
    }

    fn neighbors(&self, project: &str, seeds: &[u64], max_depth: u32) -> Result<Vec<Related>> {
        if seeds.is_empty() || max_depth == 0 {
            return Ok(Vec::new());
        }
        // Seeds go in as a JSON array so the query text stays fixed.
        let seed_json =
            serde_json::to_string(&seeds.iter().map(|s| sql_id(*s)).collect::<Vec<_>>())
                .map_err(err)?;
        let rows: Vec<Row> = self.with(|c| {
            diesel::sql_query(
                "WITH RECURSIVE
                   seed(id) AS (SELECT value FROM json_each(?1)),
                   adj(a, b) AS (
                     SELECT src, dst FROM mem_edge WHERE project = ?2
                     UNION ALL SELECT dst, src FROM mem_edge WHERE project = ?2),
                   walk(id, depth) AS (
                     SELECT id, 0 FROM seed
                     UNION
                     SELECT adj.b, walk.depth + 1 FROM walk JOIN adj ON adj.a = walk.id
                     WHERE walk.depth < ?3)
                 SELECT n.project AS project, n.kind AS kind, n.text AS text,
                        MIN(w.depth) AS depth
                 FROM walk w JOIN mem_node n ON n.project = ?2 AND n.id = w.id
                 WHERE w.id NOT IN (SELECT id FROM seed)
                 GROUP BY n.id ORDER BY depth, n.text",
            )
            .bind::<Text, _>(seed_json)
            .bind::<Text, _>(project)
            .bind::<Integer, _>(max_depth as i32)
            .load(c)
            .map_err(err)
        })?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Related {
                    entry: Entry {
                        project: r.project,
                        kind: Kind::parse(&r.kind)?,
                        text: r.text,
                    },
                    depth: r.depth as u32,
                })
            })
            .collect())
    }
}
