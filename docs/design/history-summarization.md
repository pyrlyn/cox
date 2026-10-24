# History summarization

Status: implemented in this PR (draft); see `docs/how-it-works.md`. Branch: `feat/history-summarization`, from
`origin/main` (not stacked on `feat/agent-context-caching`: it does not need
the MCP memo; the context blocks it builds on are already on `main`).

## What exists on main

- `cox-core/src/compact.rs` (T8.1, T28.3): when a request crosses
  `[context] compact_at` (fraction of the model window, default 0.75) every
  turn but the last `keep_turns` (default 2) is replaced by one summary from
  the `compact` job. Append-only: the rollout keeps the originals and
  `Compacted.dropped` names the turns a rebuild skips.
- The `compact` job is routed through `router.rs` (`Job::Compact`), whose
  default tier is the low-cost one (`[jobs]` config).
- The summary already carries structured sections (files, errors, request,
  todos) built from the transcript, not from the model's word alone.

## What this PR adds

1. **Threshold**: keep `compact_at`; add an absolute
   `compact_at_tokens` override (whichever is reached first), documented.
2. **Placement**: Ivan asked for the summary "in the system prompt". Putting
   it in `system[0..=2]` would change the cached prefix on every compaction
   and invalidate the cache for the whole tool list. So the summary is a
   block **right after the stable prefix** (system + tool schemas), first in
   history, with its own breakpoint: the prefix stays a cache hit, and the
   summary itself becomes the next cached segment.
3. **Prompt**: the compact prompt asks explicitly for facts, decisions and
   open tasks in fixed sections, so a later step can parse and carry them.
4. **Cross-session brief**: on compaction the summary is stored in
   `cox-store` (`session_briefs`), keyed by the canonical workspace root (full
   git-root path, not the folder name). A new top-level session in the same
   root gets it, capped, in the volatile `system[3]` block right after the
   prefix breakpoint. A different root never reads it. Config: `[context]
   session_brief = true|false`, `session_brief_chars`. Not saved on session
   end, only on compaction.

## Tests (stub model, no network)

- fixture conversation with known facts/decisions/todos -> summary keeps them;
- old messages are evicted from the next request, last `keep_turns` verbatim;
- next request and a new session in the same project see the summary;
- a session in another project sees nothing (negative test, foreign brief);
- stable prefix bytes and breakpoints unchanged by compaction.

## Expected savings (estimate)

For a session that would otherwise run at the window limit, sending a ~2k
summary plus two turns instead of ~100k+ history cuts input per request by
roughly 80–95% after compaction (estimate, not measured).
