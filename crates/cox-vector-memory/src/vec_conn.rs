// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Diesel connection with the sqlite-vec extension (`vec0`, `vec_distance_*`).
//!
//! Diesel 2.3 does not expose the raw `sqlite3*` handle, so
//! `sqlite3_enable_load_extension` + `sqlite3_load_extension` cannot be called
//! on a diesel connection, and we do not want to ship a `.dylib` anyway.
//! Instead sqlite-vec is compiled in (the `sqlite-vec` crate) and its
//! `sqlite3_vec_init` is registered once per process with
//! `sqlite3_auto_extension`; SQLite then runs it on every connection opened
//! afterwards. `load_extension` stays disabled the whole time. All unsafe code
//! of this crate lives in [`register`].

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Float, Text};

use crate::{Entry, Error, Hit, Kind, Result, VectorStore, entry_id};

fn err(e: impl std::fmt::Display) -> Error {
    Error(format!("sqlite-vec: {e}"))
}

/// Registers sqlite-vec as an auto extension exactly once per process.
// The only unsafe in this crate; scoped here instead of crate-wide.
#[allow(unsafe_code)]
fn register() -> Result<()> {
    static RC: OnceLock<i32> = OnceLock::new();
    let rc = *RC.get_or_init(|| {
        type Init = unsafe extern "C" fn(
            *mut libsqlite3_sys::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const libsqlite3_sys::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        // SAFETY: `sqlite3_vec_init` is a C function with exactly the
        // extension-entry signature above (sqlite-vec is built with
        // SQLITE_CORE, so the api pointer is unused). Both it and
        // `sqlite3_auto_extension` link against the one libsqlite3-sys that
        // diesel uses, so there is a single SQLite symbol set. The pointer is
        // 'static and auto_extension is thread-safe and called once here.
        unsafe {
            let init: Init = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
            libsqlite3_sys::sqlite3_auto_extension(Some(init))
        }
    });
    if rc == libsqlite3_sys::SQLITE_OK {
        Ok(())
    } else {
        Err(Error(format!(
            "sqlite-vec: sqlite3_auto_extension failed: {rc}"
        )))
    }
}

/// A [`SqliteConnection`] guaranteed to have sqlite-vec loaded.
pub struct VecSqliteConnection(SqliteConnection);

impl VecSqliteConnection {
    pub fn establish(url: &str) -> Result<Self> {
        register()?;
        let mut c = SqliteConnection::establish(url).map_err(err)?;
        // Wait on concurrent writers instead of failing with SQLITE_BUSY.
        c.batch_execute("PRAGMA busy_timeout = 5000; SELECT vec_version();")
            .map_err(err)?;
        Ok(Self(c))
    }

    /// sqlite-vec version string, e.g. `v0.1.6`.
    pub fn vec_version(&mut self) -> Result<String> {
        #[derive(QueryableByName)]
        struct V {
            #[diesel(sql_type = Text)]
            v: String,
        }
        let r: V = diesel::sql_query("SELECT vec_version() AS v")
            .get_result(&mut self.0)
            .map_err(err)?;
        Ok(r.v)
    }

    pub fn into_inner(self) -> SqliteConnection {
        self.0
    }
}

impl Deref for VecSqliteConnection {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        &self.0
    }
}

impl DerefMut for VecSqliteConnection {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        &mut self.0
    }
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// [`VectorStore`] on a `vec0` virtual table (cosine KNN inside sqlite-vec).
pub struct SqliteVecStore {
    project: String,
    conn: Mutex<VecSqliteConnection>,
}

impl SqliteVecStore {
    /// Opens `path`; `dim` is fixed per file (vec0 column `float[dim]`).
    pub fn open(path: &Path, project: &str, dim: usize) -> Result<Self> {
        let mut conn = VecSqliteConnection::establish(&path.to_string_lossy())?;
        conn.batch_execute(&format!(
            "CREATE TABLE IF NOT EXISTS mem_vec0_meta (
               rowid INTEGER PRIMARY KEY, project TEXT NOT NULL, id INTEGER NOT NULL,
               kind TEXT NOT NULL, text TEXT NOT NULL, UNIQUE (project, id));
             CREATE VIRTUAL TABLE IF NOT EXISTS mem_vec0 USING vec0(
               project TEXT partition key, embedding float[{dim}] distance_metric=cosine);"
        ))
        .map_err(err)?;
        Ok(Self {
            project: project.to_owned(),
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, VecSqliteConnection>> {
        self.conn
            .lock()
            .map_err(|_| Error("sqlite-vec: poisoned lock".into()))
    }
}

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    text: String,
    #[diesel(sql_type = Float)]
    distance: f32,
}

#[derive(QueryableByName)]
struct RowId {
    #[diesel(sql_type = BigInt)]
    rowid: i64,
}

impl VectorStore for SqliteVecStore {
    fn upsert(&self, entry: &Entry, vector: Vec<f32>) -> Result<()> {
        if entry.project != self.project {
            return Err(Error("entry belongs to another project".into()));
        }
        let mut c = self.lock()?;
        c.transaction(|c| {
            let r: RowId = diesel::sql_query(
                "INSERT INTO mem_vec0_meta (project, id, kind, text) VALUES (?, ?, ?, ?)
                 ON CONFLICT (project, id) DO UPDATE SET text = excluded.text
                 RETURNING rowid",
            )
            .bind::<Text, _>(&entry.project)
            .bind::<BigInt, _>(entry_id(entry) as i64)
            .bind::<Text, _>(entry.kind.as_str())
            .bind::<Text, _>(&entry.text)
            .get_result(c)?;
            diesel::sql_query("DELETE FROM mem_vec0 WHERE rowid = ?")
                .bind::<BigInt, _>(r.rowid)
                .execute(c)?;
            diesel::sql_query("INSERT INTO mem_vec0 (rowid, project, embedding) VALUES (?, ?, ?)")
                .bind::<BigInt, _>(r.rowid)
                .bind::<Text, _>(&entry.project)
                .bind::<Binary, _>(to_blob(&vector))
                .execute(c)
                .map(|_| ())
        })
        .map_err(err)
    }

