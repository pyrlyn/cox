// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One vector-memory suite run against every backend: persistence,
//! relevance, score threshold, project isolation, dedup, stable rendering.

use std::path::Path;

use cox_vector_memory::{
    Embedder, HashEmbedder, Kind, RecallConfig, VectorMemory, VectorStore, render_block,
};

const DIM: usize = 256;

fn suite<S: VectorStore>(open: impl Fn(&Path, &str) -> S) {
    let mem = |root: &Path, p: &str| {
        VectorMemory::new(
            p,
            open(root, p),
            HashEmbedder::new(DIM),
            RecallConfig::default(),
        )
    };

    // Fact persists across close and reopen.
    let dir = tempfile::tempdir().unwrap();
    {
        let m = mem(dir.path(), "proj-a");
        m.remember(Kind::Fact, "User prefers rust nextest for running tests")
            .unwrap();
    }
    let m = mem(dir.path(), "proj-a");
    let hits = m.recall("run tests with nextest in rust").unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].entry.text.contains("nextest"));

    // Similarity ranks the relevant fact first.
    let dir = tempfile::tempdir().unwrap();
    let m = mem(dir.path(), "proj-a");
    m.remember(Kind::Fact, "Database migrations use diesel with sqlite")
        .unwrap();
    m.remember(Kind::Fact, "Release builds are signed on macOS runners")
        .unwrap();
    m.remember(Kind::SessionSummary, "Fixed hakari drift in workspace hack")
        .unwrap();
    let hits = m.recall("diesel migrations with sqlite table").unwrap();
    assert!(hits[0].entry.text.contains("diesel"), "{hits:?}");

    // Irrelevant does not surface (score threshold), block stays None.
    let q = "weather forecast tomorrow kyiv";
    assert!(m.recall(q).unwrap().is_empty());
    assert_eq!(m.context_block(q).unwrap(), None);

    // No cross-project leak, same root.
    let a = mem(dir.path(), "proj-x");
    let b = mem(dir.path(), "proj-y");
    a.remember(Kind::Fact, "Secret token rotation happens every friday")
        .unwrap();
    assert!(b.recall("secret token rotation friday").unwrap().is_empty());
    assert_eq!(a.recall("secret token rotation friday").unwrap().len(), 1);

    // Duplicate fact deduplicates.
    m.remember(Kind::Fact, "Commit lines stay under 100 chars")
        .unwrap();
    m.remember(Kind::Fact, "Commit lines stay under 100 chars")
        .unwrap();
    assert_eq!(m.recall("commit lines chars").unwrap().len(), 1);

    // Rendered block is byte-stable.
    let one = m.context_block("diesel sqlite migrations").unwrap();
    assert!(one.is_some());
    assert_eq!(one, m.context_block("diesel sqlite migrations").unwrap());
    assert_eq!(render_block(&[]), None);
    assert_eq!(HashEmbedder::new(DIM).dim(), DIM);
}

#[test]
fn sqlite_backend() {
    suite(|root, p| {
        cox_vector_memory::SqliteVectorStore::open(&root.join("vec.sqlite"), p).unwrap()
    });
}

#[cfg(feature = "qdrant-edge")]
#[test]
fn qdrant_edge_backend() {
    suite(|root, p| cox_vector_memory::QdrantEdgeStore::open(root, p, DIM).unwrap());
}
