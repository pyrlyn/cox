// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fallback [`VectorStore`]: vectors as little-endian f32 blobs in SQLite,
//! exact cosine search by scanning one project's rows. This is NOT Qdrant;
//! it exists because `qdrant-edge` 0.9.0 cannot build in the cox workspace
//! (parking_lot feature conflict). Fine for thousands of entries per project.

use std::path::Path;
use std::sync::Mutex;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Text};

use crate::{Entry, Error, Hit, Kind, Result, VectorStore, entry_id};

fn err(e: impl std::fmt::Display) -> Error {
    Error(format!("sqlite-vec: {e}"))
}

pub struct SqliteVectorStore {
    project: String,
    conn: Mutex<SqliteConnection>,
}

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    text: String,
    #[diesel(sql_type = Binary)]
    vec: Vec<u8>,
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

impl SqliteVectorStore {
    /// Opens the store file at `path`, scoped to `project`.
    pub fn open(path: &Path, project: &str) -> Result<Self> {
        let mut conn = SqliteConnection::establish(&path.to_string_lossy()).map_err(err)?;
        conn.batch_execute(
            "CREATE TABLE IF NOT EXISTS mem_vec (
               project TEXT NOT NULL, id INTEGER NOT NULL, kind TEXT NOT NULL,
               text TEXT NOT NULL, vec BLOB NOT NULL, PRIMARY KEY (project, id));",
        )
        .map_err(err)?;
        Ok(Self {
            project: project.to_owned(),
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, SqliteConnection>> {
        self.conn
            .lock()
            .map_err(|_| Error("sqlite-vec: poisoned lock".into()))
    }
}

impl VectorStore for SqliteVectorStore {
    fn upsert(&self, entry: &Entry, vector: Vec<f32>) -> Result<()> {
        if entry.project != self.project {
            return Err(Error("entry belongs to another project".into()));
        }
        diesel::sql_query(
            "INSERT OR REPLACE INTO mem_vec (project, id, kind, text, vec) VALUES (?, ?, ?, ?, ?)",
        )
        .bind::<Text, _>(&entry.project)
        .bind::<BigInt, _>(entry_id(entry) as i64)
        .bind::<Text, _>(entry.kind.as_str())
        .bind::<Text, _>(&entry.text)
        .bind::<Binary, _>(to_blob(&vector))
        .execute(&mut *self.lock()?)
        .map(|_| ())
        .map_err(err)
    }

    fn search(&self, vector: Vec<f32>, limit: usize, min_score: f32) -> Result<Vec<Hit>> {
        let rows: Vec<Row> =
            diesel::sql_query("SELECT kind, text, vec FROM mem_vec WHERE project = ?")
                .bind::<Text, _>(&self.project)
                .load(&mut *self.lock()?)
                .map_err(err)?;
        let mut hits: Vec<Hit> = rows
            .into_iter()
            .filter_map(|r| {
                let score = cosine(&vector, &from_blob(&r.vec));
                let kind = Kind::parse(&r.kind)?;
                (score >= min_score).then(|| Hit {
                    entry: Entry {
                        project: self.project.clone(),
                        kind,
                        text: r.text,
                    },
                    score,
                })
            })
            .collect();
        // Ties broken by text so the order (and the rendered block) is stable.
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.entry.text.cmp(&b.entry.text))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }
}
