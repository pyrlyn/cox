// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Graph memory on plain SQLite: depth limits, cascade, isolation, merge.

use cox_vector_memory::{Entry, GraphStore, Kind, Related, Relation, SqliteGraphStore};

fn fact(project: &str, text: &str) -> Entry {
    Entry {
        project: project.into(),
        kind: Kind::Fact,
        text: text.into(),
    }
}

fn texts(v: &[Related]) -> Vec<(&str, u32)> {
    v.iter().map(|r| (r.entry.text.as_str(), r.depth)).collect()
}

/// Chain a - b - c - d in `project`; returns the ids.
fn chain(g: &SqliteGraphStore, project: &str) -> [u64; 4] {
    let ids = ["a", "b", "c", "d"].map(|t| g.add_node(&fact(project, t)).unwrap());
    g.add_edge(project, ids[0], ids[1], &Relation::DependsOn)
        .unwrap();
    g.add_edge(project, ids[2], ids[1], &Relation::RelatedTo)
        .unwrap();
    g.add_edge(project, ids[2], ids[3], &Relation::WorkedWith)
        .unwrap();
    ids
}

fn open() -> (tempfile::TempDir, SqliteGraphStore) {
    let dir = tempfile::tempdir().unwrap();
    let g = SqliteGraphStore::open(&dir.path().join("graph.sqlite")).unwrap();
    (dir, g)
}

#[test]
fn traversal_respects_depth() {
    let (_d, g) = open();
    let ids = chain(&g, "p");
    assert_eq!(
        texts(&g.neighbors("p", &[ids[0]], 1).unwrap()),
        vec![("b", 1)]
    );
    assert_eq!(
        texts(&g.neighbors("p", &[ids[0]], 2).unwrap()),
        vec![("b", 1), ("c", 2)]
    );
    assert!(g.neighbors("p", &[ids[0]], 0).unwrap().is_empty());
}

#[test]
fn delete_node_cascades_edges() {
    let (_d, g) = open();
    let ids = chain(&g, "p");
    assert_eq!(g.edge_count("p").unwrap(), 3);
    g.delete_node("p", ids[1]).unwrap();
    assert_eq!(g.edge_count("p").unwrap(), 1);
    assert!(g.neighbors("p", &[ids[0]], 2).unwrap().is_empty());
}

#[test]
fn no_cross_project_leak() {
    let (_d, g) = open();
    let a = chain(&g, "p1");
    let b = chain(&g, "p2");
    assert_ne!(a, b, "same text in two projects must get different ids");
    assert!(
        g.add_edge("p1", a[0], b[1], &Relation::RelatedTo).is_err(),
        "composite FK must reject a cross-project edge"
    );
    for r in g.neighbors("p1", &[a[0]], 2).unwrap() {
        assert_eq!(r.entry.project, "p1");
    }
    assert!(g.neighbors("p2", &[a[0]], 2).unwrap().is_empty());
}

#[test]
fn vector_hits_expand_through_graph() {
    use cox_vector_memory::{
        HashEmbedder, RecallConfig, SqliteVectorStore, VectorMemory, render_block_with_related,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteVectorStore::open(&dir.path().join("vec.sqlite"), "p").unwrap();
    let m = VectorMemory::new("p", store, HashEmbedder::new(256), RecallConfig::default());
    let g = SqliteGraphStore::open(&dir.path().join("graph.sqlite")).unwrap();
    let seed = "Payments service uses postgres replicas";
    let near = "Replica lag alerts page the oncall";
    let far = "Oncall rotation is weekly";
    let beyond = "Weekly sync happens on monday";
    m.remember(Kind::Fact, seed).unwrap();
    let ids = [seed, near, far, beyond].map(|t| g.add_node(&fact("p", t)).unwrap());
    g.add_edge("p", ids[0], ids[1], &Relation::RelatedTo)
        .unwrap();
    g.add_edge("p", ids[1], ids[2], &Relation::RelatedTo)
        .unwrap();
    g.add_edge("p", ids[2], ids[3], &Relation::RelatedTo)
        .unwrap();
    let (hits, related) = m
        .recall_with_graph(&g, "postgres replicas for payments", 2)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(texts(&related), vec![(near, 1), (far, 2)]);
    let block = render_block_with_related(&hits, &related).unwrap();
    assert!(block.contains(near) && block.contains(far) && !block.contains(beyond));
}
