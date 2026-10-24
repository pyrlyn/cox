# Long-term memory: vectors and graph (experimental)

Crate: `crates/cox-vector-memory`. Status: **experimental, not wired into the
session loop yet**. The storage backend is beta and may change.

## How it works

1. **Remember.** A user fact or a past session summary is embedded and stored
   as a point in an embedded [Qdrant Edge](https://qdrant.tech/edge/) shard
   (`qdrant-edge = "=0.9.0"`, Apache-2.0, in-process, no server). Each project
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

## Enable / disable

The crate is not used by any binary yet, so it is off by default. Backend:

- `qdrant-edge` cargo feature (default on) builds `QdrantEdgeStore`.
- `cargo build -p cox-vector-memory --no-default-features` drops Qdrant Edge
  (graph and traits only).

Wiring into the session (config flag, memory tool integration, context
assembly) is a follow-up; see the PR.

## Testing

```sh
cargo nextest run -p cox-vector-memory
# or
cargo test -p cox-vector-memory
```

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
