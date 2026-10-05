# Design: crate split (T30.18)

## Problem

D1 fixed ten in-tree crates. The creator's rule is to split the project into crates as far as possible. Measured (R§4.3.4), the ten crates mix three things that change at different rates:

- heavy or platform-specific dependencies: syntect, tree-sitter grammars, tiktoken, otel, landlock/seccompiler;
- trust guards;
- the logic around them.

The headless surface also imports the whole TUI for one function (`plain.rs:21` → `cox_tui::text::sanitize`). Measurable question: does every crate exist for a reason stated below? Does no crate hold something that could stand alone for one of those reasons?

## Rule: when a module becomes a crate

A module becomes its own crate when at least one of these holds:

- **(a) Dependencies.** It is the only user of a heavy or platform-gated dependency.
- **(b) Guard.** It is one of the four trust guards. The guard stays single: it moves, and the old path re-exports it, so nothing forks.
- **(c) Size.** It is a leaf, meaning no `use crate::` into its old crate, and it is at least ~500 LOC.
- **(d) Reuse.** A second crate needs it without the rest of its current crate.

A module stays where it is when it is in a cycle, when it is small with no heavy dependencies, or when it changes together with its neighbours.

## Target graph

Seventeen new crates (`cox-models` comes from T30.24 (U4)), twenty-seven in total. The arrows are the new `deps.rs` rules.

| Crate | From | Why | Depends on |
|---|---|---|---|
| `cox-sanitize` | `cox-tui/text.rs` | (b)(d): the guard for `plain.rs`, git output and ACP without the TUI | — |
| `cox-render` | `cox-tui`: theme, color, svg, markdown, diff, glyph (~2.6k) | (a) syntect, two-face, pulldown-cmark, colorsaurus | sanitize |
| `cox-sandbox` | `cox-tools`: `sandbox/*`, `path.rs` (~920) | (a)(b) nix, landlock, seccompiler; `Policy` and `confine` | protocol |
| `cox-syntax` | `cox-tools`: `outline.rs` + the bash parser setup | (a) tree-sitter + 5 grammars | — |
| `cox-search` | `cox-tools`: `grep.rs`, `glob.rs` (~870) | (a) ignore, grep-searcher, nucleo | protocol, sandbox |
| `cox-patch` | `cox-tools/v4a` (~990) | (c) — only the pure `parse`/`stage` engine; the `apply_patch` `Tool` impl stays in `cox-tools` so `path::confine` keeps its single call site | protocol |
| `cox-web` | `cox-tools/web_fetch.rs` | (a) reqwest leaves `cox-tools` — only the pure fetch/extract engine; the `web_fetch` `Tool` impl stays in `cox-tools` (`ToolCx`, input parsing) | protocol |
| `cox-permission` | `cox-core/permission` (448) | (b) `Engine`; pure | protocol |
| `cox-models` | T30.24 (U4) + `usage.rs` price table | (d) the catalog for core, provider and doctor; pure | protocol |
| `cox-tokens` | `cox-provider/tokens.rs` | (a) tiktoken BPE data | protocol |
| `cox-provider-http` | `http.rs`, `retry.rs`, `sse.rs` (~500) | (d) shared by every wire | protocol |
| `cox-provider-anthropic` | `anthropic/*` (~2.1k) | (a)(c) typify build step | protocol, models, provider-http |
| `cox-provider-openai` | `openai/*` (~2.2k) | (a)(c) async-openai | protocol, models, provider-http |
| `cox-provider-jev` | `jev.rs` | (c) | protocol, models, provider-http |
| `cox-provider-testkit` | `scripted.rs`, `replay.rs` (~750) | (d) dev-dependency of every test | protocol |
| `cox-telemetry` | `cox/telemetry.rs` | (a) five otel crates | — |
| `cox-config` | `cox/config_load.rs`, `config_cmd.rs` (~990) | (c)(d) the one config owner; figment, toml_edit | protocol, models |
| `cox-voice` | new (P54, A123) | (a) whisper.cpp (`whisper-rs`, a cmake C++ build), `cpal` platform audio, `rubato`; linked only behind `crates/cox`'s `voice` feature, off by default | protocol |
| `cox-cursor-cloud` | new (P56, A123) | (a) the Cursor Cloud Agents wire types (T56.1) and client (T56.2): the one socket to `api.cursor.com`; not a `Provider`, not session assembly | protocol, provider-http (from T56.2) |

