# Agent context caching

Status: in progress (draft PR). Savings figures are **estimates**.

cox is an agent that owns its context, so token savings come from four layers.
This note maps each layer onto what exists and names the gap this branch closes.

**Top criterion: a cache is never served to the wrong request.** Every layer
keys on the full content that determines the answer, and each has negative
tests that fail on a foreign hit.

## 1. MCP tool-result memoization (new)

- Where: `cox-mcp/src/memo.rs`, used by `McpTool::call` (`cox-mcp/src/client.rs`).
- Key: server + tool + canonical JSON of the arguments (object keys sorted
  recursively; values, types and array order are significant).
- Only approved tools whose effective risk is `ReadOnly` are memoized;
  write/destructive or pending-contract tools never are. Errors never stored.
- Bounds: per-server TTL and entry cap; oldest entry evicted first.
- Config: `[mcp_servers.<name>] memo_ttl_secs` (default 0 = off) and
  `memo_max_entries` (default 128). Opt-in because a read-only tool can still
  return live data.
- Scope: one memo per connected server per session, never shared across
  sessions/conversations.
- Estimate: fewer external calls (10–30% in doc-heavy sessions); with re-read
  dedup (`dedup.rs`) an identical result becomes a pointer, not a resend.

## 2. Context blocks (exists)

`cox-core/src/context.rs` builds every request from ordered blocks:
`system[0]` base prompt, `system[1]` instructions, `system[2]` project
context/repo map/skills (prefix breakpoint), `system[3]` volatile rules and
style, tool schemas sorted by name, history with rolling breakpoints.
Blocks are rebuilt from session state each request (no cross-conversation
store), so reuse happens only in the provider cache, which is content-keyed.

## 3. History summarization

Moved to its own draft PR (#487, `docs/design/history-summarization.md`).

## 4. Provider prompt cache (exists)

`cox-provider-anthropic/src/request.rs` places up to four `cache_control`
breakpoints; OpenAI caches the same byte-stable prefix automatically. No
timestamps or ids in `system[0..=2]` (snapshot-tested).

## Tests added

- memo: identical request hits (underlying call count 1); different tool,
  argument value, type, nesting, array order, server -> miss and fresh call;
  key-order-only difference -> hit; TTL expiry and cap eviction -> fresh call;
  mutating/pending tools never cached; error results never cached.
- context blocks are rebuilt from the session's own state per request and
  are already covered by `context.rs` snapshot tests; no new cache store, so
  no new isolation test is needed there.

## Bench

`cargo test -p cox-mcp --test client bench -- --ignored --nocapture`: 40
calls over 10 distinct requests, fake server with 20 ms per call
(Mac, 2026-10-24, one run):

| memo | server calls | wall time | model tokens |
| --- | --- | --- | --- |
| off (`memo_ttl_secs = 0`) | 40 | 888 ms | baseline |
| on (`memo_ttl_secs = 300`) | 10 | 223 ms | same as off |

The memo returns the same text the server would, so it does not change what
the model reads: its saving is external calls and latency (-75% each here).
Model tokens are saved by the prefix cache (layers 2 and 4) and by re-read
dedup, which this branch does not change.

Real-traffic hit rate: not available. The local `~/.cox` has 10 rollouts
and none has an MCP call, and the `usage` ledger in `cox.db` is empty, so
there is nothing real to replay. Nothing is estimated in its place.

## How to test

`cargo nextest run -p cox-mcp memo` (unit + integration, including the
negative foreign-hit tests).

## Limitations

- Off by default; a read-only tool can still return live data, so pick a TTL.
- Memory only, per server, per session; not shared or persisted.
- Only tools whose approved contract says `readOnlyHint` are memoized.

## Expected savings (estimate)

Stable prefix ~15–25k tokens, cache reads ~10% of input price: a 20-request
session saves roughly 70–85% of prefix input tokens versus no caching
(estimate). Layers 2–4 already earn that; layer 1 saves external API calls.
