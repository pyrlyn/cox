// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Embedded Qdrant Edge memory: persistence, relevance, threshold, isolation.

#![cfg(feature = "qdrant-edge")]

use cox_vector_memory::{
    Embedder, HashEmbedder, Kind, QdrantEdgeStore, RecallConfig, VectorMemory, render_block,
};

const DIM: usize = 256;

fn memory(root: &std::path::Path, project: &str) -> VectorMemory<QdrantEdgeStore, HashEmbedder> {
    let store = QdrantEdgeStore::open(root, project, DIM).expect("open store");
    VectorMemory::new(project, store, HashEmbedder::new(DIM), RecallConfig::default())
}

#[test]
fn fact_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let m = memory(dir.path(), "proj-a");
        m.remember(Kind::Fact, "User prefers rust nextest for running tests").unwrap();
    }
    let m = memory(dir.path(), "proj-a");
    let hits = m.recall("how should tests run, nextest?").unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].entry.text.contains("nextest"));
}

#[test]
fn similarity_ranks_relevant_first() {
    let dir = tempfile::tempdir().unwrap();
    let m = memory(dir.path(), "proj-a");
    m.remember(Kind::Fact, "Database migrations use diesel with sqlite").unwrap();
    m.remember(Kind::Fact, "Release builds are signed on macOS runners").unwrap();
    m.remember(Kind::SessionSummary, "Fixed hakari drift in workspace hack").unwrap();
    let hits = m.recall("add a diesel migration for sqlite table").unwrap();
    assert!(!hits.is_empty());
    assert!(hits[0].entry.text.contains("diesel"), "{hits:?}");
}

#[test]
fn irrelevant_does_not_surface() {
    let dir = tempfile::tempdir().unwrap();
    let m = memory(dir.path(), "proj-a");
    m.remember(Kind::Fact, "Database migrations use diesel with sqlite").unwrap();
    let hits = m.recall("weather forecast tomorrow kyiv").unwrap();
    assert!(hits.is_empty(), "unrelated query leaked: {hits:?}");
    assert_eq!(m.context_block("weather forecast tomorrow kyiv").unwrap(), None);
}

#[test]
fn no_cross_project_leak() {
    let dir = tempfile::tempdir().unwrap();
    let a = memory(dir.path(), "proj-a");
    let b = memory(dir.path(), "proj-b");
    a.remember(Kind::Fact, "Secret token rotation happens every friday").unwrap();
    let hits = b.recall("secret token rotation friday").unwrap();
    assert!(hits.is_empty(), "project b saw project a: {hits:?}");
    assert_eq!(a.recall("secret token rotation friday").unwrap().len(), 1);
}

#[test]
fn duplicate_fact_is_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let m = memory(dir.path(), "proj-a");
    m.remember(Kind::Fact, "Commit lines stay under 100 chars").unwrap();
    m.remember(Kind::Fact, "Commit lines stay under 100 chars").unwrap();
    assert_eq!(m.recall("commit lines chars").unwrap().len(), 1);
}

#[test]
fn block_is_byte_stable() {
    let dir = tempfile::tempdir().unwrap();
    let m = memory(dir.path(), "proj-a");
    m.remember(Kind::Fact, "Docs live under docs directory in english").unwrap();
    let one = m.context_block("where do docs live").unwrap();
    let two = m.context_block("where do docs live").unwrap();
    assert!(one.is_some());
    assert_eq!(one, two);
    assert_eq!(render_block(&[]), None);
    assert_eq!(HashEmbedder::new(DIM).dim(), DIM);
}