After the split:

- `cox-provider` keeps `Priced`, the registry and `backend_for`, moved from `crates/cox`. It depends on the wires, models and tokens.
- `cox-tools` keeps the tool registry and the remaining tools. It depends on sandbox, syntax, search, patch and web.
- `cox-core` depends on protocol, permission and models, all pure, so D2 holds.
- `cox-tui` depends on core, protocol, render and sanitize.

**Not split, with reasons.**

- `cox-core` loop: `session` ↔ `turn` is a cycle. The leaves `router`, `rollout`, `budget`, `cache_diag`, `dedup`, `redact` and `truncate` are each under 500 LOC, have no heavy dependencies, and change with the loop.
- `cox-store` (1.9k): only it may hold diesel (D9), so any sub-crate would need diesel too.
- `cox-ext` (2.8k): five loaders of a few hundred LOC each, sharing one dependency set.
- `cox-mcp` (1.7k) and `cox-acp` (1.4k): each already isolates its one heavy dependency.
- `cox-tui` TEA core (`state`, `app`, `view`, `keymap`, `vim`, `composer`): changes together.
- `cox`: `doctor`, `stats`, `sessions` and `run` are commands over every crate.

## Migration (each card green on its own; cards T32.1–T32.16 in `plan.md` P32, C*n* = T32.*n*)

Each card does four things:

1. `git mv`s the files into one new crate.
2. Leaves a `pub use` at the old path, so callers and the guard names in AGENTS.md keep working. The AGENTS.md trust list and layout table are updated in the same card.
3. Adds the crate's rule to `crates/cox/tests/deps.rs`.
4. Changes no logic.

Size limit: at most three crates touched per card. Moved lines do not count toward the 200-LOC limit; edited lines do.

Order:

1. C1 `cox-sanitize`, then C2 `cox-render`. This first pair records `cargo build --timings` before and after, as the evidence for (a).
2. C3 `cox-sandbox`, C4 `cox-syntax`, C5 `cox-search`, C6 `cox-patch`, C7 `cox-web`.
3. C8 `cox-permission`, C9 `cox-telemetry`, C10 `cox-tokens`.
4. C11 `cox-provider-testkit`, C12 `cox-provider-http`.
5. The provider wires after T30.21–T30.26 (U1–U6), so they move once, already unified: C13 anthropic, C14 openai, C15 jev.
6. C16 `cox-config` last. Its `anyhow` becomes a `thiserror` enum, because AGENTS.md keeps `anyhow` in `crates/cox` only.

## Falsifier

If C2's `--timings` shows no incremental-build gain on a `state.rs` edit, rule (a) is weaker than assumed. Re-rank the remaining (a)-only cards (C5, C7, C9, C10) before doing them.

**Outcome (T32.2, 2026-09-26; numbers in `research.md` §4.3.4).** The gain on a `state.rs` edit is real but negligible: the `cox-tui` unit goes from 0.42 s to 0.39 s, and wall time from 1.40 s to 1.34 s. The falsifier fires in substance: rule (a) is weaker than assumed, because the heavy dependencies were never rebuilt on an edit anyway. The re-rank it asks for is moot, since C5, C7, C9 and C10 had already landed before C2 ran. Keep rule (a) only where a split also serves (b), a guard, or (d), reuse by a crate that must not pull in the rest, as C2 does: a surface can render without the TUI, and `deps.rs` pins the highlighters to one crate.