    fn search(&self, vector: Vec<f32>, limit: usize, min_score: f32) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows: Vec<Row> = diesel::sql_query(
            "SELECT m.kind, m.text, v.distance FROM mem_vec0 v
             JOIN mem_vec0_meta m ON m.rowid = v.rowid
             WHERE v.embedding MATCH ? AND v.k = ? AND v.project = ?
             ORDER BY v.distance, m.text",
        )
        .bind::<Binary, _>(to_blob(&vector))
        .bind::<BigInt, _>(limit as i64)
        .bind::<Text, _>(&self.project)
        .load(&mut **self.lock()?)
        .map_err(err)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let score = 1.0 - r.distance;
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
            .collect())
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(QueryableByName)]
    struct Id {
        #[diesel(sql_type = BigInt)]
        rowid: i64,
    }

    fn fresh() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("v.db").to_string_lossy().into_owned();
        (d, p)
    }

    #[test]
    fn extension_loaded() {
        let mut c = VecSqliteConnection::establish(":memory:").unwrap();
        assert!(c.vec_version().unwrap().starts_with('v'));
    }

    #[test]
    fn vec0_knn_returns_nearest() {
        let mut c = VecSqliteConnection::establish(":memory:").unwrap();
        c.batch_execute("CREATE VIRTUAL TABLE t USING vec0(e float[3]);")
            .unwrap();
        for (id, v) in [
            (1, [1.0, 0.0, 0.0]),
            (2, [0.0, 1.0, 0.0]),
            (3, [0.0, 0.0, 1.0]),
        ] {
            diesel::sql_query("INSERT INTO t (rowid, e) VALUES (?, ?)")
                .bind::<BigInt, _>(id)
                .bind::<Binary, _>(to_blob(&v))
                .execute(&mut *c)
                .unwrap();
        }
        let r: Vec<Id> =
            diesel::sql_query("SELECT rowid FROM t WHERE e MATCH ? AND k = 2 ORDER BY distance")
                .bind::<Binary, _>(to_blob(&[0.1, 0.9, 0.0]))
                .load(&mut *c)
                .unwrap();
        assert_eq!(r.iter().map(|x| x.rowid).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn many_connections_and_threads() {
        let (_d, p) = fresh();
        {
            let mut c = VecSqliteConnection::establish(&p).unwrap();
            c.batch_execute("CREATE VIRTUAL TABLE t USING vec0(e float[2]);")
                .unwrap();
        }
        let hs: Vec<_> = (0..8i64)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let mut c = VecSqliteConnection::establish(&p).unwrap();
                    c.batch_execute("PRAGMA busy_timeout = 5000;").unwrap();
                    assert!(c.vec_version().is_ok());
                    diesel::sql_query("INSERT INTO t (rowid, e) VALUES (?, ?)")
                        .bind::<BigInt, _>(i + 1)
                        .bind::<Binary, _>(to_blob(&[i as f32, 1.0]))
                        .execute(&mut *c)
                        .unwrap();
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let mut c = VecSqliteConnection::establish(&p).unwrap();
        let r: Vec<Id> =
            diesel::sql_query("SELECT rowid FROM t WHERE e MATCH ? AND k = 1 ORDER BY distance")
                .bind::<Binary, _>(to_blob(&[5.0, 1.0]))
                .load(&mut *c)
                .unwrap();
        assert_eq!(r[0].rowid, 6);
    }

    #[test]
    fn store_upsert_search_and_isolation() {
        let (_d, p) = fresh();
        let a = SqliteVecStore::open(Path::new(&p), "a", 2).unwrap();
        let b = SqliteVecStore::open(Path::new(&p), "b", 2).unwrap();
        let e = |pr: &str, t: &str| Entry {
            project: pr.into(),
            kind: Kind::parse("fact").unwrap(),
            text: t.into(),
        };
        a.upsert(&e("a", "x"), vec![1.0, 0.0]).unwrap();
        a.upsert(&e("a", "y"), vec![0.0, 1.0]).unwrap();
        a.upsert(&e("a", "y"), vec![0.0, 1.0]).unwrap();
        b.upsert(&e("b", "z"), vec![1.0, 0.0]).unwrap();
        let h = a.search(vec![0.9, 0.1], 5, 0.0).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].entry.text, "x");
        assert!(h[0].score > 0.9);
        assert_eq!(b.search(vec![1.0, 0.0], 5, 0.0).unwrap().len(), 1);
    }
}
