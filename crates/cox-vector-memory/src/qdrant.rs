// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! [`VectorStore`] on embedded Qdrant Edge: one on-disk shard per project.

use std::path::Path;

use qdrant_edge::{
    Distance, EdgeConfigBuilder, EdgeShard, EdgeVectorParams, NamedQuery, PointInsertOperations,
    PointOperations, PointStruct, QueryEnum, SearchRequestBuilder, UpdateOperation,
    WithPayloadInterface,
};
use serde_json::json;

use crate::{Entry, Error, Hit, Kind, Result, VectorStore, entry_id, project_dir};

fn err(e: impl std::fmt::Display) -> Error {
    Error(format!("qdrant-edge: {e}"))
}

pub struct QdrantEdgeStore {
    project: String,
    shard: EdgeShard,
}

impl QdrantEdgeStore {
    /// Opens (or creates) the shard for `project` under `root`.
    pub fn open(root: &Path, project: &str, dim: usize) -> Result<Self> {
        let dir = project_dir(root, project);
        let populated = std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some());
        let shard = if populated {
            EdgeShard::load(&dir, None).map_err(err)?
        } else {
            std::fs::create_dir_all(&dir).map_err(err)?;
            let config = EdgeConfigBuilder::new()
                .vector("", EdgeVectorParams::new(dim, Distance::Cosine).build())
                .build();
            EdgeShard::new(&dir, config).map_err(err)?
        };
        Ok(Self { project: project.to_owned(), shard })
    }
}

impl VectorStore for QdrantEdgeStore {
    fn upsert(&self, entry: &Entry, vector: Vec<f32>) -> Result<()> {
        if entry.project != self.project {
            return Err(Error("entry belongs to another project".into()));
        }
        let point = PointStruct::new(
            entry_id(entry),
            vector,
            json!({"project": entry.project, "kind": entry.kind.as_str(), "text": entry.text}),
        );
        self.shard
            .update(UpdateOperation::PointOperation(PointOperations::UpsertPoints(
                PointInsertOperations::PointsList(vec![point.into()]),
            )))
            .map_err(err)
    }

    fn search(&self, vector: Vec<f32>, limit: usize, min_score: f32) -> Result<Vec<Hit>> {
        let req = SearchRequestBuilder::new(
            QueryEnum::Nearest(NamedQuery { query: vector.into(), using: None }),
            limit,
        )
        .score_threshold(min_score)
        .with_payload(WithPayloadInterface::Bool(true))
        .build();
        let points = self.shard.search(req).map_err(err)?;
        Ok(points
            .into_iter()
            .filter_map(|p| {
                let pl = p.payload?.0;
                let s = |k: &str| pl.get(k).and_then(|v| v.as_str()).map(str::to_owned);
                let entry = Entry {
                    project: s("project")?,
                    kind: Kind::parse(&s("kind")?)?,
                    text: s("text")?,
                };
                Some(Hit { entry, score: p.score })
            })
            .collect())
    }

    fn flush(&self) -> Result<()> {
        self.shard.flush().map_err(err)
    }
}
