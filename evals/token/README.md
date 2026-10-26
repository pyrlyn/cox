# Token-economy bench (T8.5)

Measures each D6 mechanism's savings in **context-token-turns** (sum of
`Usage::context_tokens` over every provider call of a replay).

## Run

```bash
just bench
```

Offline, no network, no API key. Replays 5 recorded sessions 6 times each
(baseline + one run per mechanism with that mechanism disabled); a few
seconds on a laptop.

## Method

`crates/cox/examples/bench.rs` replays every transcript through the real
`Session` loop: `Scripted` provider fed with specs generated from the
transcript, real `read`/`grep`/`glob` tools over `workspace/`, the two
never-called edit tools (`edit`, `apply_patch`) so the `edit_tools` toggle
has schemas to drop, plus two never-called deferred tools (`web_fetch`,
`ask_user`) so the deferred toggle has schemas to include. The loop writes real ledger rows to a
`MemoryStore`; the bench sums them. Toggles are the real config flags:

| mechanism | disabled by |
|---|---|
| archive (D6a) | `tool_output_visible_bytes = u32::MAX`, head/tail 1M |
| dedup (D6b) | `dedup_window_turns = 0` (live table and history `read` dedup) |
| outline (D6c) | transcript `read` calls rewritten `outline` → `text` |
| deferred (D6d) | `deferred_tools = false` |
| compaction (D6f) | `compact_at = 1.0`, `microcompact_after_turns = u32::MAX`, no `/compact` |
| microcompact6 | `microcompact_after_turns = 6` (the default before it was 4) |
| edit-tools-both | `edit_tools = "both"` (both edit schemas in every request) |
| prefix (D6e) | **emulated** (see below) |

Baseline is shipped defaults plus one `/compact` after the 4th turn (the
`Scripted` provider cannot return a context-length error, so auto-compact
never fires offline; the manual compact runs the real `compact()` code).

`prefix` cannot be observed offline: the `Scripted` provider reports no
cache usage, so stable vs unstable order costs the same tokens. Instead the
bench counts the stable-prefix bytes (`assemble` output minus an empty
system, via the real `estimate`) that an unstable order would force the
server to re-receive on every call after the first. It is cache-write
volume, not context tokens — read it as an upper bound, not a bill.

## Sessions

`sessions/*.jsonl`: one JSON object per line. The first line carries the
`summary` text consumed by the baseline `/compact`; every other line is one
user turn: `user`, `assistant` (text accompanying the tool calls), `calls`
(`tool` + flat string `input` map) and `final` (follow-up text). Secrets
must never appear here — the bench greps nothing, so keep them out by hand
at record time.

`workspace/` is the replay target: a tiny widget library plus
`data/big.rs` (generated 1 000-line file for truncation/outline/dedup
effects). Transcripts exercise each mechanism on purpose: big-file reads
(archive), repeated identical reads (dedup), `mode: "outline"` reads
(outline), 6-turn length (compaction). `long-review.jsonl` is a 16-turn
review that re-reads files more than `dedup_window_turns` apart, so the
history `read` dedup and earlier microcompaction have something to do.

## Re-recording

There is no cassette step: transcripts are hand-written for mechanism
coverage, and tool outputs come from the fixture files at replay time, so
editing either file is the re-record. Keep turns deterministic — no clock,
no glob order dependence beyond what the committed files give.
