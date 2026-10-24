# Long-term memory: vectors and graph (experimental)

Crate: `crates/cox-vector-memory`. Status: **experimental, not wired into the
session loop yet**. The storage backend is beta and may change.

## How it works

1. **Remember.** A user fact or a past session summary is embedded and stored
   as a point in an embedded [Qdrant Edge](https://qdrant.tech/edge/) shard
   (`qdrant-edge = "=0.9.0"`, Apache-2.0, in-process, no server), or in the
   SQLite fallback (see "Backends" below). Each project
   gets its own on-disk shard (`<root>/<sha256(project)[..8]>`), and every point
   also carries `project` in its payload. The point id is a hash of
   project + kind + text, so the same fact is stored once.
2. **Recall.** The new request is embedded and searched by cosine similarity.
   Top-k hits (default 5) below `min_score` (default 0.35) are dropped, so an
   unrelated request injects nothing.
3. **Graph expansion.** Relations between entries (`depends_on`, `related_to`,
   `worked_with`, custom) live in plain SQLite tables `mem_node` / `mem_edge`.
   Nodes use the same ids as the Qdrant points. From the vector hits a
   depth-limited `WITH RECURSIVE` walk (1-2 hops, direction ignored) adds
   related entries. Edges have composite foreign keys `(project, id)` with
   `ON DELETE CASCADE`, so deleting a node deletes its edges and an edge can
   never join two projects.
4. **Inject.** `render_block` / `render_block_with_related` produce a
   `<long_term_memory>` block in a fixed order. Put it **after** the stable
   system prompt and tool catalog so the provider prompt cache still covers
   the prefix. With no hits the block is `None` and the request is unchanged.

Storage and embeddings are behind traits: `VectorStore` (impl
`QdrantEdgeStore`), `GraphStore` (impl `SqliteGraphStore`), `Embedder`.
`HashEmbedder` is a deterministic, offline bag-of-words embedder used in tests
and as a no-network fallback; it matches shared words, not meaning. cox has no
embeddings API yet; a provider or local (LM Studio / nomic-embed) embedder
plugs in through `Embedder`.

## Backends and the Qdrant Edge conflict

`qdrant-edge` 0.9.0 always enables parking_lot `deadlock_detection`; cox
already enables `send_guard` through `nucleo`. parking_lot refuses both at
once, so Qdrant Edge **cannot build inside the cox workspace today**. Hence:

- Default vector backend: `SqliteVectorStore` (vectors as f32 blobs, exact
  cosine scan per project). It is **not Qdrant**; fine for thousands of
  entries per project.
- `QdrantEdgeStore` stays behind the opt-in `qdrant-edge` feature (off) and
  is excluded from hakari (`.config/hakari.toml`). It passes the same test
  suite when the crate is built standalone, outside the workspace.
- Unblock: upstream making `deadlock_detection` optional in qdrant-edge, or
  cox dropping `send_guard` from nucleo's path.

## Enable / disable

The crate is not used by any binary yet, so memory is off. Backends:
`SqliteVectorStore` (always built), `QdrantEdgeStore` (`qdrant-edge`
feature, standalone only for now).

Wiring into the session (config flag, memory tool integration, context
assembly) is a follow-up; see the PR.

## Testing

```sh
cargo nextest run -p cox-vector-memory
# or
cargo test -p cox-vector-memory
```

The vector suite runs once per backend (`sqlite_backend`, and
`qdrant_edge_backend` with `--features qdrant-edge` outside the workspace).
Covered: a fact survives closing and reopening the shard; similarity ranks the
relevant fact first; an unrelated query surfaces nothing (score threshold);
no cross-project leak (vector and graph); duplicate facts deduplicate; the
rendered block is byte-stable; traversal returns depth 1-2 and not beyond;
node delete cascades its edges; vector hits expand through the graph.

## Limitations

- Qdrant Edge is **beta** (since Aug 2026); the API may change. It sits behind
  `VectorStore` so it can be swapped. Backend choice is not final (TBD).
- `qdrant-client` was not used: it is a gRPC client that needs a server.
- `HashEmbedder` is lexical only. Real semantic recall needs a real embedder.
- Graph: the `sqlite-graph` extension was evaluated and not used: alpha
  (v0.1.0-alpha), last push Dec 2025, no variable-length paths, and it needs a
  loadable C extension. Plain SQLite + recursive CTE covers 1-2 hop walks.
  Future alternatives to watch: LadybugDB (`lbug`), `agdb`.
- The graph stores a copy of kind and text so traversal does not need a
  Qdrant lookup; both stores must be written together.

## Future graph backends

Current graph memory is stored in plain SQLite tables (`nodes`, `edges`) and
traversed with recursive CTEs via diesel, behind the `GraphStore` trait.

[Grafeo](https://github.com/GrafeoDB/grafeo) (crate `grafeo` 0.6.0, released
2026-10-23) is a candidate second backend: pure Rust, embedded library mode
(no server), Apache-2.0, GQL/Cypher queries, built-in vector search. It would
be added behind a cargo feature flag once it reaches 1.0 or gains a second
active maintainer (currently almost all commits are by one author).

There is no implementation change for now; SQLite remains the only backend.

## sqlite-vec backend (`sqlite-vec` feature)

Why `load_extension` is not used: diesel 2.3 does not expose the raw
`sqlite3*` handle of a `SqliteConnection`, so `sqlite3_enable_load_extension`
and `sqlite3_load_extension` cannot be called on it, and shipping a separate
`vec0.dylib` would be fragile anyway.

How it works: the `sqlite-vec` crate (=0.1.6) compiles sqlite-vec into the
binary against the same bundled `libsqlite3-sys` that diesel uses (one SQLite
symbol set). `VecSqliteConnection::establish` registers `sqlite3_vec_init`
once per process with `sqlite3_auto_extension`, so SQLite initialises the
extension on every connection opened afterwards, then checks
`SELECT vec_version()`. Dynamic extension loading stays disabled. The only
`unsafe` block is in `vec_conn::register` (scoped `#[allow(unsafe_code)]`).
`VecSqliteConnection` derefs to `SqliteConnection`, so it is used with diesel
as is.

`SqliteVecStore` implements `VectorStore` on a `vec0` table with
`distance_metric=cosine` and a `project` partition key; entry metadata lives in
`mem_vec0_meta` keyed by the same rowid.

- Enable: `cargo build -p cox-vector-memory --features sqlite-vec`.
- Disable: omit the feature (default); `SqliteVectorStore` (blob scan) stays.
- Test: `cargo test -p cox-vector-memory --features sqlite-vec` (vec_version,
  vec0 KNN order, 8 threads on separate connections, store upsert/search/
  project isolation).

Limitations: the dimension is fixed per database file (`float[dim]`); KNN in
vec0 0.1.6 is brute force (no ANN index); sqlite-vec is pre-1.0; registration
is process-wide, so every SQLite connection in the process gets the extension;
not yet wired into the agent.
