// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Long-term agent memory as vectors: user facts and past session summaries
//! are embedded, stored per project, and recalled by similarity for a new
//! request. The recalled block is meant to sit right after the stable prompt
//! prefix so the provider prompt cache still covers that prefix.
//!
//! Storage and embeddings sit behind traits ([`VectorStore`], [`Embedder`]).
//! Target store: embedded Qdrant Edge (`qdrant-edge` feature, beta, no
//! server); it cannot build inside the cox workspace yet (parking_lot
//! feature conflict with nucleo), so [`SqliteVectorStore`] is the default.
//! [`HashEmbedder`] is a deterministic offline embedder used in tests
//! and as a no-network fallback; it matches shared words, not meaning.

// Experimental crate: public items are documented in docs/vector-memory.md.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

mod graph;
mod sqlite_vec;
pub use graph::{GraphStore, Related, Relation, SqliteGraphStore};
pub use sqlite_vec::SqliteVectorStore;
#[cfg(feature = "sqlite-vec")]
mod vec_conn;
#[cfg(feature = "sqlite-vec")]
pub use vec_conn::{SqliteVecStore, VecSqliteConnection};

#[cfg(feature = "qdrant-edge")]
mod qdrant;
#[cfg(feature = "qdrant-edge")]
pub use qdrant::QdrantEdgeStore;

/// What a memory entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A durable fact about the user or project.
    Fact,
    /// A summary of a past session.
    SessionSummary,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Fact => "fact",
            Kind::SessionSummary => "session_summary",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(Kind::Fact),
            "session_summary" => Some(Kind::SessionSummary),
            _ => None,
        }
    }
}

/// One stored memory entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub project: String,
    pub kind: Kind,
    pub text: String,
}

/// A recalled entry with its similarity score (cosine, higher is closer).
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub entry: Entry,
    pub score: f32,
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Turns text into a dense vector of a fixed dimension.
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// A vector store scoped to one project.
pub trait VectorStore: Send + Sync {
    /// Inserts or replaces `entry`; the same text and kind map to one id.
    fn upsert(&self, entry: &Entry, vector: Vec<f32>) -> Result<()>;
    /// Top `limit` entries of this project scoring at least `min_score`.
    fn search(&self, vector: Vec<f32>, limit: usize, min_score: f32) -> Result<Vec<Hit>>;
    /// Persists pending writes.
    fn flush(&self) -> Result<()>;
}

/// Stable point id for an entry: identical facts deduplicate on upsert.
pub fn entry_id(entry: &Entry) -> u64 {
    let mut h = Sha256::new();
    h.update(entry.project.as_bytes());
    h.update([0]);
    h.update(entry.kind.as_str().as_bytes());
    h.update([0]);
    h.update(entry.text.as_bytes());
    let d = h.finalize();
    u64::from_le_bytes(d[..8].try_into().expect("8 bytes"))
}

/// Directory of one project's store under `root`: a hash of the project key,
/// so projects never share a shard on disk.
pub fn project_dir(root: &Path, project: &str) -> PathBuf {
    let d = Sha256::digest(project.as_bytes());
    let hex: String = d[..8].iter().map(|b| format!("{b:02x}")).collect();
    root.join(hex)
}

/// Recall settings.
#[derive(Clone, Copy, Debug)]
pub struct RecallConfig {
    pub top_k: usize,
    /// Hits below this cosine score are dropped as irrelevant.
    pub min_score: f32,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            min_score: 0.35,
        }
    }
}

/// Facade the agent uses: remember, recall, render a context block.
pub struct VectorMemory<S, E> {
    project: String,
    store: S,
    embedder: E,
    config: RecallConfig,
}

impl<S: VectorStore, E: Embedder> VectorMemory<S, E> {
    pub fn new(project: impl Into<String>, store: S, embedder: E, config: RecallConfig) -> Self {
        Self {
            project: project.into(),
            store,
            embedder,
            config,
        }
    }

    pub fn remember(&self, kind: Kind, text: &str) -> Result<()> {
        let entry = Entry {
            project: self.project.clone(),
            kind,
            text: text.trim().to_owned(),
        };
        let v = self.embedder.embed(&entry.text)?;
        self.store.upsert(&entry, v)?;
        self.store.flush()
    }

    pub fn recall(&self, query: &str) -> Result<Vec<Hit>> {
        let v = self.embedder.embed(query)?;
        let hits = self
            .store
            .search(v, self.config.top_k, self.config.min_score)?;
        // Defence in depth: the store is already per project.
        Ok(hits
            .into_iter()
            .filter(|h| h.entry.project == self.project)
            .collect())
    }

    /// Vector recall, then graph expansion from the hit nodes up to
    /// `max_depth` hops (1-2 recommended). Related entries already in the
    /// hits are dropped so nothing is injected twice.
    pub fn recall_with_graph<G: GraphStore>(
        &self,
        graph: &G,
        query: &str,
        max_depth: u32,
    ) -> Result<(Vec<Hit>, Vec<Related>)> {
        let hits = self.recall(query)?;
        let seeds: Vec<u64> = hits.iter().map(|h| entry_id(&h.entry)).collect();
        let related = graph
            .neighbors(&self.project, &seeds, max_depth)?
            .into_iter()
            .filter(|r| r.entry.project == self.project)
            .filter(|r| !hits.iter().any(|h| h.entry == r.entry))
            .collect();
        Ok((hits, related))
    }

    /// The block to place after the stable prompt prefix; `None` when nothing
    /// relevant was found, so the request stays byte-identical to no memory.
    pub fn context_block(&self, query: &str) -> Result<Option<String>> {
        Ok(render_block(&self.recall(query)?))
    }
}

/// [`render_block`] plus a section of graph-related entries.
pub fn render_block_with_related(hits: &[Hit], related: &[Related]) -> Option<String> {
    let mut out = render_block(hits)?;
    if !related.is_empty() {
        out.truncate(out.len() - "</long_term_memory>".len());
        out.push_str("related:\n");
        for r in related {
            out.push_str(&format!("- [{}] {}\n", r.entry.kind.as_str(), r.entry.text));
        }
        out.push_str("</long_term_memory>");
    }
    Some(out)
}

/// Renders hits in a fixed order and format so equal hits give equal bytes.
pub fn render_block(hits: &[Hit]) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let mut out = String::from("<long_term_memory>\n");
    for h in hits {
        out.push_str(&format!("- [{}] {}\n", h.entry.kind.as_str(), h.entry.text));
    }
    out.push_str("</long_term_memory>");
    Some(out)
}

/// Deterministic offline embedder: hashed bag of lowercase words, L2
/// normalised. Shares no meaning across synonyms; for tests and fallback.
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = vec![0f32; self.dim];
        for w in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
        {
            let d = Sha256::digest(w.to_lowercase().as_bytes());
            let i = u64::from_le_bytes(d[..8].try_into().expect("8 bytes")) as usize % self.dim;
            v[i] += 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter_mut().for_each(|x| *x /= n);
        }
        Ok(v)
    }
}
