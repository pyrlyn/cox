# cox

https://github.com/pyrlyn/cox

A modular terminal coding agent in Rust (coxswain: steers work while models, tools, and extensions row). TUI, headless, ACP, MCP.

## Cloud review findings (2026-10-08)

New bugs, dead code and moves from a read-only Cursor cloud review of clean `main` (agent `bc-31eef317-b77c-5beb-976e-00002188574d`; full report: `cloud/cox.md` in the private `listepo/roadmap` repo). They form phase P64: T64.1–T64.23, ordered P0, P1, P2. **confirmed** means seen in the tree; **suspected** means plausible from the code but not proven. Crate paths are under `crates/`; line numbers are as of the review. None of these is in the task table yet: to take one, add its row and write a card the usual way (§2).

| ID | Priority | Kind | Status | Where | Fix |
| --- | --- | --- | --- | --- | --- |
| T64.1 | P0 | bug | done | `cox-config/src/load.rs` project guards | Project and `.claude` hooks ran unsandboxed. Project hook commands are reverted; user config and `~/.claude` hooks stay. |
| T64.2 | P0 | bug | confirmed | `load.rs:453-470`; `cox-session/src/lib.rs:231-235`; `cox-core/src/turn.rs:505` | A project `core.workspace_roots` is not guarded and becomes `ToolCx.roots` for `confine`. Revert project roots outside the git root and the user's roots. |
| T64.3 | P0 | bug | confirmed | `cox-protocol/src/config.rs:515-517`; `GUARDED_KEYS` at `load.rs:453-470` | A project can set `providers.*.base_url` or add providers, and the user's API keys follow that URL. Revert project `base_url`, `api_key_env` and new `[providers.*]` tables. |
| T64.4 | P1 | bug | confirmed | `load.rs:197-206`; `cox-permission/src/lib.rs:317-318` | A project may set `permissions.mode = auto` (only `bypass` is reverted). Revert any project mode wider than the user layer. |
| T64.5 | P1 | bug | confirmed | `load.rs:228-237`; `cox-sandbox/src/sandbox/mod.rs:199-207` | A project may set `sandbox.network = true` and extra `sandbox.writable`. Revert them unless the user layer already allows them. |
| T64.6 | P1 | bug | confirmed | `cox-permission/src/policy.rs:33-37`; no approval check in the project guards | A project may set `permissions.approval = on-failure`, which auto-runs confined `Exec`. Refuse a project approval looser than the user layer. |
| T64.7 | P1 | bug | confirmed | `load.rs:1085-1120` (test keeps `mcp.servers["new"]`); LSP revert at `:295-323` | A project may add MCP stdio servers. Revert project-added or changed `mcp.servers` the way `lsp.servers` is reverted. |
| T64.8 | P1 | bug | confirmed (no test run) | `cox-core/src/turn.rs:286-291`; `cox-tools/src/write.rs:102-107`; `cox-tools/src/edit.rs:80-85` | `write`/`edit` are `Concurrency::Parallel`, so same-path calls in one batch race and the last rename wins. Group the parallel batch by `touches()` path. |
| T64.9 | P1 | bug | confirmed (documented design) | `cox-sandbox/src/sandbox/landlock.rs:53-65` | Landlock grants read of `/`, so read-only and workspace-write sandboxes can read the whole disk. Narrow reads to the workspace roots, or refuse Landlock where read isolation is required. |
| T64.10 | P1 | bug | confirmed | `cox-session/src/mcp.rs:122-130`, `:158-165`; `cox-session/src/sandbox.rs:89-97` | An MCP stdio server that cannot be wrapped under Landlock runs with host privileges plus a notice. Refuse or quarantine it. |
| T64.11 | P1 | bug | confirmed | `.github/workflows/ci.yml:3-11`, `:470-481` | `revert-on-failure` needs a push to `main`, but `ci.yml` has no `push` trigger, so the job never runs. Add the trigger or delete the job. |
| T64.12 | P2 | bug | confirmed | `cox-web/src/lib.rs:48-66` | The `web_fetch` client fallback drops `.timeout(TIMEOUT)`. Fail closed, or set the timeout on the fallback. |
| T64.13 | P2 | bug | confirmed | `cox-mcp/src/auth.rs:228-256` | The OAuth loopback forwards the first TCP connection's query; `state` is checked only later. Accept only `GET /callback` and keep listening until `state` matches. |
| T64.14 | P2 | bug | confirmed | `cox-store/src/lock.rs:100-105` | An unreadable or corrupt session lock file becomes `Holder::default()` (pid 0). Treat it as busy with an unknown holder. |
| T64.15 | P2 | bug | suspected | `cox-sandbox/src/path.rs:41-76`; `cox-tools/src/read.rs:89-96` | `confine` then `std::fs::read`/`write` leaves a symlink-swap window. Open with `O_NOFOLLOW`/`openat`, or serialize readers and writers per path. |
| T64.16 | P2 | bug | suspected (a comment says it is intentional) | `cox-permission/src/lib.rs:256-268` | The Bash read-deny skips relative tokens: `deny Read(~/.ssh/**)` misses `cat id_rsa` run inside `~/.ssh`. Resolve relative tokens against the session cwd. |
| T64.17 | P2 | bug | confirmed | `cox-protocol/src/config.rs:1416-1417`; `cox-session/src/lib.rs:552-559` | A project may point `memory.dir` anywhere. Guard it like the other path keys. |
| T64.18 | P2 | dead code | confirmed | `cox-core/src/session.rs:53-55` (`#[allow(dead_code)]`); set at `compact.rs:413` | `State::Compacting` is written but never matched. Match it in the UI/status, or drop it and its allow. |
| T64.19 | P2 | dead code | confirmed | `crates/cox/Cargo.toml:106-107` | `crates/cox` depends on `keyring`, but `cox/src` has no `keyring::` (unused since T37.31). Remove the dependency. |
| T64.20 | P2 | dead code | confirmed | `cox-protocol/src/agent.rs:57` | `AgentDef::restrict` has only test callers. Call it where `cox-core`/`cox-session` open a child, or make it `pub(crate)`. |
| T64.21 | P2 | dead code | confirmed | `report.html` (39,581 bytes, tracked, not ignored); `AGENTS.md:7` | Gitignore it if it is generated, or move it under `docs/` and update `AGENTS.md`. |
| T64.22 | P2 | move | confirmed | `cox-provider-http/src/http.rs:84-88` and `cox-mcp/src/auth.rs:69-100` → one keyring helper | Share `keyring_enabled` and the `Entry::new("cox", …)` policy (~20 lines). Extract a `secret-store` crate only when a third consumer appears. |
| T64.23 | P2 | move | confirmed | `brand/build.mjs:1-18` → `@pyrlyn/brand` | Upstream only the generic merge/table code. `tokens.json` and the logos stay here. |

Already tracked here, not added again: `cox-cursor-cloud` with no workspace dependents and `Store::cloud_run_*` used only in `cloud_runs.rs` (dead code D2/D3, move M7) are P56 work (T56.x).

Not added: the provider stack and the permission/sandbox/sanitize/mcp/config crates are already extracted (T32), so moves M1/M2 are only renames; the release workflow already lives in `pyrlyn/ci`, and `scripts/release.sh` should stay (M5); the 10 `#[allow(dead_code)]` sites are test- or feature-gated apart from T64.18; the Jev client is not dead.

| # | Status | Priority | Complexity | Readiness | Agent |
| --- | --- | --- | --- | --- | --- |
| T33.14.2 | todo | P2 | 3 | 0% | |
| T33.34 | todo | P2 | 4 | 0% | |
| T33.36 | todo | P2 | 4 | 0% | |
| T33.40.1 | todo | P1 | 5 | 0% | |
| T33.40.3 | todo | P2 | 4 | 0% | |
| T33.40.4 | todo | P2 | 4 | 0% | |
| T33.40.5 | todo | P2 | 3 | 0% | |
| T33.40.6 | todo | P2 | 4 | 0% | |
| T33.40.7 | todo | P2 | 3 | 0% | |
| T33.40.9 | todo | P2 | 4 | 0% | |
| T33.40.10 | todo | P3 | 3 | 0% | |
| T33.40.11 | todo | P2 | 2 | 0% | |
| T33.40.12 | todo | P2 | 3 | 0% | |
| T33.40.13 | todo | P2 | 3 | 0% | |
| T33.40.14 | todo | P2 | 3 | 0% | |
| T33.40.15 | todo | P2 | 3 | 0% | |
| T33.40.16 | todo | P2 | 2 | 0% | |
| T33.40.17 | todo | P3 | 2 | 0% | |
| T33.43 | todo | P1 | 2 | 0% | |
| T33.45 | todo | P2 | 4 | 10% | |
| T33.45.3 | todo | P2 | 4 | 0% | |
| T33.45.4 | todo | P2 | 3 | 0% | |
| T33.45.5 | todo | P2 | 4 | 0% | |
| T33.45.6 | todo | P2 | 3 | 0% | |
| T33.45.7 | todo | P2 | 4 | 0% | |
| T33.45.8 | todo | P2 | 3 | 0% | |
| T33.45.9 | todo | P2 | 3 | 0% | |
| T33.45.10 | todo | P2 | 2 | 0% | |
| T35.10 | todo | P3 | 2 | 0% | |
| T37.32 | todo | P1 | 3 | 0% | |
| T37.32.2 | todo | P1 | 3 | 0% | |
| T37.32.3 | in progress | P1 | 2 | 90% | Claude Code / opus-5.5 |
| T37.33 | todo | P1 | 3 | 0% | |
| T39.7 | todo | P3 | 2 | 0% | |
| T43.6 | todo | P3 | 3 | 40% | |
| T53.5 | todo | P3 | 3 | 0% | |
| T53.6 | todo | P3 | 2 | 0% | |
| T53.7 | todo | P3 | 2 | 0% | |
| T53.8 | todo | P3 | 2 | 0% | |
| T53.9 | todo | P3 | 1 | 0% | |
| T56.6 | todo | P3 | 4 | 0% | |
| T56.7 | todo | P3 | 4 | 0% | |
| T56.8 | todo | P3 | 3 | 0% | |
| T56.9 | todo | P3 | 1 | 0% | |
| T56.10 | todo | P3 | 2 | 0% | |
| T57.1 | todo | P1 | 2 | 90% | |
| T57.4 | todo | P1 | 4 | 0% | |
| T57.5 | todo | P1 | 4 | 0% | |
| T57.6 | todo | P2 | 3 | 0% | |
| T57.7 | todo | P1 | 2 | 90% | |
| T57.8 | todo | P1 | 5 | 0% | |
| T57.9 | todo | P1 | 2 | 0% | |
| T57.10 | todo | P2 | 2 | 0% | |
| T57.11 | todo | P2 | 3 | 0% | |
| T57.12 | todo | P3 | 2 | 0% | |
| T57.13 | todo | P2 | 3 | 0% | |
| T58.1 | todo | P1 | 4 | 0% | |
| T58.2 | todo | P2 | 2 | 0% | |
| T58.3 | todo | P1 | 3 | 0% | |
| T58.5 | todo | P2 | 3 | 0% | |
| T58.6 | todo | P2 | 3 | 0% | |
| T58.7 | todo | P2 | 4 | 0% | |
| T58.8 | todo | P2 | 3 | 0% | |
| T58.9 | todo | P2 | 3 | 0% | |
| T58.10 | todo | P2 | 3 | 0% | |
| T58.11 | todo | P2 | 2 | 0% | |
| T58.12 | todo | P2 | 4 | 0% | |
| T58.13 | todo | P2 | 3 | 0% | |
| T58.14 | todo | P2 | 4 | 0% | |
| T58.15 | todo | P2 | 3 | 0% | |
| T58.16 | todo | P3 | 2 | 0% | |
| T58.17 | todo | P2 | 2 | 0% | |
| T58.18 | todo | P2 | 4 | 0% | |
| T58.19 | todo | P3 | 3 | 0% | |
| T58.20 | todo | P3 | 3 | 0% | |
| T58.21 | todo | P3 | 3 | 0% | |
| T58.22 | todo | P2 | 4 | 0% | |
| T58.23 | todo | P3 | 2 | 0% | |
| T58.24 | todo | P2 | 3 | 0% | |
| T58.25 | todo | P2 | 1 | 0% | |
| T58.26 | todo | P3 | 2 | 0% | |
| T58.27 | todo | P3 | 2 | 0% | |
| T58.28 | todo | P2 | 3 | 0% | |
| T58.29 | todo | P2 | 3 | 0% | |
| T58.30 | todo | P3 | 3 | 0% | |
| T59.4 | todo | P2 | 4 | 0% | |
| T59.6 | todo | P3 | 3 | 0% | |
| T59.7 | todo | P3 | 4 | 0% | |
| T61.1 | todo | P1 | 1 | 0% | |
| T61.2 | todo | P1 | 2 | 0% | |
| T61.3 | todo | P1 | 2 | 0% | |
| T61.4 | todo | P1 | 2 | 0% | |
| T61.5.1 | todo | P2 | 3 | 0% | |
| T61.5.2 | todo | P2 | 3 | 0% | |
| T61.5.3 | todo | P2 | 3 | 0% | |
| T61.6 | todo | P2 | 2 | 0% | |
| T61.7 | todo | P2 | 2 | 0% | |
| T61.8 | todo | P3 | 1 | 0% | |
| T61.9 | todo | P2 | 3 | 0% | |
| T61.10 | todo | P3 | 2 | 0% | |
| T61.11 | todo | P3 | 3 | 0% | |
| T63.2 | todo | P2 | 3 | 0% | |
| T63.4 | todo | P2 | 4 | 0% | |
| T63.4.1 | todo | P2 | 2 | 0% | |
| T63.4.2 | todo | P2 | 3 | 0% | |
| T63.4.3 | todo | P2 | 3 | 0% | |

## Reference

Name: **cox** — the coxswain steers the boat and calls the strokes; the crew (models, tools, MCP servers) does the rowing. Binary `cox`, crates `cox-*`, home `~/.cox/`.

How to read this file: §0 decisions are settled; §1 is the design every task must conform to (types, schemas, algorithms, surfaces); §2 is how a task is worked; numbered tasks that are finished live in `done.md`; later approved work is in `roadmap.md`; §6 amendments; §7 risks. Active work, when any, is the table at the top of this file.

## 0. Decisions (read before any task)

| # | Decision | Why (evidence in research.md) |
|---|----------|-------------------------------|
| D1 | **One Cargo workspace, one static binary and one macOS app that links `cox-ffi` as a static library (`docs/design/desktop.md`, A67). A module is its own crate when it alone uses a heavy or platform-gated dependency, is a trust guard, is a ≥ 500-LOC leaf, or is needed by another crate without the rest of its own (`docs/design/crates.md`, A47); `crates/cox/tests/deps.rs` holds the graph, including: only `cox-ffi` depends on `uniffi`; `cox-session` and `cox-app` depend on neither `clap` nor `anyhow`. No dylib plugin host. One WASM plugin host (extism) from v0.2: `docs/design/plugins.md`; it reaches the core only through traits in `cox-protocol`.** Extensibility in v0.1 is *data and processes*: instruction files, `SKILL.md`, command and subagent markdown, hook subprocesses, MCP servers. A WASM host (extism) is v0.2. | Claude Code, Codex, Gemini CLI and Copilot all reach their ecosystems through markdown + hooks + MCP, not through in-process plugins (R§2). A plugin ABI is the one thing that cannot be changed later; defer it until the `Tool`/`Event` contract has survived a release. |
| D2 | **The core is a pure state machine: `Submission` in, `Event` out.** `cox-core` owns turns, context assembly, permissions, routing, compaction. It never touches the network, filesystem or a process except through traits defined in `cox-protocol`. TUI, `stream-json`, ACP, the JSONL rollout and the desktop app (through `cox-app`, A67) are consumers of one event stream. `cox-app`'s timeline fold is a pure function of that stream: replaying a rollout yields the same patches as the live run. | Codex's SQ/EQ protocol is the reason it ships a TUI, an `exec` mode, an app-server for IDEs and an MCP server from one core (R§1.2). It is also what makes the loop testable without a model: a scripted provider plus a golden event log. |
| D3 | **Own thin provider layer; no LLM framework crate.** `cox-provider` implements the Anthropic Messages API (streaming, tool use, `cache_control`, adaptive thinking, `effort`, `fallbacks`, `count_tokens`), the OpenAI Responses API, and OpenAI Chat Completions (Ollama, vLLM, LM Studio, llama.cpp, OpenRouter, DeepSeek). SSE via `eventsource-stream`. **Where wire types come from (A40):** (1) a maintained Rust SDK's *types* when one exists (OpenAI: `async-openai` types only), else (2) types generated with typify from the vendor's published spec, vendored in the repo (Anthropic), else (3) hand-written. Transport, retry, SSE state machine, `ProviderEvent` mapping and the ledger stay ours in every case; SDK code is a `wire` module inside the provider, extracted to a crate only when a second consumer appears. Login: API keys only — Claude subscription OAuth is forbidden to third parties (R§4.3.1); ChatGPT login waits for an OpenAI document permitting it. | rig/genai lag the wire formats that decide cost: cache breakpoints, thinking-block replay, server tools, per-message effort, refusal fallbacks (R§4.3). Each provider is ~500 LOC; a framework is a dependency on someone else's release cadence. Codex hand-rolls its client too and ships `eventsource-stream 0.2.3` (R§1.3). |
| D4 | **Adopt existing formats verbatim instead of inventing ones.** `AGENTS.md` (and `CLAUDE.md`) hierarchy; Agent Skills `SKILL.md`; Claude Code hook JSON protocol and `.claude/settings.json` permission-rule syntax (`Bash(npm run test:*)`), `.claude/commands/*.md`, `.claude/agents/*.md`; `.mcp.json`; Codex `apply_patch` (V4A) grammar; `--output-format stream-json`. cox-native equivalents live under `.cox/` with the same schemas. | A user with a Claude Code or Codex setup gets cox for free, and the rtok hook stack works unchanged (R§3). Every one of these is documented and already read by ≥ 2 agents. |
| D5 | **Route by job tier, never by guesswork, never up.** Three tiers in config: `cheap` (default `claude-haiku-4-5`; any local model), `code` (default `claude-sonnet-5`; `claude-opus-5` when the user picks it or the task is flagged large), `think` (`claude-fable-5-1`, only via `/think` or `--deep`, always confirmed). Jobs pinned to `cheap`: session title, compaction summary, tool-result summarisation, commit message, memory extraction, explore/search subagents, background shell and HTTP subagents, hook-driven LLM calls. Every request carries a `job` tag into the ledger. | User constraint. Claude Code's silent Haiku delegation is its most-cited complaint (R§2.1); Copilot's auto-routing is praised because it is explicit and discounted. Anthropic's own guidance: measure the capable model at lower `effort` before building a cascade, because caches are model-scoped (R§4.4). |
| D6 | **Token economy is core, not a plugin.** (a) every tool output is archived before the model sees it; the model sees head/tail + `expand <id>`; (b) identical read/grep within N turns returns "unchanged, see #id"; (c) `read` has `lines=` and `mode=outline` (tree-sitter); (d) tool schemas beyond the core eight are deferred and found through a `tool_search` tool; (e) prefix is byte-stable: tools → system → instruction files → last cache breakpoint → volatile; (f) compaction is append-only, keeps the last two turns verbatim, runs on `cheap`; (g) one per-request `usage` row with cache read/write; (h) session and monthly budget caps. Metric: *context-token-turns*. | rtok measured 3–40 % real savings from external hooks against 60–95 % vendor claims; the difference is that hooks cannot touch what the model sees. A native agent can (R§4.1–4.2). Minimum cacheable prefix is 512 tokens on the Claude 5 family and 4 096 on Haiku 4.5, so one volatile byte in the system prompt costs the whole cache (R§6 ledger #21). |
| D7 | **Sandbox on by default.** macOS: Seatbelt profiles via `sandbox-exec`. Linux: bubblewrap when present, else Landlock + seccomp. Sandbox modes `read-only` / `workspace-write` / `danger-full-access`; approval policies `untrusted` / `on-request` / `on-failure` / `never` (Codex vocabulary) combined with Claude-style allow/deny rules. `.git` and `.cox` stay read-only inside `workspace-write`. Windows: no sandbox, loud warning, `on-request` forced. | Both Codex and Claude Code converged on exactly this pair of mechanisms (R§1.4, R§2.1, ledger #11). Instruction files are requests; the sandbox is the guarantee. |
| D8 | **Edits are diff-shaped.** `edit` = exact `str_replace` with a whitespace-insensitive fallback and a uniqueness check; `apply_patch` = V4A grammar (Add/Update/Delete, `@@` context, progressive matching). `write` is for new files; rewriting an existing file over 200 lines is denied with a hint. | 5–20× fewer output tokens than whole-file writes (R§4.2); OpenAI models are trained on V4A and Claude on `str_replace`, so supporting both removes a class of edit failures. |
| D9 | **One SQLite file plus human-readable rollouts, through a sync ORM.** `~/.cox/cox.db` (Diesel 2.2 `sqlite` + bundled `libsqlite3-sys` 0.30 with FTS5, WAL): sessions, usage ledger, tool-output archive index, memory. Typed Diesel models and `schema.rs`; migrations embedded with `diesel_migrations`; FTS5 virtual tables via `diesel::sql_query` (Diesel cannot model `VIRTUAL TABLE`). `cox-store` is the only crate that contains SQL. Each session is also `~/.cox/sessions/<id>.jsonl` — the event stream itself — used for resume, replay tests and export. Archived payloads over 16 KiB live under `~/.cox/archive/`. | Same choice as rtok D13 (user request): typed models make the ledger queries (`stats`, budget, cache diagnostics) joins instead of hand-written SQL, and Diesel is sync, so hooks, tests and `cox stats` need no async runtime. Async ORMs (SeaORM, SQLx) would need a runtime per hook. Codex stores rollouts as JSONL; Claude Code uses JSONL; engram/claude-mem converge on SQLite+FTS5 (R§1.5). |
| D10 | **TUI = ratatui 0.30 + crossterm 0.29 in TEA form, inline viewport.** `State`, `update(State, Msg) -> State`, `view(&State, Frame)`. Inline (non-alternate-screen) rendering so native scrollback keeps the transcript. Every widget has an `insta` snapshot through `TestBackend`; end-to-end through `portable-pty` + `vt100`. | Codex made the same choices and tests them the same way (R§1.6). TEA makes `update` a pure function that a test can drive without a terminal. |
| D11 | **Four surfaces from day one: `cox` (TUI), `cox run -p` (headless; `text`/`json`/`stream-json`), `cox acp` (Agent Client Protocol 2.0 for Zed/JetBrains/neovim), `cox mcp` (built-in tools as an MCP server).** Each is ≤ 300 LOC over the event stream. A fifth, the macOS app (P37, A67), consumes the same stream through `cox-app`; `cox-ffi` holds no logic: every exported function or method is a one-expression forward into `cox-app`, with type mapping only through `types.rs`'s `#[uniffi::remote]` declarations, and a test enforces it (A90, which replaces the 300-LOC limit and A88's count), while the view model lives in `cox-app` and the Swift code outside the Cargo workspace. | D2 makes them cheap; ACP is what gets a terminal agent into editors without an extension per IDE (R§3.2); `cox mcp` lets Claude Code or Codex borrow cox's tools. |
| D12 | **No test touches the network or needs an API key.** `Provider` has `Scripted` (fixtures) and `Replay` (recorded cassettes, re-recorded on demand with `cox record`) implementations; tools run in `tempfile` trees; the patch parser and `str_replace` have `proptest` suites; transcripts and TUI frames are `insta` snapshots; the real binary is driven by `assert_cmd` against `COX_HOME`. Evals (Terminal-Bench adapter) are a separate, opt-in `just eval`. | A coding agent is a distributed system with a nondeterministic component; the only cheap regression suite is one that replays events instead of models (R§5). |
| D13 | **One config file; every flag is a key.** `~/.cox/config.toml` < `<git root>/.cox/config.toml` < `COX_<SECTION>_<KEY>` < flags, via clap 4 (derive) + figment + toml_edit. `cox config show --sources` reports provenance. `.claude/settings.json` permissions are *imported* (read-only) when present, as are hooks from the user's `~/.claude`; a repository's hook commands are reverted (T64.1). `.env` / `.env.local` (dotenvy, T0.7) are not a config layer: they inject unset process env before figment reads `COX_*`, and never override variables already set (CI, `COX_HOME=...` tests). | Same rule as rtok D12/D14; it worked. Headless and ACP runs are launched with fixed command lines, so flags alone cannot configure them. Local API keys live in `.env`, which gitignores. |
| D14 | **Everything not written by cox is untrusted, and extensions fail open.** Model output, tool results, MCP responses, hook stdout, skill files and repository instruction files pass the guards in `AGENTS.md` → Trust boundaries. A broken hook, server or skill is warned about and skipped. | Aider's credential leak and Claude Code's escape-sequence incidents are both "trusted text from the wrong side" bugs (R§2.2). |
| D15 | **Each component is designed against the field before it is built.** Every P-phase's first task is a ≤ 1-page `docs/design/<component>.md`: the problem in one measurable number, what Claude Code / Codex / Pi / OpenCode / aider do, what cox does and why it is at least as good, and what would falsify it. Written by the `code` tier; reviewed, not written, by `think`. | rtok D15. Copying a competitor caps cox at that competitor. |
| D16 | **Observability is `tracing` with an optional OpenTelemetry GenAI exporter.** Spans carry `gen_ai.operation.name`, `gen_ai.provider.name`, `gen_ai.request.model`, `gen_ai.usage.*`. Off by default; `cox stats` reads the ledger locally. | Codex ships opentelemetry 0.31 (R§1.3); the GenAI semconv is still experimental, so it stays behind a feature flag. |

Deferred to **v0.2+** (not rejected): LSP client (diagnostics into context); Gemini provider; image input and `ratatui-image`; web search provider abstraction beyond Anthropic server tools; A2A; voice (being built as P54, A123: local `whisper-rs` push-to-talk); `gix` instead of shelling out to `git`; aider-style repo map with PageRank; two-model architect/editor mode.

## 1. Architecture

```
            ┌────────────────────────────── cox (one binary) ──────────────────────────────┐
 terminal ──┤ cox            cox-tui  ─┐                                                    │
 script  ───┤ cox run -p     stream-json┤  Submission ▶ ┌──────────┐ ▶ Event                │
 Zed/IDE ───┤ cox acp        cox-acp  ─┤───────────────▶│ cox-core │───────────────▶ rollout │
 other agent┤ cox mcp        (server) ─┘                └────┬─────┘  (.jsonl, ledger)       │
            │                                  traits in cox-protocol │                       │
            │        ┌──────────────┬──────────────┬──────────┴──────┬──────────────┐        │
            │   cox-provider    cox-tools       cox-mcp          cox-store      cox-ext        │
            │   Anthropic       read/edit/      rmcp client      SQLite +       AGENTS.md      │
            │   OpenAI Resp.    apply_patch     stdio/HTTP       archive        skills/cmds    │
            │   OpenAI Chat     bash+sandbox    OAuth            FTS5           hooks/agents   │
            │   Scripted/Replay grep/glob/outline                               settings.json  │
            └──────────────────────────────────────────────────────────────────────────────┘
```

### 1.1 Crates

| Crate | Owns | Key deps (pinned in T0.1; versions verified in R§4.5) |
|-------|------|------|
| `cox` | clap surface, dispatch, `doctor`, `config` (printing, and the flag layer built from `Cli`), `stats`, `expand`, `record`, `sessions`, `self update` | clap 4.6, anyhow, dotenvy 0.15 |
| `cox-config` | the one config owner (T32.16; split out of `cox`): figment layering (default/user/project/env/flag), validation, `cox config set` editing and the `docs/config.jsonschema` drift test. Errors are a `thiserror` enum | figment, toml_edit 0.25, thiserror |
| `cox-session` | session assembly as a library (T37.1; split out of `cox`): `open(SessionSpec)` → session, effective config and typed `Warning`s — provider, tools, MCP, skills, hooks, plugins, fork/handoff/resume lineage, external agents; login-shell environment (T37.11). No clap, no anyhow, no printing | async-trait, tokio-util, agent-client-protocol (moved from `cox` with the external-agent code), nix `signal` (T37.11: process-group kill of a slow login shell) |
| `cox-app` | the UI-agnostic app core (T37.8–T37.10, T37.38): `Timeline` fold to serde `TimelinePatch`es, tool summaries and `ToolGroup`, the coalescing `Controller`, `Workspace`, `Inbox`, `Intent`/`dispatch`, `Completer`. No terminal toolkit, no CLI crate | tokio (drain task), serde_json; cox-render without `ratatui`; chrono 0.4 (no default features; `clock`, `std`): local midnight and the ISO week start for the Context tab's project totals (T37.29.3.3); portable-pty 0.9.0 (MIT): the terminal pane's PTY (T51.3); nix (MIT; `signal`, `process`): closing a terminal signals its process group (T51.3); url 2.5.8 (MIT OR Apache-2.0): the browser tools pass only http/https (T51.7); async-trait (MIT OR Apache-2.0): the async host traits (P52); toml_edit 0.25 (MIT OR Apache-2.0, already in the tree through cox-config): the welcome hero reads `Cargo.toml`'s `[workspace] members` (T37.49) |
| `cox-ffi` | the macOS app's UniFFI surface (T37.14): one tokio runtime, `App` and `SessionHandle` objects, the foreign `AppHost` trait, `#[uniffi::remote]` mirrors of cox-app types, a fixture recorder. `staticlib` + `lib`; the only crate that depends on uniffi | uniffi 0.32.2 (proc-macros, no UDL; default features off); dev: syn 3.0.5 (`full`, `parsing`; T37.39.1: `tests/forward_only.rs` parses the FFI sources); async-trait (MIT OR Apache-2.0): the async `AppHost` methods UniFFI exports (P51, P52) |
| `desktop/` | the macOS app (P37, not a Cargo crate): Swift packages under `desktop/macos/Packages/`, the design tokens and their generator under `desktop/design/` | node 24.21.0 (mise) with npm `style-dictionary` 5.5.5 (T37.17: DTCG tokens → Swift, asset colours, CSS); SwiftLint 0.65.1 (mise, aqua; T37.18: DS§9 no-literal rules) and SwiftLintPlugins at the same version in each package; `swift-format` from the Xcode toolchain; swift-collections 1.7.1 (T37.16: `OrderedDictionary` timeline store); swift-dependencies 1.17.1 (the stores' launch-wide services as `@Dependency` values, DT§4.6); swift-snapshot-testing 1.19.6 (T37.19: `CoxUI` snapshot tests); swift-property-based 2.0.1 (T63.1: `CoxModel` property tests) |
| `desktop/windows/` | the Windows app (P58, A127; planned, not a Cargo crate): a WinUI 3 + C# solution over `cox-ffi`'s C# bindings, logic in `cox-app` | planned by A127: .NET SDK 10.0 LTS (10.0.12), Windows App SDK 2.5.1, uniffi-bindgen-cs on uniffi 0.32 (blocked, T58.1); candidates CommunityToolkit.Mvvm 8.4.2, xunit.v3 4.0.1, FlaUI.UIA3 5.0.0, Verify.XunitV3 33.1.5 (`research.md` §10) |
| `cox-protocol` | `Submission`, `Event`, `Item`, `ToolCall`, `ToolResult`, `Usage`, `Config`, traits `Provider`, `Tool`, `Store`, `Hook` | serde, serde_json, schemars 1, thiserror 2, base64 0.23 (`image`, T40.1) |
| `cox-core` | `Session` state machine, turn loop, context assembly, cache breakpoints, `Router` (job → tier → model), compaction, budget, subagent spawning | tokio 1, tracing 0.1, base64 0.23 (T37.6: attached text files) |
| `cox-models` | the model catalog: id → context window, max output, efforts, capabilities, price; built-in rows < config < user `prices.toml` (T30.24). Pure: parses embedded or caller-supplied strings only | serde, thiserror, figment |
| `cox-provider` | the provider registry and `from_env`; `Scripted` and `Replay` (the `Provider` glue over `cox-provider-testkit`); usage extraction; re-exports the wires at the old `anthropic` and `openai` paths | reqwest 0.12 (rustls) |
| `cox-provider-anthropic` | the Anthropic Messages wire (T32.13; split out of `cox-provider`): request building, stream parsing, wire types from the vendored spec, `schema/` | reqwest 0.12, typify 0.8 (build.rs, T30.10/T30.12) |
| `cox-provider-openai` | the OpenAI Responses and Chat wires (T32.14; split out of `cox-provider`) | reqwest 0.12, async-openai 0.42 (`response-types` only, T30.11) |
| `cox-tools` | `read`, `grep`, `glob`, `edit`, `apply_patch`, `write`, `bash`, `todo`, `ask_user`, `agent`, `tool_search`, `web_fetch`, `expand`, `docs_resolve` / `docs_query` / `docs_fetch` (T65.1); the LSP stdio JSON-RPC client (`lsp::client`, T41.2) | similar 3.2, nix, thiserror (`LspError`, T41.2), url 2.5 (LSP `file://` URIs, T41.3), sha2 0.11 (docs cache digest, T65.1; already a workspace dependency), zstd 0.13 (docs.rs `json.zst`, T65.1; already in the lockfile via wasmtime) |
| `cox-sandbox` | `path::confine`, `sandbox::{seatbelt,bwrap,landlock}` (T32.3; split out of `cox-tools`): path confinement to the workspace roots and the platform sandbox front door. `cox-tools` re-exports both as `path` and `sandbox` | landlock 0.4.7, seccompiler 0.5, nix |
| `cox-patch` | the V4A patch engine (T32.6; split out of `cox-tools`): `parse` text ↔ AST, `stage` progressive hunk matching. Pure: no filesystem, no `ToolCx`; the `apply_patch` `Tool` impl stays in `cox-tools` (`v4a::tool`) so `path::confine` keeps one call site. `cox-tools` re-exports it as `v4a` | proptest 1.11 (dev) |
| `cox-syntax` | tree-sitter and its grammars (T32.4; split out of `cox-tools`): `outline` (signature extraction for `read`'s outline mode) and `parse_bash` (the parser behind `bash`'s risk classifier). `cox-tools` re-exports `outline` at its old path | tree-sitter 0.27 + bash/rust/typescript/python/go grammars |
| `cox-tokens` | token counting (T32.10; split out of `cox-provider`): `estimate`, `count_openai` (tiktoken), `count_anthropic` (the count-tokens endpoint). `cox-provider` re-exports it at the old `tokens` path | tiktoken-rs 0.12, reqwest 0.12 |
| `cox-permission` | the permission `Engine` (T32.8; split out of `cox-core`): `Outcome`, the rule grammar, path rules. Pure; `cox-core` re-exports it at the old `permission` path | globset (path rules, T2.2); dev only: `cox-config`, `tempfile` (T56.4's A122 test) |
| `cox-search` | the grep and glob engines (T32.5; split out of `cox-tools`): `grep::search`, `glob::find`, `rank_by_query`, `workspace_files`. Pure; the `GrepTool`/`GlobTool` impls stay in `cox-tools` so `path::confine` keeps one call site | ignore 0.4.33, grep-searcher 0.1.17, grep-regex 0.1.14, globset, nucleo 0.5 |
| `cox-web` | the `web_fetch` engine (T32.7; split out of `cox-tools`): client, streaming GET with cancellation and a byte cap, HTML → text. `WebFetchTool` stays in `cox-tools` | reqwest 0.12 |
| `cox-telemetry` | tracing setup and the OpenTelemetry stack behind the `otel` feature (T32.9; split out of `cox`); `init` takes plain values, not `Config` | tracing-subscriber, tracing-appender 0.2, opentelemetry 0.32 (+ sdk, otlp, tracing bridge, appender), thiserror |
| `cox-provider-http` | HTTP plumbing shared by every wire (T32.12; split out of `cox-provider`): `http` (client, `resolve_key`, `resolve_key_with`, error mapping), `retry`, `sse`. `cox-provider` re-exports all three at their old paths | reqwest 0.12, keyring 4, eventsource-stream 0.2.3 |
| `cox-provider-testkit` | the pure scenario and cassette helpers behind `Scripted`/`Replay` (T32.11; split out of `cox-provider`): scenario parsing, event building, cassette hashing, secret redaction, cassette writing | figment, sha2 |
| `cox-mcp` | MCP client (stdio, Streamable HTTP, OAuth), server discovery (`.mcp.json`, config), tool namespacing `mcp__<server>__<tool>`, `cox mcp` server, tool-contract quarantine (T64.24) | rmcp 3.2 (`client`, `server`, `auth`, `transport-io`, `transport-child-process`, `transport-streamable-http-client-reqwest`), async-trait (server tools as `Tool` impls, T7.6), keyring 4 (OAuth tokens as `cox/mcp/<server>`, T22.5), reqwest 0.13 (the version rmcp implements its HTTP client trait for; the workspace row stays 0.12 for the providers), sha2 (the tool-contract hash, already a workspace dependency, T64.24) |
| `cox-store` | `~/.cox/cox.db` Diesel models, `schema.rs`, embedded migrations, rollout writer/reader, archive, FTS5 search (`sql_query`), ledger queries | diesel 2.2 (`sqlite`, `returning_clauses_for_sqlite_3_35`, `r2d2` off), diesel_migrations 2.2, libsqlite3-sys 0.30 (`bundled`), directories 6, keyring 4 |
| `cox-ext` | instruction-file hierarchy, `SKILL.md`, commands, subagent definitions, hook runner (Claude JSON protocol), `.claude/settings.json` import | serde_yaml (frontmatter), shlex, tokio + nix `signal` (hook runner: `sh -c` with a process-group kill on timeout, T7.4), regex 1 (hook `matcher` regexes, T22.3) |
| `cox-sanitize` | `sanitize`, `sanitize_with`, `truncate` (T5.6; split out of `cox-tui` by T32.1): strips escape sequences, C0 controls, bidi overrides and zero-width runs from untrusted text before it reaches the terminal; width-aware truncation. `cox-tui` re-exports it as `text` | unicode-width 0.2 |
| `cox-i18n` | localization (`docs/i18n.md`): embedded gettext catalogs (`po/messages.pot`; `en.po` source and fallback, `ru.po`, `uk.po`), `Plural-Forms` evaluation, locale negotiation, a per-message fallback chain, `t`/`tr!` with `{name}` placeholders; the `po-export` bin writes Apple `.strings`/`.stringsdict` and Windows `.resw`. No workspace dependency | polib 0.3 (MIT): parses the embedded `.po` files in pure Rust (no libintl/C, so macOS and Windows need nothing installed); maintained (0.3.0, Dec 2025), two small deps; chosen over `gettext` 0.4 (`.mo` only, last release 2019), `i18n-embed` 0.16 with its gettext backend (the same `gettext` crate plus rust-embed, a proc macro and `i18n.toml`) and `tr` 0.1 (a global translator with positional `{}` arguments). The plural-expression evaluator is in-crate (`src/plural.rs`): polib has none and the others keep theirs private. unic-langid 0.9 (MIT OR Apache-2.0): language tag parsing for negotiation; sys-locale 0.3 (MIT OR Apache-2.0): the OS UI languages when the env names none |
| `cox-render` | themes and colour tokens, colour-depth mapping, markdown with syntax highlighting, diffs, SVG export, glyph sets, OSC 8 link marking and `Look` (split out of `cox-tui` by T32.2): pure rendering, text and settings in, ratatui spans and buffers out. `cox-tui` re-exports every module at its old path | syntect, two-face, pulldown-cmark, terminal-colorsaurus, similar, toml_edit |
| `cox-tui` | TEA app, composer (tui-textarea-2 0.13, the ratatui-0.30 fork of tui-textarea 0.7), transcript cells, streaming markdown (pulldown-cmark 0.13 → spans; the plan said 0.10, same Tag/TagEnd API), syntect 5 highlighting, diff view, approval modal, status line, `/` commands, `@` file picker, `text::sanitize`, OSC 11 background detection for `tui.theme = "auto"` (T22.6), theme files and `/theme` (T24.2) | ratatui 0.30.2 (`scrolling-regions`, T23.2), crossterm 0.29, nucleo 0.5, pulldown-cmark 0.13, syntect 5.3 (fancy-regex, no onig), two-face 0.3 (`syntect-fancy`; ~250 syntaxes, +0.33 MiB — T24.3), unicode-width 0.2, arboard 3, terminal-colorsaurus 1.0, toml_edit 0.25, similar 3.2 (word diffs, the approval modal's proposed edit — T24.5), base64 0.23 (the OSC 52 payload, A81) |
| `cox-acp` | Agent Client Protocol 2.0 server: session/prompt, permission requests, client fs/terminal | agent-client-protocol 2.0 |
| `cox-plugin-api` | plugin manifest (`plugin.toml`), ABI v1 payloads, TUI widget tree, capability names; schemas `docs/plugin.schema.json` and `docs/plugin-abi.schema.json` with drift tests. Pure; builds for `wasm32-unknown-unknown` so the guest SDK can use it; `cox-protocol` re-exports it as `plugin` (A52, P33) | serde, serde_json, schemars 1, thiserror |
| `cox-plugin` | the WASM host: discovery, package digest, grant check, one worker per plugin, host functions (`cox:host/v1`), and the protocol-trait adapters `PluginHooks`, `WasmTool`, `PluginProvider`, `EventTap`, `Advisor` (A52, P33) | extism 1.30.0 (`default-features = false`: no ureq, no URL or file loading; `wasmtime-exceptions` on, A61), wasmtime 43 (declared only for the `anyhow` feature extism needs without its defaults), sha2 (package digest), figment (`plugin.toml`), reqwest 0.12 (`cox_http`, T33.14.1), tokio-util (`Provider` cancellation, T33.18); linked into `crates/cox` behind the default-on `plugins` feature (A55) |
| `cox-plugin-sdk` (`plugins/sdk`, the separate guest workspace, never a `crates/*` member) | the Rust guest SDK (T33.27): typed wrappers for every PL§4 export and `cox:host/v1` host function, the `register!` macro, and the wire (`{"Ok"\|"Err"}` host replies) that other-language guests copy; builds for `wasm32-unknown-unknown` | extism-pdk 1.4.1 (`default-features = false`: no extism `http`, no msgpack), cox-plugin-api (path) |
| `cox-voice` | push-to-talk dictation (P54, A123): `Transcriber` (whisper.cpp through `whisper-rs`, model loaded once), `Recorder` (default input device, mono, resampled to 16 kHz, capped length), and the `Dictation` impl the TUI receives. Its own crate under D1: a heavy C++ build and platform audio. Behind `crates/cox`'s `voice` feature, off by default; audio never leaves the process | whisper-rs 0.16.0 (Unlicense; whisper.cpp MIT; cmake), cpal 0.18.2 (Apache-2.0), rubato 5.0.0 (MIT OR Apache-2.0) |
| `cox-cursor-cloud` | the Cursor Cloud Agents API client (P56, A123; P56 under way, terms go-ahead given 2026-10-03): hand-written wire types (A40 step 3, never generated from or copied out of Cursor's unlicensed OpenAPI file), create/run/stream/cancel/usage. The one place a socket to `api.cursor.com` opens; not a `Provider`; the host driver that maps runs to task events lives in `cox-session` | cox-provider-http (reqwest, eventsource-stream), serde; no new dependency |

Planned by A127 (not used yet; `toolchain.md` gets each row when its card lands): `cox-ffi` adds `cdylib` to its crate types for the Windows app (T58.1); process-wrap 10.0.1 (Apache-2.0 OR MIT; job objects on Windows, process groups on Unix) in `cox-ext`, `cox-session`, `cox-tools`, `cox-app` and `crates/cox`, pending the creator's approval (A127 open question 5; T57.5); portable-pty 0.9 (already a workspace dependency) in `cox-tools` for the Windows `bash` (T57.8); the `desktop/windows/` row above.

Dev-deps (workspace): insta 1.48, proptest 1.11, wiremock 0.6, rstest 0.26, assert_cmd 2, predicates 3, assert_fs, tempfile 3, pretty_assertions, trycmd 1.2 (`cox run -p` output fixtures, P48), vt100 0.16, portable-pty 0.9, libfuzzer-sys 0.4 (fuzz crate only); tools: cargo-nextest, cargo-deny, cargo-audit, cargo-insta, cargo-dist, cargo-fuzz (nightly job only).

Dependency direction (enforced by a test in T0.1 that parses `cargo metadata`): `cox` → everything; `cox-tui`, `cox-acp` → `cox-core`, `cox-protocol`, `cox-sanitize` (and `cox-tui` → `cox-render`, `cox-acp` → `cox-sandbox` for the ACP client's `path::confine`, T35.3); `cox-render` → `cox-protocol`, `cox-sanitize`; `cox-sanitize` → no workspace crate; `cox-core` → `cox-protocol`, `cox-permission` (and may use `cox-models`); `cox-sandbox`, `cox-config`, `cox-models`, `cox-permission`, `cox-tokens`, `cox-patch`, `cox-web`, `cox-provider-http`, `cox-provider-testkit`, `cox-mcp`, `cox-store`, `cox-ext` → `cox-protocol` only; `cox-provider-anthropic`, `cox-provider-openai` → `cox-protocol`, `cox-models`, `cox-provider-http`; `cox-protocol` → `cox-plugin-api` only (the `plugin` re-export, T33.1); `cox-syntax`, `cox-search`, `cox-telemetry` → no workspace crate; `cox-tools` → `cox-protocol`, `cox-sandbox`, `cox-patch`, `cox-syntax`, `cox-search`, `cox-web`; `cox-provider` → `cox-protocol`, `cox-models`, `cox-tokens`, `cox-provider-http`, `cox-provider-testkit`, `cox-provider-anthropic`, `cox-provider-openai`; `cox-plugin-api` → no workspace crate; `cox-plugin` → `cox-protocol`, `cox-plugin-api`, `cox-sanitize`; only `cox-plugin` depends on extism (A52). No crate below `cox` depends on `cox-core`, and `cox-core` does not depend on `cox-plugin`.

### 1.2 The contract every crate shares (`cox-protocol`)

All types derive `Serialize, Deserialize, Debug, Clone, PartialEq`; enums are `#[serde(tag = "type", rename_all = "snake_case")]` so the rollout is greppable. Ids are newtypes over `String` (ULID): `SessionId`, `TurnId`, `ItemId`, `CallId`, `ArchiveId`.

```rust
pub enum Submission {
    UserTurn { text: String, attachments: Vec<Attachment>, confirm_think: bool },
    Approve { call_id: CallId, decision: Decision },          // Decision: Allow | AllowForSession | Deny { reason } | Edit { input }
    Interrupt,                                               // cancels the running turn; tools get the cancel token
    Compact { focus: Option<String> },
    SwitchModel { tier: Tier, model: Option<ModelId> },       // None = tier default
    SetPermissionMode(PermissionMode),                        // default | plan | auto | bypass
    Command(SlashCommand),                                    // parsed by the surface, executed by the core
    HookResult { hook_id: String, outcome: HookOutcome },     // hook runner is outside the core
    Background { call_id: CallId },                           // Ctrl+B: detach a running bash/agent call into a task (T27.1)
    UserShell { command: String, share: bool },               // composer `!`/`!!`: bash via the engine and sandbox; history only on share (T25.3)
    Redo,                                                     // /redo: rewind code to the last rewind's own pre-images, one step (T26.4)
    Shutdown,
}

pub enum Event {
    SessionStarted { session: SessionId, config_digest: String, cwd: PathBuf },
    TurnStarted   { turn: TurnId, job: Job, tier: Tier, model: ModelId },
    ItemStarted   { item: ItemId, kind: ItemKind },          // ItemKind: UserMessage | AssistantMessage | Thinking | ToolCall | ToolResult | Summary | Notice
    TextDelta     { item: ItemId, text: String },
    ThinkingDelta { item: ItemId, text: String },
    ToolCallRequested { call: ToolCall },                    // ToolCall { id, name, input: Value, risk: Risk, subject: String }
    ApprovalRequired  { call: ToolCall, why: Why },          // Why: RuleAsk { rule } | Risk(Risk) | SandboxDenied { detail } | Policy(ApprovalPolicy)
    ApprovalDecided   { call_id: CallId, decision: Decision, by: DecidedBy }, // User | Rule | Session | Policy | Hook
    ToolCallOutput    { call_id: CallId, delta: String },     // streaming stdout/stderr, already sanitised for display
    ToolCallDone      { call_id: CallId, result: ToolResult },// ToolResult { ok: bool, visible: String, archive: Option<ArchiveRef>, bytes: u64, duration_ms: u64, diff: Option<Diff> }
    ItemDone      { item: ItemId },
    Usage         { turn: TurnId, usage: Usage },
    Compacted     { summary: ItemId, dropped: Vec<ItemId>, before_tokens: u32, after_tokens: u32, reason: CompactReason }, // CompactReason: PreCall | PostTurn | Manual | ContextTooLong ("pre-call" …); absent in old rollouts = post-turn
    TaskCreated   { task: TaskId, label: String, tier: Tier },
    TaskCompleted { task: TaskId, result_item: ItemId, cost_usd: f64, exit_code: Option<i32>, archive: Option<ArchiveId> }, // exit code + archive: shell tasks (T27.1)
    ModelSwitched { tier: Tier, from: ModelId, to: ModelId },
    Notice        { level: Level, text: String },            // Level: Info | Warn | Budget | Security
    TurnDone      { turn: TurnId, stop: StopReason },        // EndTurn | MaxTurns | Interrupted | Budget | Refusal { detail } | Error
    Error         { error: CoreError, fatal: bool },
}

pub struct Usage {
    pub input_tokens: u32, pub output_tokens: u32,
    pub cache_read_tokens: u32, pub cache_write_tokens: u32,
    pub estimated: bool,                                     // true when the provider gave no usage and cox estimated
    pub cost_usd: f64, pub latency_ms: u64,
}

pub struct Request {                                         // provider-neutral; providers translate, nothing above knows a wire format
    pub tier: Tier, pub job: Job, pub model: ModelId,
    pub system: Vec<SystemBlock>,                            // SystemBlock { text, cache: bool }
    pub tools: Vec<ToolSpec>,                                // already filtered: deferred tools absent unless discovered
    pub messages: Vec<Message>,                              // Message { role: User | Assistant, content: Vec<Content> }
    pub effort: Effort, pub max_tokens: u32, pub thinking: Thinking, // Thinking: Off | Adaptive
    pub cache_breakpoints: Vec<usize>,                       // indices into system+messages, ≤ 3
    pub stop_sequences: Vec<String>,
}
pub enum Content { Text(String), Thinking { text, signature: Option<String> }, ToolUse { id, name, input }, ToolResult { call_id, content: String, is_error: bool }, Image { media_type, data_b64 }, Pointer { archive: ArchiveRef, summary: String } }

pub enum ProviderEvent { MessageStart { model }, TextDelta(String), ThinkingDelta(String), ToolUseStart { id, name }, ToolUseInputDelta(String), ToolUseEnd, Stop(StopReason), Usage(Usage), Retrying { attempt, after_ms }, Error(ProviderError) }

pub struct ToolSpec { pub name: String, pub description: String, pub input_schema: Value, pub deferred: bool, pub risk: Risk, pub concurrency: Concurrency } // Risk: ReadOnly | Write | Exec | Destructive; Concurrency: Parallel | Exclusive

#[async_trait] pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> Caps;                          // Caps { cache: bool, thinking: bool, server_tools: bool, count_tokens: bool, max_context: u32 }
    async fn stream(&self, req: Request, sink: mpsc::Sender<ProviderEvent>, cancel: CancellationToken) -> Result<Usage, ProviderError>;
    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError>;
}
#[async_trait] pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn subject(&self, input: &Value) -> String;              // what permission rules match on: path, command line, url, mcp name
    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError>;
}
pub struct ToolCx { pub roots: Vec<PathBuf>, pub cwd: PathBuf, pub sandbox: SandboxPolicy, pub archive: Arc<dyn Archive>, pub cancel: CancellationToken, pub output: mpsc::Sender<String>, pub session: SessionId, pub call: CallId }
pub struct ToolOutput { pub text: String, pub is_error: bool, pub diff: Option<Diff>, pub structured: Option<Value> } // text is untruncated; the core archives and truncates

pub trait Store: Send + Sync {                               // sync on purpose (D9)
    fn open(home: &Path) -> Result<Self, StoreError> where Self: Sized;
    fn session_create(&self, s: &SessionRow) -> Result<(), StoreError>;
    fn rollout_append(&self, id: &SessionId, ev: &Event) -> Result<u64, StoreError>;
    fn rollout_read(&self, id: &SessionId) -> Result<Vec<Event>, StoreError>;
    fn usage_insert(&self, row: &UsageRow) -> Result<(), StoreError>;
    fn archive_put(&self, a: &ArchivePut) -> Result<ArchiveId, StoreError>;
    fn archive_get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError>;
    fn memory_search(&self, q: &str, limit: usize) -> Result<Vec<MemoryHit>, StoreError>;
}
#[async_trait] pub trait Hook: Send + Sync { async fn run(&self, event: HookEvent, payload: Value, timeout: Duration) -> HookOutcome; } // Continue | Block { reason } | Modify { input } | Failed { error }
```

### 1.3 The turn loop (`cox-core`)

States of `Session`: `Idle → Assembling → Streaming → (AwaitingApproval ⇄ RunningTools) → Streaming … → Finishing → Idle`, plus `Compacting` (entered from `Idle` after `TurnDone`) and `Interrupted` (from any state; drains tools, emits `TurnDone{Interrupted}`).

```
on Submission::UserTurn(text):
  1. hooks: UserPromptSubmit (may block or rewrite text)
  2. history.push(UserMessage(text)); turn = new TurnId
  3. loop:
     a. req = assemble(history, config)                 # §1.9 order; exactly one movable breakpoint
     b. (tier, model, effort) = router.pick(job=Main)   # think requires confirm_think == true
     c. budget.check(estimate(req)) else TurnDone{Budget}
     d. provider.stream(req) → forward deltas as Events; collect tool_use blocks; usage row
     e. if stop == EndTurn: break
        if stop == MaxTokens: push assistant partial, continue once, else break with Notice
        if stop == Refusal: TurnDone{Refusal}; break
        if stop == ToolUse:
           calls = collected tool_use blocks (1..n)
           for each call (in parallel up to core.parallel_tools, Exclusive tools serialised):
             i.   hooks: PreToolUse (Block → result is_error with reason; Modify → new input)
             ii.  decision = engine.decide(call)         # §1.8; Ask → emit ApprovalRequired, await Submission::Approve
             iii. if Deny: result = error("denied: <why>")
             iv.  else run tool under sandbox policy; stream ToolCallOutput; on SandboxDenied and policy==on-failure → ApprovalRequired{SandboxDenied} → rerun unsandboxed only if approved
             v.   archive full output BEFORE truncation; visible = truncate(head/tail, pointer trailer)
             vi.  dedup: if hash(name,input) seen within dedup_window and no write to its subject since → visible = "unchanged since #<id>"
             vii. hooks: PostToolUse / PostToolUseFailure
           history.push(UserMessage(all tool results, in call order))   # one message; parallel tool use breaks otherwise
           continue loop
  4. hooks: Stop; TurnDone{EndTurn}
  5. if context_tokens ≥ compact_at × max_context: enter Compacting (§1.10) on cheap tier, then Idle
```

Rules the loop enforces, testable one by one: (1) all tool results for one assistant message go back in one user message, in the order the calls were emitted; (2) an `ApprovalRequired` blocks only that call — other approved parallel calls proceed; (3) `Interrupt` cancels the provider stream and every running tool via the shared token, then emits the partial assistant item and `TurnDone{Interrupted}`; (4) no `Event` is emitted after `TurnDone` for that turn; (5) the archive row exists before the model sees truncated text; (6) the request built after resume from the rollout is byte-identical to the one a live session would have built.

### 1.4 Routing table (D5)

| Job | Tier | Default model | Effort | Note |
|-----|------|---------------|--------|------|
| main coding turn | `code` | `claude-sonnet-5` | `high` | `/model opus` switches for the session; never auto |
| large refactor flagged by user | `code` | `claude-opus-5` | `xhigh` | user picks |
| `/think`, `--deep` plan | `think` | `claude-fable-5-1` | `high` | confirm prompt shows price ($10/$50 per MTok) |
| compaction summary | `cheap` | `claude-haiku-4-5` | — | output ≤ 2 k tokens |
| tool-result summary, title, commit message, memory extraction | `cheap` | `claude-haiku-4-5` | — | batched where possible |
| explore / search subagent | `cheap` | `claude-haiku-4-5` | — | read-only tools; result ≤ 1 k tokens |
| background shell / HTTP subagent | `cheap` | `claude-haiku-4-5` or local | — | `bash`, `web_fetch` only |
| local-only mode | all | Ollama/vLLM model | — | `cox --provider local` |

Prices for the ledger (Anthropic first-party, from the Claude API reference; re-verify in T1.7): Haiku 4.5 $1/$5, Sonnet 5 $2/$10, Opus 5 $5/$25, Fable 5.1 $10/$50 per MTok; cache write 1.25×, cache read 0.1× of input (Fable 5.1 cache read $0.25/MTok). `config/prices.toml` carries `verified_on` per row; a row older than 90 days makes `cox doctor` warn.

### 1.5 Testing pyramid (D12)

| Level | What | How | Where |
|-------|------|-----|-------|
| unit | parsers (SSE, V4A, frontmatter, permission rules), `str_replace`, truncation, cache-order assembly | plain tests + `proptest` | each crate |
| contract | `Provider` against recorded HTTP | `wiremock` serving `fixtures/<provider>/*.sse` | `cox-provider` |
| loop | full turns with `Scripted` provider; golden `Event` JSONL | `insta` on the event stream | `cox-core/tests` |
| tool | every tool in a tempdir; sandbox denial paths | `tempfile`, `assert_fs` | `cox-tools/tests` |
| TUI | each widget and whole frames | `TestBackend` + `insta` | `cox-tui` |
| binary | `cox run -p` and `cox` under a PTY | `assert_cmd`, `portable-pty` + `vt100` | `tests/` |
| eval (opt-in) | Terminal-Bench adapter, 10 in-repo tasks | `just eval`, real provider, ledger diff | `evals/` |

Fixture conventions: `fixtures/<provider>/<name>.sse` is the raw SSE body; `<name>.request.json` the request cox sent; `<name>.events.jsonl` the golden `ProviderEvent`s. Loop fixtures: `cox-core/tests/scenarios/<name>.toml` (scripted replies per turn) + `<name>.events.snap` (insta). Secrets are redacted at record time (`cox record --redact` replaces `sk-…` and `Bearer …` with `«redacted»`); a test in T1.5 greps fixtures for key patterns.

### 1.6 Configuration schema (`config/default.toml`, embedded; every key documented in `docs/config.md` by test)

```toml
[core]
home = "~/.cox"                 # COX_HOME overrides
workspace_roots = []            # empty = git root of cwd, else cwd; extra roots via --add-dir
max_turns = 200                 # per UserTurn, counts provider calls
parallel_tools = 4
log_level = "info"              # tracing filter; file log at ~/.cox/logs/cox.log

[tiers.cheap]
provider = "anthropic"
model = "claude-haiku-4-5"
effort = "low"
max_tokens = 4096

[tiers.code]
provider = "anthropic"
model = "claude-sonnet-5"
effort = "high"
max_tokens = 16384
thinking = "adaptive"

[tiers.think]
provider = "anthropic"
model = "claude-fable-5-1"
effort = "high"
max_tokens = 32768
thinking = "adaptive"
confirm = true                  # cannot be set false in project config

[jobs]                          # job → tier; values must name a tier
main = "code"
plan = "think"
compact = "cheap"
title = "cheap"
summarize = "cheap"
commit = "cheap"
memory = "cheap"
explore = "cheap"
shell = "cheap"
hook = "cheap"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"   # else keyring entry "cox/anthropic"
cache_ttl = "5m"                    # "5m" | "1h"
fallbacks = true                    # fallbacks: "default" + beta header
timeout_s = 120
max_retries = 4

[providers.openai]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
api = "responses"                   # "responses" | "chat"

[providers.local]
base_url = "http://localhost:11434/v1"
api = "chat"
model = "qwen3-coder"
context_window = 32768              # local servers do not report it

[context]
compact_at = 0.75                   # fraction of max_context
keep_turns = 2
microcompact_after_turns = 6
tool_output_visible_bytes = 8192
tool_output_head_lines = 60
tool_output_tail_lines = 20
dedup_window_turns = 8
instruction_budget_tokens = 8000
memory_budget_tokens = 800
deferred_tools = true

[permissions]
mode = "default"                    # default | plan | auto | bypass (bypass only via flag)
approval = "on-request"             # untrusted | on-request | on-failure | never
allow = []                          # rule strings, §1.8
ask = []
deny = ["Read(~/.ssh/**)", "Read(~/.aws/**)", "Bash(rm -rf /*)"]
import_claude_settings = true
allow_for_session_persists = false

[sandbox]
mode = "workspace-write"            # read-only | workspace-write | danger-full-access
network = false
writable = []                       # extra writable roots
readonly_in_workspace = [".git", ".cox", ".claude"]
linux_backend = "auto"              # auto | bwrap | landlock | none

[budget]
session_usd = 5.0
monthly_usd = 100.0
warn_at = 0.8
cheap_counts = true

[tui]
vim = false
theme = "auto"                      # auto | dark | light
inline = true
show_thinking = "collapsed"         # collapsed | hidden | full
mouse = true

[hooks]
timeout_s = 60
fail_open = true

[mcp]
timeout_s = 30
deferred = true
servers = {}                        # [mcp.servers.<name>] command/args/url/env/sandbox — same shape as .mcp.json, plus sandbox=false to opt a named stdio server out of the sandbox wrap (default true, T33.42)

[plugins]
enabled = true                      # --no-plugins / COX_PLUGINS_ENABLED; project config may turn it off, never back on (T33.6)

[memory]
enabled = true
extract = false                     # end-of-session extraction on cheap tier
dir = ""                            # default ~/.cox/projects/<slug>/memory

[telemetry]
otel = false
endpoint = ""

[record]
redact = true
```

Precedence (D13): embedded defaults < `~/.cox/config.toml` < `<git root>/.cox/config.toml` < `.claude/settings.json` (permissions/hooks/env only, imported) < `COX_<SECTION>_<KEY>` (e.g. `COX_TIERS_CODE_MODEL`) < CLI flags. Before figment runs, `dotenvy` loads `.env` then `.env.local` walking up from cwd (T0.7); missing files are ignored; already-set variables are left alone, so a key that arrived only via `.env` still shows as `env` in `cox config show --sources`. Project config may not raise `budget.*`, set `permissions.mode = "bypass"`, set `sandbox.mode = "danger-full-access"`, set `tiers.think.confirm = false` or add hook commands (T64.1); violations are reported by `cox config show` and ignored. `cox config show --sources` prints every effective key with its origin; `cox config set <key> <value>` edits the user file with `toml_edit` preserving comments.

### 1.7 Storage schema (`cox-store`)

Directory layout under `COX_HOME`:

```
~/.cox/
  config.toml
  cox.db                     # SQLite, WAL, FTS5
  sessions/<ulid>.jsonl      # rollout: one Event per line
  archive/<ulid>             # archived tool outputs > 16 KiB (smaller ones inline in the db)
  logs/cox.log               # tracing-appender, daily rotation
  projects/<slug>/memory/    # MEMORY.md + one file per fact (Claude Code layout)
  cassettes/<name>/          # cox record output
```

```sql
CREATE TABLE migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  cwd TEXT NOT NULL, project_slug TEXT NOT NULL, title TEXT, parent_id TEXT,
  rollout_path TEXT NOT NULL, turns INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0, state TEXT NOT NULL CHECK (state IN ('open','closed','error'))
);
CREATE TABLE usage (
  id INTEGER PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), turn INTEGER NOT NULL,
  job TEXT NOT NULL, tier TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
  cache_read_tokens INTEGER NOT NULL, cache_write_tokens INTEGER NOT NULL,
  estimated INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL, latency_ms INTEGER NOT NULL,
  context_tokens INTEGER NOT NULL,            -- what the model saw this call (for context-token-turns)
  created_at TEXT NOT NULL
);
CREATE INDEX usage_session ON usage(session_id, turn);
CREATE INDEX usage_day ON usage(created_at);
CREATE TABLE archive (
  id TEXT PRIMARY KEY, session_id TEXT NOT NULL, call_id TEXT NOT NULL, tool TEXT NOT NULL,
  subject TEXT, bytes INTEGER NOT NULL, sha256 TEXT NOT NULL,
  inline BLOB, path TEXT, created_at TEXT NOT NULL,
  CHECK ((inline IS NULL) <> (path IS NULL))
);
CREATE TABLE memory (
  id INTEGER PRIMARY KEY, project_slug TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL,
  kind TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(project_slug, name)
);
CREATE VIRTUAL TABLE memory_fts USING fts5(name, body, project_slug UNINDEXED);
CREATE VIRTUAL TABLE rollout_fts USING fts5(session_id UNINDEXED, turn UNINDEXED, text);
CREATE TABLE mcp_tool_trust (
  server TEXT NOT NULL, tool TEXT NOT NULL, hash TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('approved')), updated_at TEXT NOT NULL,
  PRIMARY KEY (server, tool)
);
```

ORM rules (D9): the DDL above is `migrations/<stamp>_init/up.sql` + `down.sql`, embedded with `diesel_migrations::embed_migrations!` and applied on `Store::open`; `schema.rs` is generated by `diesel print-schema` and committed (a test asserts it matches the migrations); each table has a `Queryable`/`Insertable` model in `cox-store/src/models.rs`; FTS5 tables are created and queried with `diesel::sql_query` and `QueryableByName` structs; one `SqliteConnection` behind a `Mutex` (no pool — a single-process CLI), `PRAGMA`s set on open. No other crate may depend on `diesel`; the direction test in T0.1 also asserts that.

Rollout line format: `{"seq":17,"ts":"2026-09-02T10:11:12.345Z","event":{"type":"text_delta","item":"…","text":"…"}}`. `seq` is monotonic per session; the reader tolerates a truncated last line (crash during write). `TextDelta`/`ThinkingDelta`/`ToolCallOutput` are coalesced on read into their items; resume rebuilds `history` from `ItemStarted`/`ItemDone` pairs and `Compacted`.

### 1.8 Permission rules and the decision algorithm (`cox_core::permission::Engine`)

Rule grammar (Claude Code's, verbatim): `Tool`, `Tool(subject)`, `Tool(prefix:*)`; file tools take a glob (`Read(~/.ssh/**)`, `Edit(src/**)`), `Bash` takes a command prefix (`Bash(npm run test:*)`, `Bash(git commit:*)`), MCP tools match `mcp__<server>__<tool>` or `mcp__<server>__*`, `WebFetch(domain:example.com)`, `CloudAgent(<github owner>/<repo>)` (T56.4: asks in every mode including `auto` and `bypass`, denied in `plan`, lifted only by an exact allow rule in the user's own config; a bare `CloudAgent` allow, a session grant and a project `allow` never lift it). Tool names are matched case-insensitively against cox's names and their Claude aliases (`Read`=`read`, `Edit`=`edit`, `Write`=`write`, `Bash`=`bash`, `Grep`=`grep`, `Glob`=`glob`, `WebFetch`=`web_fetch`, `Agent`=`agent`).

Decision order for a `ToolCall` with `risk` and `subject`:

1. `deny` rules (user, project, imported): first match → `Deny`.
2. `PermissionMode::Bypass` → `Allow` (flag-only mode; banner shown).
3. `PermissionMode::Plan`: `Risk::ReadOnly` → `Allow`; everything else → `Deny("plan mode")` — no prompt, so the model learns to plan.
4. `allow` rules: first match → `Allow`.
5. `ask` rules: first match → `Ask(RuleAsk)`.
6. Session grants (`AllowForSession` with the same tool + subject prefix) → `Allow`.
7. By risk: `ReadOnly` → `Allow`; `Write` → `Allow` in `auto`, else `Ask(Risk)`; `Exec` → `Ask(Risk)` unless the command is classified safe (T3.7 classifier: read-only commands like `ls`, `cat`, `git status`, `cargo test`, no redirects, no `sudo`) and mode is `auto`; `Destructive` → `Ask` in every mode except `Bypass`.
8. Approval policy adjusts step 7: `untrusted` → anything not from an `allow` rule asks; `on-request` → as above; `on-failure` → `Exec` runs sandboxed without asking and asks only when the sandbox denies; `never` → any `Ask` becomes `Deny` (headless default unless `--approve on-request`).
9. A `Deny` or `Ask` carries `Why`; the model sees the reason in the tool result so it can choose another approach.

The engine is pure: `decide(&self, call, mode, policy, grants) -> Decision`. Rules are compiled once (`globset` for paths, tokenised prefix for bash). Table-driven tests (T2.2) cover 30 rule/call pairs; `proptest` checks that adding a `deny` never turns a `Deny` into anything else.

### 1.9 Context assembly and cache layout

```
 ┌ system[0]  tool specs, non-deferred, sorted by name, canonical JSON        ┐ byte-stable for the session
 │ system[1]  cox system prompt (versioned string, no date, no cwd)           │  cache breakpoint 1 (after system[2])
 │ system[2]  instruction files: AGENTS.md/CLAUDE.md chain, skills index,     ┘
 │            repo map last (A74, off by default)
 │ system[3]  volatile: date, cwd, git branch, memory index, permission mode      no cache (changes daily / per turn)
 │ messages   [Summary item if compacted]
 │            history … (older tool results microcompacted to pointers)         cache breakpoint 2 = end of previous turn
 │            this turn's user message + tool results                           cache breakpoint 3 = last message (moves every call)
 └
```

Invariants: bytes of `system[0..=2]` are identical across all calls of a session unless the user changes instruction files, tools are discovered via `tool_search` (discovered tools are appended to `system[0]`, which invalidates breakpoint 1 once; `Notice` explains it), a `/repomap refresh` changes the map's bytes (idle only, announced by a `Notice`, recorded by `Event::RepoMapBuilt`, attributed by `cache_diag`; identical bytes change nothing), or compaction rebuilds the map on the prefix restart it already causes (A74). Anthropic allows 4 breakpoints; cox uses 3 so a fourth is free for experiments. OpenAI providers ignore breakpoints (automatic prefix caching) but still benefit from the stable order. `Request.cache_breakpoints` are indices; the Anthropic translator turns them into `cache_control: {"type": "ephemeral", "ttl": …}`.

Token accounting per call writes `context_tokens` (input + cache read + cache write) to the ledger; `context-token-turns` for a session is the sum. T8.5 measures each D6 mechanism by toggling it and replaying recorded sessions.

### 1.10 Compaction and microcompaction

Trigger: after `TurnDone`, when `context_tokens_last_call ≥ context.compact_at × max_context` (`post-turn`, checked at the next turn's start), or on `/compact [focus]` (`manual`), or when a provider returns a context-length error (`context-too-long`; compaction runs before retrying once), or before any provider call inside a turn whose assembled request estimates at or over that threshold (`pre-call`, T28.3: ⌈bytes/4⌉, refined by `Provider::count_tokens` within 10 % of it; microcompaction of every result outside `keep_turns` first, then compaction and one re-assembly; still over → `Notice(Budget)` + `TurnDone{Budget}`). `Compacted.reason` names which.

Algorithm (append-only, D6f):
1. `PreCompact` hooks run with `{trigger, focus}`; a hook may `Block` (compaction skipped, notice shown).
2. Items to summarise = all items older than the last `keep_turns` turns, excluding items already dropped. Pointers replace archived tool results in the summariser input.
3. Request on the `compact` job (cheap tier): system = "You are compacting a coding session…" + focus; user = the items rendered as a transcript; output ≤ 2 048 tokens with fixed sections: Goal · Decisions · Files touched (paths) · Open todo · Errors seen · Next step.
4. On success: append `Item::Summary`, emit `Compacted{summary, dropped, before, after}`. The rollout keeps every original line; `dropped` ids are skipped when building requests. On failure: `Notice(Warn)`, nothing changes.
5. `PostCompact` hooks; instruction files are re-read (Claude Code behaviour) but only re-emitted if their bytes changed.

Microcompaction (no model call): when building a request, tool results older than `microcompact_after_turns` turns are replaced by `Content::Pointer { archive, summary: "<tool> <subject>: N bytes, exit 0" }`. The rollout is untouched.

### 1.11 Tool catalogue (core eight are non-deferred; the rest are found by `tool_search`)

| Tool | Input schema (required first) | Risk | Output | Notes |
|------|-------------------------------|------|--------|-------|
| `read` | `path`; `lines: "a-b"`; `mode: "text"\|"outline"` | ReadOnly | text with line numbers, or outline | size cap → pointer; binary → refuse with hint; PNG/JPEG/GIF/WebP ≤ 3.75 MB → image (T40.4) |
| `grep` | `pattern`; `path`; `glob`; `context: n`; `max_results` (100) | ReadOnly | `path:line: text` | ripgrep libs; respects `.gitignore`; cap → pointer |
| `glob` | `pattern`; `path`; `limit` (200) | ReadOnly | paths sorted by mtime, nucleo-ranked when `query` given | |
| `edit` | `path`, `old`, `new`; `replace_all: bool` | Write | unified diff | exact → whitespace-insensitive; ambiguity is an error listing match lines |
| `apply_patch` | `patch` (V4A text) | Write | per-file diff summary | Add/Update/Delete/Move; `Destructive` if it deletes > 5 files |
| `write` | `path`, `content` | Write | bytes written | existing file > 200 lines → error "use edit" |
| `bash` | `command`; `shell` (`sh`\|`bash`\|`zsh`\|`fish`\|`dash`\|`ksh`\|`tcsh`\|`nu`\|`pwsh`, default `sh`); `timeout_s` (120); `background: bool` | Exec / Destructive (classified) | streamed stdout+stderr, exit code | sandboxed; env allowlist; cwd = workspace; the shell enum is the allowlist and resolves in fixed dirs, never `PATH` |
| `todo` | `items: [{id, text, state}]` | ReadOnly | rendered list | state drives the TUI todo panel |
| `expand` | `id` (archive id); `lines: "a-b"` | ReadOnly | archived bytes (capped, further pointers) | deferred: false (always present, tiny schema) |
| `ask_user` | `question`; `options: [..]` | ReadOnly | the answer | blocks the turn; headless → error unless `--answer` |
| `tool_search` | `query`; `detail: "summary"\|"full"` (default `summary`) | ReadOnly | up to 5 hits as `{name, description}`, or the full spec when `detail` is `full`; `structured.discovered` is always the names | BM25 over name + description; a summary carries no `input_schema` (T65.2) |
| `mcp_exec` | `code`; `timeout_s` (clamped to 1–30, default 30) | Write | the program's final print only, capped at 8 000 bytes | deferred false, with `bash` and `tool_search`; one new sandboxed `python3 -I -u` per call; `describe` is name and description, never `input_schema` (T65.1) |
| `web_fetch` | `url`; `max_bytes` | ReadOnly (network) | readable text | Anthropic server tool passthrough when available; else reqwest + readability; domain rules |
| `diagnostics` | `path` | Exec until its language server runs, then ReadOnly | `path:line:col: severity: message [source code]`, sorted, summary last | deferred; one lazily started LSP server per language under the session sandbox, killed at session end; with no server an is_error result that points to `bash` (T41.6) |
| `agent` | `task`; `preset: "explore"\|"shell"\|<name>`; `tier`; `tools: [..]`; `budget_usd`; `background: bool` | inherits max of its tools | result text ≤ cap, summarised on cheap if over | subagent = nested `Session` with its own rollout, parent id set |
| `memory_save` / `memory_search` | `name, body` / `query` | Write / ReadOnly | id / hits | P10 |
| `docs_resolve` | `name` | ReadOnly | `cargo/<name>/<version>` | deferred; `Cargo.lock` text, no `cargo` subprocess (T65.1) |
| `docs_query` | `name`, `query`; `version` | ReadOnly | at most 5 snippets, 400 characters each | deferred; `~/.rtok/docs/<name>/<version>` if that directory exists, else `~/.cox/docs`; `name=llms` searches a root `llms.txt` and downloads nothing (T65.1) |
| `docs_fetch` | `name`; `version` | ReadOnly | cache summary | deferred; one GET of `https://docs.rs/crate/<name>/<version>/json.zst`, no `Authorization` header, 30 s timeout; subject is that URL; version comes from the lockfile, never `latest`, unless the lockfile has no entry (T65.1) |
| `mcp__<server>__<tool>` | server's schema | from server annotations, default Write | server result, archived like any tool | deferred by default |

Every tool's `subject()` is what rules match on: the confined path, the command line, the URL, or the namespaced MCP name.

### 1.12 CLI surface (`crates/cox`)

```
cox [PROMPT] [--continue | --resume <id>]           interactive TUI; PROMPT is the first turn
cox run -p <prompt> [--output-format text|json|stream-json] [--max-turns N] [--allowed-tools a,b]
        [--permission-mode default|plan|auto|bypass] [--approve never|on-request] [--answer <text>]
        [--continue | --resume <id>] [--deep]        headless; exit 0 ok · 1 error · 2 denied · 3 budget · 4 interrupted
cox sessions [--grep <q>] [--json] [--limit N]       list / search rollouts
cox expand <archive-id> [--lines a-b]                print archived tool output
cox stats [--session <id> | --day | --month] [--cache] [--json | --csv]
cox config show [--sources] | get <key> | set <key> <value> | path
cox doctor [--json]
cox record <name> -p <prompt> [--redact] [--provider ...] re-record a cassette with a real key
cox mcp [--allow-write] [--tools a,b]                serve built-in tools over MCP stdio
cox mcp login|logout <server>                        HTTP server OAuth token (T22.5)
cox mcp trust [<server>]                             approve one server's tool definitions, or list pending and changed (T64.24)
cox acp                                              Agent Client Protocol server on stdio
cox ext list [--json]                                instruction files, skills, commands, agents, hooks, MCP servers in effect
cox self update [--version v]
Global: --provider <name> --model <id> --tier <tier>=<model> --sandbox <mode> --budget <usd> --cwd <dir> --add-dir <dir>
        --home <dir> -v/-vv --json (machine output where supported) --no-hooks --no-mcp
```

Every flag maps to a config key (T0.3 test); `--permission-mode bypass` and `--sandbox danger-full-access` are flag-only and print a persistent banner.

### 1.13 TUI keymap and slash commands (`cox-tui`)

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| `Enter` | send | `Shift+Enter` / `Alt+Enter` / `Ctrl+Enter` | newline (`Ctrl+Enter` reserved for send-now, T25.1) |
| `Esc` | interrupt turn / close modal | `Ctrl+C` ×2 within 1 s | quit |
| `Shift+Tab` | cycle permission mode default → plan → auto; the prompt glyph follows (`>` `▷` `»` `!`); `Tab` completes an `@`/`/` token | `Ctrl+O` | transcript overlay (full scrollback, search `/`) |
| `Ctrl+T` | toggle thinking visibility | `Ctrl+E` | expand last tool output |
| `@` | file picker (nucleo) | `/` at line start | command palette |
| `y` / `s` / `n` / `e` in approval modal | allow / allow for session / deny / edit command | `Ctrl+R` | prompt history search |
| `PageUp/PageDown`, mouse wheel | scroll transcript | `Ctrl+L` | redraw |
| `Ctrl+G` | diff view: the working tree against `HEAD`, per-file blocks; `PageUp/PageDown` scroll, `Esc` closes (T15.3) | `Ctrl+B` | move the running `bash`/`agent` call to the background; the turn goes on (T27.1) |

Slash commands (parsed in the surface, executed as `Submission::Command`): `/model [tier] [model]`, `/effort [low|high|xhigh]` (session-wide, clamped per model, T16.4), `/think <prompt>` (confirm dialog with price), `/compact [focus]`, `/cost`, `/permissions`, `/sandbox <mode>`, `/resume`, `/sessions`, `/expand <id>`, `/agents` (live sessions in this workspace, T16.3), `/skills`, `/hooks`, `/mcp`, `/doctor`, `/clear` (new session, same cwd), `/vim`, `/help`, `/quit`. Markdown files in `.claude/commands` and `.cox/commands` appear in the same palette (T7.3).

Status line (one row): `sonnet-5 · ctx 41% · $0.83 · workspace-write · 2 tasks · [plan]`.

### 1.14 Error taxonomy

| Crate | Enum | Variants |
|-------|------|----------|
| `cox-provider` | `ProviderError` | `Auth`, `RateLimited { retry_after }`, `Overloaded`, `BadRequest { message }`, `ContextTooLong { max, got }`, `Refusal { detail }`, `Network`, `Timeout`, `Cancelled`, `Parse { line }`, `Unsupported { feature }` |
| `cox-tools` | `ToolError` | `Denied { why }`, `Confined { path, root }`, `SandboxDenied { detail }`, `Timeout`, `NotFound`, `Ambiguous { matches }`, `TooLarge { bytes, cap }`, `Binary`, `Io`, `Cancelled` |
| `cox-core` | `CoreError` | `Budget { spent, cap }`, `Interrupted`, `Provider(ProviderError)`, `ExternalAgent { agent, message }`, `Tool { call, error }`, `Compaction`, `Config { key, message }`, `Store(StoreError)`, `Hook { id, error }` |
| `cox-store` | `StoreError` | `Open`, `Migrate { from, to }`, `Corrupt { path }`, `NotFound`, `Io`, `Sqlite` |
| `cox-ext` | `ExtError` | `Frontmatter { path, line }`, `HookTimeout`, `HookCrashed { status }`, `TooLarge { path, budget }`, `Cycle { path }` |
| `cox-mcp` | `McpError` | `Spawn`, `Handshake`, `Auth`, `Timeout`, `Transport`, `ToolFailed { server, tool }` |

Retryable: `RateLimited`, `Overloaded`, `Network`, `Timeout` (provider) — exponential backoff 1 s × 2ⁿ, jitter, max 4, honouring `retry-after`. Fatal to the turn, not the session: everything else. Fatal to the session: `StoreError::Corrupt`, `Config`.

### 1.15 Cross-cutting invariants (each is a named test somewhere in §3)

1. `prefix_bytes_identical_between_turns` (T2.3) · 2. `truncate_is_lossless_via_archive` (T2.5) · 3. `all_tool_results_return_in_one_message` (T2.1) · 4. `deny_beats_allow` (T2.2) · 5. `compaction_keeps_last_two_turns_verbatim` (T8.1) · 6. `resume_builds_identical_request` (T2.4) · 7. `no_event_after_turn_done` (T2.1) · 8. `every_request_has_a_usage_row` (T1.7) · 9. `think_requires_confirmation` (T9.1) · 10. `broken_hook_is_skipped_not_fatal` (T7.4) · 11. `sandbox_denies_write_outside_workspace` (T4.1/T4.2) · 12. `every_flag_has_a_config_key` (T0.3) · 13. `no_crate_below_cox_depends_on_core` (T0.1) · 14. `sanitize_strips_escapes` (T5.6) · 15. `plugin_tool_specs_frozen_within_session` (T33.12) · 16. `plugin_grant_reasked_on_digest_or_widening` (T33.6) · 17. `plugin_failure_is_skipped_not_fatal` (T33.3).

## 2. Working agreement for agents

See `AGENTS.md`. In short: claim `open` tasks only; ≤ 200 LOC and ≤ 3 files per task (tests count; fixtures and snapshots do not); the Check is not optional; `Model:` records who did it. Task model guidance: **haiku** for scaffolding, fixtures, snapshot updates, docs, shell/CI; **sonnet** for most code; **opus** for the state machine, permission engine, sandbox, compaction, V4A, MCP client, ACP; **fable** never writes code — it reviews `docs/design/*.md` when a phase gate asks for it.

Task block format used in §3:

```
#### T<phase>.<n> <title>
Model: <tier> · Status: open · Depends: <task ids> · Size: ~<LOC>
Goal: one sentence, measurable.
Files: the files this task creates or edits (≤ 3 source files).
Steps: numbered; each step is something a reviewer can see in the diff.
Check: a bash block that exits 0 when the task is done; run under `mise exec`.
Done when: the observable state after the Check, plus what must be in done.md.
Out of scope: what the next task does, so the agent does not do it here.
```

Work only on `main` — no `cox/<task-id>` or other task branches. Commit `<task-id>: <title>` on `main`; any new dependency needs a row in §1.1 and a reason in the commit. If the Check cannot pass without exceeding the size limit, split the task with an amendment in §6 and do the first half. Skipped or failing steps are reported in `done.md`, never silently. The human is the only author: no agent adds a `Co-Authored-By` trailer, a "Generated with …" line or itself as author to a commit, merge or PR, whatever its harness defaults to.

Don't duplicate code or logic — find the existing helper and reuse it, or extract one shared helper at the responsible layer. Never a per-caller guard-patch. A new snippet is checked with `jscpd` (`check_duplication`) before committing.

Every implemented task is marked `Status: done <date>` and moved to `done.md` with its Check output. Code in the tree whose task still sits in `plan.md` as `open` or `in progress` is unfinished work.

## 3. Phases and tasks

### 3.0 Dependency graph and critical path

```
P0 ─▶ P1 ─▶ P2 ─▶ P3 ─▶ P4 ─▶ P5(rest) ─▶ P6 ─▶ P7 ─▶ P8 ─▶ P9 ─▶ P10 ─▶ P11 ─▶ P12
        │            └──▶ T5.1–T5.3 (TUI slice, after T2.4)
        └──▶ T8.3, T8.4 (ledger tooling) can start after T1.7
```

Critical path to M1 ("talks"): T0.1 → T0.2 → T0.3 → T0.4 → T1.1 → T1.2 → T1.5 → T2.1 → T2.3 → T2.4 → T5.1 → T5.2 → T5.3. Everything else in P0–P2 can run in parallel with it (T0.5, T0.6, T0.7, T1.3, T1.4, T1.6–T1.8, T2.2, T2.5–T2.8).

P22–P30 (A27): P22 first; T22.1–T22.6 may proceed independently, then T22.7 follows and depends on all six. Then P24.1 (colour tokens) → P24.2 (themes) and P23.0 (`Caps`) → P23.1 (Kitty keys) → P25.1/P25.2 (queue, `Shift+Tab`). P26.1 → P26.2 → P26.3/P26.4 is the checkpoint chain. P23, P27 and P28 are independent of each other and of P26; P29 and P30 come last. Complexity per task is in the top table (1 easiest … 5 hardest).

### P0 — Scaffold (goal: `cox --version`, config, doctor, CI green)

### P1 — Provider (goal: one real streamed turn with tool use through each wire format, all replayable)

### P2 — Core loop (goal: a session that runs turns, calls tools, asks permission, resumes)

### P3 — Tools (goal: the eight core tools, diff-shaped edits, everything confined)

### P4 — Sandbox (goal: `workspace-write` enforced on macOS and Linux)

### P5 — TUI (goal: a daily-driver terminal UI with snapshots for every state)

### P6 — Headless and MCP server (goal: scripts and other agents can drive cox)

### P7 — Extensions (goal: a Claude Code or Codex user's setup works unchanged)

### P8 — Context economy (goal: measured savings, cache hit rate visible)

### P9 — Routing and subagents (goal: D5 enforced end to end)

### P10 — Memory (goal: cross-session memory with zero model cost by default)

### P11 — ACP and IDE (goal: cox inside Zed and JetBrains)

### P12 — Quality and release (goal: v0.1 installable and measured)

### P13 — Observability follow-up (goal: the exporter honours the standard resource variables and the documented smoke commands run as written)

Rationale in §6 A10. T13.4 and T13.5 are in `done.md`.

### P14 — TUI presentation (goal: the TUI renders correctly on any terminal font, glyph set, colour depth and language)

Rationale in §6 A11. What already exists and is *not* redone here: syntect highlighting of fenced code blocks (T5.3), `unicode-width` wrapping/truncation of wide and combining text (T5.3, T5.6), `tui.theme = auto|dark|light` resolved in `crates/cox/src/session.rs`.

### P15 — Git in the surfaces (goal: the branch, what changed and the diff are visible without leaving cox)

Rationale in §6 A13. Not redone here: unified-diff rendering (`cox-render/src/diff.rs`, T5.4; in `cox-tui` until T32.2), the nucleo picker (T5.2), running git (the `bash` tool, A12).

### P16 — Concurrent sessions on one workspace (goal: every cox process on a workspace knows what the others are doing, and the TUI shows it)

Rationale in §6 A14. Not redone here: the `/` palette with nucleo ranking (T5.2 — `/sessions`, `/agents`, `/model` already complete), hooks (T7.4), `cox sessions` (T10.3/A13), subagent tasks (T9.2).

### P17 — Surface leftovers (goal: §1.12 resume/continue/first prompt, OpenAI retry, doctor prices, website matches the shipped binary)

Rationale in §6 A20. T17.1–T17.7 are in `done.md`.

### P18 — TUI resume and the remaining website pages (goal: `cox --resume` opens the TUI; the site covers tools, compat, IDE and the walkthrough)

Rationale in §6 A21. T18.1–T18.6 are in `done.md`.

### P19 — v0.2 scoping gates (goal: each roadmap v0.2 line gets a phase-gate design doc; no runtime code yet)

Rationale in §6 A23. Branch `plan/v0.2-scoping`, one commit per task, one draft PR into `main` (PR #24).
Out of scope for the whole phase: any change under `crates/` — only `docs/design/v0.2-*.md`,
`plan.md`, and `roadmap.md` move here.

### P20 — ketch-model release (goal: releases publish first, tag last, installable via ketch and Homebrew)

Rationale in §6 A24. Branch `release/ketch-model`, one commit per task, PR #26 into `main`; P19 (PR #24) stays untouched.

### P21 — TypeSafe Jev as a decision model (goal: typed choice/score/boolean calls with probabilities where cox routes, classifies and gates — design doc first, no runtime code yet)

Rationale in §6 A25. Jev is a decision model (System One), not a chat or coding agent: state + typed questions in, choice/score/boolean with probabilities and confidence out. It fits cox at the exact points where cox already reduces a turn to a narrow judgment — router tier pick, permission risk, compaction triggers, memory salience, skill/command matching. No new dependency lands until the design doc fixes the boundary: Jev answers never bypass the permission engine, never touch the filesystem, and lose to the local default whenever the key, the network or the confidence is missing (fail open on extensions, same as hooks/skills/MCP).
Out of scope for the whole phase: any change under `crates/` — only `docs/design/v0.2-jev.md`, `plan.md`, and `roadmap.md` move here, mirroring the P19 scoping-gate shape. T21.1–T21.2 (below) are the implementation the gate allowed: provider wiring only, no call sites yet.

### P22 — Trust (goal: every config key, hook event and documented command does what the docs say; evidence in research.md §8.5 #32)

### P23 — Terminal capabilities (goal: one probe, every feature optional, `doctor` shows the verdict)

### P24 — Looks (goal: a reviewer calls it beautiful; every state has a snapshot and an SVG)

Done when: the two snapshots show highlighting and the release binary grows by less than 1 MiB (number in the commit message).
Out of scope: language auto-detection beyond file extension and first-line shebang.

### P25 — Composer and flow (goal: the keys a Claude Code or Codex user already has in their fingers)

### P26 — Checkpoints and rewind (goal: `/rewind` that also covers what the shell changed)

### P27 — Agents you can see (goal: no "raw scaffolding noise")

### P28 — Context and cost visibility (goal: the ledger and the routing are visible, not just recorded)

### P29 — Accessibility (goal: usable with a screen reader and without motion)

### P30 — Lean profile and footprint (goal: numbers cox can publish that no vendor does)

### P32 — Crate split (goal: every crate exists for a reason in `docs/design/crates.md`; D1 as amended by A47)

Every card in this phase:

1. `git mv`s the named files into `crates/<crate>/`. The new `lib.rs` opens with a `//!` header, and `Cargo.toml` takes only the dependencies those files use.
2. Leaves a `pub use` at the old path, so callers and the guard names keep working.
3. Adds the crate's rule to `crates/cox/tests/deps.rs`.
4. Updates the AGENTS.md layout table (and the trust list for a guard) and the plan.md §1 crate list.

No logic changes. At most three crates are touched. Moved lines do not count toward the 200-LOC limit; edited lines do.
Common check: the three commands in AGENTS.md are green, `deps.rs` has the crate's rule, and the card's own line holds.

### P33 — WASM plugins (goal: one package adds a status segment, a hook, a deferred tool and a provider without a cox release; §1.15 invariants 1, 8, 10 and 15–17 green; ≤ 50 ms warm start per plugin)

Rationale in §6 A52; the design is `docs/design/plugins.md` (cited below as PL§n).

Every card in this phase:

- stays within 200 LOC and 3 source files (manifests, generated schemas, snapshots and fixtures do not count);
- leaves a test that fails without it;
- documents what it adds (`docs/design/plugins.md` if the design moves, `docs/config.md` through the drift test for new keys, `docs/plugins.md` user docs from T33.27 on);
- runs the three standard commands.

Host unit tests use inline WAT (R§4.3.5 P15); no `.wasm` is ever committed. **Blockers** (everything after them depends on them): T33.1, T33.2, T33.3, T33.5, T33.6.

#### T33.14.2 Filesystem preopens

Split from T33.14 by the creator 2026-10-03 because the preopens wait on T33.43; the `cox_http` half is T33.14.1.
Depends: T33.9, T33.43 (preopens stay off until wasmtime ≥ 48, A55) · Size: ~120 · Files: `crates/cox-plugin/src/fs.rs`
Goal: WASI preopens come only from `fs` and pass `confine`; reads mount `ro:`; `.git` and `.cox` are never writable. WASI is on only when `wasi = true` or `fs` is set.
Check: `fs_write_to_dot_git_is_refused`, `wasi_ctx_has_no_env`.

#### T33.34 Go: SDK wrapper, template, example

Depends: T33.29, T33.43 (go-pdk v1.1.3 needs `wasip1`, and the host keeps WASI off until then, A55) · Size: ~200 · Files: `plugins/sdk-go/cox.go`, `plugins/examples/go/main.go`, `plugins/templates/go/*.tmpl`; `plugins/mise.toml` gets go and tinygo; CI job `plugin-examples`
Goal: a thin Go package over `github.com/extism/go-pdk` (v1.1.3) for the PL§4 exports and host functions (`//go:wasmimport` in `cox:host/v1`). The same example is built with TinyGo `-target wasip1 -buildmode=c-shared` (`wasi = true`). `cox plugin new --lang go`.
Check: `plugin_example_go` is `#[ignore = "needs go and tinygo: run just plugin-examples go"]` locally and runs in the `plugin-examples` CI job, where a missing toolchain fails the job; it asserts the same rollout effect as T33.28.

#### T33.36 Kotlin: thin PDK, template, example

Depends: T33.35, A61 (extism `wasmtime-exceptions` is on; `exception_handling_module_loads`), T33.43 (Kotlin's stdlib imports `wasi_snapshot_preview1::random_get`, and the host keeps WASI off until then, A55, A63) · Size: ~200 · Files: `plugins/sdk-kotlin/src/…/Cox.kt`, `plugins/examples/kotlin/src/…/Main.kt`, `plugins/templates/kotlin/*.tmpl`; `plugins/mise.toml` gets java, gradle and kotlin
Goal: a cox-owned minimal Kotlin PDK over the raw extism imports (there is no maintained one, R§4.3.5 P30), the same example, and `--lang kotlin`.
Draft: the PDK (`plugins/sdk-kotlin`, ~150 lines over the raw extism imports and `cox:host/v1`) and the example (`plugins/examples/kotlin`, id `example-kotlin`, same behaviour as the Rust example, `gradle pluginPackage`) build with Kotlin 2.4.20, Gradle 9.8.0 and OpenJDK 27.0.0 and are kept on branch `wip/t33.36-kotlin`. Left to do once WASI is on: templates, `Lang::Kotlin`, `PLUGIN_LANGS`, `just plugin-examples kotlin`, the CI step, the e2e, docs and `toolchain.md` rows.
Check: `plugin_example_kotlin` is ignored with its reason locally and runs in the CI job.

#### T33.40 Jev as the first plugin

Rationale: A25/P21, evidence `research.md` §4.3.6 (cited below as J§n/Jn). These seventeen cards test the ABI-form provider, the models, and the decision-point capabilities end to end against a real (wiremocked) use case, prove or refute PL§12 falsifier 3, and carry out `docs/design/plugins.md` §14 decision 8 (C1): the built-in `typesafe` client leaves the core once the plugin reaches parity. T33.18, T33.20 and T33.21 above already carry the amendments this use case needed.

#### T33.40.1 ABI: two-phase decide, own-provider call-out, batched questions — blocker

Depends: T33.2, T33.14.1, T33.15, T33.18, T33.20 · Size: ~180 · Files: `crates/cox-plugin-api/src/abi.rs`, `crates/cox-plugin/src/advisor.rs`, `crates/cox-plugin/src/net.rs`
Goal: close the gap that PL§12 falsifier 3 predicts (J§4.1–4.3). Today a decision plugin can reach its own provider only through a deadlock (`cox_model_call` into its own `cox_provider_stream`) or a ledger bypass (`cox_http` from `cox_decide`). This card adds a path with neither.
Plan:
1. ABI changes:
   - `DecideOut = Advice(Option<Advice>) | Call(ModelCall)`;
   - the new optional export `cox_decide_resume(DecideResume { question, events }) -> Option<Advice>`;
   - `ModelCall.target = Tier(t) | OwnProvider { name, model }`;
   - `Question.items: Vec<QuestionItem>` (one answer per item).
   
   These are minor additions: `api` stays 1. Regenerate `docs/plugin-abi.schema.json`.
2. `PluginAdvisor`: on `Call`, check that the target is a `[[provider]]` of the same plugin. Then run it through the budget gate → the provider registry (`PluginProvider`) → `Priced`, which writes one `usage` row with `job = plugin:<id>`. Then call `cox_decide_resume` with the events. The point's latency budget covers all three steps. Any failure along the way is `None`, and the local default applies.
3. `net.rs`: `cox_http` to a provider section's host is allowed only inside `cox_provider_stream`. Everywhere else it returns `NotInThisContext`.
4. PL§4: add the exports and state the context rule.
Check:
- `decide_call_out_writes_one_usage_row`: a WAT guest returns `Call`, and its own `cox_provider_stream` returns fixed events;
- `decide_call_out_over_budget_falls_back`;
- `decide_cannot_target_another_plugins_provider`;
- `http_to_provider_host_outside_provider_stream_is_refused`;
- `batched_question_answers_each_item`;
- `abi_schema_matches_committed_file`;
- invariant 8 `every_request_has_a_usage_row` still green.

#### T33.40.3 Jev provider export

Depends: T33.40.2, T33.16, T33.18 · Size: ~170 · Files: `plugins/jev/src/provider.rs`, `plugins/jev/src/lib.rs`, `crates/cox-plugin-fixtures/build.rs`
Goal: `cox_provider_stream` speaks System One over `cox_http` (PL§7a ABI form). The host injects the key.
- A decision call (`Job::Plugin("jev")` with the JSON body in its one user message) is sent verbatim.
- A request from a tier that names `typesafe` gets the old lossy mapping and one `Notice(Warn)`: "typesafe is a decision model; no tier should route to it" (J13).
- Map the status codes from J4 to the ABI error kinds, so the host's retry treats 429 and 529 as transient.
- Pass usage through unchanged; the host applies the estimate floor (T33.18).
- Add the `[[models]]` rows.
- `cox-plugin-fixtures` also builds `jev.wasm`.
Check:
- guest tests `decision_call_body_is_sent_verbatim`, `tier_request_uses_lossy_mapping_and_warns` and `status_529_maps_to_transient`;
- `cargo build -p cox-plugin-fixtures` produces both fixtures, and with the wasm target missing it fails naming `mise install`.

#### T33.40.4 Jev plugin as a provider, end to end and offline

Depends: T33.40.3, T33.7 · Size: ~180 · Files: `tests/plugins_jev.rs` (+ `tests/fixtures/jev/*.json`, fixtures)
Goal: prove the provider path through the real binary with no network and no keychain (J§8).
Plan:
1. A scratch `COX_HOME`. Install and enable the built fixture package from its local folder (C3) with `--yes`.
2. Main turns come from `COX_PROVIDER=scripted`.
3. Jev is wiremock on 127.0.0.1, reached through `[providers.typesafe] base_url` in the scratch config, with `TYPESAFE_API_KEY=test-key` in the env.
4. A test-only scripted command makes one decision call. It reuses T33.40.1's WAT harness pattern.
Check:
- `jev_request_carries_host_injected_bearer` and `jev_key_never_reaches_guest`;
- `jev_call_writes_usage_row_with_plugin_job` (provider `typesafe`, model `jev-1.13.0`, cost from the plugin catalog row);
- `jev_call_blocked_by_budget_falls_back`;
- `jev_401_is_one_notice_and_fail_open`;
- `jev_529_retries_max_retries_times`;
- `three_failures_disable_decide_export`;
- `cox plugin list --json` shows the provider and model contributions.

#### T33.40.5 Parity: the `typesafe` table configures the plugin's section

Depends: T33.40.4 · Size: ~120 · Files: `crates/cox/src/session.rs`, `crates/cox-plugin/src/provider.rs`, `crates/cox/src/doctor.rs`
Goal: while both exist, one name serves one client.
- When the `jev` plugin is loaded, the provider `typesafe` is its `PluginProvider`. The `[providers.typesafe]` table (the built-in default, or the user's) supplies `base_url`, `api_key_env`, `timeout_s`, `max_retries` and `model`, and the user wins, as for declarative sections.
- Without the plugin, the built-in `JevProvider` runs as today.
- `cox doctor` says which client serves `typesafe`.
Check: `typesafe_table_overrides_plugin_transport`, `builtin_jev_used_when_plugin_absent`, and a doctor snapshot for each case (in a scratch `COX_HOME`).

#### T33.40.6 `risk` advisor: raise only

Depends: T33.21 (as amended: ask only when the outcome could change), T33.40.1, T33.40.4 · Size: ~190 · Files: `plugins/jev/src/risk.rs`, `plugins/jev/src/lib.rs`, `tests/plugins_jev.rs`
Goal: J§5.1. There is one request per tool batch. The state is the task, the cwd, the sandbox mode and, for each call, `tool`, `subject`, `input` and `classifier_risk`, never tool output. The questions are the `severity` Score and the `irreversible`, `external_effect` and `exfiltration` Nouls. The thresholds sit in `[plugins.jev.risk]` with the J§5.1 defaults. The advice is "raise to Destructive" or none, and the core keeps `max(builtin, advised)`. `capabilities.decide` gains `risk`.
Check:
- guest tests:
  - `risk_state_never_contains_tool_output`;
  - `risk_thresholds_raise_on_any_hazard`;
  - `risk_low_answers_give_no_advice`;
  - `risk_config_overrides_thresholds`;
- e2e in `Auto` mode with a confining sandbox, where a scripted `git push --force origin main` is auto-allowed without the plugin:
  - a high fixture gives `Ask`, headless denies, and `Advised { point: risk, applied: true }` is in the rollout;
  - a low fixture leaves the call allowed;
  - a wiremock delay past 200 ms leaves it allowed with `Advised { applied: false }`;
  - `one_jev_request_per_tool_batch` (the wiremock count for a batch of 3);
- `risk_advice_cannot_lower_risk` still green.

#### T33.40.7 Eval E1: risk escalation against the classifier alone

Depends: T33.40.6 · Size: ~200 · Files: `evals/src/cox_evals/jev_risk.py`, `evals/tests/test_jev_risk.py`, `evals/risk/commands.yaml` (data)
Goal: measure J§5.1 against the no-Jev baseline at a hard budget cap.
- The corpus has about 300 labelled commands: must-ask (force push, publish, deploy, remote delete, pipe-to-shell, credential exfiltration) and benign (build, test, grep, formatting, local git). The labels and a one-line reason are in the file.
- Each command is one scripted bash call in `Auto` mode with the sandbox on. The scripted provider costs $0, and Jev is live.
- The run is repeated without the plugin as the baseline.
Plan:
1. A `cox_evals` module with a registry entry, not a script (eval-tooling rule).
2. Read the ledger total for `job = plugin:jev` after each batch, and stop at `--max-usd 0.10`.
3. Metrics:
   - the recall gain on must-ask commands the baseline auto-allows;
   - the false-raise rate on benign commands;
   - the late-fallback rate at 200 ms and at 500 ms;
   - p50 and p95 latency;
   - $.
4. The live run needs `TYPESAFE_API_KEY` from the creator. `--dry-run` runs the baseline only.
5. Record the table in R§5 with the Jev model version (`jev-1.13.0`, pinned), the date and the reproduce command.
Falsifier: if the false-raise rate exceeds 10 %, or the p95 late-fallback rate exceeds 20 %, `risk` is not recommended by default, and the user guide says so.
Check: `just test-evals` is green offline (corpus schema, metric maths, budget stop, command line). R§5 has the E1 table.

#### T33.40.9 `route` advisor: downgrade only

Depends: T33.40.8, T33.40.1, T33.40.4 · Size: ~170 · Files: `plugins/jev/src/route.rs`, `plugins/jev/src/lib.rs`, `tests/plugins_jev.rs`
Goal: J§5.2, plugin side.
- The state is the prompt, the todo list, a summary of the last turn and the number of files touched.
- The questions are the `tier` Choice over the offered tiers (with the plugin's descriptions and `other` → `code`) and the `wants_depth` Noul.
- The advice is `cheap` only when:
  - the choice is `cheap`;
  - confidence ≥ 0.8;
  - `wants_depth` < 0.3;
  - the last turn did not error.
- `capabilities.decide` gains `route`.
Check:
- guest tests `route_needs_high_confidence`, `route_keeps_code_after_error_turn` and `route_other_means_code`;
- e2e over two scripted turns:
  - a fixture choosing `cheap` at 0.9 gives a main-turn `usage` row on tier `cheap` and `Advised { applied: true }`;
  - at 0.7 the turn runs on `code`;
  - a fixture naming `think` is ignored, and the static pick runs.

#### T33.40.10 Eval E2: route downgrade against the static pick

Depends: T33.40.9 · Size: ~180 · Files: `evals/src/cox_evals/harness.py` (the `jev-route` preset), `evals/tests/test_harness.py`, `evals/tasks/11-…14-*.yaml` (data: four tasks that need the code tier, such as a multi-file rename with a failing test)
Goal: measure J§5.2 with money on the line and a hard cap. Every task runs twice, as the baseline and with `[plugins.decide] route = "jev"`, on real Anthropic tiers with live Jev.
Plan:
1. The preset is a registry entry.
2. Caps: `--budget` per run, and a total cap of `--max-usd 3.00` read from the ledger, after which the run stops.
3. Metrics:
   - pass rate;
   - $ per task by (tier, job);
   - the downgrade rate;
   - cache read/write tokens;
   - Jev p95 latency.
4. Keys (`ANTHROPIC_API_KEY`, `TYPESAFE_API_KEY`) come from the creator's env. They are never read from the keychain.
5. R§5 gets the table with the model versions and the date.
Falsifier: if any task that passes in the baseline fails with the plugin, or the median saving per task is below 15 %, `route` stays off by default, and the guide says so.
Check: `just test-evals` is green offline (preset expansion, cap stop, the paired table). R§5 has the E2 table.

#### T33.40.11 User guide: the Jev plugin

Depends: T33.40.6, T33.40.9 · Size: ~150 · Files: `docs/plugins/jev.md`, `docs/plugins.md` (link), `crates/cox/tests/doc_examples.rs`
Goal: one page for users. It covers:
- what Jev is and is not (J13);
- building it from `plugins/jev` (`just plugin jev`) and `cox plugin install <dir>` (C3);
- the key through `TYPESAFE_API_KEY` or the keyring entry `cox/typesafe`;
- enabling points in `[plugins.decide]`;
- per point, the exact state sent to TypeSafe (J17);
- costs in `cox stats` as `plugin:jev`;
- fail-open behaviour and `cox doctor` rows;
- the E1/E2 results and the defaults they justify;
- the migration from `[providers.typesafe]` (J§7).
Check: `doc_examples` parses every `toml` block on the page against `Config` or the plugin manifest schema and fails on drift.

#### T33.40.12 Remove the built-in Jev client

Depends: T33.40.5, T33.40.6 (parity: the provider and one advisor are served by the plugin) · Size: ~120 edited (the deleted `jev.rs` does not count) · Files: `crates/cox-provider/src/jev.rs` (deleted), `crates/cox-provider/src/lib.rs`, `crates/cox/src/session.rs`
Goal: C1, step 1.
- The `typesafe` arm of `backend_for_with` returns the plugin's `PluginProvider` when the plugin is loaded.
- Otherwise it returns a typed "provided by plugin jev, not loaded" error, which the router step (T33.40.13) turns into fail-open.
- `jev.rs`'s tests now live in `plugins/jev` (T33.40.2).
Check: `rg -n 'jev' crates/cox-provider/src` is empty; `typesafe_backend_without_plugin_is_typed_error`; the three standard commands green.

#### T33.40.13 Router and `ProviderId` without Jev

Depends: T33.40.12 · Size: ~120 · Files: `crates/cox-protocol/src/types.rs`, `crates/cox-core/src/router.rs`, `crates/cox-core/src/session.rs`
Goal: C1, step 2.
- `ProviderId::Jev` goes, replaced by T33.18's plugin bucket; it is never serialized in events (J§7).
- The `typesafe` pin in `Router::pick` goes; plugin sections resolve generically.
- A tier naming a legacy plugin provider (the table `("typesafe", "jev")`) with the plugin absent fails open (D14): one `Notice(Warn)` "Jev moved to a plugin: build `plugins/jev` and run `cox plugin install <dir>`", and that tier uses its `default.toml` provider and model for the session.
Check: `legacy_typesafe_tier_without_plugin_uses_default_tier_with_notice`, `typesafe_tier_with_plugin_routes_to_plugin_section`, `unknown_provider_still_errors`; `docs/protocol.jsonschema` regenerated.

#### T33.40.14 Config tombstone, `default.toml`, schema and doctor

Depends: T33.40.13 · Size: ~150 · Files: `crates/cox-protocol/src/config.rs`, `crates/cox/src/doctor.rs`, `crates/cox-protocol/default.toml` (config data; `docs/config.md` and the config schema are generated)
Goal: C1, step 3. An old config loads.
- `JevProviderConfig` becomes `LegacyTypesafe`: the same keys, still `deny_unknown_fields`, an `Option` with no default section.
- It stays a named field, so the table can never fall into the flattened `custom` map as a chat-shaped `CompatibleProviderConfig` (J§7).
- Its knobs feed the plugin's `typesafe` section. Its `models` rows join the config catalog layer.
- The `[providers.typesafe]` block leaves `default.toml`.
- The doctor's key check covers plugin provider sections generically. A leftover table without the plugin is a doctor warning with the install pointer.
Check:
- `old_typesafe_table_loads_with_notice` (a config file from before this change);
- `typesafe_table_never_becomes_compatible_section`;
- `unknown_key_in_typesafe_table_still_rejected`;
- a doctor snapshot;
- the config drift test and `every_flag_has_a_config_key` green.

#### T33.40.15 Catalog, prices and vendor script without Jev

Depends: T33.40.14, T33.16 · Size: ~110 · Files: `crates/cox-models/src/catalog.rs`, `crates/cox-models/src/price.rs`, `scripts/vendor/src/cox_vendor/models.py` (+ its tests; `prices.toml` is regenerated by the script, A48)
Goal: C1, step 4.
- The `typesafe` special cases in `Catalog::load` and `price.rs` go.
- `cox-vendor models` stops keeping `jev-latest` as a known exception, and its re-run drops the row from `prices.toml`.
- Jev's price and window now come only from the plugin row. `cox doctor` shows `source = plugin:jev`, and T30.27's price-sync row stays green.
Check: `jev_price_comes_from_plugin_row`, `catalog_without_plugin_has_no_jev_row`, the vendor pytest suite, `just vendor models --check` clean.

#### T33.40.16 Docs and plan sweep after the removal

Depends: T33.40.15 · Size: ~80 · Files: `docs/design/providers.md`, `docs/design/crates.md`, `docs/design/v0.2-jev.md` (+ `plan.md`)
Goal: no doc describes a built-in Jev.
- `providers.md` loses the Jev family.
- `crates.md` loses the `cox-provider-jev` row.
- `v0.2-jev.md` gets a closing note that the integration shipped as the `jev` plugin (A52, T33.40), with the eval verdicts.
- In `plan.md`, T32.15 is dropped with a reason, and the PL§2 example uses `name = "typesafe"`.
Check: `rg -n -i 'jev|typesafe' crates docs` matches only the plugin, the tombstone, the migration notice and the history. All docs drift tests are green.

#### T33.40.17 Optional: record live fixtures (needs the creator's key)

Depends: T33.40.6, T33.40.9 · Size: ~150 · Files: `scripts/vendor/src/cox_vendor/jev_fixtures.py`, `scripts/vendor/tests/test_jev_fixtures.py`, `tests/fixtures/jev/*.json` (data)
Goal: replace the hand-built fixtures, which follow the documented shapes, with recorded ones.
- The script sends the plugin's own `risk` and `route` question sets for a handful of fixed states.
- It redacts with the rollout scrubber, writes the bodies and records the model version and the date.
- It runs only with `TYPESAFE_API_KEY` set by the creator. It never reads the keychain.
Check: the offline pytest (body construction, redaction, no key means a clear exit) is green. With recorded fixtures, T33.40.4 and T33.40.6 stay green unchanged.

#### T33.43 Bump extism to a release on wasmtime ≥ 48 and drop the advisory ignores

Depends: an extism release after v1.30.0 that pins wasmtime ≥ 48 (extism `main` already pins 48; checked 2026-09-26) · Size: ~30 · Files: `Cargo.toml`, `Cargo.lock`, `deny.toml`
Goal: move the workspace `extism` and the direct `wasmtime` (declared only for its `anyhow` feature) to that release, then remove the `RUSTSEC-2026-0222`, `RUSTSEC-2026-0269`, `RUSTSEC-2026-0316` and `RUSTSEC-2026-0327` entries (0316 needs wasmtime >= 48.0.3, 0327 needs >= 48.0.4) from `deny.toml` `ignore` (A55, research.md P39). Also check whether the direct `wasmtime` declaration is still needed. The bump was approved in advance by the creator (A55), but only onto a published crates.io release, never a git dependency. It unblocks the WASI preopens in T33.14.2. If no such release exists by 2026-12-31, bring it back to the creator.
Check: `cargo deny check advisories` passes with no wasmtime ignores; the `cox-plugin` tests and `slim_build_has_no_wasm_runtime` pass; `scripts/footprint.sh` stays within the 20 MiB budget (PL§12).

#### T33.45 Design the plugin API: shared, terminal-only and desktop-only

Depends: — · Size: split at claim time (design doc first, then one card per API part, the docs and the examples) · Priority: P2 · Complexity: 4 · Files: `docs/design/plugins.md`, `crates/cox-plugin-api/src/{manifest,abi}.rs`, `crates/cox-plugin/src/{grant,hostfn}.rs`, `plugins/sdk/`, `plugins/examples/`, `docs/plugins.md` (+ `docs/ru/`, `docs/uk/`)

Context: plugins were built for cox in the terminal (P33, PL§1–§13). The desktop app already draws the PL§8 widget slots through `cox-app` (`plugin_ui.rs`: `panel`, `status.left`/`status.right`, `overlay`, `/<id>:<name>` commands, T52.17; `tool:`/`item:` renderers, T52.23), but nothing in the API says which surface a feature belongs to: a plugin cannot declare where it runs, `[capabilities.ui]` mixes terminal-only parts (leader keys) with shared ones, and there is no desktop-only API at all. Plugins must work both in the terminal (CLI/TUI) and in the desktop app.

Goal: one plugin API in three parts — a **shared API** for what both surfaces offer, a **terminal-only API** and a **desktop-only API** — with a defined behaviour when a plugin asks for a part the current surface does not have, then its documentation and a Rust example that uses all three.

API requirements:
- A shared API for desktop and terminal (features available in both).
- A separate API only for terminal-only features.
- A separate API only for desktop-only features.

Steps:
1. **Spell out the API structure** in a new PL§ (from the existing system, `cox-plugin-api` `manifest.rs`/`abi.rs`/`ui.rs` and `cox-plugin` `grant.rs`/`hostfn.rs`), concretely:
   - *Shared API* (both surfaces; today's `api = 1` minus the terminal-only parts): exports `cox_init`, `cox_on_event`, `cox_hook`, `cox_decide`/`cox_decide_resume`, `cox_tool_*`, `cox_provider_stream`, `cox_command`, `cox_render`, `cox_render_item`, `cox_shutdown`; manifest `[capabilities]` `events`, `hooks`, `tools`, `invoke`, `context`, `kv`, `model`, `net`, `fs`, `decide`, `[[provider]]`, `[[models]]`, `[[mcp]]`, `[[external_agents]]`, `[[agents]]`; host functions in `cox:host/v1` (`cox_log`, `cox_notify`, `cox_kv_*`, `cox_context`, `cox_invoke_tool`, `cox_model_call`, `cox_http`, `cox_output`/`cox_cancelled`, `cox_redraw`); the closed `Widget` tree with `StyleToken` roles and the `panel`, `status.left`/`status.right` and `overlay` slots and `ui.render` targets, which both surfaces draw (sized in cells: the TUI's grid, the app's `monoCode` cells).
   - *Terminal-only API*: what only a terminal has — `ui.keys`/`KeyDecl`/`cox_key` under `plugin.leader`, the status row's 24-column segment budget and narrow-terminal drop order, anything that depends on the inline viewport (D10); proposed additions are listed as such.
   - *Desktop-only API*: what only the app has — proposed, each with its native control and the DS§ catalogue row it maps to: e.g. an inspector tab slot, a toolbar item, a sidebar section, actionable notifications, palette actions with an SF Symbol, links and images in widgets; the app draws them natively from the declarative tree, never plugin-supplied UI code.
   - *Declaring surfaces*: a manifest field (proposed `surfaces = ["terminal", "desktop"]`, default both) plus surface tables (`[capabilities.terminal]`, `[capabilities.desktop]`) beside the shared `[capabilities]`; `InitIn` gains `surface` (`terminal`, `desktop`, `headless`, `acp`) so a plugin adapts at run time; the grant line (T33.6) names the surface parts it asks for.
   - *Calls to an unavailable surface*: a plugin whose `surfaces` excludes the current one is not loaded, with a notice (fail open, never fatal); a surface-only capability on the other surface is left out of `granted` and its `InitOut` entries are dropped with a notice, as an ungranted one is today; a surface-only host function returns a new `AbiError::NotOnThisSurface`, like `NotInThisContext`, never a trap; headless and ACP still call no UI export.
   - *Versioning*: the parts are additive, so they stay `api = 1` under PL§4's minor rule (unknown fields ignored both ways); surface host functions get their own namespaces (`cox:tui/v1`, `cox:desktop/v1`) so a surface part can grow without touching `cox:host/v1`; a breaking change to any part raises the ABI major; `cox plugin list` shows each plugin's surfaces.
   Record the decisions as a §6 amendment before code; split the build into cards (manifest + grant, ABI + host functions per part, TUI wiring, app wiring through `cox-app`/`cox-ffi`), each with its own Check.
2. **Once the API is built, generate the plugin API documentation**: the shared, terminal-only and desktop-only references in `docs/plugins.md` (or `docs/plugins/api.md`), generated from `docs/plugin.schema.json` and `docs/plugin-abi.schema.json` where possible, in English with `docs/ru/` and `docs/uk/` translations.
3. **Examples**: a small Rust plugin in `plugins/examples/` on `cox-plugin-sdk` that uses the shared API (a command and a status segment), the terminal-only API (a leader key) and the desktop-only API (one desktop-only contribution), and loads on both surfaces, each part showing only where it exists; built by `cox-plugin-fixtures` like `plugins/examples/rust`.

Check: the API structure section is in `docs/design/plugins.md` with its §6 amendment; manifest and ABI drift tests (`docs/plugin.schema.json`, `docs/plugin-abi.schema.json`) pass with the new fields; a `cox-plugin` test loads the example in a terminal and a desktop session and finds each surface-only part present on its own surface and dropped with a notice (or `NotOnThisSurface`) on the other; `docs/plugins.md`, `docs/ru/` and `docs/uk/` cover all three parts; `just test` green.

Progress (2026-10-03): step 1 is done — §15 of `docs/design/plugins.md` and amendment A134, approved by the creator; the build is T33.45.1–T33.45.10 below. This card closes when they are done.

#### T33.45.3 Host: load filter, granted filter, `cox:desktop/v1`

Depends: T33.45.1, T33.45.2 · Files: `crates/cox-plugin/src/hostfn.rs`, `crates/cox-plugin/src/live.rs`, `crates/cox-session/src/plugins.rs` · Design: `docs/design/plugins.md` §15, A134

Check: `plugin_outside_its_surfaces_is_skipped_with_a_notice`, `other_surface_init_entries_are_dropped_with_a_notice`, `desktop_notify_on_terminal_is_not_on_this_surface` (inline WAT, no build).

#### T33.45.4 TUI: links, image `alt`, surfaces in listings

Depends: T33.45.2, T33.45.3 · Files: `crates/cox-tui/src/plugin_ui.rs`, `crates/cox/src/plugin_cmd.rs` · Design: `docs/design/plugins.md` §15, A134

Check: insta snapshots of a linked span with and without OSC 8 and of an image's `alt`; `cox plugin list --json` shows `surfaces`.

#### T33.45.5 App core: inspector tab, toolbar and palette actions

Depends: T33.45.3 · Files: `crates/cox-app/src/plugin_ui.rs`, `crates/cox-app/src/palette.rs`, `crates/cox-app/src/live.rs` · Design: `docs/design/plugins.md` §15, A134

Check: `cox-app` tests: the tab renders only while selected, an action runs its command, an action naming an undeclared command is dropped, `plugin_keys` is gone. Coordinate with T58.4 on `live.rs`.

#### T33.45.6 App core: notifications, links and images

Depends: T33.45.5 · Files: `crates/cox-app/src/plugin_ui.rs`, `crates/cox-app/src/app.rs`, `crates/cox-app/src/wire.rs` · Design: `docs/design/plugins.md` §15, A134

Check: a plugin notice with actions reaches `Host::notify`'s test double and its action runs the command; a remote link goes through `confirm_open_url`; the app-server wire round-trips the new payloads.

#### T33.45.7 FFI and Swift: the desktop contributions

Depends: T33.45.6 · Files: `crates/cox-ffi/src/{session,types}.rs`, `desktop/macos/Packages/CoxUI/…` (Inspector, SessionToolbar, CommandPalette, PluginWidgetView), `desktop/design/DESIGN.md` §6 rows · Design: `docs/design/plugins.md` §15, A134

Check: `forward_only` passes; a snapshot per new view in light and dark; an unknown SF Symbol draws `puzzlepiece.extension` (settles §15.4's **unverified** note).

#### T33.45.8 SDK and the surfaces example

Depends: T33.45.3 · Files: `plugins/sdk/src/lib.rs`, `plugins/examples/surfaces/` (new), `crates/cox-plugin-fixtures/build.rs` · Design: `docs/design/plugins.md` §15, A134

Check: a `cox-plugin` test loads the example (a command and a status segment, a leader key, an inspector tab and a notification) in a terminal and a desktop session and finds each surface-only part present on its own surface and dropped with a notice or `NotOnThisSurface` on the other.

#### T33.45.9 Docs: the three-part plugin API reference

Depends: T33.45.1, T33.45.2 · Files: `docs/plugins.md` (or `docs/plugins/api.md`), its generator and drift test · Design: `docs/design/plugins.md` §15, A134

Check: every field of `docs/plugin.schema.json` and `docs/plugin-abi.schema.json` appears under its part (shared, terminal-only, desktop-only), checked by test.

#### T33.45.10 Docs: Russian and Ukrainian translations

Depends: T33.45.9 · Files: `docs/ru/plugins.md`, `docs/uk/plugins.md` (new directories) · Design: `docs/design/plugins.md` §15, A134

Check: both cover all three parts and every heading of the English page; `just test` green.

**Order.** T33.1 → T33.2 → T33.3 → T33.4 → T33.5 → T33.6 is the critical path. After it these can run in parallel:

- T33.7–T33.8;
- T33.9 → (T33.10, T33.11, T33.12, T33.14.1, T33.15); T33.43 → T33.14.2;
- T33.16 → T33.17 → T33.18;
- T33.19 (after T32.3) → T33.42;
- T33.20 → T33.21.

TUI: T33.22 → T33.23 → (T33.24, T33.25, T33.26). SDK and languages: T33.27 → T33.28 → T33.29 → T33.30, then T33.31 → T33.32 → T33.33; T33.34; T33.35 → T33.36; T33.37 → T33.38. Then T33.39; T33.40.1–T33.40.17 (own order below); T33.41 and T33.42 whenever they are wanted. The top table gets rows T33.1–T33.39, T33.41–T33.42 and T33.40.1–T33.40.17; P2 by default, P1 for the blockers T33.1–T33.6 and T33.40.1, P3 for the optional cards (T33.41, T33.40.17), the paid eval E2 (T33.40.10) and the Kotlin/Dart feasibility spikes (T33.35, T33.37).

**T33.40 order.**

- Main line: T33.40.1 and T33.40.2 (in parallel) → T33.40.3 → T33.40.4 → T33.40.5.
- Risk: T33.40.6 → T33.40.7.
- Route: T33.40.8, which can start after T33.20 → T33.40.9 → T33.40.10.
- Docs: T33.40.11 after T33.40.6 and T33.40.9. Its results section is filled by T33.40.7 and T33.40.10.
- Removal (C1): T33.40.12 → T33.40.13 → T33.40.14 → T33.40.15 → T33.40.16, starting once T33.40.5 and T33.40.6 are done. It does not wait for the evals.
- T33.40.17 runs whenever the creator has a key.

The paid runs are T33.40.7 (≤ $0.10, approved) and T33.40.10 (≤ $3, needs the creator's go-ahead each time).

### P34 — Subagents (goal: a subagent can be a custom named definition, capped in number, able to ask the user, and able to exchange follow-up messages with its parent and its siblings — all through the parent's own `Submission`/`Event` stream)

Rationale in §6 A53. The design doc for the messaging cards is `docs/design/subagent-messaging.md` (T34.0, cited below as SM§n).

Every card in this phase:

- stays within 200 LOC and 3 source files (generated schemas and fixtures do not count);
- leaves a test that fails without it;
- documents what it adds (`docs/design/subagent-messaging.md` if the design moves, `docs/protocol.jsonschema`/`docs/config.jsonschema` through their drift tests for new variants or keys);
- runs the three standard commands.

**Blockers** (everything after them depends on them): T34.0 (blocks T34.4–T34.9).

### P35 — External agents from plugins (Cursor first) (goal: a plugin can declare an external CLI agent that appears to the model as a subagent preset, driven over its own official headless protocol, sandboxed and grant-gated like every other plugin capability)

Rationale in §6 A54; the design is `docs/design/external-agents.md` (T35.0, cited below as EA§n). Evidence `research.md` §4.3.8 (Cursor, checked 2026-09-26).

Every card in this phase:

- stays within 200 LOC and 3 source files (manifests, generated schemas, snapshots and fixtures do not count);
- leaves a test that fails without it;
- documents what it adds (`docs/design/external-agents.md` if the design moves, `docs/plugin.schema.json` through its drift test for the new manifest capability, `docs/plugins.md` user docs from T35.9);
- runs the three standard commands.

**Blockers** (everything after them depends on them): T35.0, T35.1, T35.2, and the P33/P34 work this phase builds on — T33.6 (grants and granted-only loading, which implies T33.1–T33.5), T33.19 and T33.42 (sandboxed stdio spawn for a plugin-brought process), T34.1 (custom preset dispatch) and T34.5 (parent-routed follow-up messages).

#### T35.10 Optional: live check against a real Cursor account (needs the creator's key)

Depends: T35.7 · Size: ~130 · Files: `scripts/vendor/src/cox_vendor/cursor_live_fixtures.py` (+ its tests), `tests/fixtures/cursor/*.json` (data, recorded from one real run)
Goal: with the creator's own `CURSOR_API_KEY` and the installed CLI, one real `agent -p --output-format stream-json` and one real `agent acp` run against a scratch repo, recorded into the same fixture shape T35.7 already consumes — confirms the documented event shapes still match a real CLI release; never runs in CI, matches T33.40.17's shape.
Check: the recorded fixture round-trips through T35.7's mapper unchanged; the script's own test asserts it never touches a real key by default (opt-in env var required).

### P36 — Compound shell commands (goal: a `Bash(<prefix>:*)` rule or a session grant covers exactly the commands it names, never a command chained after them)

Rationale in §6 A62.

### P39 — Gemini (goal: `[providers.gemini]` with `GEMINI_API_KEY` runs a streamed multi-round tool loop through the shared Chat client, thought signatures included, replayable offline)

Rationale in §6 A70.

Gate: `docs/design/v0.2-gemini.md` (T19.3). Falsifiers checked on 2026-09-28 against https://ai.google.dev/gemini-api/docs/openai (page "Last updated 2026-09-02 UTC"):

- Falsifier 2 (auth) does not fire. The endpoint is `https://generativelanguage.googleapis.com/v1beta/openai/` with `Authorization: Bearer $GEMINI_API_KEY`, the same bearer shape the Chat client already sends.
- Falsifier 1 (loop features) fires partly. Streaming, tools and `image_url` data URIs are supported, and `reasoning_effort` maps to Gemini thinking levels. But the page says Gemini 3 carries thought signatures in Chat Completions, and https://ai.google.dev/gemini-api/docs/thinking (last updated 2026-09-25) says signatures must be sent back exactly as received. The Chat wire has no way to carry a signature today.
- Decision: type-2 is kept. The shared Chat client gets a generic signature passthrough (T39.1–T39.4) instead of a new type-1 wire. The field path `tool_calls[i].extra_content.google.thought_signature` is **unverified**: the page that documented it, https://ai.google.dev/gemini-api/docs/thought-signatures, is now a "moved" notice (last updated 2026-08-18). The fixture in T39.6 encodes that path, and T39.7 (live, needs the creator's key) confirms it. If T39.7 shows a signature the Chat wire cannot carry, the gate's rule applies: promote Gemini to type-1 through a §6 amendment.
- Also from the same page: "Support for the OpenAI libraries is still in beta." Vertex AI stays out of scope, as the gate says.

Every card in this phase:

- stays within 200 LOC and 3 source files (manifests, generated schemas, vendored data, fixtures and snapshots do not count);
- leaves a test that fails without it;
- regenerates `docs/protocol.jsonschema` / `docs/config.jsonschema` through their drift tests when it adds a variant or key;
- runs the three standard commands.

**Blockers:** T39.1 → T39.2 → T39.3 is the signature path; T39.5 is independent; T39.6 needs T39.3 and T39.5.

### T39.7. Optional: live check against the real Gemini endpoint (needs the creator's key)

- Model: haiku
- Depends: T39.6
- Size: ~100
- Priority: P3
- Complexity: 2
- Goal: with the creator's own `GEMINI_API_KEY`, one real two-round tool loop is recorded into the T39.6 fixture shape. This confirms or refutes the `extra_content.google.thought_signature` path. It never runs in CI, and it has the same shape as T35.10.
- Files: `scripts/vendor/src/cox_vendor/gemini_live_fixtures.py` (+ its test). Data: `crates/cox/tests/fixtures/gemini/*.sse`, recorded and redacted.
- Steps:
  1. The script is opt-in: it runs only when `COX_LIVE_GEMINI=1` is set. It records both SSE bodies and redacts the key.
  2. Re-run T39.6 against the recorded fixtures.
  3. If the signature arrives anywhere else, or a replay without it is accepted, record that in research.md and bring it back to the creator. The gate's type-1 fallback applies.
- Check:
  ```bash
  cd scripts/vendor && mise exec -- uv run pytest -q -k gemini_live && cd ../..
  mise exec -- cargo nextest run -p cox --test gemini_compat
  ```
- Done when: the fixture comes from a real response dated in research.md, or the card goes back to the creator with the mismatch.
- Out of scope: any code change to the wire. A mismatch becomes a new card.

---

### P40 — Images (goal: an image from the user or from `read` reaches every wire as `Content::Image`, bounded in size, never in the cached prefix, lossless through the archive, identical after resume)

Rationale in §6 A71.

Gate: `docs/design/v0.2-images.md` (T19.4).

- `Content::Image { media_type, data_b64 }` already exists, and all three wires already translate it:
  - Anthropic: a base64 `image` block.
  - Responses: `input_image`.
  - Chat: `image_url` data URIs.
- What is missing is everything before the wire:
  - the core drops `UserTurn.attachments` (`crates/cox-core/src/session.rs` ~764, ~1171);
  - the rollout ignores attachments;
  - `read` refuses binary files;
  - the token estimate skips images;
  - no surface can attach an image.
- The gate's `bytes | path` is resolved at the edge. A model path goes through `read`, and so through `path::confine`. The core and the wires see only bytes, so neither ever touches the filesystem (D2).

Limits used below, checked 2026-09-28:

- Anthropic, https://platform.claude.com/docs/en/build-with-claude/vision:
  - JPEG, PNG, GIF and WebP;
  - 10 MB per image base64 on the API, 5 MB on Bedrock and Google Cloud;
  - a 32 MB request;
  - at most 4784 visual tokens per image on high-resolution models, 1568 on others;
  - images may sit beside `tool_result` blocks.
- OpenAI, https://developers.openai.com/api/docs/guides/images-vision (no page date):
  - PNG, JPEG, WebP and non-animated GIF;
  - 512 MB per request.
- Gemini, https://ai.google.dev/gemini-api/docs/image-understanding (last updated 2026-09-23):
  - PNG, JPEG, WebP, HEIC and HEIF, with **no GIF**;
  - 20 MB total inline request.

cox caps one image at 3,750,000 raw bytes, which is 5,000,000 base64 bytes: the smallest documented per-image limit. It accepts only the four formats Anthropic and OpenAI share. A GIF sent to Gemini fails at the provider with the provider's own error (see open questions).

Every card in this phase: same four bullets as P39.

**Blockers:** T40.1 blocks everything. T40.5 lands before T40.6 (the strip filter is a no-op until tool images exist, so the order never breaks invariant 6).

### P41 — LSP diagnostics (goal: a deferred ReadOnly `diagnostics` tool returns file:line:col diagnostics from one sandboxed stdio LSP server per language per session, killed when the session ends)

Rationale in §6 A72.

Gate: `docs/design/v0.2-lsp.md` (T19.2). The entry point is fixed as a deferred ReadOnly tool; the core never spawns (D2). The implementation choices this phase makes:

- **Where the code lives:** `crates/cox-tools/src/lsp/`; no new crate.
- **Spawn:** the server is spawned through the same `sandboxed_argv` wrap as MCP stdio servers (`crates/cox/src/session.rs:796`, T33.42), injected into the tool as a closure because `cox-tools` cannot depend on `crates/cox`.
- **Shutdown:** a new default no-op `Tool::shutdown` that the root `Session::end()` calls.
- **Protocol subset:**
  - `initialize`/`initialized`;
  - `didOpen`/`didChange`/`didSave`;
  - push `publishDiagnostics` with a quiet period, or pull `textDocument/diagnostic` when the server advertises `diagnosticProvider`;
  - `shutdown`/`exit`.
- **Falsifier 1** (no server for a language): a tool error naming `bash` and the project's own checker.
- **Falsifier 2** (volume): the normal archive and truncate path, with no new channel.

LSP crates checked on the crates.io API on 2026-09-28:

| Crate | Latest | Released | Verdict |
| --- | --- | --- | --- |
| `lsp-types` | 0.97.0 | 2024-06-04 | Stale |
| `async-lsp` | 0.2.4 | 2026-04-24 | Pins `lsp-types` ^0.95 |
| `lsp-server` | 0.10.0 | 2026-07-16 | rust-analyzer's own; sync, crossbeam, server-side |
| `ls-types` | 0.0.6 | 2026-03-08 | tower-lsp-community fork, 0.0.x |
| `tower-lsp-server` | 0.23.0 | — | Server-side |

None is a maintained, async, client-side fit, so framing (~60 LOC) and the few wire types are hand-rolled on `serde_json` and `tokio`. The one exception is `url` for file URIs (see Open questions).

Every card in this phase: same four bullets as P39.

**Blockers:** T41.1, T41.2 and T41.5 can run in parallel. Then T41.3 → T41.4 → T41.6 → T41.7 → T41.8.

### P42 — Architect/editor modes (goal: `/mode architect|editor` and `--mode` switch a named preset over permission mode and main tier, with no second loop and no change to the cache prefix)

Rationale in §6 A73.

Design decision carried by every card: a mode is a preset over the **permission mode** and the **main tier** only. It never removes tools from system[0] (that would break invariant 1 mid-session); "write tools denied / bash read-only only" is delivered by `PermissionMode::Plan` through `cox_permission::Engine`, which already allows `Risk::ReadOnly` (including classifier-safe `bash`) and denies the rest. `/mode` can only narrow the configured permission mode (`narrower`), so a mode never widens what `permissions.mode` allows.

### P43 — Repo map (goal: a git-recency-ranked symbol map built once per session, placed last in the byte-stable system[2], refreshed only by `/repomap refresh` or compaction)

Rationale in §6 A74.

Creator's decision (2026-09-28): built ONCE at session start; ranked by recent git changes, not prompts; in the byte-stable cache prefix; refreshed only by an explicit `/repomap refresh` (a deliberate, announced prefix change) or at compaction (which already restarts the cache); never rebuilt automatically mid-session.

Prerequisite finding: system[2] today is the stub `INSTRUCTIONS` constant (`context.rs`, "stub until T7.1") and the core passes an empty skills index; instruction files never reach the request. T43.3 adds the first real system[2] content path; open question 8 asks whether instruction files/skills get their own card on the same path.

### T43.6. Bench the map on and off

Model: claude-sonnet-5 · Status: in progress · Depends: T43.5 · Size: ~60 · Priority: P3 · Complexity: 3

Goal: falsifier 1 of the gate — does the map pay for its tokens — and the default budget.

Files:
- `evals/` bench preset (existing registry module, one file)
- `research.md`
- `crates/cox-protocol/src/config.rs` (only if the default changes)

Steps:
1. Add a bench preset pair `repomap-off` / `repomap-2k` to the existing `evals/` registry (no one-off script).
2. `just bench` both on the same tasks; record tool-call count, input tokens, cache-read tokens, pass rate with the command, date and commit in `research.md`.
3. If the map saves tokens net, set the default (e.g. 2000) and regenerate schemas; otherwise keep 0 and note the result in the gate doc.

Check:
```bash
just bench
mise exec -- cargo nextest run --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when: `research.md` has the numbers with sources and the default is set from them.

Out of scope: ranking changes.

---

### P44 — Worktrees (goal: one session ↔ one worktree made visible and enforced, and no worktree is ever created without the user saying so)

Rationale in §6 A75.

Already shipped by T27.3: `cox --worktree <name>`, `Presence.worktree` and `PresenceHook::with_worktree`, the `⧉` status segment, `offer_worktree_removal`. P44 closes the remaining gate items.

### P45 — Subagent extras (goal: per-agent permission narrowing, grant-gated plugin agent definitions, and `@name` invocation from the composer)

Rationale in §6 A76.

Prerequisite finding: `AgentTool::call` clones `parent.config`, and `Session::build` sets the child's mode from `config.permissions.mode`, so a child ignores the parent's live mode (Shift+Tab to plan still spawns an `auto` child). T45.1 fixes that first; T45.2 narrows from there.

### P46 — TUI extras (goal: a user command can own one status row, and a theme can be edited with live preview and saved without leaving cox)

Rationale in §6 A77.

Every card in this phase stays within 200 LOC and 3 source files (manifests, generated schemas, snapshots and fixtures do not count), leaves a test that fails without it, and runs the three standard commands. Order: T46.1 → T46.2; T46.3 in parallel; T46.4 after both. T46.5 → T46.6 → T46.7.

Decisions this phase fixes (from Claude Code's documented behaviour, https://code.claude.com/docs/en/statusline, checked 2026-09-28, adapted to cox's guards):

- The script's first stdout line is one extra row above the built-in T28.1 status row; the built-in segments stay (Claude Code does the same: "renders in its own row above the built-in footer badges and does not replace them").
- Triggers: whenever the built-in status input changes (model, ctx, cost, mode, sandbox, git, busy), debounced 300 ms (Claude Code: 300 ms); a new trigger kills the in-flight run; optional `refresh_s` timer (0 = off, else ≥ 1, as Claude Code's `refreshInterval`).
- Timeout 2 000 ms (`timeout_ms`, 100–10 000). Non-zero exit, timeout or empty output blanks the row (Claude Code: "go blank"); never fatal (D14).
- stdin is JSON that reuses Claude Code's field names where the meaning is the same, so an existing script runs unchanged (D4): `session_id`, `cwd`, `workspace.current_dir`, `workspace.project_dir`, `model.id`, `model.display_name`, `cost.total_cost_usd`, `context_window.used_percentage`, `context_window.context_window_size`, `version`; cox adds `permission_mode`, `sandbox_mode`, `git.branch`. `COLUMNS` is set to the terminal width, as Claude Code does.
- The output is untrusted: `cox_sanitize::sanitize` strips every escape sequence, so ANSI colours and OSC 8 links in the script's output are dropped (the row is drawn in `theme.dim`). No second sanitizer.
- The command runs under `cox_sandbox::sandbox::command` with a read-only policy and no network (the session's own policy only when the user chose `danger-full-access`), env cleared to `CHILD_ENV_ALLOWLIST` plus `COLUMNS`. A host whose sandbox cannot build the command disables the row with one warning; it never runs unsandboxed.
- A project `.cox/config.toml` cannot set `tui.status_line.command` (added to the project-config guard list): it would run on every TUI start in a cloned repo before any prompt. Claude Code gates the same key behind workspace trust.
- Theme editor (Crush, `charmbracelet/crush` at `ae84854`, checked 2026-09-28): in the Themes dialog `ctrl+e` opens `ThemeEditor` on the highlighted theme (`theme.go` `EditTheme` binding); it lists palette slots with a swatch and an inline input; `up/down/tab` move; every keystroke previews live; an invalid colour is flagged and the old one kept (`applyInput`); `enter`/`ctrl+s` saves a user theme file (`base` + palette) in the config dir's `themes/`; `esc` reverts. cox copies that flow onto its own T24.2 files: `/theme` picker, `Ctrl+E` on the highlighted colour theme, one row per `Theme` token for the variant in use, `Enter` saves, `Esc` reverts. Built-ins cannot be shadowed by a user file (`State::theme_catalog` doc), so editing a built-in saves `<name>-custom`. Crush's rename/revert/delete keys (`ctrl+r`/`ctrl+d`/`ctrl+x`) are out of scope.

### P47 — MCP elicitation (goal: an MCP server's `elicitation/create` reaches the person through the T22.1 question modal in the TUI and `--plain`, and is declined by construction everywhere no person can answer)

Rationale in §6 A78.

Evidence (checked 2026-09-28): the current MCP revision is 2026-07-28, which replaces server-initiated `elicitation/create` with Multi Round-Trip Requests — the server returns an `InputRequiredResult` carrying `elicitation/create` in `inputRequests`, and the client retries with `inputResponses` (https://modelcontextprotocol.io/specification/2026-07-28/changelog, major change 7; https://modelcontextprotocol.io/specification/2026-07-28/client/elicitation). rmcp 3.4.0 (in `Cargo.lock`) negotiates `2025-11-25` by default (`ProtocolVersion::LATEST`) and handles both shapes through one hook: `ClientHandler::create_elicitation` answers a 2025-11-25 server request, and `Peer::call_tool` routes 2026-07-28 `input_required` rounds through the same local handler (`src/service/client.rs`). cox today serves `()` as its client handler, whose default declines and declares no `elicitation` capability — so no server may elicit. Client MUSTs from the spec: show which server asks; clear decline and cancel; for form mode let the user review and modify before sending; for URL mode show the full URL and host, get consent, never auto-open or prefetch.

Decisions:

- Form mode only in T47.1–T47.3; URL mode is T47.4 (P3).
- The capability is declared only where a person can answer: the TUI and `--plain` (both already pass a question channel to `session::open`). `cox run -p` declares nothing, so a conforming server never elicits; a non-conforming one gets `decline`. `--answer` is not applied: it is free text for `ask_user`, not a typed form.
- ACP: ACP v1 now has its own `elicitation/create` with `clientCapabilities.elicitation.{form,url}` (https://agentclientprotocol.com/protocol/v1/elicitation.md), and `agent-client-protocol-schema` 1.9.1 in the lock carries it. But `cox acp` sessions connect no MCP servers today (`crates/cox/src/acp_cmd.rs` builds `session::tools` plus client tools only), so there is nothing to forward: ACP declares nothing. Forwarding over ACP `elicitation/create` is the path the day ACP sessions get MCP servers; it is not a card here.
- No new guard: `cox_permission::Engine` already allowed the MCP call; the person is the gate for what they type. Server name, message, titles and options are shown through the modal, which already runs `sanitize` on every string. Answers go to the server only — never into the transcript, the rollout or the model's context.
- A person answering must not trip `mcp.timeout_s`: the call's deadline stops counting while a question is open.

### P48 — `trycmd` fixtures for `cox run -p` (goal: the full output of `cox run -p` in text, json, stream-json, a denied write and a bad format is a reviewed fixture, not hand-parsed asserts)

Rationale in §6 A79.

`trycmd` 1.2.1 (crates.io API, checked 2026-09-28: published 2026-07-21, MIT OR Apache-2.0, `rust_version` 1.85 vs cox's 1.98, repository assert-rs/snapbox) is a new dev-dependency for cox. It is not a new crate for the workspace root: `apps/rtok` pins `trycmd = "1.2.1"` and `apps/ketch` `"1.2"`, and `rust.md` lists it under Tests with the rule "full command output goes through `trycmd` fixtures … `assert_cmd` + `predicates` stay for exit codes and partial matches". Against the plan.md testing note of 2026-09-17: it is not in cox's "Already covered" list, but the shared catalog it points to lists it as already in use across apps and it duplicates nothing cox has (`insta` covers TUI frames, `assert_cmd` partial matches). New transitive crates in `Cargo.lock`: `snapbox`, `humantime`, `humantime-serde` (and their small deps). The divan benchmark from the same `ideas.md` line is excluded (benchmarks are out).

### P49 — Later scope gates (goal: remote control, Windows sandbox, voice input, MCP Apps and Cursor's Cloud Agents API each get a one-page gate doc with a verdict; no runtime code)

Rationale in §6 A80.

Same shape as P19 (T19.1–T19.7, `docs/design/v0.2-*.md`): each doc opens with the two `//!` lines, then `## Problem` (one measurable number), `## The field` (primary sources with URL and date checked; secondary ones marked **unverified**), `## cox` (the entry point through existing seams, and which existing guard — `cox_permission::Engine`, `cox_sandbox::path::confine`, `cox_sandbox::sandbox::Policy`, `cox_sanitize::sanitize` — covers it; never a second one), `## Falsifiers`, `## Review` (verdict: build / defer / reject; a "build" verdict proposes cards as a §6 amendment draft for the creator, it does not add them). Named `docs/design/v0.3-<name>.md`: the v0.2 lines are all gated (`roadmap.md` v0.2) and these come after. Out of scope for the whole phase: any change under `crates/`; any new dependency.

Common check, with `$f` the card's doc:

```bash
f=docs/design/v0.3-<name>.md
test -f "$f"
for h in '## Problem' '## The field' '## cox' '## Falsifiers' '## Review'; do grep -q "^$h" "$f" || exit 1; done
test "$(grep -c 'checked 2026-' "$f")" -ge 3
test -z "$(GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=false git status --porcelain -- crates)"
```

### P50 — Gaps found while planning P39–P49 (goal: the model sees the project's instruction files and skills, and a subagent never runs with wider permissions than its parent)

Rationale in §6 A81.

### P31 — Beta readiness (goal: the v0.1 definition of done in §4 holds for everything cox can prove without a paid key)

Rationale in §6 A50. T31.1–T31.5 are in `done.md`; T31.2 landed as a no-op (see A50 and its done.md card — T30.23 had already made Jev construction fallible). Still open against §4, all outside the code: the paid eval run and the cache-read ratio (T30.3, a funded `ANTHROPIC_API_KEY`), and a signed macOS release (the `MACOS_CERTIFICATE` / `MACOS_CERTIFICATE_PWD` repository secrets).

### P37 — Native macOS client, M1 (goal: a SwiftUI app over the cox crates that is a complete daily driver — sessions, transcript, approvals, review, settings — with the view layer free of business logic and every visual built from one token set and one component catalogue)

Rationale in §6 A67. Design: `docs/design/desktop.md` (DT§n); view layer and tokens: `desktop/design/DESIGN.md` (DS§n) and `desktop/design/tokens/`; evidence `research.md` §9; mockups `desktop/design/mockups/`.

Every card in this phase:
- keeps business logic in Rust (`cox-session`, `cox-app`); Swift views render view state and send intents (DS§1);
- uses tokens only in SwiftUI — no literal colour, size, font, radius, shadow or duration outside `Tokens/` and `Foundations/` (DS§9);
- builds each visual from the DS§6 catalogue, adding a variant or a catalogue row in the same change rather than a copy;
- leaves a test that fails without it (`insta` in Rust, swift-snapshot-testing in Swift);
- is split at claim time when it exceeds 200 LOC or 3 files — the catalogue cards (T37.19–T37.21) always are;
- adds its crate's row to the `AGENTS.md` layout table when it creates a crate (T37.1, T37.8, T37.14); T37.14 also updates `AGENTS.md` "What this is" and `docs/how-it-works.md` "The four surfaces" to name the app.

Swift dependencies are in `research.md` §9.5 and A67; a new one needs the same check (most used, maintained, licence compatible with both GPLv3 and the royalty-free option, A68) or our own package with its own card.

#### T37.32 Signing, notarization, Sparkle, bundled CLI, Homebrew cask

Split into T37.32.1 and T37.32.2 (A106). Depends: T37.15 · Size: ~150 · Files: `.github/workflows/release.yml`, `desktop/macos/Cox.xcodeproj/…`, `scripts/desktop/…`
Goal: a Developer ID-signed, notarized app with Sparkle 2 updates and the `cox` CLI inside the bundle (DT§7). New dependency Sparkle (§1.1 row).
Check: `spctl --assess` accepts the release build; the appcast validates.

#### T37.32.2 Developer ID signing, notarization, Sparkle, bundled CLI, Homebrew cask

Depends: T37.32.1 · Size: ~150 · Files: `.github/workflows/release.yml`, `scripts/desktop/…`
Goal: the rest of T37.32 (A106): a Developer ID-signed, notarized app with Sparkle 2 updates and the `cox` CLI inside the bundle (DT§7), and a Homebrew cask. New dependency Sparkle (§1.1 row). Waits for the creator's Developer ID certificate, App Store Connect API key, Sparkle EdDSA key, appcast host and tap repository as GitHub Actions secrets.
Check: `spctl --assess` accepts the release build; the appcast validates.

The bundle id is `io.github.pyrlyn.cox`, confirmed by the creator (A135, T37.32.3). The Developer ID import (`.github/actions/macos-signing`) and the app's re-signing and DMG (`scripts/desktop/dmg.sh`) exist since T37.32.3; this card adds Hardened Runtime, notarization, Sparkle, the bundled CLI and the cask.

#### T37.32.3 Debug macOS app DMG on CI, every feature, Developer ID-signed

Depends: T37.32.1 · Size: ~200 · Files: `.github/workflows/desktop-build.yml`, `.github/actions/desktop-macos/action.yml`, `.github/actions/macos-signing/action.yml`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/desktop/dmg.sh`, `desktop/macos/project.yml`, `justfile`
Goal: the first build of the macOS app someone can download from CI and run (A135): Debug, not a release build, with every macOS feature the app has; started by hand (Actions -> desktop build) and by every release, which attaches the DMG.
Plan:
1. Survey (Explore agent, 2026-10-03): no feature needs an Info.plist usage key or an entitlement (no microphone, Apple Events, URL scheme or sandbox; notifications, Keychain, App Intents, Spotlight, hotkeys, menu-bar extra, WebKit and the PTY need none); the Rust core links with its plugin host on; there is no desktop voice UI and the Swift side never looks for a bundled CLI, so neither is added (the CLI in the bundle stays T37.32.2). The gap was signing: ci.yml builds with `CODE_SIGNING_ALLOWED=NO` and nothing packages the app.
2. `.github/actions/macos-signing`: release.yml's Developer ID import moved into a composite action (one copy), with `required: false` falling back to ad-hoc for a manual run without the secrets.
3. `.github/actions/desktop-macos`: Xcode 27, the Metal toolchain, the Rust pin, CoxFFI.xcframework and the arm64 check, shared by ci.yml's `desktop-macos` job and the new workflow.
4. `scripts/desktop/dmg.sh` (`just desktop-dmg`): re-signs `Cox.app` inside-out with `COX_SIGN_IDENTITY` or ad-hoc, keeping the build's identifier, entitlements and flags; checks it (`codesign --verify --deep --strict`, Developer ID authority when signed); writes `Cox-<version>-<build>-debug-arm64.dmg` with an /Applications link, signed and verified.
5. `.github/workflows/desktop-build.yml` (`workflow_dispatch` and `workflow_call`): build number = run number; artifact `desktop-macos-dmg`. release.yml's `desktop` job calls it with the tag's commit and `require-signing`, and `publish` uploads the DMG and its checksum with the CLI archives.
6. Bundle id `io.github.pyrlyn.cox` in project.yml.
Check: actionlint 1.7.12 on the three workflows and shellcheck 0.11.0 on `dmg.sh` add no finding; `dmg.sh` run ad-hoc on a locally built Debug `Cox.app` signs the nested debug dylibs, keeps `get-task-allow`, and writes a DMG `hdiutil verify` accepts; the PR's `desktop-macos` job passes through the shared action; after the merge, Actions -> desktop build produces a Developer ID-signed DMG whose app opens on another Mac after Open Anyway.
Out of scope: Hardened Runtime, notarization, Sparkle, the bundled CLI, the app cask (T37.32.2).

#### T37.33 Performance budget suite

On hold by the creator (A107). Depends: T37.23 · Size: ~120 · Files: `justfile`, `desktop/macos/Benchmarks/…`, `research.md`
Goal: `just desktop-bench` measures cold start, first frame of a 2 000-block session, stream frame time and memory against DT§1 budgets; results go into `research.md`.
Check: the suite runs locally and in the nightly job; every budget has a measured row.

### P51 — Desktop M2 (goal: the rest of the terminal inside the app — a sandboxed terminal pane, a browser pane the agent can read and screenshot, pop-out windows and tabs, a menu-bar extra with a global hotkey, Spotlight and App Intents, and per-hunk revert — plus the dark glass look; DT§3.2)

Plan (A121): one agent implements P51's cards in table order on branch `p51-roadmap`, one commit per card, without building or running tests (the creator's instruction); T51.1 waits for the creator's approval of the dark renders. A verification pass builds and tests each commit before the branch merges into `p37-desktop`.

Rationale in §6 A121. Design: `docs/design/desktop.md` DT§3.2, §4, §10; view layer `desktop/design/DESIGN.md`; approved looks: mockups 24-terminal-pane-m2, 25-browser-preview-m2, 26-menu-bar-extra-m2 in `desktop/design/mockups/mockups.html`. Starts after the P37 cards that touch the same files.

Every card in this phase keeps the P37 rules (business logic in `cox-app`, `cox-ffi` a one-expression forwarder per A90, tokens only in SwiftUI, a catalogue row for each new visual, a test that fails without it) and:
- Swift never spawns a process, never touches `~/.cox` or git, never decides a permission (DT§4.6);
- every model- or page-originated string passes `cox_sanitize::sanitize` in Rust before it becomes a block or reaches the model (DT§10);
- a Swift card that draws an M2 screen compares it with its mockup by `npm run diff` in `desktop/design`;
- a new dependency is named in its card and gets its §1.1 and `toolchain.md` rows in the same commit.

### P52 — Desktop M3 (goal: beyond a single agent — Claude Agent, Codex, Gemini CLI and Cursor sessions in the same sidebar over ACP, best-of-n across models in worktrees, plugin panels drawn natively from the `Widget` tree, and remote sessions over SSH through `cox app-server`; DT§3.3)

Rationale in §6 A121. Design: DT§3.3, §4.4 (patch types are serde so the same stream can go over a socket), §4.7 G9, §10; PL§8 (`Widget` tree); EA (external agents, P35) and `crates/cox-acp` (`client.rs`, `terminal.rs`, T35.3/T35.11) and `crates/cox-session/src/external_agents.rs` (T35.13). Approved look: mockup 27-external-agents-acp-m3. Starts after P51.

Every card keeps the P51 rules, and:
- an external agent's own tool calls are guarded by the process sandbox T35.2's wrap put it under; only what it asks cox for (`session/request_permission`, `fs/*`, `terminal/*`) meets `cox_permission::Engine` and `cox_sandbox::path::confine` (EA§2, T35.3); cox's provider keys are never in its environment (`CHILD_ENV_ALLOWLIST`);
- an external agent's usage is its own billing: no `usage` row is invented for it, and the cost pill shows "—" as the mockup does;
- a remote session never sends API keys or the Keychain's secrets over the wire and never forwards the ssh agent;
- plugin widgets reach the screen only through `cox_sanitize::sanitize` and PL§8's limits, as in the TUI.

### P53 — Plugin distribution (goal: a plugin installs from git or a URL through the same validation and per-digest grant as a local folder, and the plugin API, the Rust SDK and the Go SDK are ready to publish once `api = 1` is frozen)

Rationale in §6 A121. Design: `docs/design/plugins.md` PL§1 (package, digest, install, update), §4 (ABI and versioning), §9, §12, §13. Install cards (T53.1–T53.4) come first and do not wait for the ABI freeze.

Every card in this phase:
- ends every install in the existing local-directory install: validate the manifest, digest the tree, copy into `versions/<digest12>/`, ask for the grant — no second path, no plugin runs before its grant;
- treats a downloaded or cloned tree as untrusted repository content: nothing in it runs during install, symlinks and paths that leave the staging directory are refused;
- stops before anything outward-facing: no `cargo publish`, no tag push, no release — the card prepares metadata, docs and a dry run, and the creator publishes.

#### T53.5 Freeze ABI `api = 1`

Depends: T33.14.1, T33.14.2, T33.18, T33.34, T33.40.1, and the creator's confirmation that the ABI is stable · Size: ~100 · Files: `docs/plugin-abi.v1.schema.json` (frozen copy), the compatibility test in `crates/cox-plugin-api`, `docs/design/plugins.md` §4
Goal: PL§12 falsifier 3 has been checked by the Jev plugin (T33.40.1) and the open ABI cards have landed, so `api = 1` is frozen: a committed copy of the v1 ABI schema and a test that the current `docs/plugin-abi.schema.json` only adds optional fields, exports and host functions to it (PL§4's minor-change rule); PL§4 records the freeze.
Check: `mise exec -- cargo nextest run -p cox-plugin-api abi_v1_changes_are_additive_only abi_schema_drift`.

#### T53.6 `cox-plugin-api` ready for crates.io

Depends: T53.5 · Size: ~80 · Files: `crates/cox-plugin-api/Cargo.toml`, `crates/cox-plugin-api/README.md` (new), `release-plz.toml`
Goal: `description`, `readme`, `documentation`, `keywords`, `categories`, `include` and the licence (`MIT OR Apache-2.0`, A68) set; rustdoc on every public item with a compiling example; the name checked free on the crates.io API (URL and date in the commit message); release-plz configured to publish this crate only when the creator runs it, the rest of the workspace stays `publish = false`. Stops before `cargo publish`.
Check: `mise exec -- cargo publish --dry-run -p cox-plugin-api` succeeds; `mise exec -- cargo doc -p cox-plugin-api --no-deps` has no warnings; `cargo package --list -p cox-plugin-api` lists no test fixtures.

#### T53.7 `cox-plugin-sdk` ready for crates.io

Depends: T53.6 · Size: ~80 · Files: `plugins/sdk/Cargo.toml`, `plugins/sdk/README.md` (new), `plugins/Cargo.toml`
Goal: the same metadata and docs for the guest SDK; its `cox-plugin-api` dependency carries a version beside the path; the name checked free. Stops before `cargo publish`; its full dry run needs `cox-plugin-api` on crates.io, which the creator publishes first.
Check: `cargo package --list --no-verify` in `plugins/` lists the expected files; `cargo doc --no-deps -p cox-plugin-sdk` has no warnings; after the creator publishes `cox-plugin-api`, `cargo publish --dry-run -p cox-plugin-sdk` succeeds.

#### T53.8 Go SDK module ready to tag

Depends: T33.34, T53.5 · Size: ~70 · Files: `plugins/sdk-go/go.mod`, `plugins/sdk-go/README.md`, `plugins/sdk-go/doc.go`
Goal: the Go SDK from T33.34 as a module at its repository path (`github.com/pyrlyn/cox/plugins/sdk-go`) with package docs and an example; the tag it needs (`plugins/sdk-go/v0.1.0`) is written in the README and the card's report for the creator to push. Stops before the tag push.
Check: `go vet ./...` and `go test ./...` in `plugins/sdk-go` (toolchain from `plugins/mise.toml`); `GOFLAGS=-mod=mod go list -m` prints the module path.

#### T53.9 Templates and docs use the published SDKs

Depends: T53.7, T53.8, and the creator's publish of both crates and the Go tag · Size: ~60 · Files: `plugins/templates/rust/Cargo.toml.tmpl`, `plugins/templates/go/go.mod.tmpl`, `docs/plugins.md`
Goal: `cox plugin new` generates a plugin that depends on the published `cox-plugin-sdk` version and Go module tag instead of a path into this repository, and `docs/plugins.md` shows the published names; the in-repo examples keep their path dependencies.
Check: `mise exec -- cargo nextest run -p cox plugin_new_` (template snapshots re-recorded on purpose); `just plugin-examples rust` and `just plugin-examples go` build a freshly scaffolded plugin.

### P54 — Voice input (goal: push-to-talk in the TUI turns speech into a prompt on this machine with local whisper, and submits it when the draft was empty; no audio leaves the machine, nothing is downloaded without the user asking)

Rationale in §6 A123 (3). Design: `docs/design/v0.3-voice.md` option 2 (local whisper in its own crate).

New dependencies (crates.io API, checked 2026-09-29): `whisper-rs` 0.16.0 (Unlicense; https://crates.io/crates/whisper-rs, released 2026-03-12; repository https://codeberg.org/tazz4843/whisper-rs, last commit 2026-03-14, not archived; already pinned `=0.16.0` by `apps/runa` and listed in the workspace `rust.md`), which builds whisper.cpp (MIT) through `whisper-rs-sys` 0.15.0 (Unlicense) with cmake and a C++ compiler; `cpal` 0.18.2 (Apache-2.0; https://crates.io/crates/cpal, released 2026-08-16; https://github.com/RustAudio/cpal, last commit 2026-09-20; not yet in `rust.md`); `rubato` 5.0.0 (MIT OR Apache-2.0; https://crates.io/crates/rubato, released 2026-08-10; in `rust.md`). Program: `cmake` through mise. Models: ggml files from https://huggingface.co/ggerganov/whisper.cpp (licence MIT; `ggml-tiny.en.bin` 77,704,715 bytes, `ggml-base.en.bin` 147,964,211, `ggml-small.en.bin` 487,614,201; Hugging Face API, checked 2026-09-29), never in the repository.

Every card in this phase:
- stays within 200 LOC and 3 source files (manifests, generated schemas, snapshots, fixtures and the docs rows a card must add do not count, as in P35);
- keeps `whisper-rs`, `cpal` and `rubato` inside `cox-voice` (a `deps.rs` rule), and `cox-voice` behind the `voice` cargo feature of `crates/cox`, off by default, so the C++ build never becomes a requirement of every build;
- never sends audio anywhere, never writes it to disk, the rollout or the ledger (local transcription is not a request and has no `usage` row);
- never downloads a model on its own: only `cox voice model download`, after the user confirms;
- runs `cox_sanitize::sanitize` on a transcript before it reaches the composer; `cox_permission::Engine` is not involved (dictation calls no tool). No new guard;
- adds its new dependency's `toolchain.md` row (and the workspace `rust.md` row when the crate is new there) in the same commit.

### P55 — MCP Apps, option (a) (goal: cox never advertises or renders an MCP App UI and shows the text and structured result of such a tool unchanged, test-backed)

Rationale in §6 A123 (4). Design: `docs/design/v0.3-mcp-apps.md` option (a); (b) deferred, (c) waits for ACP.

### P56 — Cursor Cloud Agents as a background-task backend (goal: a granted plugin's `[[cloud_agents]]` entry runs a background task as a Cursor Cloud Agent run that survives the laptop closing, with the off-machine step approved through `Engine` and a $0 `billed_externally` usage row carrying Cursor's tokens)

Rationale in §6 A123 (5). Design: `docs/design/v0.3-cursor-cloud.md`; the local-CLI sibling is P35 (EA).

**Terms go-ahead given (creator, 2026-10-03, A123 (5)); local work only.** The go-ahead starts P56 without a real Cursor key or any network call in tests; T56.10 stays untouched until the creator supplies a key. It answers the terms reading in A123 (5): the Acceptable Use Policy prohibits accessing the Service by automated or non-human means, by bot, script or otherwise (https://cursor.com/acceptable-use-policy, last updated 2026-08-11, checked 2026-09-29, paraphrased), which read literally covers any program that calls the API. The go-ahead is recorded in A123.

Every card in this phase:
- stays within 200 LOC and 3 source files (manifests, generated schemas, snapshots, fixtures and the docs rows a card must add do not count);
- never copies, vendors or generates from Cursor's OpenAPI file: wire types are hand-written from the public endpoint docs (A40 step 3), citing the page URL and date checked in the module header;
- never puts the user's name, email or git identity in a request: bodies carry the prompt, the GitHub repository URL with any credentials stripped, the starting ref and the model; the User-Agent is `cox/<version>`; `/v1/me` is never called;
- reads the key from `CURSOR_API_KEY` through `cox_provider::http::resolve_key` (tests inject `resolve_key_with`; no test touches a real key or keychain) and never writes it to a log, an error or the rollout;
- runs every string from Cursor (stream text, tool names, artifact names, errors) through `cox_sanitize::sanitize`;
- makes no network call in tests (wiremock and hand-written fixtures only, D12).

#### T56.6 Host driver: a background task becomes a Cursor Cloud run

Depends: T56.2, T56.3, T56.4, T56.5 · Size: ~190 · Files: `crates/cox-session/src/cloud_agents.rs` (new), `crates/cox-session/src/lib.rs`; manifest `crates/cox-session/Cargo.toml`
Goal: each granted `[[cloud_agents]]` entry registers as a background-only subagent preset through the path P35's external agents use. On dispatch the driver resolves the repository's GitHub remote (any other host is refused before any call; credentials in the URL are stripped), warns in the approval text when local `HEAD` is not on the remote, asks `Engine` for `CloudAgent(<repo>)`, creates the run, stores it (T56.5), and maps the stream onto the T34.8 task events: `TaskCreated` at create, assistant text accumulated into the result item, and `TaskCompleted` at `done` or a terminal status. Cancelling the task cancels the run. Every string passes `sanitize`.
Check: `mise exec -- cargo nextest run -p cox-session cloud_agent_refuses_a_non_github_remote cloud_agent_strips_credentials_from_the_remote cloud_agent_denied_by_engine_makes_no_request cloud_agent_stream_maps_to_task_events cloud_agent_task_cancel_cancels_the_run` (wiremock and a fixture repository).
Done when: the tests pass.
Out of scope: foreground (blocking) runs; follow-up messages into a run (a later card if wanted).

#### T56.7 Usage row and resume

Depends: T56.6 · Size: ~160 · Files: `crates/cox-session/src/cloud_agents.rs`, `crates/cox-session/src/lineage.rs`
Goal: at a terminal state the driver reads the run's usage and writes one `usage` row through the path an external agent's reported usage already takes (EA§6; `external_agent_turn_writes_a_billed_externally_usage_row` in `cox-core`): `billed_externally`, cost $0, Cursor's input, output, cache read and cache write tokens, the requested model (or `cursor:auto` when none was named), the task's job tag; `mark_usage_recorded` makes it exactly once. On resume, a session reattaches to its non-terminal runs, and finalizes runs that finished while cox was not running (result item and usage row), which is the number the gate doc asks for.
Check: `mise exec -- cargo nextest run -p cox-session cloud_agent_usage_row_is_billed_externally_with_cursor_tokens cloud_agent_usage_row_is_written_once resume_reattaches_to_a_running_cloud_run resume_finalizes_a_run_that_finished_while_closed`.
Done when: the tests pass.
Out of scope: pricing the tokens in USD (the spend is on the user's Cursor plan).

#### T56.8 Offline end-to-end over hand-written fixtures

Depends: T56.7 · Size: ~160 · Files: `crates/cox/tests/cloud_agents.rs` (new); fixtures `tests/fixtures/cursor-cloud/*.json`, `*.sse` and a fixture plugin package with a `[[cloud_agents]]` entry
Goal: the real binary against a scratch `COX_HOME`, a wiremock server standing in for `api.cursor.com` (host override for tests only, never a manifest field), and a fixture repository with a GitHub remote: `cox run -p --output-format stream-json` dispatches a background cloud task, the approval is asked (`--answer` allow), the task events appear in the stream, the ledger holds one `billed_externally` row, and no request body contains the fixture's git user name or email.
Check: `mise exec -- cargo nextest run -p cox cloud_agent_headless_run_end_to_end cloud_agent_requests_carry_no_git_identity`.
Done when: the tests pass offline with no key.
Out of scope: a live account (T56.10).

#### T56.9 Docs: cloud agents for users and in EA

Depends: T56.8 · Size: ~90 · Files: `docs/plugins.md`, `docs/design/external-agents.md`, `docs/design/v0.3-cursor-cloud.md`
Goal: `docs/plugins.md` documents `[[cloud_agents]]`, the grant, the `CloudAgent(<repo>)` rule and exactly what leaves the machine; EA gains a section for the cloud path next to the local CLI; the gate doc's Review links the shipped cards.
Check: `mise exec -- cargo nextest run -p cox-plugin-api plugin_schema_matches_committed_file` still passes; every relative link in the three docs names a file that exists (`grep -o '](\.\?[^)]*)'` over them, each path tested with `test -e`).
Done when: the docs are merged with the code.
Out of scope: marketing copy.

#### T56.10 Optional: live check against a real Cursor account (needs the creator's key)

Depends: T56.8, the creator's key and a scratch GitHub repository the creator names · Size: ~120 · Files: `scripts/vendor/src/cox_vendor/cursor_cloud_live.py` (new), `scripts/vendor/tests/test_cursor_cloud_live.py` (new); fixtures `tests/fixtures/cursor-cloud/live-*.{json,sse}` (data)
Goal: with an opt-in env var and the creator's `CURSOR_API_KEY`, one real run on the scratch repository, its stream and usage recorded into T56.8's fixture shape with ids and the key redacted; confirms the hand-written types still match the beta API. Never in CI; mirrors T35.10.
Check: the recorded fixture replays through T56.8's test unchanged; the script's own test asserts it refuses to run without the opt-in env var.
Done when: the fixture is committed and the replay passes.
Out of scope: any automated or scheduled live call.

### P57 — Windows build of the core (goal: the workspace builds and its tests run on Windows, and `cox` runs there under D7's rule — no sandbox, a loud warning, `on-request` forced)

Rationale in §6 A127. Evidence: `research.md` §10 (R10.n). P58's desktop client stands on this phase.

Every card in this phase:
- leaves Unix behaviour unchanged: Unix code moves behind `cfg(unix)`, the Windows path is `cfg(windows)`, and a Unix test that passed before still passes;
- adds no Windows sandbox backend: that stays deferred (A123 (2), `docs/design/v0.3-windows-sandbox.md`), and nothing relaxes D7's forced prompts;
- keeps A49 on Windows: no test reads or writes the Windows Credential Manager;
- is split at claim time when it exceeds 200 LOC or 3 files;
- has no benchmark or measurement card: none is planned for P57 or P58.

A new dependency in this phase (process-wrap, T57.5) needs the creator's approval (A127 open question 5) and its §1.1 and `toolchain.md` rows in the same change.

#### T57.1 Windows CI job: `cargo check` over a growing crate list

Depends: — · Size: ~60 · Files: `.github/workflows/ci.yml`, `justfile`
Goal: a `windows` job on `windows-latest` (Windows Server 2025, R10.6.2) that runs `just windows-check`: `cargo check` for the crates that already build on Windows (R10.1.10), listed once in the `justfile`. Each later P57 card adds its crates to the list in the same change. The job is required for a merge to `main` only when T57.11 lands. Whether mise and `.github/actions/rust` run on a Windows runner is unverified (R10.8); this card settles it or adds the Windows setup step.
Check: the job is green on the branch; `just windows-check` lists its crates in one place; `mise exec -- cargo check --workspace` on macOS is unchanged.
Remaining (A130): the code is merged (commit 7b1297ed / 6c57b93d) and `just windows-check` passes locally through cargo-xwin; the Windows-host agent runs the Check on Windows, then enables the CI job (`if: false` today).

#### T57.4 `path::confine` on Windows paths

Depends: T57.1 · Size: ~180 · Files: `crates/cox-sandbox/src/path.rs`
Goal: `confine` is the only guard between a model's path and the disk on Windows (no sandbox), so it must reject every Windows escape: drive-letter and name case (compare case-insensitively), `\\?\` and `\\.\` prefixes (which turn off `..` resolution), UNC paths, reserved device names (`CON`, `NUL`, `COM1`…, also with an extension), alternate data streams (`file:stream`), 8.3 short names and junctions resolving outside the roots (R10.4.6). Unix behaviour is untouched. Pure string cases are tested on every host; junction and short-name cases run in the Windows job only.
Check: `mise exec -- cargo nextest run -p cox-sandbox confine_rejects_verbatim_prefix confine_rejects_reserved_device_names confine_rejects_alternate_data_streams confine_is_case_insensitive_on_windows confine_rejects_a_junction_out_of_the_root` (the last two under `cfg(windows)`, run by T57.1's job).

#### T57.5 One kill path for process trees: hooks and the session environment

Depends: T57.1 (process-wrap 10.0.1 approved, A128 (2); add it to `rust.md` and `toolchain.md` in the same change) · Size: ~180 · Files: `crates/cox-ext/src/hooks.rs`, `crates/cox-session/src/env.rs`, `crates/cox-ext/Cargo.toml`
Goal: today a hook and the login-shell probe start in their own process group and time out with `killpg` (R10.1.6). Replace both with process-wrap 10.0.1 (R10.4.3): a process group on Unix, a job object on Windows (`TerminateJobObject` ends the whole tree, R10.4.2), one helper shared by both call sites and by T57.6. The login-shell probe itself (R10.1.7) becomes `cfg(unix)`; on Windows the session uses the process environment as it is (R10.8 notes this is unverified), and the card records what Windows gives. nix drops out of `cox-ext` and `cox-session` on Windows.
Check: `mise exec -- cargo nextest run -p cox-ext -p cox-session hook_timeout_kills_the_grandchild login_shell_timeout_kills_the_tree`; both crates join `just windows-check`.

#### T57.6 The same kill path for the lsp server, the status line and external agents

Depends: T57.5 · Size: ~120 · Files: `crates/cox-tools/src/lsp/server.rs`, `crates/cox/src/status_line.rs`, `crates/cox-session/src/external_agents.rs`
Goal: the three remaining `process_group(0)` spawns (R10.1.6) use T57.5's helper, so a Windows child tree dies with its job. External agents still refuse to start with no sandbox backend, as today (R10.1.9); that refusal is kept and its message names D7.
Check: `mise exec -- cargo nextest run -p cox-tools -p cox-session -p cox lsp_server_shutdown_kills_the_tree status_line_timeout_kills_the_tree external_agent_refuses_without_a_backend`.

#### T57.7 `cox-tools` compiles on Windows

Depends: — · Size: ~80 · Files: `crates/cox-tools/Cargo.toml`, `crates/cox-tools/src/bash/mod.rs`, `crates/cox-tools/src/lib.rs`
Goal: nix moves to `[target.'cfg(unix)'.dependencies]` in `cox-tools`; the pty, `setsid`, `killpg`, poll and termios code goes behind `cfg(unix)`; on Windows `bash` returns a typed "not available on Windows yet" error until T57.8. Nothing else changes. `cox-tools` joins `just windows-check`.
Check: T57.1's job passes with `cox-tools` in the list; `mise exec -- cargo nextest run -p cox-tools` on macOS is unchanged.
Remaining (A130): the code is merged (commit 6c57b93d) and passes `just windows-check` through cargo-xwin; the Windows-host agent runs the Check on Windows.

#### T57.8 `bash` on Windows: ConPTY inside a job object

Depends: T57.2, T57.5, T57.7 · Size: ~200 · Files: `crates/cox-tools/src/bash/windows.rs` (new), `crates/cox-tools/src/bash/mod.rs`, `crates/cox-tools/Cargo.toml`
Goal: the `cfg(windows)` `bash` runs the T57.2 shell on a ConPTY through portable-pty 0.9 (already in the workspace; `NativePtySystem` is ConPTY on Windows, R10.4.1), with the child in a job object through T57.5's helper so a timeout or cancel ends the whole tree. Output capture, the byte cap, archiving and background shells keep the shared code paths; only spawning, reading and killing differ. No sandbox is applied (D7); T57.3's forced policy asks first.
Check: in the Windows job, `cargo nextest run -p cox-tools windows_bash_runs_and_captures_output windows_bash_timeout_kills_the_tree windows_bash_background_shell_is_listed_and_killed`.

#### T57.9 `cox-app` and `crates/cox` compile on Windows

Depends: T57.5 · Size: ~100 · Files: `crates/cox-app/src/terminal.rs`, `crates/cox-app/Cargo.toml`, `crates/cox/src/self_update.rs`
Goal: `cox-app`'s terminal kill path uses T57.5's helper and nix moves to `cfg(unix)`; `self_update`'s `PermissionsExt` becomes `cfg(unix)`; the Unix-only tests (symlinks, file modes; R10.1.10) are marked `cfg(unix)` in the same change or in a sub-card when they exceed the 3 files. `cox-app` and `cox` join `just windows-check`.
Check: T57.1's job passes with `cox-app` and `cox` in the list; `mise exec -- cargo nextest run -p cox-app -p cox` on macOS is unchanged.

#### T57.10 Home, config and keys on Windows

Depends: T57.9 · Size: ~80 · Files: `crates/cox-config/src/load.rs`, `crates/cox-session/src/doctor.rs`
Goal: `cox_home()` resolves to `%USERPROFILE%\.cox` when `HOME` is unset (it falls back to `.` if both are, R10.1.8; that fallback becomes an error on Windows). keyring 4.2.0 already stores keys in the Windows Credential Manager (R10.4.5), so no dependency changes. `cox doctor` names the key store as "Windows Credential Manager" and the sandbox as "none (D7): every command asks". No test touches the Credential Manager (A49).
Check: `mise exec -- cargo nextest run -p cox-config -p cox-session cox_home_uses_userprofile_without_home doctor_names_credential_manager_on_windows doctor_reports_no_sandbox_under_d7` (the lookups injected, so they run on every host).

#### T57.11 Full workspace on Windows: check, clippy and nextest

Depends: T57.4, T57.6, T57.8, T57.9 · Size: ~60 · Files: `.github/workflows/ci.yml`, `justfile`
Goal: the Windows job runs `cargo clippy --workspace --all-targets -- -D warnings` and `cargo nextest run --workspace` instead of the crate list, and the "minus Windows" matrix override in `ci.yml` (R10.1.1) is removed. A test that cannot run on Windows is `cfg(unix)` with a one-line why, never ignored.
Check: the Windows job is green on `main`; `just windows-check` is removed or points at the workspace.

#### T57.12 Windows release target in cargo-dist

Depends: T57.11 (x64 and ARM64, A128 (4)) · Size: ~20 · Files: `dist-workspace.toml`, `.github/workflows/release.yml` (regenerated by `dist init`)
Goal: add `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` (A128 (4)) and the `powershell` installer to cargo-dist 0.32.0 (R10.1.2, R10.6.1). Signing, publishing and the release itself stay the creator's steps; cargo-dist's Windows signing covers x86_64 only (R10.6.1).
Check: `dist plan` lists the Windows artifacts; `dist build --artifacts=local --target x86_64-pc-windows-msvc` succeeds in the Windows job.

#### T57.13 End to end: the real binary on Windows

Depends: T57.8, T57.11 · Size: ~150 · Files: `crates/cox/tests/windows_e2e.rs` (new), `crates/cox/tests/scenarios/windows-*.toml` (data)
Goal: `cox run -p` against a scratch `COX_HOME` and the scripted provider on the Windows runner: one turn that reads, edits and runs a shell command, asserting the D7 security notice, that the command asked under the forced `on-request`, the rollout on disk and a `usage` row. No network, no key.
Check: in the Windows job, `cargo nextest run -p cox --test windows_e2e`.

**Order.** T57.1, T57.3 and T57.7 start at once; T57.2 waits on the creator. Then T57.4 and T57.5 (after the process-wrap approval) → T57.6, T57.8, T57.9 → T57.10 and T57.11 → T57.12 and T57.13.

### P58 — Windows desktop client, M1 (goal: a WinUI 3 app over `cox-ffi` with the DT§3.1 feature set — sessions, transcript, approvals, review, settings — with no logic re-implemented in C#, every visual built from the same design tokens, and D7's no-sandbox warning always visible)

Rationale in §6 A127. Evidence: `research.md` §10 (R10.n). The macOS client (P37, `docs/design/desktop.md`, DT§n) is the model: same `cox-app` core, same FFI surface, same fixtures. Scope is M1 only (DT§3.1); M2 and M3 (terminal pane, browser pane, pop-out windows, tray and hotkey, ACP host, best-of-N, plugin panels) are an idea in `ideas.md`, not planned. T58.1 is a gate: no other P58 card is claimed before it passes.

Every card in this phase:
- keeps business logic in Rust (`cox-session`, `cox-app`); C# renders view state and sends intents. C# never spawns a process, reads or writes `~/.cox` or git, or decides a permission (DT goal 1, DT-3); a decision found missing in `cox-app` is moved there first (T58.4);
- uses tokens only in XAML and C#: no literal colour, size, font, radius or duration outside the generated resource dictionary (T58.8), mirroring DS§9;
- builds each visual from one control catalogue (T58.3), adding a style or template there rather than a copy;
- gives every interactive element an `AutomationProperties.Name` (Narrator and T58.29 both depend on it);
- leaves a test that fails without it (`insta` in Rust, xUnit in C#; UI automation or snapshot per T58.29 and T58.30);
- is split at claim time when it exceeds 200 LOC or 3 files (manifests, project files, generated bindings, generated XAML, fixtures and snapshots do not count);
- names any new package in §1.1 and `toolchain.md` in the same change; candidate versions are in §1.1's A127 note (R10.2, R10.7).

Packaging, signing and a Store listing are the creator's steps; T58.28 prepares, it never publishes.

#### T58.1 Gate: C# bindings for `cox-ffi` generate and round-trip

Depends: a uniffi-bindgen-cs release on uniffi 0.32 (PR #176 open, R10.2.2) with async callback interfaces returning `Task<T>` (issue #165 open, R10.2.3); checked 2026-09-29 · Size: ~120 · Files: `crates/cox-ffi/Cargo.toml`, `scripts/desktop/csharp.sh` (new), `desktop/windows/Cox.Core.Tests/RoundTrip.cs` (new)
Goal: pattern of T33.43 (A55). `cox-ffi` adds `cdylib` to its crate types (C# loads `cox_ffi.dll` through P/Invoke; D1 names only a static library, see A127); `scripts/desktop/csharp.sh` builds it for `x86_64-pc-windows-msvc` and runs the pinned uniffi-bindgen-cs into `desktop/windows/Cox.Core/Generated/`. The version in use is recorded in §1.1 and `toolchain.md`. A test creates an `App` over the scripted provider, opens a session, sends a prompt, receives patches, and implements `AppHost` in C# including one async method. cox-ffi's uniffi 0.32.2 is not changed. If no such release exists by 2026-12-31, `cox-ffi` does not move back to uniffi 0.31; it keeps waiting for upstream and carries a fork under `forks/` with PR #176 applied until a release ships (A127 open question 1, resolved by A129).
Check: in the Windows job, `scripts/desktop/csharp.sh && dotnet test desktop/windows/Cox.Core.Tests` passes `App_opens_a_session_and_streams_patches` and `AppHost_async_method_is_awaited`; the generated sources are reproducible (a second run leaves `git diff` empty).

#### T58.2 Design doc for the Windows client

Depends: T58.1 · Size: ~250 lines of prose · Files: `docs/design/desktop-windows.md` (new), `AGENTS.md`, `docs/design/desktop.md` (§1 pointer only)
Goal: the Windows counterpart of DT, short where DT already decides: the C# project layout and its dependency rules; threading (`DispatcherQueue` for patches, the FFI runtime owned by Rust); how each DT§3.1 feature maps to WinUI controls; the Fluent mapping of the tokens; notifications and badge; packaging options (T58.28); testing (T58.29, T58.30); trust boundaries (DT§10 plus D7's missing sandbox). It proposes D1's new wording (a C# app loading `cox-ffi` as a DLL) for the creator, and answers or lists A127 open questions 3, 4 and 7. `AGENTS.md` names `desktop/windows/`.
Check: every R10, DT§ and A-number reference resolves to an existing row or section; the creator approves the doc before T58.10.

#### T58.3 Solution layout under `desktop/windows/`

Depends: T58.1 · Size: ~150 · Files: `desktop/windows/Cox.sln` (new), `desktop/windows/Directory.Packages.props` (new), `desktop/windows/global.json` (new); project files per the list below
Goal: mirror `desktop/macos/Packages` (DT§6): `App` (WinUI 3 entry, window, resources), `Cox.Core` (generated bindings, T58.1), `Cox.Model` (stores, the counterpart of CoxModel), `Cox.UI` (controls catalogue and views), `Cox.Transcript` (the transcript list), `Cox.Platform` (host bridge, notifications, packaging hooks) and `Cox.Tests`. `global.json` pins the .NET SDK (10.0 LTS, R10.2.6); central package versions in `Directory.Packages.props`; `Cox.Model` references no WinUI assembly so its tests run headless. A `desktop-windows` CI job builds the solution and runs `dotnet test`.
Check: `dotnet build desktop/windows/Cox.sln -c Debug` and `dotnet test desktop/windows/Cox.Tests` in the Windows job; a test asserts `Cox.Model` has no reference to `Microsoft.WindowsAppSDK`.

#### T58.5 C# client contract, fixture client and the session store

Depends: T58.3 · Size: ~200 · Files: `desktop/windows/Cox.Model/CoreClient.cs` (new), `desktop/windows/Cox.Model/SessionStore.cs` (new), `desktop/windows/Cox.Tests/SessionStoreTests.cs` (new)
Goal: the counterpart of `CoreClient.swift`: `ICoreClient` / `ISessionClient` over the generated types, a `FixtureCoreClient` that replays `desktop/macos/Fixtures/*.json` (the same files; they move to a shared `desktop/fixtures/` only if the creator asks), and `SessionStore` applying `TimelinePatch`es into an observable block list (CommunityToolkit.Mvvm candidate, R10.7.1).
Check: `dotnet test desktop/windows/Cox.Tests --filter SessionStore` passes `Replaying_read_and_reply_yields_the_recorded_blocks` and `Patch_replace_keeps_block_identity`.

#### T58.6 The other stores

Depends: T58.5, T58.4 · Size: split at claim time, one sub-card per store · Files: `desktop/windows/Cox.Model/*Store.cs`, their tests
Goal: sidebar, composer, settings, inspector tabs, search, onboarding and inbox stores, each the counterpart of the CoxModel store of the same name, holding view state only and sending intents.
Check: one xUnit test per store over the fixture client.

#### T58.7 Live client: `LiveCoreClient`, dispatcher and host bridge

Depends: T58.5 · Size: ~180 · Files: `desktop/windows/Cox.Model/LiveCoreClient.cs` (new), `desktop/windows/Cox.Platform/HostBridge.cs` (new), `desktop/windows/Cox.Tests/LiveCoreClientTests.cs` (new)
Goal: the real client over `cox-ffi`: patches arrive on the FFI runtime and are posted to the UI thread through `DispatcherQueue`, coalesced per frame; a consumer that stops reading never stalls a turn (DT§4.5; the Rust test already exists). `HostBridge` implements `AppHost`: `open_url` with the DT§10 confirmation for non-`https` schemes, and secrets written through the one path the CLI reads (keyring, the Windows Credential Manager, R10.4.5). The M2 browser methods return "not available".
Check: `dotnet test --filter LiveCoreClient` passes `Scripted_turn_reaches_the_ui_thread_in_order` and `Non_https_link_asks_before_opening` with the scripted provider build and a scratch `COX_HOME`.

#### T58.8 Design tokens as a XAML resource dictionary

Depends: T58.3 · Size: ~120 · Files: `desktop/design/style-dictionary.config.mjs`, `desktop/design/xaml/format.mjs` (new), `desktop/design/xaml/format.test.mjs` (new); manifest `desktop/design/package.json` (test glob); generated `desktop/windows/App/Tokens.g.xaml`
Goal: the same DTCG source (`desktop/design/tokens/*.json`) that produces the Swift tokens and `tokens.css` also produces a `ResourceDictionary` with `ThemeDictionaries` for Light, Dark and HighContrast, mapped onto the Fluent theme resources where a token has a Fluent counterpart (accent, surface, stroke, text) so system controls follow. The accent uses the cox token accent, as on macOS; a future "follow the system accent" setting may come later (A127 open question 7, resolved by A129). One source of truth: no token is edited in XAML.
Check: `npm --prefix desktop/design run build` writes `Tokens.g.xaml` and leaves `git diff` empty on a second run; `npm --prefix desktop/design test` runs the formatter test, which asserts every colour token has Light, Dark and HighContrast values.

#### T58.9 Windows Fluent mockups

Depends: T58.8 · Size: ~200 · Files: `desktop/design/mockups/windows.html` (new), `desktop/design/mockups/render.sh`, `desktop/design/mockups/README.md`
Goal: as T37.44.x did for macOS: one page, screens selected by hash, drawn from `tokens.css` with Fluent structure (title bar with Mica, `NavigationView` sidebar, command bar, `InfoBar` for D7, content dialogs, Segoe UI Variable), covering every DT§3.1 feature plus the D7 banner, in Light, Dark and High Contrast; `render.sh` gains a page option (today it always loads `mockups.html`) and renders them at 2x. The creator approves the mockups before T58.10–T58.27 start; each visual card is then compared with its screen (T58.30, or by eye until then).
Check: `desktop/design/mockups/render.sh` with the Windows page and its screen ids writes one PNG per screen and mode; the macOS screens still render unchanged; the README lists the Windows screens.

#### T58.10 App shell: window, Mica, navigation

Depends: T58.7, T58.9 · Size: ~180 · Files: `desktop/windows/App/MainWindow.xaml`, `desktop/windows/App/MainWindow.xaml.cs`, `desktop/windows/App/App.xaml`
Goal: one window per workspace at 1440×900 clamped to the screen (A126 (3)), Mica backdrop on Windows 11 with the solid fallback elsewhere and in High Contrast (R10.3.6), sidebar and content split, title bar showing project and branch.
Check: T58.29's smoke test opens the window; its size is clamped on a 1280×720 test display.

#### T58.11 Sidebar: projects and sessions

Depends: T58.10, T58.6 · Size: ~180 · Files: `desktop/windows/Cox.UI/Sidebar.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Projects and sessions sidebar" and "Several live sessions at once": "Needs you" and "Running" on top, projects with sessions newest first, switching never stops a session.
Check: xUnit over the sidebar store; the smoke test switches sessions while one runs.

#### T58.12 Transcript blocks

Depends: T58.10 · Size: split at claim time, one sub-card per block family · Files: `desktop/windows/Cox.Transcript/*`
Goal: DT§3.1 "Streaming transcript": user, assistant, thinking, tool calls, approvals, questions, notices, compaction and subagent blocks in an `ItemsRepeater` with stable keys from the patch ids, a truncated output expanding in place (lossless rule). No benchmark card (P57's rules).
Check: per block family, a fixture renders with the expected automation tree.

#### T58.13 Markdown: `StyledDoc` to `RichTextBlock`

Depends: T58.12 · Size: ~180 · Files: `desktop/windows/Cox.Transcript/StyledDocView.cs` (new), a test
Goal: render the neutral `StyledDoc` that `cox-render` already produces (headings, lists, code with its highlight spans, links shown with their target) into `RichTextBlock` runs; C# parses no markdown.
Check: `dotnet test --filter StyledDoc` maps every span kind of a fixture doc.

#### T58.14 Composer

Depends: T58.12 · Size: split at claim time · Files: `desktop/windows/Cox.UI/Composer*`
Goal: DT§3.1 "Composer": multi-line, `@file` and `/command` completion from `cox-app`'s `Completer`, `!` shell mode, paste and drop of images and files, queue while busy.
Check: xUnit over the composer store; the smoke test sends a prompt.

#### T58.15 Approvals

Depends: T58.12 · Size: ~180 · Files: `desktop/windows/Cox.UI/ApprovalCard.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Approvals": the inline card plus the pinned copy above the composer; allow once, allow for session showing the grant, deny with reason, edit then run; the reason text comes from `cox-app`.
Check: the smoke test approves the `approve-write` fixture's write.

#### T58.16 Questions (`ask_user`)

Depends: T58.12 · Size: ~120 · Files: `desktop/windows/Cox.UI/QuestionCard.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Questions": options as buttons plus a free-text field.
Check: xUnit: choosing an option sends the answer intent.

#### T58.17 Permission mode, model and effort controls

Depends: T58.10 · Size: ~150 · Files: `desktop/windows/Cox.UI/SessionToolbar.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Permission mode and model": command-bar controls whose change is echoed as typed state. With no sandbox the mode control shows the forced `on-request` (T57.3) and cannot select a laxer one.
Check: xUnit: a `StateChanged` patch updates the control; the laxer policy is disabled when the session reports no sandbox.

#### T58.18 Review

Depends: T58.12 · Size: split at claim time · Files: `desktop/windows/Cox.UI/Review*`
Goal: DT§3.1 "Review": changed files, unified and side-by-side diff from the `cox-render` diff model, revert a file to a checkpoint, comment on a line to send it to the agent.
Check: xUnit over the review store; a fixture diff renders both layouts.

#### T58.19 Rewind timeline

Depends: T58.12 · Size: ~150 · Files: `desktop/windows/Cox.UI/RewindGutter.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Rewind timeline": marks per turn; restore code, conversation or both; edit and resend a past prompt.
Check: xUnit: each choice sends the matching `Rewind` intent.

#### T58.20 Inspector

Depends: T58.10 · Size: split into T58.20.1–T58.20.5 at claim time · Files: `desktop/windows/Cox.UI/Inspector*`
Goal: DT§3.1 "Inspector" tabs: Changes, Plan, Context & Cost, Tasks, Info, each over the data `cox-app` already exposes for the macOS inspector (T37.29.x).
Check: per tab, an xUnit test over its store.

#### T58.21 Search and command palette

Depends: T58.10 · Size: ~180 · Files: `desktop/windows/Cox.UI/CommandPalette.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Search": a palette over actions, sessions and files plus full-text over past sessions (`rollout_search`); shortcuts Ctrl+K, Ctrl+N and Ctrl+Shift+R, the Windows counterparts of A126 (2).
Check: xUnit over the palette store; the smoke test opens it with Ctrl+K.

#### T58.22 Settings and MCP servers

Depends: T58.10, T58.4 · Size: split at claim time · Files: `desktop/windows/Cox.UI/Settings*`
Goal: DT§3.1 "Settings" and "MCP servers": fields generated from `docs/config.jsonschema` through the same `cox-app` field model the macOS app uses; each field shows its source; API keys are written through `HostBridge` (T58.7); MCP status and OAuth login in the browser.
Check: xUnit: a field's source label comes from `source_of`; a key write never reaches a file under `COX_HOME`.

#### T58.23 Onboarding and doctor

Depends: T58.10 · Size: ~150 · Files: `desktop/windows/Cox.UI/Onboarding.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Onboarding and doctor": pick a folder; the `cox doctor` checks as a checklist, including T57.10's "Windows Credential Manager" and "sandbox: none (D7)" and T57.2's shell.
Check: xUnit: a failed doctor check shows its fix text.

#### T58.24 Notifications and badge

Depends: T58.7, T58.28 · Size: ~180 · Files: `desktop/windows/Cox.Platform/Notifications.cs` (new), a test
Goal: DT§3.1 "Notifications" through `AppNotificationManager` (R10.3.4, R10.3.5): turn done with cost, budget warning, and the approval notification with Allow once, Deny and Open (A126 (4)), its arguments carrying the session and request ids; the taskbar badge counts items waiting. Not available when the app runs elevated (R10.3.4): the app says so once. Whether a text box for "deny with reason" and the badge work for the packaging model T58.28 picks is unverified (R10.8) and checked here.
Check: xUnit: the builder output for an approval has the three buttons and the ids; activating Allow once sends the decision intent.

#### T58.25 D7 no-sandbox banner

Depends: T57.3, T58.10 · Size: ~80 · Files: `desktop/windows/Cox.UI/NoSandboxBanner.xaml`, `.xaml.cs`, a test
Goal: an `InfoBar` (severity Warning) on every session with no sandbox, driven by T57.3's `Level::Security` notice, not by a C# platform check: "No sandbox on Windows: every command asks first (D7)". It cannot be dismissed for good; it collapses to a toolbar badge after the first turn.
Check: xUnit: a session whose notice list holds the D7 notice shows the banner; one without does not.

#### T58.26 Resume, fork and hand off

Depends: T58.11, T58.12 · Size: ~150 · Files: `desktop/windows/Cox.UI/SessionActions.cs` (new), a test
Goal: DT§3.1 "Resume and fork": open any past session, fork at a turn, hand off with an objective.
Check: xUnit: each action sends its intent; the smoke test resumes a fixture session.

#### T58.27 New session: in place or a new worktree

Depends: T58.11 · Size: ~120 · Files: `desktop/windows/Cox.UI/NewSessionDialog.xaml`, `.xaml.cs`, a test
Goal: DT§3.1 "Worktree per session": the choice in a `ContentDialog`; the title bar shows the branch; worktree creation stays in `cox-app`.
Check: xUnit: "new worktree" sends the worktree intent and no C# git call exists (a test scans `Cox.*` for `Process.Start`).

#### T58.28 Packaging

Depends: T58.10 · Size: ~100 · Files: `desktop/windows/App/App.csproj`, `desktop/windows/App/Package.appxmanifest`, `scripts/desktop/windows-package.ps1` (new)
Goal: packaged with external location (R10.3.1) — a sparse package giving the app identity for actionable notifications, installed by cox's own installer rather than through the Store, with no full MSIX container virtualization (A127 open question 3, resolved by A129). The bundled `cox.exe` ships next to the app, as `Cox.app/Contents/Helpers/cox` does on macOS (DT§7). Signing, a Store listing and publishing are the creator's steps.
Check: the Windows job produces the package; installing it on the runner and launching passes T58.29's smoke test.

#### T58.29 UI automation smoke test

Depends: T58.10 · Size: ~180 · Files: `desktop/windows/Cox.UITests/Smoke.cs` (new), `desktop/windows/Cox.UITests/Cox.UITests.csproj` (new)
Goal: FlaUI over UIA3 (R10.7.1, R10.7.2; WinAppDriver is unmaintained, R10.7.3) drives the app against the scripted provider and a scratch `COX_HOME`: open a project, send a prompt, see the D7 banner, approve, review, rewind (the XCUITest smoke path of DT§8). Each later card extends it.
Check: `dotnet test desktop/windows/Cox.UITests` in the Windows job.

#### T58.30 Snapshot spike: WinUI controls to PNG

Depends: T58.12 · Size: ~120 · Files: `desktop/windows/Cox.Tests/Snapshots.cs` (new), `desktop/windows/Cox.Tests/Cox.Tests.csproj`
Goal: whether a WinUI control can be rendered to PNG in a test through `RenderTargetBitmap` and compared with Verify (R10.7.1; unverified, R10.8), in Light, Dark and High Contrast. If yes, visual cards add snapshots and the diff against T58.9's mockups runs through the existing `npm run diff` (T37.44.4); if no, the card records why and visual checks stay by eye plus T58.29.
Check: the spike's test passes or the card records the failure with its cause.

**Order.** T58.1 (gate) and T58.4 first; then T58.2, T58.3 → T58.5, T58.8 → T58.7, T58.9 → the creator approves the mockups → T58.10 → the feature cards T58.11–T58.27 in parallel, T58.28 after its open question, T58.29 with T58.10, T58.30 with T58.12.

### P59 — Empryo-derived improvements (goal: compaction keeps every touched path without asking the model to remember it, tool output costs fewer tokens, and an edit learns its own new diagnostics; each card proves its gain with `just bench` or a test)

Rationale in §6 A132. Idea-only, clean-room: Empryo is BSL 1.1, no code copied. Source: the study of [proxysoul/Empryo](https://github.com/proxysoul/Empryo) (formerly SoulForge) at `669ff91`; each card cites Empryo files for the idea only, and the implementation is written from the card. Line numbers are at `ef07970`.

**Order.** T59.1, T59.2 and T59.3 first, in parallel. Then T59.4 (after T43.6 lands its numbers), T59.5, T59.6, T59.7. Then T59.8, T59.9 and T59.10 last. T59.11 (outline spans) is done; T59.8 depends on it.

Where each of the 14 portable ideas from the study lands in cox:

1. PageRank map — T59.4.
2. Edge IDF / confidence — T59.4 (edge weights).
3. Git co-change — T59.4 (cox already has `recent_changes`, `crates/cox-tools/src/git.rs:78`).
4. Blast radius under a budget — does not fit: cox has no impact tool and no symbol graph between sessions; rtok T377 serves it to cox over MCP.
5. Trigram index — does not fit: `cox-search` greps live with `grep-searcher`, and nothing persists an index between sessions; rtok T378 is gated on a measured need first.
6. Clone detection — does not fit: no persisted index to run MinHash over, and no measured question that it answers.
7. Grep → symbol intercept — does not fit: cox owns its `grep` tool and has no symbol index; the agent can call `outline` directly, and rtok T369 covers hosts it hooks.
8. Post-edit diagnostics delta — T59.3.
9. Backend fallback / LSP hygiene — T59.3 (never spawn from an edit; a dead server is skipped, not retried) and T59.6.
10. Compound tools — T59.5 (`project` check), T59.7 (`rename_symbol`).
11. Deterministic compaction state — T59.1.
12. Memory RRF / file affinity — T59.10.
13. Edit robustness — already there: `edit` has the whitespace-insensitive line-window fallback (`crates/cox-tools/src/edit.rs:162-227`); no card.
14. Shell compress / tee — T59.2 (fold repeated lines); the "tee" half is the existing archive (`cx.archive`).

#### T59.4 Repo map ranked by a file graph (PageRank + git recency + co-change)

Model: sonnet · Status: open · Depends: T43.6 · Size: ~200 · Priority: P2 · Complexity: 4

Goal: under the same byte budget, the map shows the files the next commit touches more often than the recency-only order, by at least 15 pp on a 200-commit backtest, and stays a pure function of file bytes, git order and budget (P43's byte-stable rule).

Files:
- `crates/cox-tools/src/repomap.rs`
- `crates/cox-tools/src/rank.rs` (new)
- `crates/cox-tools/src/git.rs`

Steps:
1. `rank.rs`: build a file graph from the outline tags already extracted for `render` (`repomap.rs:64`; `cox-syntax` `outline.rs:68`): edge A → B when A mentions a name defined in B, weight `ln(N/df)` of the name (names in more than 5 % of files are dropped). PageRank, damping 0.85, 20 iterations, ties in path order (Empryo idea: `repo-map.ts`, `repo-map-utils.ts`).
2. Seed the personalization vector from `recent_changes` (`git.rs:78`) and add co-change edges from the same `git log` call (commits touching > 20 files are skipped, pairs with count ≥ 2). No prompt input: the map stays built once per session.
3. `order` (`repomap.rs:53`) sorts by rank when `repomap.rank = "graph"`; default stays `"recent"` until the backtest row is in `research.md`.

Check:
```bash
just bench
mise exec -- cargo nextest run --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when: a PageRank unit test on a 4-node graph gives the known vector; two builds of the same tree are byte-identical; the backtest and T43.6-style bench rows are in `research.md`.

Out of scope: a persisted index, cross-session caching, rtok's tree-sitter version (cox is on 0.27, rtok on 0.25; no shared crate until they match).

#### T59.6 LSP `definition` and `references` tools on the running servers

Model: sonnet · Status: open · Depends: T59.3 · Size: ~190 · Priority: P3 · Complexity: 3

Goal: `definition(path, line, col)` and `references(path, line, col)` answer from a language server when one runs for the file and fall back to an `outline`/`grep` answer with a `(no server)` note otherwise, so navigation needs no extra `grep` round trip.

Files:
- `crates/cox-tools/src/lsp/client.rs`
- `crates/cox-tools/src/lsp/server.rs`
- `crates/cox-tools/src/lsp/nav.rs` (new)

Steps:
1. Hand-rolled wire types for `textDocument/definition` and `textDocument/references` next to the existing ones (`client.rs:174`; P41 rejects `lsp-types`, `plan.md:1245-1253`).
2. `Server` methods next to `start` (`server.rs:194`); results as `path:line` lines, capped and archived over the cap.
3. Use `LspPool::running_for` from T59.3; the fallback is a plain-text answer, never a spawn from this tool unless `diagnostics` would spawn too.

Check:
```bash
mise exec -- cargo nextest run --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when: fake-server tests for both requests and the fallback.

Out of scope: rename (T59.7), hover, workspace symbols.

#### T59.7 `rename_symbol` through the server's `textDocument/rename`

Model: opus · Status: open · Depends: T59.6 · Size: ~200 · Priority: P3 · Complexity: 4

Goal: one tool call renames a symbol across files using the server's `WorkspaceEdit`, applied as one approved change with the same per-file archive as `edit`, and refuses when no server runs (no text-search rename).

Files:
- `crates/cox-tools/src/lsp/rename.rs` (new)
- `crates/cox-tools/src/lsp/client.rs`
- `crates/cox-tools/src/write.rs`

Steps:
1. `prepareRename` then `rename`; convert `WorkspaceEdit` (`changes` and `documentChanges` text edits only; file create/rename ops are refused) into per-file new contents (Empryo idea: `rename-symbol.ts`).
2. Apply through `write.rs`'s path (`write.rs:42`) with the archive-before-write pattern of `edit.rs:126`; one `Engine` approval for the whole set, listing the files.
3. After apply, T59.3's diagnostics delta per file.

Check:
```bash
mise exec -- cargo nextest run --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when: fake-server test renames across two files, one approval, both archived; refusal without a server; a stale-version edit is refused.

Out of scope: rename without a server, file moves.

### P60 — Provider readiness, composer controls and glare (goal: the desktop app never starts a turn or a Best of candidate on a provider it cannot use, shows which provider runs, picks model, provider and mode in the composer, and lets the user set the glass glare; every rule lives in `cox-app` and the shared docs, so the Windows and any later Linux client repeat it without the Swift code, A139)

Every card in this phase:
- keeps the rule in Rust (`cox-app`, `cox-config`) and exposes it through `cox-ffi`; Swift only renders it (DS§1);
- updates the shared docs in the same change — behaviour in `docs/design/desktop.md` (DT§), visuals and tokens in `desktop/design/DESIGN.md` (DS§) and `desktop/design/tokens/`, settings in `docs/config.md` and `docs/config.jsonschema` — written platform-neutral, naming the Rust call a client makes, so a Windows or Linux agent can build the same feature from the docs alone;
- leaves a test that fails without it (`insta` or a unit test in Rust, swift-snapshot-testing or Swift Testing in Swift).

### P61 — Build speed (goal: a Rust change reaches the running macOS app without a fat-LTO link, an unchanged Rust core is never rebuilt on CI, and the workspace and the Swift packages each compile once per run; every card records its before/after time)

Rationale in §6 A140. Found by reading the build configuration (2026-10-07), not yet measured: the XCFramework always builds with the `dist` profile (fat LTO, one codegen unit), also under `just desktop-open`'s watcher and in CI's `swift test` job, and its `uniffi-bindgen` binary is linked the same way; the workspace has 79 integration-test binaries (`cox-core` 21, `cox` 19, `cox-tui` 15), each linked against the whole tree; `Swatinem/rust-cache` never caches workspace members, so each desktop CI job rebuilds all 35 crates; the CI matrix builds with `--all-features` and tests without them, so each target compiles the workspace twice; the six Swift packages are tested one by one, each in its own `.build`, so `CoxModel` compiles up to five times; the SwiftLint build-tool plugin runs on every build of every target.

**Order.** T61.1 first (the baseline every later card compares against). Then T61.2 → T61.3 → T61.4 (the desktop path, the largest gain). T61.5.1–T61.5.3, T61.6, T61.7 and T61.9 are independent of each other. T61.8, T61.10 and T61.11 last; each may end as "rejected" with the measurement that rejected it.

Timings go to a new `research.md` §4.3.9 "Build times", in the shape of R§4.3.4: each row is the median of 5 runs, with the machine, load average and commit. A card whose change gains nothing measurable is reverted and closed with that row.

#### T61.1 Build-time baseline

Model: haiku · Status: open · Depends: — · Size: ~0 (research.md only) · Priority: P1 · Complexity: 1

Goal: `research.md` §4.3.9 holds the numbers P61 is judged by.

Files:
- `research.md`

Steps:
1. Rust, each with `--timings` (keep the HTML out of git): a clean `cargo build --workspace`; an incremental build after touching `crates/cox-core/src/turn.rs`; the build phase of `cargo nextest run --workspace` (clean and after the same touch); `cargo build -p cox-ffi --lib --profile dist --target aarch64-apple-darwin` clean and after the touch; the full `just desktop-xcframework`.
2. Swift: `swift build --build-tests --build-system swiftbuild` per package under `desktop/macos/Packages/` (clean), and `just desktop-app` clean and after a one-line change in `CoxUI`.
3. CI: the wall time of the last green `rust / <target>` and `desktop-macos` jobs on `main`, from `gh run view`.

Check:
```bash
grep -n '4.3.9' research.md
```

Done when: §4.3.9 has a row for every step above, with the top five crates by compile time from the `--timings` report.

Out of scope: any change to the build.

#### T61.2 Fast profile for the XCFramework outside a release

Model: sonnet · Status: open · Depends: T61.1 · Size: ~40 · Priority: P1 · Complexity: 2

Goal: `just desktop-open`, `just desktop-app` and CI's `desktop-macos` job build `cox-ffi` without fat LTO; only a DMG and the desktop build workflow use `dist` (A15: what ships is `dist`).

Files:
- `Cargo.toml`
- `scripts/desktop/xcframework.sh`
- `justfile`

Steps:
1. `[profile.ffi-dev]` in `Cargo.toml`: `inherits = "release"`, `lto = false`, `codegen-units = 16`, `incremental = true`, `debug = "line-tables-only"`, with a comment saying why it exists next to `dist`.
2. `xcframework.sh` takes the profile from `COX_FFI_PROFILE` (default `ffi-dev`), validates it against `ffi-dev|dist` and prints which one it built.
3. `just desktop-dmg` and `.github/workflows/desktop-build.yml` set `COX_FFI_PROFILE=dist` (the workflow is a one-line `env:`; it does not count as a source file).

Check:
```bash
just desktop-xcframework
COX_FFI_PROFILE=dist just desktop-xcframework
just desktop-app
```

Done when: §4.3.9 has the incremental `just desktop-xcframework` time before and after; the DMG still comes from `dist` (the script's printed profile in the workflow log).

Out of scope: the bindings generator (T61.3), caching (T61.4).

#### T61.3 Bindings generator on the host dev profile, skipped when the library did not change

Model: sonnet · Status: open · Depends: T61.2 · Size: ~40 · Priority: P1 · Complexity: 2

Goal: `uniffi-bindgen` is built once in the dev profile for the host, and `xcframework.sh` regenerates the bindings and the XCFramework only when `libcox_ffi.a` changed.

Files:
- `scripts/desktop/xcframework.sh`

Steps:
1. Run the generator with `cargo run -p cox-ffi --features bindgen --bin uniffi-bindgen` and no `--profile`/`--target`: library mode reads the metadata from the archive passed as `--library`, so the generator's own profile does not matter.
2. After `cargo build`, hash the library (`shasum -a 256`) and compare with `desktop/macos/build/CoxFFI.xcframework/.source-sha256`; on a match print "unchanged" and exit 0. Write the stamp after a successful `-create-xcframework`.

Check:
```bash
just desktop-xcframework
cp desktop/macos/build/bindings/cox_ffi.swift "$TMPDIR/before.swift"
just desktop-xcframework   # prints "unchanged"
git stash && just desktop-xcframework && git stash pop
diff "$TMPDIR/before.swift" desktop/macos/build/bindings/cox_ffi.swift
```

Done when: the bindings are byte-identical to the ones the `dist`-profile generator wrote; a second run with no Rust change is a no-op; §4.3.9 has the time of that no-op.

Out of scope: CI caching (T61.4).

#### T61.4 CI: cache the XCFramework and SwiftPM

Model: sonnet · Status: open · Depends: T61.3 · Size: ~50 · Priority: P1 · Complexity: 2

Goal: a pull request that changes no Rust restores `CoxFFI.xcframework` instead of building it, and Swift dependencies come from a cache.

Files:
- `.github/actions/desktop-macos/action.yml`

Steps:
1. `actions/cache` for `desktop/macos/build/CoxFFI.xcframework` and `desktop/macos/build/bindings`, keyed on `runner.os`, `COX_FFI_PROFILE`, `hashFiles('crates/**', 'Cargo.toml', 'Cargo.lock', 'mise.toml', 'scripts/desktop/xcframework.sh')`. On a hit, skip the Rust setup, `rust-cache` and the build; the `arm64 only` check still runs.
2. `actions/cache` for `~/Library/Caches/org.swift.swiftpm` and `desktop/macos/Packages/*/.build`, keyed on `hashFiles('desktop/macos/Packages/*/Package.resolved')` with a restore key without the hash.
3. Pin both actions by commit SHA, as the rest of the workflow does.

Check: two runs of `desktop-macos` on one pull request, the second after a Swift-only commit: its log shows the cache hit and no `cargo build`.

Done when: §4.3.9 has both job times; the desktop build workflow, which shares the action, still builds `dist`.

Out of scope: sccache (T61.7).

#### T61.5.1 One integration-test binary for `cox-core`

Model: sonnet · Status: open · Depends: T61.1 · Size: ~60 (moves, plus one `main.rs`) · Priority: P2 · Complexity: 3

Goal: `crates/cox-core/tests/*.rs` (21 files) become modules of one test binary, so `nextest` links one binary instead of 21.

Files:
- `crates/cox-core/tests/it/main.rs` (new; `mod` lines only)
- `crates/cox-core/Cargo.toml` (one `[[test]] name = "it"` if auto-discovery needs it)
- the moved test files (renames; they do not count as source files)

Steps:
1. `git mv` each file into `tests/it/`; `common/` and `scenarios/` move with them. `cox-app`'s dev setup reuses cox-core's harness (`crates/cox-app/Cargo.toml` comment): keep the path it uses working.
2. insta snapshot names carry the module path: move the snapshot files with `cargo insta test --accept` only after confirming each moved snapshot's content is unchanged (`git diff -M --stat` shows renames only).
3. Update every `--test <name>` for this crate in `justfile`, `.github/`, `docs/`, `AGENTS.md` and open `plan.md` cards to `--test it -E 'test(/^<name>::/)'`.

Check:
```bash
mise exec -- cargo nextest run -p cox-core
mise exec -- cargo nextest run -p cox-app
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
```

Done when: the test count before and after is equal (`nextest list`); §4.3.9 has the `-p cox-core` build-phase time before and after.

Out of scope: other crates (T61.5.2, T61.5.3).

#### T61.5.2 One integration-test binary for `cox`

Model: sonnet · Status: open · Depends: T61.5.1 · Size: ~60 · Priority: P2 · Complexity: 3

Goal: as T61.5.1 for `crates/cox/tests/` (19 files), except the binaries a command names by itself: `docs` and `ide` (`just docs-check`, CI's docs job), `deps` (named in `AGENTS.md`), `trycmd` and `tui_e2e` (own fixtures and terminal) stay separate.

Files: as T61.5.1, for `crates/cox`.

Steps: as T61.5.1. `plugin_example_dart` keeps its test name, which `just plugin-examples` filters on.

Check:
```bash
mise exec -- cargo nextest run -p cox
just docs-check
```

Done when: equal test counts; §4.3.9 has the before/after.

Out of scope: `cox-tui` and the rest.

#### T61.5.3 One integration-test binary for `cox-tui`, `cox-tools`, `cox-ext` and `cox-app`

Model: sonnet · Status: open · Depends: T61.5.2 · Size: ~80 · Priority: P2 · Complexity: 3

Goal: as T61.5.1 for the remaining crates with more than one test file; `cox-tui`'s `screenshots` stays separate (named by `--test screenshots`).

Files: as T61.5.1, for each crate.

Steps: as T61.5.1, one commit per crate.

Check:
```bash
mise exec -- cargo nextest run --workspace
mise exec -- cargo insta test --check
```

Done when: equal test counts per crate; §4.3.9 has the workspace build-phase time after all three cards.

Out of scope: unit tests in `src/`.

#### T61.6 CI: one feature set per target

Model: haiku · Status: open · Depends: T61.1 · Size: ~5 · Priority: P2 · Complexity: 2

Goal: the `rust` matrix compiles the workspace once per target: `build-command`, clippy and nextest use the same features, so nextest reuses the build.

Files:
- `.github/workflows/ci.yml`

Steps:
1. `test-command: cargo nextest run --workspace --all-features` (the voice packages are already installed by `setup-command`). If a test fails only under `--all-features`, stop and report it rather than dropping the flag.
2. Make `just check-all` match, so local and CI runs share artifacts.

Check: the job log of `rust / aarch64-apple-darwin` shows nextest starting with no `Compiling` line for a workspace crate after the build step.

Done when: §4.3.9 has the job time before and after for all three targets.

Out of scope: `pyrlyn/ci`'s shared `ci-rust.yml` (its own repository).

#### T61.7 sccache for local builds and the in-repository CI jobs

Model: sonnet · Status: open · Depends: T61.1 · Size: ~40 · Priority: P2 · Complexity: 2

Goal: dependencies compiled in one worktree are reused by another without sharing a `target/` (sharing one breaks, as seen 2026-09-23), and the CI jobs outside the shared `rust` matrix reuse compiled crates across runs.

Files:
- `mise.toml`
- `.github/actions/desktop-macos/action.yml`
- `toolchain.md` (and the workspace `rust.md` row, as its rule asks)

Steps:
1. Pin sccache in `mise.toml` with a reason comment. Set `RUSTC_WRAPPER` only when sccache is on `PATH` (fail open: a missing sccache never fails a build), `SCCACHE_CACHE_SIZE` bounded for this disk, and `CMAKE_C_COMPILER_LAUNCHER`/`CMAKE_CXX_COMPILER_LAUNCHER` so whisper.cpp (`cox-voice`) is cached too.
2. Leave `CARGO_INCREMENTAL` alone: sccache skips incremental workspace crates and caches their dependencies, which is the gain.
3. CI: `mozilla-actions/sccache-action` (pinned by SHA) with the GitHub Actions cache backend in the desktop action and the `sandbox-landlock`, `plugins` and `plugin-examples` jobs.

Check:
```bash
mise exec -- cargo build -p cox && sccache --show-stats
```
then the same in a second worktree with a fresh `target/`: the stats show cache hits for the dependencies.

Done when: §4.3.9 has the clean build of the second worktree with and without sccache.

Out of scope: the shared `rust` matrix (lives in `pyrlyn/ci`; proposed there separately).

#### T61.8 Optimized build scripts and proc macros in dev

Model: haiku · Status: open · Depends: T61.1 · Size: ~5 · Priority: P3 · Complexity: 1

Goal: decide by measurement whether `[profile.dev.build-override] opt-level = 3` speeds the dev build (diesel, serde, schemars, uniffi and clap macros; typify's `build.rs`).

Files:
- `Cargo.toml`

Steps:
1. Add the override; record a clean and an incremental `cargo build --workspace`.
2. Keep it only if both are not slower; otherwise revert.

Check:
```bash
mise exec -- cargo build --workspace --timings
```

Done when: §4.3.9 has the row and the verdict.

Out of scope: other profile changes.

#### T61.9 One build graph for the Swift package tests

Model: sonnet · Status: open · Depends: T61.4 · Size: ~60 · Priority: P2 · Complexity: 3

Goal: the six packages' test targets build in one Xcode build graph, so `CoxModel`, swift-snapshot-testing and SwiftLintPlugins compile once per run instead of once per package.

Files:
- `desktop/macos/project.yml` (a `CoxTests` scheme over every package test target, serial, as `swift test --no-parallel` is today)
- `.github/workflows/ci.yml` (`desktop-macos`: `xcodebuild test -scheme CoxTests -derivedDataPath desktop/macos/build/DerivedData` replaces the per-package loop)
- `justfile` (`just desktop-test`)

Steps:
1. Keep `SNAPSHOT_ARTIFACTS` and the failing-snapshot upload working; keep the 60-minute timeout.
2. Cache `desktop/macos/build/DerivedData` in CI next to T61.4's SwiftPM cache.
3. If a package's tests cannot run under the scheme (the `ColorResource` issue, SwiftPM #9655), keep that one package on `swift test` and say why in the workflow.

Check: `just desktop-test` locally, and the `desktop-macos` job on a pull request, run the same number of tests as the per-package loop.

Done when: §4.3.9 has the job time before and after.

Out of scope: test parallelism (the UI tests share one `NSApplication`).

#### T61.10 SwiftLint plugin off during builds, on in the lint job

Model: haiku · Status: open · Depends: T61.9 · Size: ~30 · Priority: P3 · Complexity: 2

Goal: building or testing the Swift packages does not run SwiftLint on every target; the lint still gates every pull request (`desktop-macos-lint`) and runs locally through one `just` recipe. Needs the creator's confirmation that DS§9's rule is satisfied by the lint job alone, before work starts.

Files:
- the six `desktop/macos/Packages/*/Package.swift` (one shared pattern: the plugin is attached only when `Context.environment["COX_SWIFTLINT_PLUGIN"] == "1"`)
- `justfile` (`just desktop-lint`, the same command as the CI job)
- `desktop/design/DESIGN.md` (DS§9: where the lint runs)

Steps:
1. Keep the `SwiftLintPlugins` package pin, so the version check against `mise.toml` still holds.

Check: `just desktop-lint` fails on `desktop/macos/LintFixtures`; `COX_SWIFTLINT_PLUGIN=1 swift build` in one package still lints.

Done when: §4.3.9 has an incremental `just desktop-app` after a one-line `CoxUI` change, before and after.

Out of scope: lint rules.

#### T61.11 No feature-unification rebuilds between `just test` and `just check-all`

Model: opus · Status: open · Depends: T61.1 · Size: ~50 · Priority: P3 · Complexity: 3

Goal: switching between `just test` (a `-p` subset) and `just check-all` (`--workspace`) does not rebuild dependencies because their features unify differently, or the card is rejected with the reason.

Files:
- `Cargo.toml`
- a workspace-hack crate if chosen (`crates/cox-workspace-hack`)
- `docs/design/crates.md`

Steps:
1. Measure the rebuild first: `just check-all`, then `just test --changed-since HEAD~1` and count the dependency crates `cargo` recompiles. Zero → close as not needed.
2. Compare cargo's own workspace feature unification (check whether it is stable in the pinned Rust; cite the cargo docs) with `cargo-hakari`. hakari makes every member depend on the hack crate: `crates/cox/tests/deps.rs` (`ffi_depends_only_on_app_and_protocol`, `app_has_no_terminal_or_cli`, the "only X depends on Y" rules) must still pass; a hack crate that drags `ratatui` or `clap` into `cox-ffi`'s graph is rejected.

Check:
```bash
mise exec -- cargo nextest run -p cox --test deps
```

Done when: §4.3.9 has the rebuild count before and after, or the rejection and its reason.

Out of scope: changing which crates depend on which.

---

### P63 — Desktop architecture and test hardening (goal: SessionStore's patch rules are checked against generated inputs, not only hand-picked cases; a pull request re-runs only the Swift packages its change can affect; `CoxModel` and `CoxCore` cannot import AppKit or SwiftUI; the stores get their clients from one dependency system instead of initializer plumbing)

Rationale in §6 A141. Found by reading `desktop/macos` (2026-10-07): `SessionStore.apply` (`Packages/CoxModel/Sources/CoxModel/SessionStore.swift`) is covered by fixture replays and a few hand-written patch lists in `SessionStoreTests.swift`; `desktop-macos` runs `swift test` in all six packages on every Swift or Rust change, 1,109 reference images included (1,083 in `CoxUI`, 26 in `CoxTranscript`); no rule stops a UI framework import in `CoxModel` or `CoxCore`, although neither has one today; `AppModel` (`App/CoxApp.swift`) passes `LaunchCore`'s clients into every store by hand.

Each card is written so an agent can do it from the card alone: what to install, where, the files, the code and the check. Libraries were checked on 2026-10-07 against their GitHub repositories and releases:

| Asked for | Found | Used instead |
| --- | --- | --- |
| `swift-check` | No Swift property-testing package by that name. `github.com/IronVelo/swift-check` is a Rust crate for searching bytes. | [x-sheep/swift-property-based](https://github.com/x-sheep/swift-property-based) 2.0.1 (2026-09-25; product `PropertyBased`; Swift Testing native, Swift 6.2+, shrinking, `.fixedSeed`; MIT) |
| `swift-testing-expectations` | No such package. The nearest, [dfed/swift-testing-expectation](https://github.com/dfed/swift-testing-expectation) 0.1.4 (2025-05-20), is an async `Expectation` for Swift Testing, not property testing. | as above |
| SwiftCheck | [typelift/SwiftCheck](https://github.com/typelift/SwiftCheck): last release 0.12.0 (2019-03-28), last push 2022-04-03, XCTest-era. Dead, so not added. | as above |
| swift-gen | [pointfreeco/swift-gen](https://github.com/pointfreeco/swift-gen): generators only, no runner and no shrinking. PropertyBased ships a fork of it. | as above |
| `swift-architecture-check` | No such package. Real architecture linters exist — [Harmonize](https://github.com/perrystreetsoftware/Harmonize), [SolidLikeARock](https://github.com/nenadvulic/solid-like-a-rock) — but each adds SwiftSyntax or another binary. | a SwiftLint `custom_rules` entry: SwiftLint 0.65.1 is already pinned (`mise.toml`) and its build-tool plugin already runs on every package target |
| swift-dependencies | [pointfreeco/swift-dependencies](https://github.com/pointfreeco/swift-dependencies) 1.17.1 (2026-08-28), `swift-tools-version: 6.4`, so it needs Xcode 27's Swift 6.4 — the toolchain CI pins and the one in use locally. MIT. | itself |

**Order.** T63.1 any time. T63.2 after T61.4 if that card is still open, since both edit the `desktop-macos` job; if T61.9 lands first, T63.2 selects scheme test targets instead of packages (step 6). T63.4 is being implemented on branch `feature/swift-dependencies` in its own pull request with tests; that pull request claims and closes the card.

#### T63.2 CI: re-run only the Swift packages a change can affect

Model: sonnet · Status: open · Depends: T61.4 (shared job; not a code dependency) · Size: ~120 (script, workflow) · Priority: P2 · Complexity: 3

Goal: on a pull request, a package whose test inputs are byte-identical to a run that already passed on `main` (or earlier on the same pull request) is not tested again; everything else runs as today. A change to `CoxUI` alone re-runs `CoxUI` and `CoxTranscript`, not `CoxModel`, `CoxCore`, `CoxPlatform` or `CoxTranscriptText`; a Rust-only change skips the 1,109 snapshot images entirely.

What is and is not feasible: SwiftPM has no per-test result cache and swift-snapshot-testing compares freshly rendered images by design, so "only the changed snapshots" cannot be done inside one package. The unit that can be skipped soundly is a package whose whole input — its sources, tests, reference images, local dependencies, pins and toolchain — did not change since a passing run. Build outputs (`.build`, the SwiftPM cache, `CoxFFI.xcframework`) are cached by T61.4, not here.

Why: the snapshot packages dominate the job's time and most pull requests touch neither them nor what they import. Risk if skipped: every Rust or unrelated Swift change keeps paying for 1,109 renders on the `xcode-27` runners. The job comment in `ci.yml` ("Always the full build … never only the changed ones") is a deliberate rule; this card changes it for pull requests only, by A141 — the app target is still built on every run.

Install: nothing new. `actions/cache/restore` and `actions/cache/save` v6.1.0, pinned by commit SHA `55cc8345863c7cc4c66a329aec7e433d2d1c52a9` (`gh api repos/actions/cache/git/ref/tags/v6.1.0`; re-check for a newer release when claiming).

Files:
- `scripts/desktop/swift_test.sh` (new): the per-package loop now inline in `ci.yml`, plus the input hash and the pass markers
- `.github/workflows/ci.yml` (`desktop-macos`: the marker restore and save around the `swift test` step, and the trigger in step 5)
- `justfile` (`just desktop-test` calls the script with `COX_SWIFT_TEST_ALL=1`, so local runs stay full)

Steps:
1. Inputs per package (the local dependency graph from the six manifests, plus what the tests read):

   | Package | Hashed paths besides its own directory |
   | --- | --- |
   | `CoxModel` | `desktop/macos/Fixtures` |
   | `CoxUI` | — (no local dependency; tokens are generated into the package) |
   | `CoxCore` | `CoxModel`, `crates/`, `Cargo.toml`, `Cargo.lock`, `scripts/desktop/xcframework.sh` (the XCFramework it links) |
   | `CoxPlatform` | `CoxModel` |
   | `CoxTranscriptText` | `CoxModel` |
   | `CoxTranscript` | `CoxModel`, `CoxTranscriptText`, `CoxUI` |

   Every package also hashes `desktop/macos/.swiftlint.yml`, `mise.toml`, `scripts/desktop/swift_test.sh`, `.github/workflows/ci.yml`, and the toolchain identity: `xcodebuild -version` and `sw_vers -productVersion` (a new image re-renders snapshots differently, so it must re-run them). A package directory includes `Tests/**/__Snapshots__`, so a re-recorded image re-runs its package.
2. The script. Hash tracked content through git, which is exact and fast on a clean checkout:

   ```bash
   #!/usr/bin/env bash
   # The Swift package tests for CI and `just desktop-test` (T63.2): each package runs unless a
   # pass marker for the exact hash of its inputs exists. COX_SWIFT_TEST_ALL=1 runs every
   # package; markers live in $COX_SWIFT_TEST_MARKERS, restored and saved by ci.yml.
   set -euo pipefail
   shopt -s nullglob
   cd "$(git rev-parse --show-toplevel)"
   pkgs=desktop/macos/Packages
   markers=${COX_SWIFT_TEST_MARKERS:-desktop/macos/build/swift-test-pass}
   mkdir -p "$markers"

   inputs() {
     case "$1" in
       CoxModel) echo "$pkgs/CoxModel desktop/macos/Fixtures" ;;
       CoxUI) echo "$pkgs/CoxUI" ;;
       CoxCore) echo "$pkgs/CoxCore $pkgs/CoxModel crates Cargo.toml Cargo.lock scripts/desktop/xcframework.sh" ;;
       CoxPlatform) echo "$pkgs/CoxPlatform $pkgs/CoxModel" ;;
       CoxTranscriptText) echo "$pkgs/CoxTranscriptText $pkgs/CoxModel" ;;
       CoxTranscript) echo "$pkgs/CoxTranscript $pkgs/CoxModel $pkgs/CoxTranscriptText $pkgs/CoxUI" ;;
       *) echo "swift_test.sh: no input list for $1; add one" >&2; return 1 ;;
     esac
   }

   toolchain=$(xcodebuild -version; sw_vers -productVersion)
   shared="desktop/macos/.swiftlint.yml mise.toml scripts/desktop/swift_test.sh .github/workflows/ci.yml"
   failed=()
   for manifest in "$pkgs"/*/Package.swift; do
     package=$(dirname "$manifest")
     name=$(basename "$package")
     paths=$(inputs "$name") || exit 1
     # shellcheck disable=SC2086 # the path lists are split on purpose
     hash=$({ git ls-files -s -- $paths $shared; echo "$toolchain"; } | git hash-object --stdin)
     if [ "${COX_SWIFT_TEST_ALL:-0}" != 1 ] && [ -e "$markers/$name-$hash" ]; then
       echo "$name: inputs unchanged since a passing run ($hash), skipped"
       touch "$markers/$name-$hash"  # still in use: keep it past the pruning below
       continue
     fi
     # swiftbuild: see SwiftPM #9655 (ColorResource symbols for Colors.xcassets).
     if swift test --no-parallel --build-system swiftbuild --package-path "$package"; then
       touch "$markers/$name-$hash"
     else
       failed+=("$name")
     fi
   done
   # Keep the marker cache small: a hash older than two weeks will not match again soon.
   find "$markers" -type f -mtime +14 -delete
   if [ ${#failed[@]} -gt 0 ]; then
     echo "swift test failed in: ${failed[*]}" >&2
     exit 1
   fi
   ```

   An unknown package fails loudly (fail closed): a seventh package must get an input list before CI can pass.
3. `ci.yml`, `desktop-macos`: the restore before `swift test`, the step calling the script, the save after it. `actions/cache` keys are immutable, so each run saves a new key and restores the newest by prefix:

   ```yaml
      - name: swift test pass markers
        uses: actions/cache/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: desktop/macos/build/swift-test-pass
          key: swift-test-pass-v1-${{ github.run_id }}-${{ github.run_attempt }}
          restore-keys: swift-test-pass-v1-

      - name: swift test
        timeout-minutes: 60
        env:
          SNAPSHOT_ARTIFACTS: ${{ runner.temp }}/snapshots
          # Only a pull request may skip; any other trigger tests every package.
          COX_SWIFT_TEST_ALL: ${{ github.event_name == 'pull_request' && '0' || '1' }}
        run: bash scripts/desktop/swift_test.sh

      - name: save swift test pass markers
        if: ${{ !cancelled() }}
        uses: actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: desktop/macos/build/swift-test-pass
          key: swift-test-pass-v1-${{ github.run_id }}-${{ github.run_attempt }}
   ```

   The save also runs after a failure, so the packages that passed keep their markers; a failed package never writes one.
4. Replace the job comment's "Always the full build: every package and the app, never only the changed ones" with the new rule: every package on `main` and on manual runs; on a pull request, only packages whose inputs changed since a passing run; the app target always.
5. Cache scope: a pull request reads caches from its own ref and from `main`, never from another pull request. `ci.yml` runs on `pull_request`, `workflow_dispatch` and `workflow_call` only, so no run on `main` writes markers today and the gain would be limited to re-pushes of one pull request. Add `push: branches: [main]` to `ci.yml` with `paths: ['desktop/**', 'crates/**', 'Cargo.lock', 'scripts/desktop/**']`, and make sure only `desktop-macos` (and the jobs it `needs`) runs on that event; ask the creator first, since it adds a macOS run per merge.
6. If T61.9 has landed (one `xcodebuild test -scheme CoxTests`), keep the same hashes and markers but pass `-only-testing:<Package>Tests` for the packages that need a run instead of looping `swift test`.
7. `justfile`: `desktop-test` runs `COX_SWIFT_TEST_ALL=1 bash scripts/desktop/swift_test.sh`.

Check:
- On this card's pull request: a first push runs all six packages and the save step stores a key. Then push a commit that touches only `crates/cox-tools/src/git.rs`: the log shows five packages "skipped" and `CoxCore` tested. Then one touching only a `CoxUI` source: `CoxUI` and `CoxTranscript` run, four skip.
- Locally: `just desktop-test` runs every package.

Done when: §4.3.9 (T61.1's table) has the `desktop-macos` job time for a Rust-only and a `CoxUI`-only pull request before and after; the failing-snapshot artifact still uploads when `CoxUI` fails.

Risks: an input the hash misses lets a broken package skip — the list is explicit and `main` always runs everything, so a miss is caught on the next `main` run (step 5) and fixed by adding the path; a flaky test that passed once stays green until its inputs change.

Out of scope: caching `.build` and DerivedData (T61.4, T61.9); splitting the job per package across runners.

#### T63.4 Dependency injection through swift-dependencies

Model: sonnet · Status: open · Depends: — · Size: ~350 across T63.4.1–T63.4.3 below (`AGENTS.md` task size) · Priority: P2 · Complexity: 4

In progress on branch `feature/swift-dependencies`, in its own pull request with tests; that pull request claims this card, keeps the table and `todo.md` in sync, and moves the card to `done.md`.

Goal: the stores in `CoxModel` read `CoreClient`, `InboxClient` and `SecretStore` through `@Dependency` instead of initializer arguments threaded from `AppModel`; `LaunchCore` still makes the one live-or-fixture choice and hands it over once with `prepareDependencies`; tests override a client per test with a trait; Xcode previews get fixture values without a core.

Why: today every new store or client means another initializer argument in `AppModel.init` (`App/CoxApp.swift`) and in every test that builds the store; `SettingsStore(client:secrets:cwd:)`, `SidebarStore(workspace:inbox:)` and `InboxStore(client:)` already carry them. With one `DependencyValues` registry, a store names what it needs, a test overrides only that, and previews fall back to `previewValue`. Risk if skipped: the plumbing grows with each store (P58's Windows client copies the pattern from DT§4.6), and a test that forgets an argument fails at compile time across many files instead of at one override.

What does not change: `SessionClient` is per-session state, not a service — each `SessionStore` owns the one `CoreClient.open` returned, so `SessionStore.init(session:)` stays. Its key below exists for previews and tests only and has no live value. `LiveCoreClient` needs `MacHost` from `CoxPlatform`, so `CoxCore` declares no live value; the app sets it.

Install (SwiftPM; no global tool), `https://github.com/pointfreeco/swift-dependencies`, `exact: "1.17.1"` (latest release, 2026-08-28; `swift-tools-version: 6.4`, so Xcode 27 / Swift 6.4, which CI and local already use). Products: `Dependencies` in library targets, `DependenciesTestSupport` in test targets. Never `DependenciesMacros`: it is the only product that builds swift-syntax.
- `desktop/macos/Packages/CoxModel/Package.swift`: package pin; `Dependencies` on `CoxClient` and `CoxModel`; `DependenciesTestSupport` on `CoxModelTests`.
- `desktop/macos/Packages/CoxPlatform/Package.swift`: the same pin; `Dependencies` on `CoxPlatform` (it owns `SecretStore`'s live value).
- `desktop/macos/Packages/CoxCore/Package.swift`: no direct pin unless a `CoxCore` type starts reading `@Dependency`; it resolves the package through `CoxModel` anyway.
- `desktop/macos/project.yml`: the package and the `Dependencies` product on the `Cox` target (`prepareDependencies` is called from `App/`).
- Every `Package.resolved` of a package that depends on `CoxModel` (`CoxCore`, `CoxPlatform`, `CoxTranscriptText`, `CoxTranscript`) gains swift-dependencies and its transitive pins (swift-concurrency-extras, swift-issue-reporting, swift-clocks, combine-schedulers, swift-syntax): commit them all, or CI's "Swift pins unchanged by the build" fails.
- `toolchain.md` (SwiftPM table row) and `plan.md` §1 (dependency row).

##### T63.4.1 Keys and values (`CoxClient`, `CoxPlatform`)

Model: sonnet · Status: open · Depends: — · Size: ~120 · Priority: P2 · Complexity: 2

Files:
- `desktop/macos/Packages/CoxModel/Package.swift`, `desktop/macos/Packages/CoxPlatform/Package.swift`, the five `Package.resolved`
- `desktop/macos/Packages/CoxModel/Sources/CoxClient/Dependencies.swift` (new)
- `desktop/macos/Packages/CoxPlatform/Sources/CoxPlatform/Dependencies+Live.swift` (new)
- `desktop/macos/Packages/CoxModel/Tests/CoxModelTests/DependenciesTests.swift` (new)

Manifest (`CoxModel`):

```swift
dependencies: [
  .package(url: "https://github.com/apple/swift-collections", from: "1.7.1"),
  .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
  // T63.4: one registry for the stores' clients; Dependencies only, never the macros.
  .package(url: "https://github.com/pointfreeco/swift-dependencies", exact: "1.17.1"),
],
targets: [
  .target(
    name: "CoxClient",
    dependencies: [.product(name: "Dependencies", package: "swift-dependencies")],
    plugins: [swiftLint]),
  .target(
    name: "CoxModel",
    dependencies: [
      "CoxClient",
      .product(name: "OrderedCollections", package: "swift-collections"),
      .product(name: "Dependencies", package: "swift-dependencies"),
    ],
    plugins: [swiftLint]),
  .testTarget(
    name: "CoxModelTests",
    dependencies: [
      "CoxModel",
      .product(name: "DependenciesTestSupport", package: "swift-dependencies"),
    ],
    plugins: [swiftLint]),
]
```

Keys in `CoxClient` (the interface module), as `TestDependencyKey`s so the live values can live where the live types are:

```swift
import Dependencies

/// Where sessions open. Live: what `LaunchCore` picked, set once by `prepareDependencies`.
public enum CoreClientKey: TestDependencyKey {
  public static let testValue: any CoreClient = UnimplementedCoreClient()
  public static let previewValue: any CoreClient =
    FixtureCoreClient(fixture: Fixture(batches: [], snapshot: []))
}

/// What needs the person, across sessions. Live: the launch's core when it is an inbox.
public enum InboxClientKey: TestDependencyKey {
  public static let testValue: any InboxClient = NoInbox()
  public static let previewValue: any InboxClient = NoInbox()
}

/// Provider keys. Live: `CoxPlatform` (the Keychain, or memory under `COX_KEYRING=off`).
public enum SecretStoreKey: TestDependencyKey {
  public static let testValue: any SecretStore = MemorySecretStore()
  public static let previewValue: any SecretStore = MemorySecretStore()
}

/// Previews and tests only: a live session always comes from `CoreClient.open`.
public enum SessionClientKey: TestDependencyKey {
  public static var testValue: any SessionClient {
    FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  }
  public static var previewValue: any SessionClient { testValue }
}

extension DependencyValues {
  public var coreClient: any CoreClient {
    get { self[CoreClientKey.self] }
    set { self[CoreClientKey.self] = newValue }
  }
  public var inboxClient: any InboxClient {
    get { self[InboxClientKey.self] }
    set { self[InboxClientKey.self] = newValue }
  }
  public var secretStore: any SecretStore {
    get { self[SecretStoreKey.self] }
    set { self[SecretStoreKey.self] = newValue }
  }
  public var sessionClient: any SessionClient {
    get { self[SessionClientKey.self] }
    set { self[SessionClientKey.self] = newValue }
  }
}

/// A test that opens a session must say which core it opens on.
struct UnimplementedCoreClient: CoreClient {
  func open(_ request: OpenSession) async throws -> any SessionClient {
    reportIssue("CoreClient.open: no core set; override \\.coreClient in this test")
    throw CancellationError()
  }
}

struct NoInbox: InboxClient {
  func inbox() -> [InboxItem] { [] }
}
```

`reportIssue` comes from IssueReporting, which `Dependencies` re-exports. A `TestDependencyKey` read in the live app without `prepareDependencies` setting it is itself reported as an issue, which is the wanted failure: the app must set the core.

Live value in `CoxPlatform` (`Dependencies+Live.swift`), moving the `COX_KEYRING` rule out of `LaunchCore`:

```swift
import CoxClient
import Dependencies
import Foundation

extension SecretStoreKey: DependencyKey {
  /// `COX_KEYRING=off`, as cargo sets it for every development run, keeps keys out of the
  /// Keychain (A49, A51).
  public static let liveValue: any SecretStore =
    ProcessInfo.processInfo.environment["COX_KEYRING"] == "off"
    ? MemorySecretStore() : KeychainSecretStore()
}
```

Check: `swift test` in `CoxModel` and `CoxPlatform`; `DependenciesTests.swift` proves that the test values are the stand-ins (`withDependencies` reads `\.secretStore` as a `MemorySecretStore`) and that `UnimplementedCoreClient.open` is reported (`withKnownIssue`).

##### T63.4.2 The stores read `@Dependency` (`CoxModel`)

Model: sonnet · Status: open · Depends: T63.4.1 · Size: ~80 + tests · Priority: P2 · Complexity: 3

Files:
- `Packages/CoxModel/Sources/CoxModel/InboxStore.swift`, `SettingsStore.swift`, `SidebarStore.swift`
- their tests in `Packages/CoxModel/Tests/CoxModelTests/`

In an `@Observable` class the wrapper must be `@ObservationIgnored`; it captures the dependency context when the store is created:

```swift
@Observable
@MainActor
public final class InboxStore {
  @ObservationIgnored @Dependency(\.inboxClient) private var client
  public init() {}
  // the rest unchanged
}
```

`SettingsStore` keeps `client` (a `SettingsClient`, the live core, not one of the four) and `cwd` as arguments and drops `secrets`:

```swift
@ObservationIgnored @Dependency(\.secretStore) private var secrets
public init(client: any SettingsClient, cwd: String) { (self.client, self.cwd) = (client, cwd) }
```

`SidebarStore` takes `inbox: InboxStore?` as today; the inbox store builds its own client. Keep each old initializer as a deprecated forwarding one until T63.4.3 moves the call sites, then delete it in the same pull request.

Tests override per test with the `DependenciesTestSupport` trait:

```swift
import CoxClient
import DependenciesTestSupport
import Testing

@testable import CoxModel

struct OneApproval: InboxClient {
  let item: InboxItem
  func inbox() -> [InboxItem] { [item] }
}

@MainActor
@Test(.dependency(\.secretStore, MemorySecretStore(["anthropic": "k"])))
func settingsReadsTheKeyFromTheStore() throws {
  let store = SettingsStore(client: FakeSettingsClient(), cwd: "/")
  // the assertions the existing SettingsStoreTests make, with no `secrets:` argument
}
```

Check: `swift test` in `CoxModel`; no test builds a store with a client argument that the store now reads from `DependencyValues`.

##### T63.4.3 App wiring (`App/`)

Model: sonnet · Status: open · Depends: T63.4.2 · Size: ~80 · Priority: P2 · Complexity: 3

Files:
- `desktop/macos/App/CoxApp.swift`, `desktop/macos/App/LaunchCore.swift`, `desktop/macos/project.yml`
- `docs/design/desktop.md` (DT§4.6: how a client reaches a store, written platform-neutral for P58)

`CoxApp` prepares the dependencies once, before any store exists; `LaunchCore.pick()` is still the one place that chooses fixture or live:

```swift
import Dependencies

@main
struct CoxApp: App {
  @State private var model: AppModel

  init() {
    let launch = LaunchCore.pick()
    // Once per launch, before the first store reads a client (DT§4.1).
    prepareDependencies {
      if let core = try? launch.core.get() {
        $0.coreClient = core
        if let inbox = core as? any InboxClient { $0.inboxClient = inbox }
      }
    }
    _model = State(initialValue: AppModel(launch: launch))
  }
  // body unchanged
}
```

`LaunchCore` loses its `secrets` field: `MacHost` and `SettingsStore` read `\.secretStore` (live value from T63.4.1). `AppModel.init` then builds `SettingsStore(client: launch.live.get(), cwd: LaunchCore.project())` and `SidebarStore(workspace: …, inbox: (try? launch.core.get()) is any InboxClient ? InboxStore() : nil)`. A fixture launch still replays through `FixtureCoreClient`, because `LaunchCore.pick()` put it in `launch.core` and `prepareDependencies` set it.

`project.yml`:

```yaml
packages:
  swift-dependencies:
    url: https://github.com/pointfreeco/swift-dependencies
    exactVersion: 1.17.1
targets:
  Cox:
    dependencies:
      - package: swift-dependencies
        product: Dependencies
```

Name clash: AppIntents has its own `@Dependency` property wrapper, used in `App/Intents/AskCoxIntent.swift` and `App/Intents/Entities.swift` (`@Dependency private var model: AppModel`, registered in `App/Intents/Shortcuts.swift` through `AppDependencyManager`). Those files must not `import Dependencies`. A file that needs both spells them `@AppIntents.Dependency` and `@Dependencies.Dependency`; the intents keep AppIntents' wrapper for `AppModel`.

Check: `just desktop-app`; launch with `-CoxFixture desktop/macos/Fixtures/edit.json` and without; Shortcuts still lists the App Intents (T51.17); `COX_KEYRING=off` still never touches the Keychain.

Done when (T63.4 as a whole): no store in `CoxModel` takes `CoreClient`, `InboxClient` or `SecretStore` as an initializer argument; `AppModel.init` passes no client to a store that reads it from `DependencyValues`; every package's tests and `desktop-macos` pass; `swift-syntax` is resolved but not built (the build log has no `SwiftSyntax` target); a fixture launch and a live launch behave as before.

Risks: `prepareDependencies` called twice or after a store read a value is reported by the library — keep it as the first statement of `CoxApp.init`; a store created inside a `Task` or a callback captures that context's values, so stores are made on the main actor at launch or in a view, as today; one more dependency tree (five transitive packages) to keep current, each needs its `toolchain.md` row.

Out of scope: `SettingsClient`, `WorkspaceClient` and the `RemoteHosts` connector as dependencies (follow-up cards if T63.4 proves out); the Windows client (P58).

#### P63 acceptance criteria

| Card | Accepted when |
| --- | --- |
| T63.2 | `scripts/desktop/swift_test.sh` runs every package locally and on non-pull-request runs; on a pull request a Rust-only change skips the five packages that do not link `CoxFFI` and a `CoxUI`-only change runs `CoxUI` and `CoxTranscript` only; pass markers are restored and saved by SHA-pinned `actions/cache` v6.1.0 steps; the job comment states the new rule; §4.3.9 has before/after job times; the app target still builds on every run |
| T63.4 | swift-dependencies 1.17.1 (`Dependencies`, `DependenciesTestSupport`) is pinned in `CoxModel`, `CoxPlatform` and `project.yml`, all `Package.resolved` files committed; `CoreClient`, `InboxClient`, `SecretStore` and `SessionClient` have keys with test and preview values, `SecretStore` a live value in `CoxPlatform`; the stores read them with `@ObservationIgnored @Dependency`; `CoxApp.init` sets the launch's choice through `prepareDependencies` and `LaunchCore` still makes it; the AppIntents `@Dependency` files are unchanged and build; tests override clients with `.dependency`/`.dependencies` traits; DT§4.6 documents it; delivered by the `feature/swift-dependencies` pull request |

### P65 — MCP results stay off the prompt until a program prints them

One sandboxed Python program may `search`, `describe` and `call` MCP tools and print one small JSON result. The host keeps every schema and every raw tool payload. `tool_search` defaults to the same summary. No new crate, no Podman, no persistent interpreter, no `save_tool`, no JSON memory directory, and nothing copied from the GPL code-execution server. Suggested id T63 is already P63 (A141), so these cards are T65.

T65.1 and T65.2 are in `done.md`.

---

### P65 — Crate docs (goal: deferred lockfile lookup of cached rustdoc, no third-party docs API)

Rationale in §6 A142. T65.1 is in `done.md`.

---

### P66 — prime-agent-derived improvements (goal: after compaction the model still knows which archived outputs expand; a subagent's answer is never lost to the cap and a background answer is collected on demand; prompt notes, memory, skills and subagents change through a reviewed, reversible refine with the base prompt fixed; `cox run -p` can keep going toward shell gates under turn, token and time limits beside the USD cap)

Rationale in §6 A147. Idea-only, clean-room: the source is the study of [PrimeIntellect-ai/prime-agent](https://github.com/PrimeIntellect-ai/prime-agent) at `afe8d14` (v0.9.8). It is MIT, but no code is copied, so no notice is needed. Each card cites prime-agent files for the idea only, and the implementation is written from the card. If a later card ever copies a substantial part, that file and `THIRD-PARTY-NOTICES` carry prime-agent's full MIT text, both copyright lines and the repository URL, and that notice is never replaced by cox's header.

**Order.** Start T66.1 and T66.2 in parallel; T66.3 follows T66.2. Refine runs T66.4 → T66.5 → T66.6 → T66.7 → T66.8. Autonomous runs T66.9 → T66.10 → T66.11. T66.12 is a design gate. T66.13 and T66.14 wait for the implementation cards a later amendment adds after it.

**Already in cox, so no card:**
- `ContextTooLong` already compacts once per user turn and then surfaces the error (`session.rs:1846-1867`, `retried_after_too_long`). That is the contract of prime-agent's `OverflowRecovery` (`pa-daemon/src/overflow_compaction.rs`).
- `microcompact` is request-only and already names each pointer's archive id (`context.rs:300-308`).

**Not taken:**
- peer sockets between agents (`pa-daemon/src/agent_messaging/`), because T34.5 routes every message through the parent;
- the Python kernel and `rlm.factory`;
- per-model prompt blocks inside the cached prefix;
- Prime's prompt prose;
- the `HarnessEntry` and `GoalState` schemas;
- the `pa-daemon` JSONL socket dialect.

#### T66.1 Compaction lists the archive ids that still expand

Model: opus · Status: open · Depends: — · Size: ~120 · Priority: P1 · Complexity: 3

Goal: after any compaction, the summary item ends with a byte-stable, bounded `## Archived outputs` section. It names every archive id from the compacted turns, so the model can still `expand` evidence it no longer sees. No earlier turn is edited.

Files:
- `crates/cox-core/src/compact.rs`
- `crates/cox-core/src/session.rs` (only if step 1 finds the gap)
- the cox-core compaction test file

Steps:
1. Check whether `inner.archives` (`session.rs:90`) is refilled when a session resumes from its rollout; only one insert was found (`session.rs:1274`). If it is not refilled, refill it from the replayed tool results here, or split that into T66.1.1 if it breaks the size limit.
2. `SurvivingHandles { kept: Vec<(ArchiveId, String)>, omitted: usize }` holds each archive id and its tool name. Build it from `inner.archives` for the call ids in `history[..cut]`, ordered by id. Only ids that `turn.rs:659-668` wrote before the model saw the short form count; nothing is named after the fact. `notice_text(&SurvivingHandles) -> String` stops at 32 entries or 2048 bytes and then writes `… and N more`. prime-agent's notice has no bound (idea: `pa-core/src/session_engine/ipython_state.rs` `notice_content`).
3. Render it as the last section of `WorkingState::render` (T59.1). It then lives in the `Summary` item text, which the rollout already replays (`rollout.rs:153-160`). `WorkingState::carry` merges, re-sorts and re-caps it on the next compaction.

Check:
```bash
mise exec -- cargo nextest run -p cox-core compaction
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when the `compaction_notice_lists_pointer_ids_and_keeps_last_turns_verbatim` insta snapshot passes:
- three archived outputs in the dropped turns appear by id;
- the last two turns are byte-identical before and after;
- compacting the same history twice gives the same bytes;
- a history with 40 ids shows 32 and `… and 8 more`.

Out of scope: a new event type, and any change to the `Content::Pointer` text or to `microcompact`.

#### T66.2 A subagent's over-cap answer is archived before the parent sees the short form

Model: Claude Code / claude-sonnet · Status: in progress · Depends: — · Size: ~80 · Priority: P1 · Complexity: 2

Goal: when a child's answer is over `result_cap_tokens`, the full text becomes an archive row first. The summary or cut that the parent receives ends with `full answer: expand <id>`. Today the answer is summarised or cut with no archive row (`subagent.rs:1138-1151`), which breaks "Lossless by default".

Files:
- `crates/cox-core/src/subagent.rs`
- `crates/cox-core/src/tasks.rs`
- the cox-core subagent test file

Steps:
1. In the cap path, call `session.archive.put` with the full answer before `summarize` runs; it is the same call `turn.rs:659-668` uses. Put the `expand` trailer after the summary or cut and before the worktree trailer.
2. The background path (`drive`, `subagent.rs:813-857`) passes that `ArchiveRef` to `Event::TaskCompleted { archive }`, which is `None` today. `notice_text` then says `full output: expand <id>`, as detached `bash` already does (`tasks.rs:159`).

Check:
```bash
mise exec -- cargo nextest run -p cox-core subagent
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when `over_cap_child_answer_is_archived_before_the_parent_sees_it` passes on the scripted provider:
- the archive row exists;
- the parent's tool result holds the summary and the id;
- `expand` returns the full answer byte for byte.

Out of scope: collecting a background answer, which is T66.3.

#### T66.3 `agent` collect: a status and a capped preview by task id

Model: sonnet · Status: open · Depends: T66.2 · Size: ~150 · Priority: P1 · Complexity: 3

Goal: the parent fetches a background child's result when it needs it. The spawn result never holds the child's answer.

Files:
- `crates/cox-core/src/subagent.rs`
- `crates/cox-core/src/tasks.rs`
- the cox-core subagent test file

Steps:
1. A background spawn returns `AgentHandle { task_id, name, status }` as `structured` (`subagent.rs:588-596` already carries the id). The history pointer line written by `publish_task_result` (`tasks.rs:96-106`) ends with `collect <task id>`.
2. Add a `collect` input to the same `agent` tool: `{"collect": ["<task id>", …], "timeout_ms": n}`. For each task it returns `status`, one of `queued`, `running`, `done`, `error` or `cancelled`. Once a task has settled, it also returns a preview within the preset's `result_cap_tokens` and, when T66.2 archived the answer, `expand <id>`. A timeout returns a snapshot, never an error (idea: `pa-core/src/session_engine/rlm_host.rs` `RlmSubagentHost::collect`; `pa-daemon/src/rlm_child_model.rs` caps its preview at 160 characters).
3. Delivery stays through the parent (T34.5). Usage rows stay where they are: in the child's session, with cap summaries under `Job::Summarize`. A foreground `agent` call is unchanged, because the parent waits for that answer by design.

Check:
```bash
mise exec -- cargo nextest run -p cox-core subagent
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when a scripted-provider test shows:
- the spawn result holds no answer text;
- `collect` on a running task returns `running`;
- after completion, `collect` returns a preview within the cap and the archive id;
- `expand` returns the full body.

Out of scope: a separate `task_output` tool; peer messages between children.

#### T66.4 Store: the `refine_events` table

Model: sonnet · Status: open · Depends: — · Size: ~120 · Priority: P2 · Complexity: 2

Goal: every refine (proposed, applied, rejected or rolled back) is one Diesel row that a later undo and `cox refine list` can read.

Files:
- `crates/cox-store/migrations/00000000000010_refine_events/{up,down}.sql`
- `crates/cox-store/src/schema.rs`
- `crates/cox-store/src/refine.rs` (new): model and queries, in the same shape as `mcp_trust.rs`

Steps:
1. Columns:
   - `id` (TEXT primary key)
   - `session_id`
   - `scope` (`local`|`global`)
   - `trigger` (`manual`|`auto`)
   - `evidence` (the model's rationale for the whole refine; prime-agent records evidence per refine, not per edit)
   - `edits` (JSON array of `{action, kind, id, before_sha, after_sha}`)
   - `outcome` (`applied`|`rejected`|`rolled_back`)
   - `rollback_of` (nullable)
   - `fingerprint`
   - `created_at`
2. Add `refine_insert`, `refine_get` and `refine_list(session, limit)` through the typed DSL, with no raw SQL outside the migration.

Check:
```bash
mise exec -- cargo nextest run -p cox-store refine
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when a row round-trips, the list is newest first, and the down migration drops the table.

Out of scope: writing files, which is T66.5.

#### T66.5 Harness entries on disk: apply, backup, rollback, fingerprint

Model: sonnet · Status: open · Depends: T66.4 · Size: ~200 · Priority: P2 · Complexity: 3

Goal: the four refinable kinds (`prompt-note`, `memory`, `skill` and `subagent`; no factory) are applied with a backup first, undone exactly, and fingerprinted by content.

Files:
- `crates/cox-ext/src/harness.rs` (new)
- `crates/cox-ext/src/lib.rs`
- the cox-ext harness test file

Steps:
1. Scope:
   - Local is the default and stays in the session: `COX_HOME/sessions/<id>/harness/<kind>/<id>.md`.
   - Global is used only when the caller asks for it, and writes where cox already reads each kind:
     - a memory fact through `memory.rs` `save_fact`/`rebuild_index`;
     - skills under `~/.cox/skills`;
     - subagents under `~/.cox/agents` (`agents.rs:49-64`);
     - prompt notes under `~/.cox/notes`.
   - Ids match `[a-z0-9-]{1,64}`, so no path built from one can leave the scope root.
2. `apply(edits) -> Applied { before, after }` copies every file it touches to `<scope>/harness-backup/<event id>/` before writing. `rollback(event id)` restores those copies. An entry created with no previous version is deleted on rollback (idea: `pa-core/src/refinement/mod.rs` `rollback_proposal`).
3. `fingerprint(scope)` is sha256 over the entries sorted by `scope\0kind\0id`, together with their content (idea: `pa-core/src/refinement/ranking.rs` `harness_digest_fingerprint`). cox-ext links the workspace `sha2`.

Check:
```bash
mise exec -- cargo nextest run -p cox-ext harness
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when tests prove that:
- rollback restores the bytes;
- an id with `/` or `..` is refused;
- the same store gives the same fingerprint;
- a local edit is invisible to another session's load.

Out of scope: proposing edits (T66.6) and the session wiring (T66.7).

#### T66.6 Core: a refine proposal from one cheap call, with the base prompt fixed

Model: opus · Status: open · Depends: — · Size: ~180 · Priority: P2 · Complexity: 3

Goal: the core turns the transcript tail, the digest of current entries and the user's `/refine` text into a validated list of edits. It writes no file, and the base prompt can never be a target.

Files:
- `crates/cox-core/src/refine.rs` (new)
- `crates/cox-core/src/prompts/refine.md` (new; written for cox, not Prime's prose)
- `crates/cox-core/tests/refine.rs`

Steps:
1. Make one call on the `Job::Memory` routing tier, the same pattern as `memory_extract.rs`. No new `Job` variant is added.
2. The model returns `{evidence, edits: [{action: create|update|delete, kind, id, content?, reason}]}`. Validation:
   - at most 5 edits;
   - `content` of at most 4 KB;
   - `kind` is one of the four;
   - `id` matches the pattern;
   - any id that names the base prompt is refused (idea: `pa-core/src/refinement/planner.rs`, "base system prompt is not editable").
   An invalid proposal yields no edits and outcome `rejected` with the reason.
3. `prompt.md` and `prompt_minimal.md` stay `include_str!` constants (`context.rs:28, 32`).

Check:
```bash
mise exec -- cargo nextest run -p cox-core refine
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when tests show that:
- an edit aimed at the base prompt is refused;
- malformed JSON is rejected;
- a valid proposal parses;
- after a refine, the base prompt bytes and `system[0..=2]` are byte-identical to before.

Out of scope: applying edits or running a refine in the background automatically.

#### T66.7 Session: apply a refine, record it, and render the digest after the cached prefix

Model: sonnet · Status: open · Depends: T66.5, T66.6 · Size: ~180 · Priority: P2 · Complexity: 3

Goal: the session layer, not the core, writes the files and the store row. Prompt notes reach the model as a tail block in the volatile slot, so a refine never moves the cache breakpoint.

Files:
- `crates/cox-session/src/refine.rs` (new): proposal → `harness::apply` → `refine_insert`, and undo through `harness::rollback`
- `crates/cox-core/src/context.rs`: the digest goes into `system[3]`, after the last cache breakpoint (`context.rs:194-214`)
- the cox-session refine test file

Steps:
1. `refine(text, scope)` and `undo(event id)` record each outcome in `refine_events`. A proposal whose fingerprint equals the stored one writes no new backup and returns `unchanged`.
2. The digest is built from the stored entries and rebuilt only when the fingerprint changes. Changes to skills and subagents take effect in the next session, because the skills index sits in the cached `system[2]`.

Check:
```bash
mise exec -- cargo nextest run -p cox-session refine
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when tests show that:
- refining twice against the same store leaves the cached prefix bytes and the digest bytes unchanged;
- an undo restores the files and writes a `rolled_back` row;
- a local note shows up in its own session's `system[3]` and not in another session's.

Out of scope: the commands, which are T66.8.

#### T66.8 `/refine` and `cox refine`

Model: sonnet · Status: open · Depends: T66.7 · Size: ~120 · Priority: P2 · Complexity: 2

Goal: the user can propose, review, apply and undo a refine from the TUI and the CLI. Global scope always needs the explicit flag.

Files:
- `crates/cox-tui/src/commands.rs` (next to `/loop`)
- `crates/cox/src/cli.rs`
- the e2e test file

Steps:
1. The TUI commands are `/refine <text> [--global]`, `/refine list` and `/refine undo [<event id>]`. A proposal is shown as a diff and applied only after confirmation.
2. The CLI commands are `cox refine list|undo <id>`.

Check:
```bash
mise exec -- cargo nextest run -p cox refine
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when an e2e run against a scratch `COX_HOME` on the scripted provider applies one local note, lists it, and undoes it.

Out of scope: running refine automatically on a timer.

#### T66.9 A pure autonomous driver beside the USD cap

Model: opus · Status: open · Depends: — · Size: ~200 · Priority: P2 · Complexity: 4

Goal: a pure function decides whether a headless run starts another turn. It continues only toward failing gates and stays within its own turn, token and time limits. `budget::decide` stays the USD cap.

Files:
- `crates/cox-protocol/src/autonomous.rs` (new): `AutonomousPolicy`, the `GateRunner` trait, `GateResult` and `AutonomousStop`
- `crates/cox-core/src/autonomous.rs` (new)
- `crates/cox-core/tests/autonomous.rs`

Steps:
1. Policy defaults, from prime-agent's `pa-core/src/autonomous/mod.rs`:
   - 3 continuations;
   - 12 turns;
   - 80 000 tokens;
   - 30 minutes;
   - 3 gate retries;
   - a 5-minute timeout per gate.
2. `decide(&State, last_stop, gate) -> Next::Continue(text) | Next::Stop(AutonomousStop)`:
   - `Error`, `Interrupted`, `Refusal` and `Budget` never continue (idea: `pa-core/src/autonomous/gates.rs` `should_autonomously_continue`).
   - Gates run in order and the first failure wins.
   - All gates passing stops the run with `GatesPassed`.
   - A failure with retries left continues, with the failing command and the tail of its output.
   - Exhausted retries stop with `GateRetriesExhausted`.
   - Each limit stops the run under its own reason.
3. `no_progress_streak` counts continuations that changed no file and repeated the same gate result. At 2 the run stops with `NoProgress`. The streak lives in the run's `State`.
4. A passed gate only means its commands passed; it is not task success. The stop reason is `GatesPassed`, and no surface prints "done" or "success" for it.
5. Autonomous mode needs at least one gate; a policy with none is a config error.

Check:
```bash
mise exec -- cargo nextest run -p cox-core autonomous
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when the tests, run with a fake `GateRunner`, show that:
- a pass stops the run;
- a fail followed by a pass stops on the second try;
- the token limit stops the run while a gate is still failing;
- an error does not continue;
- an interrupt does not continue;
- two no-progress continuations stop the run.

Out of scope: config, the shell runner and the run loop (T66.10, T66.11). The TUI does not enter this loop.

#### T66.10 `[autonomous]` config, which a project cannot set

Model: sonnet · Status: open · Depends: T66.9 · Size: ~100 · Priority: P2 · Complexity: 2

Goal: the user sets gates and limits, but a repository cannot. Gates are shell commands, so a project-set gate would let a repository run code.

Files:
- `crates/cox-protocol/src/config.rs`
- `crates/cox-config/src/load.rs`: add `autonomous` to `GUARDED_KEYS` (`load.rs:497`)
- `docs/config.jsonschema`, regenerated through the drift test

Steps:
1. The `[autonomous]` table has `gates = []`, `max_continuations`, `max_turns`, `max_tokens`, `timeout_secs`, `gate_retries` and `gate_timeout_secs`.
2. A project layer that sets any of these keys is reverted with a warning, as for the other guarded keys.

Check:
```bash
mise exec -- cargo nextest run -p cox-config
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when `project_cannot_set_autonomous_gates` passes and the schema drift test is green.

Out of scope: running the gates.

#### T66.11 `cox run -p --autonomous`: shell gates and the headless loop

Model: sonnet · Status: open · Depends: T66.10 · Size: ~180 · Priority: P2 · Complexity: 3

Goal: a headless run continues under the driver's verdict. Every gate runs inside the same sandbox as `bash`.

Files:
- `crates/cox-tools/src/gate.rs` (new): `ShellGateRunner`. It uses `sandbox::command` with the session's `Policy` and the gate timeout. The output tail is capped and archived first, as for `bash`.
- `crates/cox/src/run.rs`: after each `UserTurn`, ask `decide`; on `Continue`, submit its text as the next `UserTurn`; print the stop reason
- `tests/autonomous.rs`

Steps:
1. Only `cox run -p` reads `--autonomous`, and the interactive TUI never enters the loop. The model still edits the todo list (`cox-tools/src/todo.rs`); the driver does not.
2. The stream-json output carries one `autonomous` record per decision.

Check:
```bash
mise exec -- cargo nextest run -p cox autonomous
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --check
```

Done when, against `COX_HOME=/tmp/cox-scratch` with the scripted provider, a gate that fails once and then passes yields exactly two turns and `GatesPassed`.

Out of scope: goals that persist across runs.

#### T66.12 Design: resident sessions and a supervisor

Model: opus · Status: open · Depends: — · Size: ~0 (design doc) · Priority: P2 · Complexity: 4

Goal: settle the crate-boundary change before any code. Today the TUI owns `cox_core::Session` (`cox-tui/src/app.rs:18, 94`), and the session lock (`cox-store/src/lock.rs`) ends with the process.

Files:
- `docs/design/serve.md` (new)

Steps (each question is answered with code references):
1. Can the attach protocol be the existing app-server protocol (`docs/app-server.md`: `open`, `send`, `snapshot`, `patches`) instead of a second enum in cox-protocol? If not, why not?
2. Does the TUI consume `cox_app` patches or raw `Event`s?
3. `cox serve` runs one worker process per session under one supervisor. It restarts a failed worker after 250 ms, doubling up to 30 s, and gives up after 5 consecutive failures; a worker that lives 30 s resets the count (idea: `pa-daemon/src/supervisor.rs`, `supervisor/supervision.rs`).
4. Where does the socket live under `COX_HOME`, with what mode, and who may attach?
5. The session lock moves to the worker.
6. Reattach replays the rollout from cox-store and never a second JSONL dialect.
7. `docs/app-server.md` says a running turn keeps running on the host, but `cox app-server --stdio` exits when stdin closes (`cox-app/src/server.rs:145, 237-243`). Fix the doc or the behaviour.
8. Acceptance test for the later cards: on a scratch `COX_HOME`, start serve, run one scripted turn, kill the client, and reattach. The rollout must grow only by that turn's events.

Check:
```bash
test -s docs/design/serve.md
```

Done when the doc ends with the implementation cards (each within the size limit). The creator then approves them in a new §6 amendment, and only then does code start.

Out of scope: any code.

#### T66.13 Heartbeat re-entry for a resident session

Model: sonnet · Status: open · Depends: T66.12 and its approved cards · Size: ~150 · Priority: P3 · Complexity: 3

Goal: a resident session can receive a recurring prompt (idea: `pa-core/src/cron/store/heartbeat.rs`, a recurring job with a non-empty prompt, one per session). `cox-ext/src/presence.rs` stays a presence record and is not extended into a scheduler. The card's files and steps are written when T66.12's cards are approved.

#### T66.14 Archive old sessions without deleting them

Model: sonnet · Status: open · Depends: T66.12 and its approved cards · Size: ~150 · Priority: P3 · Complexity: 2

Goal: `cox sessions` stays short once there are hundreds of sessions. cox-store has no pruning today; its only deletes are of `memory_files`, `plugin_grants`, `plugin_kv` and `mcp_trust`. A sweep marks a session archived (new `sessions.archived_at` column) when it is older than 30 days or outside the newest 200. Rows, rollouts and archive rows are never deleted, so `cox expand` keeps working. Resident sessions and sessions with a scheduled job are never archived. `cox sessions --archived` lists the archived ones (idea: `pa-core/src/settings/manager.rs` defaults; `pa-daemon/src/session_archive.rs`).

---

## 4. Definition of done for v0.1

1. `cox` runs a multi-turn coding session against Anthropic, OpenAI Responses and a local Ollama model with the same tool set, with the sandbox on, on macOS and Linux.
2. `cargo test --workspace` passes offline with no API key in under 90 s on CI; every widget, transcript cell and loop scenario has a snapshot.
3. A user with `.claude/settings.json`, `CLAUDE.md`, `.claude/commands`, `.claude/agents`, `.mcp.json` and rtok hooks gets identical behaviour without editing them.
4. `cox stats` shows cost by tier and job; the `just bench` table in `research.md` §4.6 shows measured savings for each D6 mechanism; cache-read ratio on turn ≥ 3 of a typical session is ≥ 80 %.
5. `cox run -p` and `cox acp` pass their conformance tests; `cox mcp` serves `read`/`grep`/`glob` to Claude Code.
6. No `unwrap`/`panic!` outside tests; `cargo deny` clean; fuzz jobs green.
7. The seventeen invariants in §1.15 each have a passing, named test.

## 5. Roadmap

| Milestone | Phases | What a user can do | Tasks |
|-----------|--------|--------------------|-------|
| M1 "talks" | P0, P1, P2, T5.1–T5.3 | chat with tools in a scripted or real provider, resume a session | 26 |
| M2 "edits safely" | P3, P4, rest of P5 | daily-driver TUI: diff-shaped edits, sandboxed shell, approvals, diff view | 21 |
| M3 "fits in" | P6, P7 | headless/CI, MCP both ways, Claude Code/Codex config compatibility, hooks, skills | 13 |
| M4 "cheap" | P8, P9 | compaction, archive/expand, dedup, deferred tools, tiered routing, budgets, measured savings | 9 |
| M5 "everywhere" | P10, P11, P12 | memory, Zed/JetBrains via ACP, evals, release | 9 |
| M6 "competitive TUI" | P22–P30 | nothing documented is a no-op; themes, tool cards, queue, `Shift+Tab`, `/rewind` incl. shell changes, visible agents, `--plain`, published footprint | 48 |

Order of value if time is short: M1 → M2 → P8 (T8.1–T8.3) → P6 → P7 → the rest. M4 before M3 if cost is the pain; M3 before M4 if adoption is.

## 6. Plan amendments

- A1 D9, §1.1, §1.7, T0.4 — Diesel 2.2 (sqlite, bundled libsqlite3-sys with FTS5, diesel_migrations) replaces rusqlite as the store layer, matching rtok D13. Why: user request; typed models for ledger joins; sync so hooks and tests need no runtime. Effect on other tasks: T1.7, T8.3, T8.4, T10.1, T10.3 write Diesel queries in `cox-store`, never SQL elsewhere.
- A2 §2, `AGENTS.md` — agents work only on `main`; no `cox/<task-id>` branches. Why: user request. Effect: claim, commit and finish tasks on `main`.
- A3 §2, `AGENTS.md` — don't duplicate code or logic; reuse an existing helper or extract one shared helper at the responsible layer. Why: user request.
- A4 §2, `AGENTS.md` — every implemented task is marked done and moved to `done.md` with its Check output. Why: user request.
- A5 §1.2, `cox-protocol::Tool`, T3.5 — added `Tool::risk(&self, input) -> Risk`, defaulting to `spec().risk`; `cox-core::turn::run_tools` now asks the tool instead of reading `spec().risk`. Why: T3.5 step 4 and the §4 tool table require `apply_patch` to be `Destructive` only when a patch deletes > 5 files, and a static `ToolSpec` cannot express a per-call risk. Effect on other tasks: none — every other tool inherits the default; T2.2's permission engine keeps reading `ToolCall.risk`.
- A6 D13, §1.1, §1.6, T0.7 — `dotenvy` 0.15 on `cox` loads `.env` then `.env.local` from cwd (walk up) into unset process env before figment. Why: user request (local API keys / `COX_*` without a second config file). Effect: not a figment layer and does not override set variables, so D12 tests and CI keep winning; provenance stays `env`. `cox-core` does not take this dep.
- A7 `website/` — added a standalone Hugo documentation site using Tailwind CSS (modern home page plus architecture and configuration references). Why: user request. Effect: no runtime crate or release behaviour changes; publish with `hugo --source website`.
- A8 `website/`, `.github/workflows/deploy-pages.yml` — deploy the Hugo site to GitHub Pages from `main`, building within `website/` and publishing `website/public`. Why: user request. Effect: the deploy workflow installs the pinned Tailwind dependencies and runs only when site/workflow files change.
- A9 §1.1, §1.6, D3 — provider registry in two types (`docs/design/providers.md`). Type-1 (native `Provider` impl per *wire protocol*): `AnthropicProvider`, new `OpenAiResponsesProvider` (`POST /responses`, bearer-optional), `OpenAiChatProvider` (+`models` list, `from_parts`). Type-2 (compatible, zero code): any `[providers.<name>]` table (`CompatibleProviderConfig`: `base_url`, `api_key_env`, `api`, `model`, `context_window`, `models`) served by the shared Chat/Responses clients; seed `deepseek`, `openrouter` (curated), `moonshot`, `z-ai` with per-model `{id, context_window, efforts}` from models.dev and matching `prices.toml` rows (20 rows; `qwen3-coder` costed 0). Why: user request — adding DeepSeek/OpenRouter must be config lines, never a `DeepseekProvider` struct duplicating the Chat client. Effect: `ProvidersConfig` loses `deny_unknown_fields` (flattened `custom` map, Hooks/Mcp precedent — a typo'd table parses but fails closed in router/session); `Router::pick` accepts custom names (Local family id, section-model pin, per-model effort clamp to greatest-supported-≤-request); `provider_for` builds Responses-or-Chat per `api` (unknown `api` bails at startup); `--provider <name>` propagates to all tiers for any non-first-party name; `cox-provider::http` unifies key resolve/error mapping (5xx is now `Overloaded` on every backend); `Effort` gains `Ord`; `figment` becomes a `cox-protocol` dev-dep for the default.toml shape test. Deferred: per-model effort *enforcement* beyond the clamp (gateway models pass through), keyring fallback for custom keys (env-only), shared retry policy for OpenAI-shaped clients, `cox doctor` prices-age check.
- A10 D16, P13 — implement vendor-neutral OpenTelemetry observability as three bounded tasks: OTLP/HTTP traces+logs exporter, GenAI semantic instrumentation, then backend documentation/smoke stack. Why: user requested full AI-agent telemetry visible in Maple, SigNoz, Jaeger and Grafana. Effect: standard OTEL environment variables remain the portability contract; raw prompt/completion/tool content is opt-in only because it can contain source code and secrets; operational metadata, usage, costs and errors are always exported when telemetry is enabled.
- A11 §1.6, P14 — the TUI must render on any terminal font, glyph set, colour depth and language: one glyph table with an ASCII fallback and `[tui.icons]` overrides (T14.1), colour-depth downgrade plus `NO_COLOR` (T14.2), and syntect highlighting extended from markdown fences to file-shaped tool output and diff hunks with a configurable theme (T14.3). Why: user request. Effect: adds `tui.glyphs`, `[tui.icons]`, `tui.color`, `tui.syntax_theme` to §1.6; no new dependency (syntect and unicode-width are already in the tree); a font is the terminal's to choose, so cox's contract is width-correct, degradable output rather than font selection. Already covered and not redone: fenced-code highlighting (T5.3), wide/combining width handling (T5.3, T5.6), `tui.theme` (T5.1).
- A12 §1.11, T3.7 — `bash` gains an optional `shell` input (`sh` default, plus `bash`, `zsh`, `fish`, `dash`, `ksh`, `tcsh`, `nu`, `pwsh`); `cox_tools::sandbox::command` takes the shell path instead of hardcoding `/bin/sh`. Why: user request — command lines written for a specific shell (fish substitution, zsh globs) failed under `sh`. Effect: the schema enum *is* the allowlist, so a name the model invents is a deserialisation error and never reaches a spawn; the binary is resolved in `/bin`, `/usr/bin`, `/usr/local/bin`, `/opt/homebrew/bin` and never on `PATH`; a missing shell is `ToolError::Denied`. Risk classification stays tree-sitter-bash, which rates an unparseable (e.g. fish-only) line `Exec` — fails closed, never lower.
- A13 §1.1, §1.6, §1.13, P15 — git belongs to the *surfaces*, not to the tool catalogue: a `cox_tools::git` module (branch, worktree `+n −m`, worktree diff, local branch names) feeds a status-line segment (T15.2), a `Ctrl+G` Diff view that renders the worktree diff through the existing `cox_tui::diff` (T15.3), and git-aware completion of a shell line in the composer (T15.4). Why: user request. Effect: **no `git` tool** — `bash` (A12) already runs git, and a second exec path would need its own risk classification, permission rules and sandbox story to say what `Bash(git:*)` already says; `cox-tui` still never spawns a process, so `crates/cox` polls `cox_tools::git` and pushes the result in exactly as it already fills the `@` picker's file list; adds `tui.git = true|false` to §1.6 and `Ctrl+G` to the §1.13 keymap; no new dependency — git is shelled to, not linked. Untracked files are outside the counts and the diff, because including them means writing to the index and a status line is a reader (`GIT_OPTIONAL_LOCKS=0` for the same reason). A branch name and a diff body are repository input, so both reach the terminal through `text::sanitize` like any other untrusted string.

- A13 §1.7, §1.9, §1.12, T1.7, T10.3 — the ledger records the routed `effort` (migration `00000000000002_usage_effort`, `usage.effort TEXT` nullable, `UsageRow.effort: Option<Effort>`), and `cox sessions <ID>` prints one session's stored record: start, end, duration, turns, cost, then tokens and cost grouped by `(provider, model, effort)`. Why: user request — the database held every other dimension of a call but not the effort it ran at, and nothing joined the `sessions` row to its ledger rows in one view. Effect: the column is nullable rather than defaulted so pre-migration rows stay honest (printed `-`, never guessed `high`); `cox stats` gains an `effort` column in its table and CSV (the CSV header changed); `ledger_row` takes an effort argument; `cox sessions` gains a positional id that conflicts with `--grep`. Documented in `docs/observability.md` ("What the database records about a session").
- A14 §1.7, §1.9, §1.13, T7.4, P16 — concurrent sessions on one workspace see each other. Every session keeps a presence record `COX_HOME/presence/<session>.json` (pid, cwd, project root, `active|waiting|idle|stopped`, turn, last-edited paths, heartbeat) written by a built-in `Hook` (`cox_ext::presence::PresenceHook`, wrapping `ShellHooks`) — the seam the surface already installs, so the core still spawns nothing and opens no file. On `UserPromptSubmit` the hook reads the other records of the same project and returns them as `additional_context`; the core appends that as a second text block on the user message (never shown as the user's words, never in `system[0..=2]`, so the cache prefix is untouched), and `ShellHooks` maps Claude Code's `additionalContext` the same way, so Claude Code hooks that add context now work too. `PermissionRequest` fires when the engine escalates, so "waiting for approval" is observable. The TUI gets a `Msg` feed channel from the binary (`app::run(session, state, feed)`), which polls presence every 2 s; `/agents` lists live agents with status and the status line counts them; `/effort` is a session-wide override the router clamps like a tier effort; `/sessions` lists this project's recent sessions preloaded by the binary. Why: user request — several agents share this worktree and none knows the others' files are mid-edit. Effect: no new dependency; presence is a directory of small files rather than a table because it is process liveness, not history (a crashed process leaves a record whose stale heartbeat *is* the signal; `SessionEnd`/`Drop` remove it); the feed channel is the receiver T15.2 planned, so T15.2 sends `Msg::Git` on it instead of adding its own `select!` arm; in-place `/resume` stays out (T16.5 prints the command).
- A15 `Cargo.toml`, `justfile`, `mise.toml` — build profiles: `profile.dev` keeps line tables only and builds dependencies at `opt-level = 1` without debug info (the bulk of a debug target tree was dependency DWARF nobody steps through); `profile.dist` moves from thin to fat LTO with one codegen unit and stripped symbols, and deliberately keeps `panic = "unwind"` because `cox-mcp` turns a panicking task into a `JoinError` and keeps going (fail open on extensions). `just release` builds the dist profile and prints the binary size; `just cache` / `just cache-autoclean` wrap `cargo-cache` 0.8.3 (a mise tool, not a crate dependency) because the shared cargo home fills the disk. Why: build time and binary size; no crate, feature or runtime behaviour changes. Effect: CI builds dependencies at `opt-level = 1`, which is slower to compile once and faster to test; the release workflow is unchanged.
- A16 §1.13, T5.1 — screenshots and whole-screen tests for the TUI. `cox_tui::svg::buffer_to_svg` turns a rendered `Buffer` (plus the cursor `view` returns) into an SVG — one `<rect>` per background run, one `<text>` per styled run with `textLength` pinning the columns, ANSI/256/24-bit colours mapped to CSS — so a picture of the screen is derived from the same buffer the snapshot tests compare as text. `crates/cox-tui/tests/screenshots.rs` composes the terminal the way `app::run` does (scrollback rows for every finished cell above the 15-row inline viewport) for eleven states (fresh session, streaming reply after a read, finished turn with thinking collapsed, bash approval modal, slash palette, `@` picker, diff view, todo panel, running bash tool with background tasks, security banner with a warning and an error, light theme), snapshots each with insta and, when `COX_SCREENSHOTS=<dir>` is set, writes `<dir>/<name>.svg`; `just screenshots` regenerates `docs/screenshots/`. `tests/keys.rs` covers what the frame tests left out: `Ctrl+D`, paste, the `Ctrl+R` history picker, `ApprovalDecided` and `ModelSwitched` arriving from the runtime, `TaskCompleted`, the tick, and the pinned security banner. Why: user request — a way to see the TUI without running it, and tests for the keys and runtime messages nothing exercised. Effect: no new dependency (`unicode-width` was already a `cox-tui` dependency); nothing at runtime calls the SVG renderer; the SVGs are committed artifacts regenerated by the recipe, not built in CI.
- A17 `release-plz.toml`, `.github/workflows/release-plz.yml`, `CHANGELOG.md`, `config/`, T12.2, T12.3 — release-plz owns the version and the changelog; cargo-dist keeps building. Every push to `main` opens or refreshes the release PR (bump of `[workspace.package].version`, a `## [x.y.z]` section in `CHANGELOG.md` from the commits since the last `v*` tag); merging it creates the `v<version>` tag through the GitHub API, and that tag triggers `release.yml` exactly as a hand-pushed tag did. Config: git-only mode because nothing goes to crates.io (`git_only`, `publish = false`, `semver_check = false`); one tag `v{{ version }}` made by the `cox` package only, since release-plz tags package by package and checks only the local clone for an existing tag, so ten crates naming one tag would fail on the second; no GitHub Release from release-plz (cargo-dist creates it and takes the notes from the changelog section); `release_always = false`; one changelog written by `cox` with `changelog_include` of the nine libraries. The grep test's gitignored fixtures (`fixtures/grep/ignored.txt`, `build.log`) are written by the test instead of committed: a file both tracked and ignored is a dirty tree to release-plz, and the first release PR committed their deletion. `cliff.toml` is gone: release-plz embeds git-cliff, `changelog_config` is deprecated, and the `[changelog]` table carries the same verbatim-commit body (on `raw_message`, because git-cliff reads `T0.1: title` as a conventional type and `message` would drop the id); the flat pre-release list in `CHANGELOG.md` is replaced by the header, and the first release PR regenerates `0.1.0` from history. Why: user request. Effect: git-only mode diffs each crate against its last tag with `cargo package --workspace`, whose verify build needs every crate self-contained, so the files the crates `include_str!` move into them (`crates/cox-protocol/default.toml`, `crates/cox-provider/prices.toml`, `crates/cox-ext/agents/`) and `config/` keeps the documented paths as symlinks; the workflow needs a `RELEASE_PLZ_TOKEN` repository secret (fine-grained PAT: contents and pull requests, read and write) because a PR or tag made with the default `GITHUB_TOKEN` triggers no other workflow, so neither CI on the release PR nor cargo-dist on the tag would run; task commits are not conventional, so every release is a patch bump — a minor or major release sets the workspace version by hand on `main` first, and release-plz never lowers a version.
- A18 §2, `AGENTS.md` — the human is the only author: no agent adds a `Co-Authored-By` trailer, a "Generated with …" line or itself as author to a commit, merge or PR, whatever its harness defaults to. Why: user request. Effect: a rule that never bends in `AGENTS.md`; existing history is left as it is.
- A19 `.github/dependabot.yml` — Dependabot: cargo for the workspace and `fuzz/` (its own workspace and lockfile), GitHub Actions, npm for `website/`; minor and patch bumps grouped into one PR per ecosystem, a major bump on its own. Why: user request. Effect: `dependencies_update` stays off in `release-plz.toml` (Dependabot owns bumps, release-plz owns releases); a merged bump touches only root files, so it never appears in the changelog and never forces a release on its own; `deny.toml` in CI still gates every bump.

- A20 §1.12, T2.4, T2.6, T0.5, T12.3 — P17 closes leftovers that sat in `done.md` as "Not done": `Session::resume` so `--resume`/`--continue` reuse the rollout id; TUI positional `PROMPT`; OpenAI Chat/Responses `stream_with_retry`; `cox doctor` prices-age; website copy matches the shipped binary. Why: user request to finish remaining work after every §3 task was moved to `done.md`. Effect: no new crates; OpenAI constructors keep their signatures (default `retry::Policy`).

- A21 §1.12, T12.3, T17.3 — P18: TUI `--resume`/`--continue`, `/clear`, and Hugo pages for tools/compat/ide/how-it-works. Why: user request to finish remaining work after P17. Effect: `--resume` on `Cli` is not `global`, so `cox run --resume` stays on `RunArgs`.
- A22 `.github/workflows/ci.yml`, `release-plz.yml` — the `dtolnay/rust-toolchain@<version>` pin names the *toolchain*, and `1.120.0` does not exist (CI failed downloading it), so both workflows pin `@1.97.1`, the version `mise.toml` already pins and `mise exec -- rustc --version` reports. Why: red CI on every push. Effect: no floating toolchain; bump the five pins together with `mise.toml` when Rust moves. The `revert-on-failure` job skips pushes touching `.github/` (least privilege instead of granting `workflows: write`).
- A23 §2, §3 P19 — per-task branches and one draft PR for the v0.2 scoping slice. Why: user request `work through the plan on a separate branch, one commit per task, and open a draft PR` (translated), which overrides A2 (`main`-only) for this slice only. Effect: work happens on branch `plan/v0.2-scoping`, one commit per task (`T19.1`–`T19.7`: each commit touches ≤ 3 files, ≤ 200 LOC, message `<task-id>: <title>`), pushed as a single draft PR into `main` (PR #24); A2 stays in force for everything outside P19. Each T19 task writes its phase-gate design doc (`docs/design/v0.2-<slug>.md`, Problem / The field / cox / Falsifiers / Review) and moves its `roadmap.md` v0.2 line into the P19 card; no runtime crate changes in this slice.
- A24 §3 P20 — ketch-model release for cox (T19.8, T20.1–T20.6). Why: user request `prepare a release in ketch and on GitHub like listrepo/ketch and pyrlyn/rtok` (translated). Effect: work happens on branch `release/ketch-model`, one commit per task, PR #26 into `main`; release-plz only proposes (no tags), `release.yml` builds `cox-<target>.tar.xz` via `scripts/package.sh` and creates `v<version>` by publishing (tag iff release completed), `scripts/cask.sh` generates the Homebrew cask, `ketch.toml` + registry entry `cox/` make `ketch install cox` work. T20.6 repins the CI toolchain to `1.97.1` after Dependabot #16 broke it with nonexistent `1.120.0` (same class as A22).
- A25 §3 P21 — TypeSafe Jev as a decision model (T21.0 scope gate). Why: user request to restore and improve the Jev note that was lost in an uncommitted working-copy overwrite of `plan.md`. Effect: new phase P21 with one `open` scope-gate task T21.0 (`docs/design/v0.2-jev.md`, Problem / The field / cox / Falsifiers / Review), mirroring the P19 gate shape: design doc first, no `crates/` changes, no new §1.1 dependency until the doc fixes the boundary (Jev answers never bypass the permission engine; fail open like hooks/skills/MCP). Restores the lost facts in their correct form — Jev is TypeSafe's System One decision model (state + Choice/Score/Noul questions in, probabilities + confidence out, `POST /v1/systemone` in its own JSON format, Python/JS SDKs, no OpenAPI; LangChain middleware and Vercel AI Gateway integrations; keys via waitlist at `console.typesafe.ai`; docs index at `docs.typesafe.ai/llms.txt`) — and maps the candidate call sites (router pick, permission classification, compaction/memory salience, skill suggestion) to the cookbook patterns (intent routing, confidence-gated routing, skill suggestion, LLM guardrails).
- A26 `research.md` §8, `docs/design/improvement-plan-2026.md`, `ideas.md` — field survey of terminal coding agents (2026-09-22) and a proposed improvement plan. Why: user request to research what agent CLIs/TUIs ship in 2026, compare with cox and plan how to be more convenient and better-looking than the field. Effect: research §8 records the survey (four research agents, author-verified cox column and crate facts, ledger #29–36); the design doc holds nine proposed phases P22–P30 (trust fixes for dead config keys and the fixed-answer `ask_user`; terminal capabilities; themes and tool cards; message queue and `Shift+Tab`; checkpoints and `/rewind`; visible agents; context and cost visibility; `--plain`; lean profile and footprint) as task cards in the §2 format, with priorities, dependencies needing approval (§7 of the doc) and falsifiers; `ideas.md` lists the phases. No task is added to the §3 table or `todo.md`; no decision in §0 changes; nothing moves until the creator approves a phase.
- A27 §3 P22–P30, top table, `todo.md`, `ideas.md`, §3.0, §5 M6 — the improvement plan approved and moved into the plan (2026-09-22). Why: the creator approved the A26 proposal and asked for every task in `plan.md` with concrete step-by-step instructions and a complexity rating. Effect: 48 tasks total: 44 open and 4 done (T22.5, T26.1, T26.2, T27.3); the cards use the §2 format (Model, Depends, Size, Priority, Complexity, Goal, Files, numbered Steps, bash Check, Done when, Out of scope), and the same ids appear in the top table and `todo.md`; `ideas.md` keeps only the unapproved later gates; `docs/design/improvement-plan-2026.md` keeps the survey, principles, pitch, non-goals and falsifiers and points to §3 for the cards. Cards were corrected against the code before the move: `ask_user` already has `Answers::Surface` (T22.1 wires it), background agents are already concurrent (T9.2) so T27.1 is about `bash` tasks and `Ctrl+B`, `SessionStart`/`Notification` already exist in `HookEvent` (T22.3 fires them), `similar` is already a workspace dependency (T24.5). Four new dependencies still need approval before their task starts: ratatui `scrolling-regions` feature (T23.2), crossterm `osc52` feature (T23.4), `terminal-colorsaurus` (T22.6), `two-face` (T24.3); each card names it. No decision in §0 changes; §1.13 keymap rows and §1.2 protocol variants that a card adds (`Submission::UserShell`, `Rewind`, `Background`; `Event::Checkpoint`, `Rewound`) are amended in that task's commit.
- A28 §3 P22, T22.8 — `cox-mcp` `oauth_refresh_failure_is_a_warning` failed twice in loaded `cargo nextest run --workspace` runs (~5.5 s). Its assertion is about how an error is classified, but the whole connect ran under the bare 5 s handshake budget. Why: user request to find the real cause and make the test deterministic without weakening it. Effect: a test-only change. The test gets a connect budget a stall cannot reach; `connect_all`, the production budget and the sibling OAuth test are unchanged.
- A29 §3 P27, T27.2, T27.5 — the creator's answer to T27.2's open question: the `/agents` card is the narrow one (name, preset, tier, cost, elapsed, state from `TaskCreated`/`TaskCompleted` and the T16.1 presence records), not a new `Event::AgentProgress`. Why: user request (today). Effect: T27.2 closes without a protocol change — per-subagent model, tokens and last tool stay undone until that event exists; `Enter` on a card opening its rollout read-only is split into the new T27.5, since it needs `/agents` to become a navigable list instead of a static `Notice`.
- A30 `crates/cox-protocol/default.toml`, T22.4 — `tui.mouse` defaults to `false`. Why: the creator's decision after T22.4 made the key live. Mouse capture in the inline viewport takes the wheel from the terminal's own scrollback and plain text selection, and the key had been `true` only because nothing read it. Effect: `default.toml`, `TuiConfig::default`, `State::new` and `docs/config.md` say `false`; `tui.mouse = true` turns on the T22.4 wheel scrolling.
- A31 §3 P27, T27.4, T27.6 — T27.4's card asked for `/loop` (TUI) and `cox run --loop` (headless) in one ≤3-file task (`crates/cox-tui/src/commands.rs`, `crates/cox-tui/src/state.rs`, `crates/cox/src/run.rs`), but the headless half also needs `crates/cox/src/cli.rs` for its new `RunArgs` flags (`--loop`, `--max-iterations`) — a fourth source file, over the cap. Why: plan.md §2 ("if the Check cannot pass without exceeding the size limit, split the task"). Effect: T27.4 lands only the TUI `/loop` (`commands.rs` + `state.rs`, `docs/getting-started.md`); the headless counterpart is the new T27.6 (`cli.rs` + `run.rs`), depending on T27.4 for the shared interval grammar. No design change — same goal, same budget-cap idea (T27.6 reuses the core's existing `StopReason::Budget` rather than inventing a second cap), split only on file count.
- A32 `crates/cox-protocol/default.toml`, T22.4 — `tui.mouse` defaults to `true` again, which reverses A30. Why: the creator's later decision. Effect: `default.toml`, `TuiConfig::default`, `State::new` and `docs/config.md` say `true`; the terminal's own selection needs Shift/Option while cox runs, and `tui.mouse = false` gives it back.
- A33 §3 P22, P27, T22.9, T27.7 — the two parts of approved cards that did not fit their size limits become cards of their own: T22.9 (T22.4's click on a folded tool card unfolds it) and T27.7 (T27.4's `↻ <time>` status-line segment for an active `/loop`). Why: the creator asked for every remaining task that needs no creator input; both halves were already approved as part of T22.4 and T27.4. Effect: two rows in the top table and `todo.md`; no new dependency.
- A34 §3 P27, T27.5 — T27.5's card listed `crates/cox-tui/src/state.rs`, `crates/cox-tui/src/view.rs`, `crates/cox/src/resume.rs`, but the actual touch is `state.rs` + `view.rs` + `crates/cox-tui/tests/agents.rs` (the T27.2 snapshot test reads `/agents`'s old `Notice` cell and has to change now that it opens a modal) + `crates/cox/src/session.rs` (not `resume.rs`, which builds a turn-oriented `History` this overlay does not need — the poll loop wants the raw `Vec<Event>` `Store::rollout_read` already returns). A 3-files-only split was drafted (§6's earlier text) to land the `cox-tui` half and leave `session.rs` to a follow-up, but `session.rs`'s `match ask { Some(Ask::GitDiff) => …, None => break }` is exhaustive over `Option<Ask>`, so the compiler requires a `session.rs` edit the moment `Ask` grows `Rollout` — a stub costs the same one match arm as the real `Store::rollout_read` call, so the split would not have saved a file. Why: discovered mid-implementation, not planned; plan.md §2's split guidance assumed avoiding the file cost was possible, and it was not. Effect: T27.5 lands whole, 4 files instead of the usual 3 (state.rs, view.rs, tests/agents.rs, session.rs); no follow-up card.
- A35 §3 P30, T30.4, T30.3 — new card T30.4 (send `anthropic-workspace-id` from `ANTHROPIC_WORKSPACE_ID`) ahead of T30.3. Why: the creator's key is not scoped to a workspace, so every Anthropic call 400s; the creator chose teaching cox the header over issuing a workspace-scoped key. Effect: T30.3 step (3) runs after T30.4.
- A36 §3 P30, T30.5, T30.3 — new card T30.5 (price every provider call through a `Priced` decorator) ahead of T30.3. Why: T1.7's `ledger_row` was never wired into a production path, so every ledger row costs $0 and budgets never fire; found during T30.4's live check; the creator chose fixing it before the paid eval run. Effect: T30.3 step (3) runs after T30.5; costs recorded before this fix are $0 and stay so (history is append-only).
- A37 §3 P30, T30.6, T30.3 — new card T30.6 (the Anthropic stream emits `ToolUseEnd` on a tool block's `content_block_stop`) ahead of T30.3. Why: without it every Anthropic tool call is dropped; found by T30.3's first live task; the creator chose fixing it first. Effect: T30.3 step (3) runs after T30.6. `openai/chat.rs` never emits `ToolUseEnd` either; that is a separate, larger fix (interleaved calls by index) proposed to the creator, not part of T30.6.
- A38 §3 P30, T30.7–T30.9 — the eval scripts become a uv-managed Python package with tests (T30.7, T30.8), and the Terminal-Bench part of T30.3 becomes T30.9 (Harbor agent, colima, $1 budget). Why: the creator asked for the scripts to be a proper package with tests before TB; the old adapter could not run for real. Effect: T30.3 closes after T30.9; `just eval` runs through uv.
- A39 §3 P30, T30.10 — the Anthropic stream wire types are generated with typify from a curated JSON Schema subset of Anthropic's OpenAPI spec; the SSE → `ProviderEvent` mapping stays hand-written. Why: the creator chose typify-generated types over a hand-written `Value` walk or a full generated SDK (none exists for Rust that handles SSE). Effect: one build-time proc-macro dependency; D3 unchanged.
- A40 §0 D3, §3 P30, T30.11–T30.12 — D3 gains an order for where provider wire types come from: a maintained SDK's types, else typify over the vendor's vendored spec, else hand-written; transport, SSE mapping and the ledger stay ours; SDK code is a `wire` module inside the provider. Login stays API-key only: Anthropic forbids third-party Claude subscription login in writing, OpenAI documents nothing for ChatGPT login (R§4.3.1). Why: the creator asked providers to check for an SDK or a generatable spec before hand-writing, starting with the Claude Code and Codex replacements. Effect: `async-openai` (types only) and a vendored Anthropic spec enter the provider crate.
- A42 §3 P30, T30.14 — new card: the comparison runner becomes a tested `evals` module with registries of agents, providers and models (`cox-bench`). Why: the creator asked for a package/module supporting different providers, models and agents rather than a one-off script.
- A43 §3 P30, T30.15–T30.16 — new cards: a built-in `lmstudio` provider whose chat loop runs over LM Studio's Anthropic-compatible `/v1/messages` through the existing Anthropic provider (T30.15), and LM Studio's native `/api/v1/models` and `models/load` for the loaded context length, capabilities and load on demand (T30.16), with hand-written types (D3/A40 step 3). Why: the creator asked for LM Studio's own API as a local provider; R§4.3.2 shows the native chat endpoint takes no custom tool schemas, so the native API serves model state and the chat stays on Messages.
- A44 §3 P30, T30.17 — new card: one model of providers, models, prices and effort; design first (D15), code only through cards the creator approves. Why: the creator's rule that provider, model, price and effort handling be as unified as possible, and the LM Studio provider (T30.15) should land in that shape.
- A45 §0 D1, §3 P30, T30.18 — new card: design a finer crate split (D1's ten crates are a floor, not a target). Why: the creator's rule that the project be split into crates as far as possible. Effect: D1 changes only through the amendment T30.18 proposes.
- A46 §3 P30, T30.15, T30.16 — approved by the creator: T30.17's result. Seven implementation cards U1–U7 (table in `docs/design/providers.md` § Target shape; evidence R§4.3.3): one key resolver, one `Transport` descriptor in every provider section, constructors over it, a pure `cox-models` catalog (context, max output, efforts, capabilities, price) replacing the `Caps` literals and `ADAPTIVE_THINKING_PREFIXES`, one per-wire effort map with `Effort::Medium`, and a `cox doctor` catalog/price row. Why: the creator's rule that provider, model, price and effort handling be as unified as possible. Effect: U1–U7 are cards T30.21–T30.27; T30.15 depends on T30.21–T30.23, T30.16 on T30.24–T30.25, T32.13–T32.15 on T30.21–T30.26.
- A47 §0 D1, §3 P32 — approved by the creator ("create the tasks for crates.md"): T30.18's result. D1 becomes: "One Cargo workspace, one static binary. A module is its own crate when it alone uses a heavy or platform-gated dependency, is a trust guard, is a ≥ 500-LOC leaf, or is needed by another crate without the rest of its own (`docs/design/crates.md`); `crates/cox/tests/deps.rs` holds the graph. No WASM or dylib plugin host in v0.1." Seventeen new crates (27 in total), extracted by cards C1–C16 in the order in `docs/design/crates.md` (`cox-models` comes from T30.24, A46 U4); every card is a `git mv` plus a re-export at the old path, a `deps.rs` rule and the AGENTS.md layout row, with no logic change; moved lines do not count toward the 200-LOC limit. Why: the creator's rule that the project be split into crates as far as possible; evidence R§4.3.4. Effect: D1 reworded as above; the phase is P32, not P31, because the unmerged branch `t31-beta-mvp` already uses P31/T31.1–T31.5; C1–C16 are cards T32.1–T32.16 in the new phase P32; the provider wires (T32.13–T32.15) move after T30.21–T30.26 (A46 U1–U6).
- A48 §3 P30, AGENTS.md — new cards T30.19–T30.20 and a convention: a file no package manager fetches (a vendored API spec, a price or model table, any JSON/YAML data) is produced only by a saved, tested Python script that is re-run to update it; no hand download, no pasted rows. Why: the creator's rule. Effect: the Anthropic spec (T30.19) and the models.dev-derived `prices.toml` rows and `default.toml` model lists (T30.20) get their scripts; A46 U4's embedded catalog rows come from T30.20's script.
- A49 §3 P30, AGENTS.md — new card T30.28 and a convention, by the creator: tests never read the real OS keychain; they inject the lookup. Why: test runs prompted for the macOS login password and read the developer's real key. Effect: T30.28 runs after T30.23.
- A50 (renumbered from `t31-beta-mvp`'s own A35 — that branch's numbering was against a different, older `main`) §3 P31, §4 — beta readiness: an audit of the v0.1 definition of done found five gaps the code can close, and each became a task (T31.1–T31.5). Criterion 6 had eight `expect` calls on production paths (the three request-body builders, the Jev client, the CLI override tree, `cox config show`); criterion 5 had no test of the shipped `cox mcp` binary, and writing one showed the default selection advertised an `outline` tool that does not exist (an outline is `read` with `mode = "outline"`); the README quick start used a top-level `cox -p` that clap rejects; the `Command` doc still said most subcommands print `not implemented`. Why: user request — determine what the beta needs and do it in one branch. Effect: `cox mcp` serves `read`, `grep`, `glob` by default; the README and `Command` doc no longer lie about the CLI shape; the work landed on branch `t31-beta-mvp` rather than `main` because the user asked for one branch, and was cherry-picked onto `main` on 2026-09-26 after `main` had moved through T30.19–T30.28 in the meantime. Landing found two of the five tasks already overtaken: T30.23 (A46 U1) had independently made `JevProvider::with_key`/`new` take `&Transport` and return `Result<Self, ProviderError>`, with `session::provider_for` already propagating it — T31.2 is dropped as fully superseded, no code changed; T30.11–T30.12's typed `wire::CreateMessageParams`/`wire::CreateResponse` rewrite had already removed the `expect` this branch targeted from the Anthropic and OpenAI Responses builders, so T31.1 keeps only its `openai/chat.rs` hunk. T31.3–T31.5 landed unchanged. Not done here: the paid eval and cache-ratio measurement (T30.3) and the macOS signing secrets — the release workflow refuses an unsigned build by design, and that stays the creator's call.
- A51 §3 P30, AGENTS.md — new card T30.29, by the creator ("fix every keychain place so fakes are used"): `COX_KEYRING=off` switches the OS keyring off in the binary, and `.cargo/config.toml` sets it for every cargo-run process. Why: the T30.28 seams covered tests, but smoke runs of the rebuilt binary (`cargo run -- doctor`) still raised keychain prompts. Effect: A49's rule now covers dev runs too. (A50 is taken by the T31 landing.)

- A52 §0 D1 and the "Deferred to v0.2+" line, §1.1, §1.15, §3 P33, `roadmap.md` — WASM plugin host, approved by the creator. It implements `docs/design/plugins.md`.
- A53 §0 "Deferred to v0.2+" line, §3 new P34 — Subagents, `todo.md`, `ideas.md` — subagent support, researched at the creator's request ("add subagent support: research it, add to the plan if it is not there"), and inter-agent communication, approved by the creator ("add support for communication between subagents, and between subagents and the main agent"). That second request is explicit approval for messaging including sibling ↔ sibling, so P34 is not gated behind `ideas.md`'s "agent teams / orchestration DSL" line the way a fuller orchestration feature would be. Why: `crates/cox-core/src/subagent.rs`/`tasks.rs`, `crates/cox-ext/src/agents.rs` and the `/agents` TUI overlay already implement a one-shot, structurally depth-1 `agent` tool with two hardcoded presets and a one-shot approval relay (`relay_approval`), but §1.11's own `agent` row already documents a named custom preset (`preset: "<name>"`) and a `tier` override that the code never got, no subagent-specific concurrency cap exists (only the generic `core.parallel_tools`), `ask_user` from a subagent carries no `Source` label, and nothing lets a parent follow up with a running or finished child or lets siblings exchange messages. Separately, the "Deferred to v0.2+" line still named "git worktree isolation for subagents" as undelivered even though it shipped as T27.3 (`subagent.rs`'s `isolation: "worktree"`, tested) — fixed in this same edit, no card for it. Effect: eleven new cards (T34.0–T34.10): a ≤ 1-page design doc for parent↔child and sibling messaging (T34.0, D15), reviewed by `think`, followed by its narrow implementation split across protocol types, core routing, the `send_message` tool and three surfaces (T34.4–T34.9, each ≤ 200 LOC / 3 files); three cards independent of the messaging design (T34.1 the custom-preset/`tier` wiring, T34.2 the concurrency cap, T34.3 the `Source`-labelled `ask_user` channel); and one optional visibility gate (T34.10). Every message is routed through the parent session as a `Submission`/`Event` (D2 pure state machine) — no side channel, no direct sibling socket. `ideas.md`'s "agent teams / orchestration DSL" line is removed as its own, still-unapproved idea: P34 is deliberately narrower than it — no `SendMessage`-as-a-tool with an injected sibling roster, no teammates, no split-pane processes, no plugin-provided agent definitions, no per-`AgentDef` permission-mode override, no `@mention` invocation; the last three are added to `ideas.md` instead, one line each. No decision in §0 changes beyond dropping the stale deferred-list line.
  - **The host.** An extism 1.30.0 host in the new crate `cox-plugin`, a pure ABI and manifest crate `cox-plugin-api` (re-exported as `cox_protocol::plugin`), and a separate guest cargo workspace `plugins/` (`cox-plugin-sdk` over extism-pdk 1.4.1, examples and templates).
  - **What a plugin can contribute**, each as a manifest capability the user approves per package digest: hooks, called methods, a context snapshot, event subscription, TUI status segments, a bottom panel or overlay, slash commands and keys under a leader, custom rendering of tool results and messages, model providers (declarative `chat`/`responses` sections, or the ABI `Provider`), catalog rows (a fill-only layer between built-in and config), MCP server declarations (stdio servers run under `sandbox::Policy`), and answers at the core's decision points (the `Advisor` trait; Jev is the first user).
  - **Why.** The creator decided it: runtime extism, the full contribution set above, capabilities approved on install and enable and re-asked on changed bytes or wider capabilities, and design and plan before code. This overrides the evidence gate in `extensions.md` and `v0.2-wasm.md` (falsifier 1: three requests MCP cannot serve), which is recorded as superseded, not refuted.
  - **Effect on §0.**
    - D1's last sentence "No WASM or dylib plugin host in v0.1." becomes "No dylib plugin host. One WASM plugin host (extism) from v0.2: `docs/design/plugins.md`; it reaches the core only through traits in `cox-protocol`."
    - "WASM plugin host (extism 1.30)" leaves the Deferred-to-v0.2+ line.
    - D2, D6(e), D9 and D14 are unchanged; the design keeps each (plugins.md §§4–7, 10).
  - **Effect on §1.1.** Two crate rows (`cox-plugin-api`, `cox-plugin`) and the dependency-direction line.
  - **Effect on §1.15.** Three invariants: 15 `plugin_tool_specs_frozen_within_session`, 16 `plugin_grant_reasked_on_digest_or_widening`, 17 `plugin_failure_is_skipped_not_fatal`; §4's definition of done now names seventeen invariants, not fourteen.
  - **Effect on `roadmap.md`.** The v0.2 line "WASM plugins (extism)" moves into P33 and is deleted from the roadmap; two new roadmap lines take its place (publishing the SDK once the ABI is stable, and installing from git/URL).
  - **Effect on AGENTS.md.** Layout rows for the two crates and `plugins/`. The trust list says a plugin host function never replaces one of the four guards.
  - **Effect on other tasks.**
    - T33.19 wraps plugin-shipped MCP stdio servers with the sandbox after T32.3 (`cox-sandbox`).
    - T33.16 extends `Catalog::load` from T30.24.
    - T33.18 reuses `resolve_key` from T30.21.
  - **Creator decisions, 2026-09-26** (resolving this amendment's open questions and the ones the Jev use case raised, T33.40; recorded in full in `docs/design/plugins.md` §14):
    1. SDK/API publishing (`cox-plugin-api`, `cox-plugin-sdk`) waits for a stable ABI; it is a `roadmap.md` item, not a P33 card.
    2. Dart stays the documented MCP-stdio-server exception (PL§13); the re-check spike (T33.37) stays in the plan.
    3. Kotlin: T33.35 spikes first; if it passes, cox keeps its own thin PDK (T33.36) and turns on extism's `wasmtime-exceptions` feature only if the spike needs it.
    4. Every MCP stdio server, not only a plugin's, runs under `sandbox::Policy`, with a per-server opt-out in config — its own card, T33.42, not folded into T33.19.
    5. `route`: a plugin may only downgrade the tier (D5 holds); the core offers a downgrade only when its own cost estimate predicts a saving (T33.40.8, from the Jev research R§4.3.6 J§5.2).
    6. Install sources in v1 stay local-folder-only (PL§1); git/URL sources move to `roadmap.md`.
    7. CI gets a separate `plugin-examples` job (go, tinygo, java, gradle, kotlin, dart), as PL§13 already specified.
    8. The built-in `[providers.typesafe]` client (`crates/cox-provider/src/jev.rs`) leaves the core once the Jev plugin reaches parity — the tombstone config type, fail-open notice and removal cards are T33.40.12–T33.40.16.
    9. Jev evals: only E1 (`risk`, Jev only, capped at $0.10, T33.40.7) is approved to run now. E2 (`route`, real Anthropic plus Jev, capped at $3, T33.40.10) stays in the plan but needs the creator's explicit go-ahead before each run.
    10. `approve_hint` becomes warning-only: a plugin may add a caution note, never say a call "looks safe" (monotone like `risk`, T33.21).
    11. The `risk` advisor is enabled only by an explicit line in `[plugins.decide]`, never automatically on install; the grant dialog states exactly what data leaves the machine.
    12. T32.15 (`cox-provider-jev`) is dropped: its table row and card move to `done.md` as "Status: dropped 2026-09-26" with the reason, and it leaves `todo.md`. After parity, `jev.rs` is deleted outright (T33.40.12), not extracted into a crate.
    13. The Jev research's ABI fix (T33.40.1): `cox_decide` returns either an `Advice` or a `ModelCall`, which the host runs against the plugin's own provider through the budget gate and ledger before calling `cox_decide_resume`; `cox_http` to a provider host is allowed only inside `cox_provider_stream`; `Question` is batched. `docs/design/plugins.md` §4 and its manifest example (§2) are updated, and the example provider is named `typesafe`, not `jev` (the plugin id stays `jev`).
    14. The release ships no prebuilt Jev plugin archive; users build it from `plugins/jev` (`just plugin jev`) and `cox plugin install <dir>`.
  - **Not decided further:** anything not listed above and not in `docs/design/plugins.md` §14 stays open for a later amendment.

- A54 §3 new P35 — External agents from plugins (Cursor first), `todo.md`, `ideas.md`, `docs/design/plugins.md` §10 — Cursor as a plugin, researched at the creator's request (`research.md` §4.3.8, inserted after P34's §4.3.7). The research found Cursor has no chat/completions endpoint (the Cloud Agents API only creates and drives durable, autonomous "Cloud Agent" runs, R§4.3.8), so it cannot be a `Provider` the way T30.15 wired LM Studio; the creator resolved the resulting question — "is Cursor still wanted as a provider?" — by deciding it is not: **"Cursor has no chat or completions API, so it is not a model provider. Add Cursor as a plugin that drives the Cursor CLI `agent` in its two official headless modes: `agent -p --output-format stream-json` and `agent acp` (ACP server over stdio, JSON-RPC 2.0)."** The creator further ruled, as a hard requirement rather than a preference: **"Only official paths: the dashboard-issued API key (env var such as `CURSOR_API_KEY`, resolved like other keys, never read from tests' real keychain) and the official CLI/ACP. Never the desktop session, and never the reverse-engineered proxies."** Why: an unauthenticated survey of Cursor's eight documented programmatic surfaces (`cursor.com/docs/api`) found the CLI's `agent -p --output-format stream-json` and `agent acp` are the only ones that are (a) officially documented, (b) driven by an issued API key rather than the desktop session, and (c) shaped like something cox already knows how to consume — an external agent process, the same relationship D4 already gives Claude Code and Codex, not a model completions wire. Effect: eleven new cards (T35.0–T35.10) in a new phase P35, gated on P33's plugin-loading/grant/sandbox path (T33.6, T33.19, T33.42) and P34's custom-preset and messaging path (T34.1, T34.5) — a new plugin manifest capability `[[external_agents]]` (T35.1), a host-only spawner under the same sandbox and grant machinery as a plugin's MCP stdio server (T35.2), an ACP client adapter reusing `crates/cox-acp`'s existing `agent-client-protocol` dependency (T35.3), a host-side `stream-json` line mapper (T35.4, chosen over a WASM guest export — EA§5), wiring the granted entry into the `agent` tool's preset resolution and P34's message routing (T35.5), the Cursor plugin package itself (T35.6), an offline e2e against a fake `agent` binary replaying fixtures recorded by a `scripts/vendor` script from the documented event shapes (T35.7, no live Cursor, no key), `cox doctor` reporting (T35.8), a user guide (T35.9), and an optional live check gated on the creator's own key (T35.10, same shape as T33.40.17). `docs/design/plugins.md` §10's "No bypass" line is updated to name external-agent CLI processes alongside MCP stdio servers as the only two kinds of process a plugin brings, both sandboxed the same way, with a forward pointer to `docs/design/external-agents.md` (T35.0). No `§0` decision changes; D1's plugin sentence already covers "an in-process WASM host … reaches the core only through traits" and this phase adds no exception to it, since the process itself is always host-spawned, never guest-spawned. `ideas.md` gains one new, still-unapproved line: the Cloud Agents API (`api.cursor.com`) as a possible background-task backend, kept separate from this phase because it would be a different shape entirely (durable server-side runs, not a local subprocess) and was not part of the creator's decision above.
- A55 §1.1 `cox-plugin` row, §3 P33 (new T33.43; T33.14 depends on it), `docs/design/plugins.md` PL§11–12, `deny.toml` — T33.3 fired two PL§12 falsifiers; the creator decided both on 2026-09-26. (1) Size: linking extism grows `cox` by 16.8 MiB (51.2 → 68.0 MiB), over the 10 MiB budget, and almost all of it is cranelift and wasmtime. The budget becomes 20 MiB, and a `plugins` cargo feature on `crates/cox` (on by default) gives a slim build with no WASM runtime, enforced by `slim_build_has_no_wasm_runtime`. (2) Advisories: extism 1.30.0 pins wasmtime 43, which has RUSTSEC-2026-0222 and RUSTSEC-2026-0269 with no fix on the 43 line. The creator chose "ignore with a deadline, WASI off". Both are in `deny.toml` with reasons and a 2026-12-31 review date; WASI stays off, so the preopens in T33.14 wait for T33.43. extism cannot share one engine across plugins (`CompiledPlugin::new` builds its own), and 0222 needs the embedder to move objects between engines, which cox never does (research.md P39). Also: extism 1.30 does not build with `default-features = false` alone, which is why `wasmtime` is declared directly (research.md P38).
- A56 §3 P34 (new T34.11) — the headless orphan fix from T34.9 has a TUI twin. Why: `run_tui` (`crates/cox/src/session.rs`) never calls `interrupt` + `wait_tasks_cleared` before its runtime drops, so quitting while a detached `bash` runs may orphan the process; the creator asked to check it and, if it leaks, reuse the headless helpers with a PTY regression test. Effect: one small card; no decision changes.
- A57 §3 P35 (new T35.11) — ACP client terminals, by the creator. Why: T35.3 refuses every `terminal/*` request from an external agent, even under a sandbox grant, because serving them (sandboxed spawn, output buffer, wait, kill, release) did not fit that card; EA§4 allows them under the process's own `sandbox::Policy`. Effect: one card after T35.3; it reuses `bash`'s sandboxed spawn, `path::confine` and `cox_permission::Engine`, so no guard gains a second path. No decision changes.
- A58 §3 P35 (new T35.12) — a dedicated `CoreError::ExternalAgent`, by the creator. Why: T35.4 had to send an external agent's failure as a provider `BadRequest`, which misnames it and could trigger provider retry or fallback. Effect: one protocol variant and a regenerated `docs/protocol.jsonschema`; no decision changes.
- A59 §3 P35 (new T35.13) — T35.5 split. Why: cox-core does no I/O, so T35.5 landed the `ExternalAgent` trait, preset resolution and the usage row, and the host drivers (stream-json and ACP over T35.2's sandboxed spawn) need their own card. Effect: T35.7 also depends on T35.13; no decision changes.
- A60 §3 P33 (new T33.44) — split the session wiring out of T33.9, T33.10, T33.11 and T33.16. Why: each built its piece against a caller-supplied `PluginHost`, and the session still drops the plugins it loads, so one card must keep a live instance and install hooks, the event tap, notices and plugin models. Effect: T33.12 also depends on T33.44; no decision changes.
- A61 §1.1 `cox-plugin` row, §3 T33.36, `docs/plugins.md` "Engine features", `docs/design/plugins.md` §15 decision 3 — the creator approved extism's `wasmtime-exceptions` for the whole workspace on 2026-09-26. Why: T33.35 showed Kotlin/Wasm output does not parse without the exception-handling proposal (research.md P42–P43). Effect: every plugin's engine enables `wasm_exceptions`; `exception_handling_module_loads` holds it; T33.36 is unblocked. Its security on wasmtime 43 is not reviewed beyond RUSTSEC-2026-0222 (not guest-triggerable); revisit with T33.43. No decision changes.
- A62 §3 new P36 (T36.1), by the creator. Why: a `Bash(<prefix>:*)` allow rule and a session grant match the command line as one string, so `Bash(git:*)` allows `git status; rm -rf …` and `Bash(rm:*)` in `deny` misses `git status && rm …`. Effect: the bash tool hands the engine its command segments; deny matches any segment, allow and grants need every segment, and substitution, `eval`/`-c` and redirects ask. `cox_permission::Engine` stays the single guard and stays pure. No decision changes.
- A63 §3 T33.36 — Kotlin waits for WASI, by the creator. Why: the T33.36 draft showed that any real Kotlin/Wasm module imports `wasi_snapshot_preview1::random_get` from Kotlin's own stdlib (`Any.hashCode` → `Random.Default`), and cox loads plugins with WASI off (A55), so the host refuses the module (research.md P45); the T33.35 spike module had no WASI imports only because it did nothing (P41). The creator chose to wait rather than define `random_get` alone in the host or stub it at build time. Effect: T33.36 depends on T33.43, like T33.34 (Go); `wasmtime-exceptions` stays on (A61). No decision changes.
- A64 §3 P36 (new T36.2), by the creator. Why: T36.1's follow-ups are security gaps: an assignment prefix (`GIT_PAGER='rm x' git log`) keeps a command rated `ReadOnly`, so it runs without asking; wrappers (`nohup rm …`) and `sh -c` strings hide a command from deny. Effect: one card; `cox_permission::Engine` stays the single guard. No decision changes.
- A65 `docs/design/plugins.md` §11–12, `scripts/footprint.json` — plugin size budget 22 MiB, by the creator. Why: PR #53's footprint job failed: the full release binary is 77 764 992 B on the macOS CI runner against a baseline of 47 323 136 B, and the whole `plugins` feature now adds +20.38 MiB over the slim build (56 409 008 B), just over A55's 20 MiB. Effect: the budget is 22 MiB and the `Darwin-arm64-ci` baseline is refreshed from that CI run (startup 6.9 ms, first frame 35.7 ms, RSS 24.0 MiB), and the local `Darwin-arm64` one from a fresh release build (77 799 856 B). No decision changes.
- A66 §3 P35 (new T35.14), by the creator. Why: the PR #53 CI fix found that bwrap's private `/tmp` hides any sandboxed program under `/tmp` on Linux, including AGENTS.md's `COX_HOME=/tmp/cox-scratch` dev runs. Effect: one card; `cox_sandbox::sandbox::Policy` stays the single sandbox guard. No decision changes.
- A67 §0 D1, D2, D11; §3 new P37 — native macOS client, M1, by the creator ("create the plan and tasks", 2026-09-28), after the desktop research (`research.md` §9) and the design `docs/design/desktop.md` with its view-layer guide `desktop/design/DESIGN.md`. Why: a desktop client that uses cox as a library, better than Claude, Codex and Cursor desktop apps on transparency (cost, tokens, context), review and native feel; SwiftUI views stay free of business logic, which lives in new crates `cox-session`, `cox-app` and `cox-ffi` (UniFFI, in-process). Effect: 38 cards T37.0–T37.37, including the design-system work the creator asked for — DTCG tokens generated into Swift and asset colours, a component catalogue decomposed into foundations, atoms, molecules and organisms, lint that forbids literals — plus adjustable glass (frosted, glossy, solid) with depth and a live token meter (sent, received, tok/s). New dependencies arrive with their cards: `uniffi` (T37.14), `style-dictionary` (T37.17), SwiftLint (T37.18), swift-snapshot-testing (T37.19), Sparkle (T37.32). §0 D1, D2 and D11 change as the creator chose (option A, 2026-09-28: in-process static library over a `cox app-server` subprocess or an ACP client, which would lack cost, checkpoints, worktrees and the inbox): D1 names the macOS app as the second artifact and the `uniffi`/`clap`/`anyhow` graph rules; D2 names the app as a consumer and makes `cox-app`'s fold a pure function of the stream (T37.8 `replay_equals_live`); D11 adds the app as a fifth surface with the 300-LOC limit on `cox-ffi`. Platform floor, by the creator (2026-09-28): macOS 26 or later on Apple Silicon only, no Intel — one `aarch64-apple-darwin` slice, no universal binary. Also by the creator (2026-09-28): the Swift code lives in this repository under `desktop/macos/`, under the repository's licence; cross-block text selection in the transcript is on by default with a setting to turn it off; the ACP host (M3) stays in `roadmap.md` and moves into this plan only when a planned card is blocked by it; Swift dependencies are chosen by the agent — the most used, best-maintained fit, otherwise our own code as a separate package with its own card. Concurrent writers on `cox.db`, by the creator (2026-09-28, option A of four — over a background daemon that owns the database, the app as server with the TUI as its client, or a separate app database that would split sessions and the cost ledger): one shared database; one process drives a session under an OS file lock and others follow it read-only or fork (T37.34); read-then-write transactions are IMMEDIATE and a `data_version` feed reports other processes' commits (T37.35); an older binary refuses a newer schema (T37.36). Swift dependencies (`research.md` §9.5): Sparkle, swift-snapshot-testing, SwiftLint with SwiftLintPlugins, swift-collections; SwiftTerm in M2; `swift-format` and the Security framework are native; STTextView is rejected because its GPL-only terms would take the royalty-free option away from the app (A68); cross-block selection is decided by a spike between Textual and our own TextKit 2 package (T37.37). M2 and M3 (DT§3.2–3.3) go to `roadmap.md`.
- A68 `Cargo.toml` `license`, `crates/cox-plugin-api/Cargo.toml`, `deny.toml` — licence metadata matches the README, by the creator (2026-09-28): "GPLv3 and royalty-free", GPL-3.0-only rather than -or-later, and the plugin SDK stays MIT/Apache. Why: the workspace said `MIT OR Apache-2.0` while `LICENSE` and the README offer GPLv3, a royalty-free licence and a commercial one. Effect: workspace crates are `GPL-3.0-only OR LicenseRef-cox-Royalty-Free` (crates.io parses `LicenseRef-` ids: its `src/licenses.rs` uses `spdx` with `allow_unknown: false`, which rejects unknown names but lexes `LicenseRef-` as its own token, `spdx` 0.13.5 `src/lexer.rs:79`, checked 2026-09-28); `cox-plugin-api` stays `MIT OR Apache-2.0` because the guest SDK in `plugins/` depends on it; `deny.toml` allows only the ref, not GPL-3.0-only, so the royalty-free option cannot gain a GPL dependency. The commercial licence needs no SPDX id. No decision changes.
- A69 §3 (new P38: T38.1–T38.3), by the creator on 2026-09-28. Why: every open P33 card waits on T33.43 (no extism release after 1.30.0 pins wasmtime ≥ 48; checked on crates.io 2026-09-28) or on the creator's key, and the creator asked to fill the slots from `ideas.md`, excluding benchmarks and comparisons with other agents. Effect: three ideas move to P38 and leave `ideas.md`: the OpenAI Chat `ToolUseEnd` bug, the orphaned detached `bash` on quit and `adaptive_thinking` from models.dev. T38.3 (`adaptive_thinking`) went back to `ideas.md` the same day: models.dev `reasoning_options` has only `effort`, `toggle` and `budget_tokens` and no adaptive marker (https://models.dev/api.json and `packages/core/src/schema.ts` in sst/models.dev at `6947a51`, checked 2026-09-28), so the flag cannot be vendored without guessing. No decision changes.
- A70 §3 (new P39: T39.1–T39.7), approved by the creator on 2026-09-28 from roadmap.md v0.2.
- A71 §3 (new P40: T40.1–T40.10), approved by the creator on 2026-09-28 from roadmap.md v0.2.
- A72 §3 (new P41: T41.1–T41.9), approved by the creator on 2026-09-28 from roadmap.md v0.2.
- A73 §3 (new P42: T42.1–T42.5), by the creator: architect/editor mode is in `roadmap.md` v0.2, approved by the creator, moved into the plan on 2026-09-28. Implements `docs/design/v0.2-modes.md` (T19.7). Why: named modes instead of manual `--permission-mode plan` + `/model`. Effect: a mode is a preset over the permission mode and the main tier only, in `cox-core/src/mode.rs` beside the subagent presets; architect = `Plan` + think, and `/mode` only narrows the configured mode (`cox_permission::narrower`); tools are never filtered by mode, so system[0..2] stay byte-stable; think still needs confirmation (invariant 9) — `/mode architect` asks once, `--mode architect` is the confirmation in headless. `cox_permission::Engine` stays the single guard. The gate's "every Exec asks" row is replaced by plan's deny (open question 1). No decision changes.
- A74 §3 (new P43: T43.0–T43.6) and §1.9, by the creator: repo map is in `roadmap.md` v0.2, approved by the creator, moved into the plan on 2026-09-28, with the creator's design decision of 2026-09-28. Implements `docs/design/v0.2-repomap.md` (T19.6), amended by T43.0. Decision: the map is built once per session at session start, ranked by recent git changes (not prompts), placed last inside system[2] (no reordering; breakpoint 1 unchanged), and rebuilt only by an explicit `/repomap refresh` or at compaction — never automatically. §1.9 gains two named prefix-change exceptions beside instruction-file changes and `tool_search`: (a) `/repomap refresh` whose bytes differ — idle only, announced by a Notice that the cached prefix restarts, recorded by `Event::RepoMapBuilt`, attributed by `cache_diag`; identical bytes change nothing; (b) the compaction rebuild, which rides the prefix restart compaction already causes. Resume replays the archived map (invariant 6). Files denied by `cox_permission::Engine` never appear in the map. Default budget 0 until the T43.6 bench.
- A75 §3 (new P44: T44.1–T44.5), by the creator: worktrees are in `roadmap.md` v0.2, approved by the creator, moved into the plan on 2026-09-28. Implements `docs/design/v0.2-worktrees.md` (T19.5). T27.3 already delivered `--worktree`, `Presence.worktree` and the status segment. Why: the gate's "never runs `git worktree add` unasked" is broken by `agent(isolation: "worktree")`, which inherits a read-only risk. Effect: worktree isolation is `Risk::Destructive` (asks; denied in plan) through the Engine; a second live session on a held worktree is warned (fail open, not locked); resume returns to the recorded worktree and never recreates it. No decision changes.
- A76 §3 (new P45: T45.1–T45.6), by the creator: subagent extras from `ideas.md`, approved by the creator on 2026-09-28 (a per-`AgentDef` permission mode, plugin-provided agent definitions, `@mention` invocation; research.md §4.3.7). Decision: `permissionMode` in an agent file can only narrow the parent's live mode (`narrower`), and a child now inherits the parent's live mode instead of `config.permissions.mode` (T45.1 fixes the widening); `cox_permission::Engine` stays the single guard. Plugin agent files are declared in `plugin.toml` `[[agents]]`, appear in the grant list as `subagent:<name> <file>`, load only for `Granted` plugins, and never replace a local definition. `@name task` runs through the same `agent` tool path (`Submission::UserAgent`), so hooks, budget and approvals apply. The three ideas leave `ideas.md`.
- A77 §3 (new P46: T46.1–T46.7), by the creator on 2026-09-28 from `ideas.md`. Why: the creator approved the user-scripted status line on top of the T28.1 segments and a Crush-style theme editor on top of the T24.2 theme files. Effect: two `ideas.md` lines leave. Decisions: the script's first line is its own row above the built-in status row (built-ins stay, as in Claude Code); it runs under `cox_sandbox::sandbox::command` read-only without network (the session's policy only under `danger-full-access`), env limited to `CHILD_ENV_ALLOWLIST` plus `COLUMNS`, 300 ms debounce, in-flight run killed by a newer input, 2 s default timeout, optional `refresh_s`; stdin reuses Claude Code's field names (D4); the output passes `cox_sanitize::sanitize`, so colours and links are stripped; `tui.status_line.command` joins the project-config guard list. The theme editor follows Crush (`charmbracelet/crush` at `ae84854`, checked 2026-09-28): `Ctrl+E` on a `/theme` row, live preview, invalid colour flagged and not applied, `Enter` saves, `Esc` reverts; editing a built-in saves `<name>-custom` because built-ins cannot be shadowed. No new dependency; no §0 decision changes.
- A78 §3 (new P47: T47.1–T47.4), by the creator on 2026-09-28 from `ideas.md`. Why: MCP elicitation was approved to map onto the T22.1 question modal. Effect: the elicitation half of the "later gates" `ideas.md` line leaves. Decisions: rmcp 3.4.0 already carries client elicitation and the 2026-07-28 MRTR round through one `ClientHandler::create_elicitation` hook, so no rmcp bump; cox declares `elicitation.form` (and `url` from T47.4) only where a person can answer — the TUI and `--plain` — and `cox run -p` declares nothing (a non-conforming server gets `decline`; `--answer` is not applied); `cox acp` connects no MCP servers today, so nothing is forwarded, and ACP v1 `elicitation/create` is the recorded path for later; answers go to the server only, never to the model or the rollout; the MCP call timeout pauses while a question is open; no new guard (`Engine` already allowed the call; the modal already sanitizes). New: rmcp's `elicitation` feature in `cox-mcp` `[dev-dependencies]` only, for the test server (needs approval). No §0 decision changes.
- A79 §3 (new P48: T48.1–T48.2), §1.1, the 2026-09-17 testing note, by the creator on 2026-09-28 from `ideas.md`. Why: the creator approved `trycmd` fixtures for `cox run -p` (text, json, stream-json, denied, bad format); the divan benchmark on the same line stays out (benchmarks excluded). Effect: new dev-dependency `trycmd` 1.2.1 (crates.io, published 2026-07-21, MIT OR Apache-2.0, MSRV 1.85; already used by rtok and ketch and listed in `rust.md`), added to the testing note's "Already covered" list; the `run_cli.rs` asserts a fixture fully covers are removed; the `ideas.md` line keeps only the divan half. No §0 decision changes.
- A80 §3 (new P49: T49.1–T49.5), by the creator on 2026-09-28 from `ideas.md`. Why: the remaining later gates of the 2026 field survey (`research.md` §8.1) and the Cursor Cloud Agents API idea (`research.md` §4.3.8) were approved as scope gates. Effect: one `docs/design/v0.3-<name>.md` per item in the P19 shape (Problem / The field / cox / Falsifiers / Review, primary sources with URL and date); no runtime code, no dependency; a "build" verdict comes back as its own amendment. The "later gates" and Cursor Cloud Agents lines leave `ideas.md`. No §0 decision changes (D7's Windows line and the "voice" entry in the §0 v0.2+ list stay until a gate's verdict is approved).
- A81 §3 (new P50: T50.1; T45.1 raised to P0), by the creator on 2026-09-28, and the answers to the P39–P49 planning questions. Why: planning P42–P45 found that instruction files and the skills index never reach the model (system[2] is still the pre-T7.1 stub) and that a subagent copies the configured permission mode instead of its parent's live one, so after Shift+Tab to plan a child still runs auto (T45.1). Answers: new dependencies approved — `base64` (T40.1, also replaces the hand-rolled encoder in `cox-tui/src/term.rs`), `url` (T41.3), `trycmd` as a dev-dependency (P48) and rmcp's `elicitation` feature as a dev-dependency of `cox-mcp` (T47.2), each with its §1 row in the implementing commit; Gemini stays a type-2 preset with the signature passthrough (P39); the scripted status line is plain text, `cox_sanitize::sanitize` is unchanged (P46); the Windows gate (T49.2) stays and decides the order of a Windows build and sandbox. Every other open question from planning takes the option the cards propose. No decision in §0 changes.
- A82 §3 P50 (new T50.2, T50.3), by the creator on 2026-09-28 ("create everything there is"). Why: T45.1 found that mode changes are never written to the rollout, so a resumed session and a child woken by `TaskMessage` come back in `Default` (a child can be wider than a Plan-mode parent), and that the volatile block shows the configured mode, not the live one. Effect: two cards; `cox_permission::Engine` stays the single guard. No decision changes.
- A83 §3 P50 (new T50.4, T50.5), by the creator on 2026-09-28 ("create everything there is"). Why: T50.2 found that resume ignores the starting mode (a session started in Plan through config comes back in `Default`, i.e. wider) and that `cox --plain` keeps showing the configured mode after `/permissions`. Effect: two cards; on resume an explicit `--permission-mode` flag wins, then the recorded mode, then config. `cox_permission::Engine` stays the single guard. No decision changes.
- A84 §3 P50 (new T50.6), by the creator on 2026-09-28 ("create everything there is"). Why: `headless_run_does_not_wait_for_a_background_shell` failed under full-workspace load in two task runs the same day and stops fail-fast runs for every parallel agent. Effect: one card. No decision changes.
- A85 §3 P37, T37.38 — T37.8 built the timeline fold without the DT§4.3 tool summaries, the `ToolGroup` row and the compaction summary, and no card claimed them. Why: they are part of the approved design (A67) and the Swift views must not parse tool output. Effect: one card T37.38 after T37.8; P37 now has 39 cards T37.0–T37.38.
- A86 §3 P37, T37.39 — T37.14 put session ownership in `cox-ffi` (it depends on cox-session, core, config, store, render, tools; lib + session + host = 499 lines against D11's 300) because `deps.rs` banned `anyhow` from `cox-app`'s resolved tree, stricter than D1's "depend on". Why: D11 and DT§4.2 keep the FFI a thin forwarder so logic is tested in Rust once. Effect: T37.39 narrows the rule to direct dependencies and moves the ownership into `cox-app`.
- A87 §3 P37, T37.40–T37.43, `docs/design/desktop.md` §5.2, §9, §11 — spike T37.37 rejected Textual 0.5.0 (per-block views keep a drag in one block, copy gives plain text and HTML, no clamp API, one-document mode 5.5 s to first frame) and passed our TextKit 2 view on all four criteria (`research.md` §9.5.13). Why: T37.37's card makes the TextKit 2 view its own package with its own cards when Textual fails. Effect: four cards for `CoxTranscriptText`; T37.23 depends on them instead of T37.37. No decision changes.
- A88 §0 D11 — the `#[uniffi::remote]` type declarations in `crates/cox-ffi/src/types.rs` do not count toward D11's 300-LOC limit on `cox-ffi`, by the creator (2026-09-28). Why: they hold no logic — one declaration per type the Swift side names — and growing with the protocol is their job; the limit guards the forwarding code in `lib.rs`, `session.rs` and `host.rs` (299 lines after T37.39 and T37.30). Effect: `types.rs` may grow without splitting or generating it. No other decision changes.
- A89 §3 P37, T37.17.1, T37.17.2, T37.19.5, T37.20.5, T37.21.11, T37.22.1–T37.22.3, T37.42.1, T37.42.2 — the P37 follow-ups in `ideas.md` become cards, by the creator ("approve all", 2026-09-28). Decisions taken with them: the High Contrast palette is derived by rule (text ≥7:1, borders ≥3:1, more opaque glass, no specular); the sidebar and inspector toggles use the system `SidebarCommands`/`InspectorCommands` and their default shortcuts, replacing both DS§4's and DT§5's pairs; the bypass strip sits under the toolbar; syntax colours come from `TranscriptStyle` tokens and a `StyledDoc` span's `rgb` is ignored, so themes and High Contrast stay consistent; T37.22 lays the window out by hand instead of `NavigationSplitView`. Why: each follow-up came from a finished card's report and none changes a D-decision; the system commands give the menu items, shortcuts and VoiceOver names macOS users already know.
- A90 §0 D11, T37.39.1 — `cox-ffi`'s 300-LOC limit becomes a rule, by the creator (2026-09-28): every exported function or method is a one-expression forward into `cox-app`, and a test enforces it. This replaces A88's line count. Why: the surface was at 299 of 300 lines while the composer, approvals, onboarding and app wiring each need new calls. A fixed number would force logic-free forwards to be squeezed or merged. What D11 guards is "no logic in the FFI layer", which the rule checks directly.
- A91 T37.23.10 — streamed reasoning becomes its own `Thinking` item and a new `Event::ThinkingDone { item, duration_ms }` carries how long the model thought, by the creator (2026-09-28; chosen over `ItemDone.duration_ms`). Why: the desktop's "Thought for 12 s" header needs the timing, and live reasoning keyed to the reply item was dropped by `Timeline`.
- A92 T37.23.12 — `cox-render`'s `StyledDoc` sends a block's kind, level and marker apart from its text, by the creator (2026-09-28). Why: the desktop hides `#` markers and draws a real quote bar and list markers instead of printing the source characters; a DT-3 view-model change the TUI can ignore.
- A93 T37.23.13 — `[desktop.transcript]` gets `text_size` and `line_height`, and the transcript applies the token line heights, by the creator (2026-09-28). Why: a text size set once in config, not only by ⌘+/⌘−, and the line spacing the design tokens specify.
- A94 T37.23.15 — tokens `font.transcript.h1` 17 pt and `font.transcript.h4` 13 pt semibold beside `font.transcript.h3`, by the creator (2026-09-28). Why: headings of different levels read as different levels, as DT§5.9 sizes them.
- A95 T37.23.16 — syntax runs in edit cards take the session theme's colours, the light or dark variant chosen by the macOS appearance, by the creator (2026-09-28). Why: the TUI and the app highlight code the same way without a second palette of syntax tokens to keep in step.
- A96 T37.30.5 — `tile.settings.*` tokens mapped to macOS system colours for the Settings page tiles, by the creator (2026-09-28). Why: the mockup's coloured tiles, with colours that follow the system's light, dark and high-contrast variants.
- A97 T37.23.17 — a `quote.bar` token (about 3 pt, a stronger colour with light, dark and high-contrast variants) for the transcript's quote bar, by the creator (2026-09-28). Why: the thought's 0.5 pt hairline T37.23.12 reused is barely visible in light mode.
- A98 T37.25.1, T37.25.2, T37.25.3 — the core emits a new `Event::ContextBreakdown` (context window and its system, tools, instructions and history split) after assembling each request, and both the desktop and the TUI show it, by the creator (2026-09-28). Why: neither surface knew the window or the split; `cox_core::context::breakdown` existed but nothing called it. A separate event, not fields on the usage event, because the split is known before the request is sent and usage only after the reply.
- A99 T50.7 — `just test` runs only the tests of the crates a change touches and their dependents (`--changed-since <ref>`, default the merge-base with `origin/main`); the full run becomes `just check-all`, by the creator (2026-09-28). Why: several agents share one machine, and a full workspace run per task wastes it; CI still runs everything.
- A100 T37.19.6 — under Increase Contrast, window and pane glass keeps a quarter of its transparency (`material.highContrast.glassKeep = 0.25`), the rule A89 already applies to the palette, by the creator (2026-09-28). Why: one contrast rule for colours and materials; `material.readableFloor` (0.8) was the other option.
- A101 T37.28.2, T37.28.3, T37.28.5 — review and rewind, by the creator (2026-09-28): the Changes tab's plain Rewind restores code only; after a code-only rewind Review shows the net diff between the checkpoint copy and disk; a new `Submission` reverts one file to before turn N, checkpointing it first; a skipped restore carries its real reason instead of "too large". Why: the recommendations of T37.28.1's report, accepted as proposed.
- A102 T37.23.18 — Edit and resend fills the composer and rewinds the conversation only (not code) to before that prompt, by the creator (2026-09-28). Why: DT§5.2's rewind, without silently discarding file changes; code is restored only from the timeline.
- A103 T37.24.10 — the composer's think toggle routes one turn to the think tier through the existing `/think` (`confirm_think`) and then turns off, by the creator (2026-09-28). Why: it reuses what the core has, and the costly tier never stays on by accident; sticky or a model-level extended-thinking switch were the other options.
- A104 T37.29.3.5 — the Context tab's cache hit is available both per turn and per session; a setting picks which one the tab shows (per turn by default), by the creator (2026-09-28). Why: a turn's hit shows what the last request reused, the session's shows whether the cache-stable prefix pays off overall.
- A105 T37.29.3.5 — Compact now is disabled while a turn runs, by the creator (2026-09-28). Why: compaction rewrites the context the running turn is using; the core would refuse or race it.
- A106 T37.32, T37.32.1, T37.32.2, T37.22.3 — T37.32 splits in two, by the creator (2026-09-28): T37.32.1 is the app target and an unsigned dev build, which needs no secrets; T37.32.2 is Developer ID signing, notarization, Sparkle and the Homebrew cask, which wait for the creator's certificates and keys. T37.22.3 depends on T37.32.1. Why: the app target unblocks the window setup and the app wiring of finished views without waiting for signing secrets.
- A107 T37.33 — the performance budget suite is on hold, by the creator (2026-09-28). Why: benchmarks are skipped for now.
- A108 T37.28.6 — the Review pane's "Send to agent" follows a setting: queue the comments while a turn runs, like the composer (the default), or send them at once, by the creator (2026-09-28). Why: the same behaviour as a typed prompt by default, with the choice left to the user.
- A109 T37.19.5 — the dark-mode control highlight is a user setting, by the creator (2026-09-28): none (the dark mockup's look, the default) or white at 10% of the light highlight's strength; a second setting applies it to controls only (e1, the default, as the card says) or to every elevation level (e1–e4, including the transcript's user bubble). Why: the dark mockup gives no value, and both looks are wanted as options.
- A110 T37.22.6 — the sidebar footer's "N providers" counts the providers the user can use now (a stored or env key, or a reachable local server), not the configured sections, by the creator (2026-09-28). Why: on defaults the section count reads 9 and says nothing about what works.
- A111 T37.22.7 — a model's display name comes from models.dev (its `name`, e.g. `Claude Sonnet 5`) through the `scripts/vendor` script into the catalog; the app's model pill drops the vendor prefix (`Sonnet 5 · high`), by the creator (2026-09-28). Why: the mockup's pill, and one source for names instead of hand-kept strings.
- A112 T37.21.11 — contrast, by the creator (2026-09-28): the filter prompt uses a new placeholder token (`#69696e` in light, the dark `text.secondary` in dark) instead of changing `text.secondary`; DS§8's Frosted and Glossy contrast is measured on the glass laid over the window fill, the worst predictable case, while the snapshot wallpaper stays for looks. Why: the recommendations as proposed; the user's wallpaper is unknown.
- A113 T37.22.8, T37.22.9 — session titles, by the creator (2026-09-28): a title is generated after the first turn by one low-cost `Job::Title` request, behind a setting (`[session] auto_title`, default on), and the user can rename a session; a user's title is never overwritten. Why: every session read "Untitled session" in the app's sidebar and toolbar.
- A114 T37.44.1, T37.44.2 — a Figma file `cox desktop` (team "Ivan's Starter team") mirrors the design, and the app is styled against it, by the creator (2026-09-28): tokens become Figma variables through a saved generator script, mockup screens become frames, screen 28 is rebuilt as editable layers bound to the variables. The repository (`desktop/design/tokens`, `mockups.html`) stays the source; an edit made in Figma is carried back into the tokens and mockups. Why: the creator wants to see and edit the design in Figma and style the app from it.
- A115 T37.22.11 — selected-row contrast, by the creator (2026-09-28): the selected session row gets its own fill token `accent.selected` — light opaque `#eaf3ff`, dark `#3b9bff` at α0.14 — so every text pair on it reaches 4.5:1 in every appearance and material; `accent.soft` and its other uses (focus halo, chips, badge, DecisionBar, inspector selection) stay as they are. Why: `text.secondary` on `accent.soft` measured 3.99:1 in light Solid and 4.22:1 in dark Solid, and changing `accent.soft` itself would pale every other use.
- A116 T37.22.12 — by the creator (2026-09-28): the model pill also drops a trailing " (latest)" from a models.dev name. Why: haiku read `Haiku 4.5 (latest)`.
- A117 T37.22.13 — by the creator (2026-09-28): the catalog is refreshed with a full `cox-vendor models` run, accepting the new `medium` efforts and the OpenRouter deepseek-v4-pro price. Why: T37.22.7 wrote only names and left the rest of models.dev's changes pending.
- A118 T37.44.3 — by the creator (2026-09-28): the Figma file uses SF Pro and SF Mono, which the creator installs locally; no substitute font. Why: the app draws in SF, and a substitute would change the metrics being compared in T37.44.2.
- A119 T37.44.4–T37.44.8 — faster layout work, by the creator (2026-09-28): layout fixes iterate in the CoxUI package alone (it depends on neither the Rust core nor the XCFramework), compare a snapshot with its frame by a pixel-diff command, and split the Figma comparison by page so agents work on separate screens in parallel. Why: each layout check was rebuilding the XCFramework and the app and one agent at a time owned every screen.
- A120 T37.45.1–T37.45.5 — by the creator (2026-09-28, "do what is best"): controls the mockups show and the design doc already names but the app lacks — settings filter, provider keys and model pop-ups, permission rules editor and session grants, MCP status and log, the onboarding drop zone — become cards; the settings sidebar stays the floating glass one of DESIGN.md §6.5, not the mockups' flush 220 pt one; the M2/M3 mockups (24–27) stay out of scope. Why: the design pass (T37.44.5) found them missing, and they are features, not layout.
- A121 §3 (new P51: T51.1–T51.21, P52: T52.1–T52.22, P53: T53.1–T53.9), `roadmap.md`, by the creator (2026-09-28): every `roadmap.md` item moves into the plan as described cards — desktop M2 and the dark glass theme as P51, desktop M3 as P52 (the ACP host no longer waits for a blocked card, A67 §11 Q8), plugin install from git or a URL and the publishing of `cox-plugin-api`, `cox-plugin-sdk` and the Go module as P53. The approved looks are mockups 24-terminal-pane-m2, 25-browser-preview-m2, 26-menu-bar-extra-m2 and 27-external-agents-acp-m3 in `desktop/design/mockups/mockups.html`, which T37.44.11 and A120 left out of P37. One agent implements the cards serially without building or running tests (the creator's instruction); a later verification pass runs each card's Check, builds and tests, and only then does the card merge and move to `done.md`. Decisions taken with them: business logic stays in Rust (`cox-app`), `cox-ffi` stays a one-expression forwarder (D11, A90), Swift never spawns a process; the terminal pane runs the user's login shell in the session cwd under the session's own `cox_sandbox::sandbox::Policy`; browser page text reaching the model is untrusted and passes `cox_sanitize::sanitize` and the archive; remote SSH sessions never send API keys or forward the ssh agent; install from a URL needs a pinned sha256 and from git a named ref, and both end in the existing local-directory install and per-digest grant; publishing stops at a dry run — `cargo publish` and tag pushes are the creator's. New dependencies: SwiftTerm (research.md §9.5.2) and KeyboardShortcuts (§9.5.7) in Swift, `portable-pty` promoted from a dev-dependency to a `cox-app` dependency; each gets its §1.1 and `toolchain.md` rows in the implementing commit. Plugin install from git or a URL reverses PL§12's "out of scope" line for those two sources only (T53.1). Why: the creator approved these items in `roadmap.md` and asked to have all of them planned; `roadmap.md` is left with no items.
- A122 T22.10 — by the creator (2026-09-28): a project config may only tighten `permissions.allow`/`ask`/`deny`: it can add `deny` and `ask` rules, never remove one, and its `allow` is reverted with a notice; the same holds for the permissions imported from a repository's `.claude/settings.json` (T22.11, creator 2026-09-28). Why: T37.45.3 found that a project config replaced the lists wholesale, so a cloned repository could drop the default `~/.ssh` deny or allow `Bash` without a prompt.
- A123 §0 (the v0.2+ "voice" entry), §1.1 (new `cox-voice` and `cox-cursor-cloud` rows), §3 (new P54: T54.1–T54.7, P55: T55.1, P56: T56.1–T56.10) — the P49 gate verdicts, by the creator (2026-09-29). (1) **T49.1 remote control: defer.** Why: `docs/design/v0.3-remote-control.md` — no `agent-client-protocol` release ships the HTTP/WebSocket transport, and hand-rolling one the spec may change is falsifier 1. Effect: no card; reopens when such a release ships, with candidate A (loopback only, opt-in, per-run token, the user's own tunnel); never a hosted relay. (2) **T49.2 Windows sandbox: defer.** Why: `docs/design/v0.3-windows-sandbox.md` — there is no Windows release target, so a backend would ship to nobody (falsifier 2). Effect: no card; a Windows release target comes first as its own decision; there is never a token-only backend that relaxes D7's forced prompts; D7 is unchanged. (3) **T49.3 voice: build** option 2 of `docs/design/v0.3-voice.md` — local `whisper-rs` in its own crate `cox-voice`, push-to-talk with auto-submit in the TUI. Why: the creator wants the push-to-talk gap closed; local transcription needs no paid endpoint, no key (D3), no audio leaving the machine and no ledger unit (it is not a request and costs nothing). Effect: P54. New dependencies (crates.io API, checked 2026-09-29): `whisper-rs` 0.16.0 (Unlicense, released 2026-03-12; https://codeberg.org/tazz4843/whisper-rs last commit 2026-03-14, not archived; already pinned by `apps/runa` and in the workspace `rust.md`) with `whisper-rs-sys` 0.15.0 (Unlicense) building whisper.cpp (MIT) through cmake; `cpal` 0.18.2 (Apache-2.0, released 2026-08-16; https://github.com/RustAudio/cpal last commit 2026-09-20; new to `rust.md`); `rubato` 5.0.0 (MIT OR Apache-2.0, released 2026-08-10; in `rust.md`); program `cmake` through mise. `cox-voice` stays behind the `voice` cargo feature of `crates/cox`, off by default; whether release builds turn it on is left to the creator. Models (ggml, MIT, https://huggingface.co/ggerganov/whisper.cpp, 78–488 MB for `tiny.en`–`small.en`, checked 2026-09-29) are pinned by a `scripts/vendor` script (A48) and fetched only by `cox voice model download` after the user confirms, SHA-256 verified; never silently. A project config cannot set `voice.*`. The §0 "voice" entry stays and points at P54. (4) **T49.4 MCP Apps: build (a) only**, as the single card T55.1: cox ignores a tool's `ui://` resource and keeps its text and structured result, test-backed. Why: `docs/design/v0.3-mcp-apps.md` — (a) is today's behaviour and adds no listener, dependency or rmcp bump; no existing MCP phase fits (P47 is elicitation), so it is its own phase. Effect: P55; (b) (browser page over a loopback listener) is deferred and returns as its own gate if its falsifier 1 fires; (c) waits for ACP to carry App resources. (5) **T49.5 Cursor Cloud Agents: build** as `docs/design/v0.3-cursor-cloud.md` describes — a host-driven plugin capability `[[cloud_agents]]`, the off-machine step an `Engine` approval (`CloudAgent(<repo>)`), and $0 `billed_externally` usage rows carrying Cursor-reported tokens. Terms, from the primary sources (checked 2026-09-29; paraphrased with section numbers): the Terms of Service (https://cursor.com/terms-of-service, last updated 2026-09-03) define the Service to include Anysphere's APIs and Documentation, grant a limited right to access and use it (§1.1), forbid reproducing, modifying, translating or making derivative works of it (§1.5(ii)), and grant no implied licences (§5.1); they say nothing about third-party API clients. The Acceptable Use Policy (https://cursor.com/acceptable-use-policy, last updated 2026-08-11), which applies however the Service is accessed, lists among prohibited uses "Accessing the Service through automated or non-human means" (by bot, script or otherwise), and copying or distributing the Service. The API page (https://cursor.com/docs/cloud-agent/api/endpoints) documents API keys for users and service accounts, marks v1 as a public beta that may change before general availability, accepts GitHub repositories only, and returns per-run `inputTokens`/`outputTokens`/`cacheWriteTokens`/`cacheReadTokens`/`totalTokens` with no cost and no model. The OpenAPI file (https://cursor.com/docs-static/cloud-agents-openapi.yaml: HTTP 200, 59,113 bytes, SHA-256 `fef3a8b7272a8b1d0eb8abbc12a4f8a9745f3d56531f86566e60d926cc0dd35f`, OpenAPI 3.0.3, `info.version` 1.0.0) carries no `license`, no `termsOfService` and no copyright statement. Reading: (i) **vendoring the spec is not allowed** — no licence, §5.1 grants none by implication, and §1.5(ii) and the AUP forbid reproducing or distributing the Documentation; so A40 step (2) and the gate doc's `scripts/vendor` step are dropped, the wire types are hand-written from the public docs (A40 step 3), and nothing is generated from or copied out of the OpenAPI file. (ii) **Whether a third-party client may call the API is unresolved**: the API exists for programs, but the AUP line, read literally, forbids any program's access, and no document names third-party clients. This is new since the gate doc (T49.5 cited only the silent ToS). Effect: P56 (T56.1–T56.10), every card blocked on the creator's written go-ahead on (ii), recorded as its own amendment (or Cursor's written answer, e.g. to the spec's contact address); no request carries the user's name, email or git identity (bodies carry the prompt, the credential-stripped GitHub URL, the ref and the model; `/v1/me` is never called); `cox-cursor-cloud` adds no dependency (it builds on `cox-provider-http`). Deferred items (1), (2) and (4b) stay recorded in their gate docs and here; A80 sends nothing back to `roadmap.md` or `ideas.md`, so neither changes. Update (creator, 2026-10-03): the terms go-ahead on (ii) is given, so P56 starts; local work only, with no real Cursor key and no network call in tests, and T56.10 stays untouched until the creator supplies a key.
- A124 T52.1, P52 — by the creator (2026-09-29), approving DT§3.3.1 as written plus two decisions. (1) An external ACP agent always gets network inside its sandbox, whatever `[sandbox] network` says; file limits stay, and an `[external_agents.<name>]` entry may add writable directories for the agent's own state, which a project config may not set. Why: none of the agents can reach its API otherwise. (2) Claude runs as `claude-agent-acp --hide-claude-auth` with `ANTHROPIC_API_KEY` only and is labelled "Claude Agent", never "Claude Code" and never a claude.ai login. Why: Anthropic's terms and branding rules for third-party clients (`research.md` §9.6). Effect: T52.2's schema gains the writable-directory list and its project-config guard; mockup 27 and the P52 goal say "Claude Agent". No §0 decision changes.
- A125 T37.44.3 — by the creator (2026-09-29), replacing A118: the Figma file uses stand-in fonts (Inter for SF Pro, Roboto Mono for SF Mono); SF Pro and SF Mono stay in the HTML mockups, CoxUI and DESIGN.md. Why: with both fonts installed locally and Figma and `figma_agent` restarted, five `use_figma` probes found SF Mono absent and SF Pro listed but flagged `hasMissingFont` (115 of 135 text layers on screen 28 drew nothing); shared fonts need an Organization or Enterprise plan (https://help.figma.com/hc/en-us/articles/360039956774, checked 2026-09-29), and the account has Starter and Pro teams only. Effect: T37.44.3 is rewritten; metric comparisons (T37.44.2, T37.44.11) use the HTML renders, not Figma.
- A126 T37.44.11 follow-ups — by the creator (2026-09-29): (1) the review screen stays as DT§5.4 has it (inspector kept); mockup 08 changes, not the app (T37.44.12). (2) The command palette of mockup 12 and ⌘K, ⌘N, ⌘⇧R are built (T37.44.13). (3) New session windows open at 1440×900, clamped to the screen (T37.44.14). (4) The approval notification has Allow once, Deny and Open as in mockup 23; DT§5.6 changes (T37.44.15). (5) An external ACP agent's question waits 15 minutes (`ASK_WAIT`, T52.5) and is then cancelled. Also carded from the verification passes: the shared walker skips `.git/` (T37.44.16) and durations format per locale (T37.44.17). Why: the running app was compared with the mockups end to end (T37.44.11), and these were the differences only the creator could decide or that were bugs outside that card.
- A128 A127 open questions 2, 4, 5, 6 — by the creator (2026-09-29). (1) **Shell on Windows**: Git Bash when present, else PowerShell, for `bash`, `!` and hooks (T57.2). (2) **process-wrap 10.0.1** is approved as the one kill path for process trees (T57.5). (3) **D7 on Windows** forces only the less strict policy: `on-failure` becomes `on-request`; `untrusted` and `never` (stricter; `never` turns every `Ask` into `Deny`) stay as configured (T57.3). (4) **Windows 10 and later, x64 and ARM64** for the desktop and the release (Mica on Windows 11, the solid fallback on 10; T57.12, T58.10). Why: the P57/P58 cards waited on these answers. Effect: T57.2, T57.3, T57.5 and T57.12 lose their creator dependency; A127 questions 1, 3 and 7 stay open.
- A130 P57, P58 — by the creator (2026-09-29): Windows builds and tests run on a Windows host by an agent started on that machine, not on the macOS host; the CI `windows` job stays off (`if: false`) until that agent has built and tested the work, and is enabled after. The macOS host takes no P57/P58 card except the T58.4.n moves (they change only `cox-app` and the Swift client and are checked on macOS); every other P57/P58 card, including writing its code, is the Windows-host agent's (creator, 2026-09-29). `just windows-check` (cargo-xwin) stays for that agent's convenience. Why: cross-checks cannot run the code, and CI should not be the first place Windows code runs. Effect: T57.1 and T57.7 go back to `todo` at 90% with a Remaining line.
- A131 §3 P33 (new T33.45), by the creator (2026-10-01): the plugin API is designed in three parts — shared (terminal and desktop), terminal-only and desktop-only — with a per-plugin surface declaration, a defined behaviour for a call to an unavailable surface and versioning under PL§4, then documented (English, `docs/ru`, `docs/uk`) and shown by a Rust example using all three. Why: plugins were designed for the terminal; the desktop app draws the shared widget slots but the API names no surface. No §0 decision changes.
- A132 §3 P37 (new T37.46–T37.49), by the creator (2026-10-02): the desktop app is synced with the latest Figma file (`KA9a0R7n6P0QbwDn92e167`, frames 01, 02, 14, 22). UI the app lacked is built in CoxUI (`AppIcon`, `WelcomeHero`, `TurnGutter`, `RewindMenu`) and existing UI is updated (`PromptActions` gets labelled buttons); data no core call returns yet is mocked behind a protocol with a `// MOCK:` implementation (`WelcomeService`, `RewindPreviewService`), and each mocked or unwired feature gets one card that replaces the mock, done one at a time. Why: the creator's Figma sync rule. No §0 decision changes.
- A129 T58.4.4, T58.4.6, T58.4.14, T58.1, T58.8, T58.28, T37.44, T52.17, T51.22 follow-ups — by the creator (2026-09-29). (1) **Sidebar filter (T58.4.4/T58.4.5)**: the filter moves to Rust; matching the localized `age` part is dropped; diacritic folding is kept only if a crate already in the workspace provides it (no new dependency), otherwise the filter stays case-insensitive only. (2) **Short model names (T58.4.6/T58.4.14)**: the core adds a separate `short_name` field; `Status.model_name` and `ModelChoice.display_name` keep their existing meaning, so the TUI and ACP do not change. (3) **Patch application and the tool tail cut** stay mirrored in each client, checked by fixture replay, not lifted into the protocol. (4) **Windows M2/M3** pieces (terminal, browser, tray, ACP, best-of-N, plugin panels) stay uncarded until a Windows M2/M3 is planned; the `ideas.md` line already covers them. (5) **Packaging (T58.28)**: packaged with external location (a sparse package) for actionable-notification identity, installed by cox's own installer, not full MSIX virtualization — answers A127 open question 3. (6) **Accent colour (T58.8)**: the cox token accent, as on macOS; a "follow the system accent" setting may come later — answers A127 open question 7. (7) **Bindings way out (T58.1)**: do not move `cox-ffi` back to uniffi 0.31; keep waiting for upstream (PR #176 adds uniffi 0.32); if the gate is not met by its review date, carry a fork under `forks/` with #176 applied — answers A127 open question 1. (8) New card **T37.44.18** "Terminal well token is opaque dark": `surface.terminal` becomes an opaque dark value matching mockup 24 (`#15161a`) in both appearances, with a High Contrast value as the token pipeline requires. (9) New card **T37.44.19** "Mockup 24 keeps the composer": the composer stays above the terminal pane; mockup 24 changes to match. (10) New card **T52.23** "Desktop draws plugin `tool:`/`item:` renderers", split for size into **T52.23.1** (Rust: `cox-app` render path, a `Block` field, `cox-ffi` types) and **T52.23.2** (Swift: CoxModel and the CoxUI tool card); T52.17's renderer-widgets-inside-tool-cards part moves here, so T52.17 closes on its panel, status-segment, overlay and command parts. (11) New card **T51.23** "Panes draw from `glass.fill` and `glass.border`": CoxUI's pane fill and rim move off `surface.*`/`separator` onto the T51.1 glass tokens, and the specular sweep's white literal (`Specular.swift`, `TranscriptView.swift`) gets a token in the same change; light and dark snapshots are re-recorded on purpose. Why: the creator's decisions on the CoxModel audit's open items and the design follow-ups the verification passes and A127/A128 left open. Effect: T58.4.4, T58.4.6, T58.4.14, T58.1, T58.8 and T58.28 are rewritten; T52.17's card gains one sentence; T37.44.18, T37.44.19, T52.23.1, T52.23.2 and T51.23 are new cards; no other card or status changes.
- A127 §1.1 (planned `cox-ffi` `cdylib`, planned `desktop/windows/` row, the "Planned by A127" note), §3 (new P57: T57.1–T57.13, P58: T58.1–T58.30) — a Windows build of the core and a Windows desktop client, by the creator (2026-09-29). (1) **UI stack: WinUI 3 + C# over the in-process Rust core through `cox-ffi` (UniFFI).** C# bindings are generated by uniffi-bindgen-cs (NordSecurity). All logic stays in Rust (`cox-app`), as in the Swift client (DT goal 1: no logic re-implemented in the UI). (2) **Scope: M1 parity only**, the DT§3.1 feature set. M2 and M3 (terminal pane, browser pane, pop-out windows, tray and hotkey, ACP host, best-of-N, plugin panels) are not in these phases and not in `roadmap.md`; `ideas.md` holds them as one line. (3) **Sandbox: as D7 says.** On Windows there is no sandbox, a loud warning, and `on-request` forced; the Windows sandbox stays deferred (A123 (2), `docs/design/v0.3-windows-sandbox.md`); the UI shows the warning (T58.25). This supersedes DT§1 "Non-goals (v1): Windows/Linux GUI" for Windows (Linux GUI stays a non-goal); `docs/design/desktop.md` §1 carries a pointer. It is also the "Windows release target first, as its own decision" that A123 (2) asked for: T57.12 adds the target, the release stays the creator's step. Facts behind the cards, checked 2026-09-29 (`research.md` §10, ledger #41–#44): uniffi-bindgen-cs's latest release `v0.11.0+v0.31.0` is on uniffi 0.31, cox-ffi pins 0.32.2, the 0.32 upgrade is open PR #176 and async callback interfaces are broken (issue #165), so T58.1 is a gate like T33.43; the current Windows App SDK is 2.5.1 (the 1.8 line's servicing ended 2026-09-24); .NET 10 is the LTS; D7's forced `on-request` is only a doc comment today (T57.3); keyring 4.2.0 and portable-pty 0.9.0 already have Windows backends; `nix` and process groups are the blockers. D1 names one macOS app linking `cox-ffi` as a static library; a C# app loads it as a DLL, so T58.1 adds `cdylib` and T58.2 proposes D1's new wording. Open questions for the creator: (1) **Bindings way out** if T58.1's gate is not met by 2026-12-31: wait, move `cox-ffi` to uniffi 0.31 (a version change), or carry a fork under `forks/`. (2) **Shell on Windows** for `bash`, `!` and hooks (T57.2): Git Bash else PowerShell (Claude Code), `pwsh` → Windows PowerShell → `cmd` (Codex), or Git Bash required. (3) **Packaging** (T58.28): MSIX with virtualization off, packaged with external location, or unpackaged self-contained. (4) **Minimum Windows version and architectures**: Windows 10 or 11 only (Mica needs Windows 11, with a solid fallback), x64 only or also ARM64 (cargo-dist's Windows signing covers x64 only). (5) **process-wrap 10.0.1** as the one kill path for process trees (T57.5; not in `rust.md`; alternatives win32job or raw `windows-sys`). (6) **Which policies the D7 rule forces**: only `on-failure` becomes `on-request`, or also `untrusted` and `never` (T57.3 keeps the stricter two until answered). (7) **Accent colour**: the cox token accent, or the user's Windows accent (T58.8). Why: the creator wants the desktop client on Windows with the same core and no second implementation of its logic. Effect: P57 and P58; no existing card changes; P58's feature cards wait on T58.1 and on the creator's approval of T58.9's mockups.
- A131 §3 P7 (new T7.8; numbered A67 on its branch, renumbered on merge because main's A67 is P37), by the creator. Why: T7.1 landed the instruction-chain loader but left wiring it into `context::assemble` for later (its done.md "Not done" line), and no later card picked it up, so `system[2]` is still a one-line stub and no surface sends `AGENTS.md`/`CLAUDE.md` to the model (§4 item 3). Effect: one card; the surface loads the chain and hands the text to `cox-core` (D2); the stub stays only for a workspace with no instruction file, so its prefix bytes do not change. The card touches more than three source files because the ACP factory builds its session without `open`, and the e2e needs the scripted provider to match on system blocks. No decision changes.
- A132 §3 (new P59: T59.1–T59.10) — Empryo-derived improvements, from a study of proxysoul/Empryo at `669ff91` (2026-10-02). Idea-only, clean-room: Empryo is BSL 1.1 (commercial, embedded and hosted use not granted; Change Date 2030-03-15), no code copied, and cards cite its files for the idea only. Why: deterministic compaction state, output folding, post-edit diagnostics and a graph-ranked map are the parts that port to Rust and have a falsifiable bench or test. Effect: ten new cards in a new phase; no existing card, default or status changes; the study ideas that do not fit cox or already exist in it are listed with a one-line reason in P59.
- A134 §3 P33 (T33.45 step 1; build cards T33.45.1–T33.45.10 build cards T33.45.1–T33.45.10), proposed by Claude Code / opus-5.5 under T33.45 and approved by the creator (2026-10-03); the cards are in §3 P33 and the task table, and T33.45 stays open until they land. Design: `docs/design/plugins.md` §15. Decisions: (1) **Surfaces.** `SessionSpec.surface` maps to a plugin `Surface`: `tui` and `plain` → `terminal`, `app` (local or over `cox app-server`) → `desktop`, `headless`, `acp`. (2) **Shared API** = `api = 1` minus keys, plus `InitIn.surface` and `Span.link` (OSC 8 in the terminal, a click-to-open link in the app). The 24-cell segment budget, the drop-first order and the 8-row panel are shared host layout, not terminal-only: `cox-app` already applies them (`SEGMENT_COLS`, `PANEL_ROWS`). (3) **Terminal-only API** = leader keys (`[capabilities.terminal] keys`; `ui.keys` stays an alias under `api = 1`); `cox:tui/v1` is reserved with no function yet. (4) **Desktop-only API** = an inspector tab, toolbar items, palette actions (both run a declared command through `cox_command`), actionable notifications (`cox_desktop_notify` in `cox:desktop/v1`) and package images in widgets; no sidebar section (a plugin lives in one session, the sidebar spans the window), no menu-bar items, never plugin UI code. (5) **Declaring**: `surfaces = [...]`, default every surface; `[capabilities.terminal]`, `[capabilities.desktop]`; new grant lines `terminal.keys`, `desktop.inspector|toolbar|palette|notify`; a stored `ui.keys` reads as `terminal.keys`; one grant covers both surfaces. (6) **Unavailable surface**: not loaded with a `Notice(Info)`; the other surface's grant left out of `granted`; its `InitOut` entries dropped with one `Notice(Info)`; a surface host function answers `AbiError::NotOnThisSurface`, checked before the grant; values degrade (image → `alt`, link → text). (7) **Versioning**: the ABI parts stay `api = 1`; the manifest denies unknown keys, so a cox older than this refuses a manifest using the new keys and skips the plugin (fail open). Why: the creator's A131. No §0 decision changes. The creator confirmed (2026-10-03): `plain` is `terminal`; a plugin skipped for its surface gets a `Notice(Info)`; an older cox skips a manifest with the new keys rather than relaxing the unknown-key rule. The creator did **not** confirm leaving out a sidebar section in (4): that stays an open question for the creator, and no card below depends on it. Cards (≤ 200 LOC and ≤ 3 files each; together they cover T33.45's original Check): T33.45.1–T33.45.10 in §3 P33.
- A135 §3 P37 (new T37.32.3), T37.32.2, `desktop/macos/project.yml` — by the creator (2026-10-03): the first macOS app build on CI is a Debug build with every macOS feature, not a release build; it runs by hand and with every release; the bundle id is `io.github.pyrlyn.cox` (the repository moved to pyrlyn). Why: a build to download and try before T37.32.2's notarized release. Effect: T37.32.3; T37.32.2 keeps Hardened Runtime, notarization, Sparkle, the bundled CLI and the cask, and reuses the signing action and `dmg.sh`. No decision in §0 changes; no new dependency.
- A136 `scripts/brand_icons.py`, `desktop/macos/App/AppIcon.icon`, CoxUI `Brand.xcassets` — by the creator (2026-10-06): the brand pack's logo (`brand/logo/`) is used for the project's icons and logos, and every icon or logo is produced by a program or script, never hand-copied. `just brand-icons` derives the macOS app icon (an Icon Composer document: the mark's tile colour as the fill, the rest of the mark as its layer, so macOS 26 draws its own squircle and grid) and CoxUI's vector `CoxMark` from `brand/logo/cox-mark.svg`; CI's `desktop-tokens` job runs its tests and `--check`. The empty `AppIcon.appiconset` is removed; `AppIcon()` draws the mark instead of the `cx` placeholder, and the README shows it straight from `brand/logo/`. Why: the app shipped with no icon and a placeholder mark. The PNG exports in `brand/logo/png/` are rendered the same way, by resvg (new mise pin `aqua:linebender/resvg`); the landing-v1 512 px copy, a hand export, is dropped. CI checks the outputs in `desktop-macos-lint`, on the Apple Silicon runners where the PNG bytes match a local render. Effect: one new tool (resvg); the `tile.app.*` and `font.mono.appIcon` tokens are now unused but stay until the next Figma token sync.
- A137 A7, A8, A19 — by the creator (2026-10-06): the Hugo site in `website/` and its `deploy-pages` workflow are removed; `docs/` stays the one place for user docs, published by `sync-docs` to <https://pyrlyn.github.io/landing/cox/docs/>. Dependabot's npm entry for `/website` goes with it; README, CONTRIBUTING, `docs/site.md` and `docs/sonarcloud-setup.md` point at `docs/` and the landing site. Why: one documentation source instead of two that drift. Effect: the site's own `architecture` and `screens` pages, which had no `docs/` counterpart, move to `docs/architecture.md` and `docs/screens.md` (images from `docs/screenshots/`, the `just screenshots` output); the already-published GitHub Pages site at `pyrlyn.github.io/cox` is no longer updated until Pages is turned off in the repository settings.
- A138 §3 P22 (T22.12) — by the creator (2026-10-07): `cox config set` (and the desktop's `set_json_in`, which shares `set_value_in`) checks the edited user file by deserializing `Config` from the `default` layer plus the edited text, through the loader's own figment and error mapping, and writes nothing when that fails. Why: `set` wrote out-of-range and unknown-variant values (`desktop.appearance.depth 1.5`) that every later load rejected, so one command left the user's config unloadable. Effect: `ConfigError` gains `Rejected(CoreError)`; the check leaves out the project, env, flag and Claude layers, so `set` never refuses a valid edit because of another layer, and the desktop's write-then-rollback in `cox_app::settings::set` stays for the full layered view.
- A139 §3 (new P60: T60.1–T60.10), `roadmap.md` — by the creator (2026-10-07), after a Best of run where every candidate failed with "provider auth failed": (1) the app never starts a turn or a Best of candidate on a provider it cannot use — the provider comes from `tiers.code.provider` and must be in `usable_providers` (A110), and the user can also pick another provider's model in the window before the first turn; (2) the provider is shown in the model chip as an icon, its name and a problem badge; (3) the toolbar's model capsule and Ask/Plan/Auto control move into the composer, whose chips already show them; (4) a glare slider (`desktop.appearance.specular`, a 0–1 scale on the material's sweep) joins Appearance; (5) the Best of compare shows the real failure reason, never `$-0.00`, and no actions on a failed candidate; (6) choosing Bypass from the composer's mode menu asks for a confirmation first, since the menu puts it one click away (the old toolbar control offered it only while it was on); the Bypass strip stays (creator, 2026-10-07: "do what is best"); (7) every desktop improvement updates the shared docs (DT§, DS§, `docs/config.md`) so the Windows and any later Linux client can repeat it. Why: the creator's request. Effect: P60; switching provider mid-session goes to `roadmap.md`. No §0 decision changes.
- A140 §3 (new P61: T61.1–T61.11), by the creator (2026-10-07): build speed for Rust and Swift — a fast profile for the XCFramework outside a release, the bindings generator on the host dev profile and skipped when the library is unchanged, CI caches for the XCFramework, SwiftPM and DerivedData, one integration-test binary per crate, one feature set per CI target, sccache locally and in the in-repository CI jobs, one Xcode build graph for the Swift package tests, the SwiftLint plugin off during builds (after the creator confirms DS§9), and two measured experiments (`build-override`, feature unification). Why: an analysis of the build configuration (2026-10-07) found the XCFramework always linked with fat LTO, 79 integration-test binaries, workspace members never cached on CI, the workspace compiled twice per CI target and `CoxModel` up to five times per Swift test run. Effect: thirteen cards; A15 still holds — what ships is `dist`; every card records before/after timings in `research.md` §4.3.9 and is reverted if it gains nothing. No decision in §0 changes; sccache and, if T61.11 chooses it, cargo-hakari are tools added by their cards with `toolchain.md` rows.
- A141 §3 (new P63: T63.1–T63.4), by the creator (2026-10-07): four improvements for the macOS app's architecture and tests — property-based tests for `SessionStore` with x-sheep/swift-property-based (the asked-for `swift-check` and `swift-testing-expectations` do not exist as property-testing libraries, and SwiftCheck is unmaintained); pull requests re-run only the Swift packages whose inputs changed since a passing run, through content-hash pass markers in `actions/cache` (snapshots cannot be skipped one by one inside a package); SwiftLint custom rules that keep AppKit and SwiftUI out of `CoxModel` and `CoxCore` (the asked-for `swift-architecture-check` does not exist, and SwiftLint 0.65.1 is already pinned); and the stores' clients through pointfreeco/swift-dependencies 1.17.1, implemented on branch `feature/swift-dependencies`. Why: the creator's request, after a review of the desktop stack (SwiftUI with Observation stores, manual initializer injection, 1,109 snapshot images re-rendered on every desktop CI run). Effect: four cards (T63.4 in three parts); `ci.yml`'s "always the full build" rule for `desktop-macos` changes for pull requests only (T63.2) — `main`, manual runs and `just desktop-test` still run every package, and the app target is built on every run. No decision in §0 changes; each new SwiftPM dependency gets its `toolchain.md` row in its card.
- A142 §3 P59 (new T59.11) — an outline row carries an inclusive end line (`start-end: signature`), so a follow-up `read` with `lines` does not guess where the item stops (2026-10-08). Why: `collect` kept only the start line. Effect: one card; no symbol index, no new `read` parameter, no new dependency. T59.8 resolves symbols from that span and otherwise stays as written.
- A143 §1.11, §3 (new P65: T65.1, T65.2) — one sandboxed program fans out MCP calls, and `tool_search` can answer with names and descriptions. Why: a tool result of thousands of bytes should reach the model only when the program prints a summary, and a search hit should not carry `input_schema` until the caller asks for the full spec. Suggested id T63 is already P63 (A141), so the cards are T65. Effect: no new crate and no new dependency; `describe` is name and description only; each call starts a new `python3 -I -u` under `sandbox::command` with network off and one fresh temp directory as its only writable root; session registration of the tool waits for a later card because T65.1 is at its file cap. Nothing is copied from the GPL code-execution server. No decision in §0 changes.
- A144 §3 P59 (T59.11, done 2026-10-08) — a JSON tool result folds to one line per node before the visible cut. Why: a single-line design or AST dump larger than `tool_output_visible_bytes` collapsed to the archive trailer. Effect: one card, in `done.md`; `compact.rs`, `dedup.rs` and `fold_repeats` stay as they are. No §0 decision changes. The fold, its call site and the tests are one Check, so they landed together past the ~200 line cap (`json_tree.rs` is the pure function; splitting it would leave neither half able to pass).
- A145 §1.1, §1.11, §3 (new P65: T65.1) — deferred crate-doc tools, claimed 2026-10-08. `docs_resolve`, `docs_query` and `docs_fetch` stay out of the default prompt (`deferred: true`, D6d). The lockfile names the version; a local `items.jsonl` answers the query; only `docs_fetch` GETs `https://docs.rs/crate/<name>/<version>/json.zst` (no `Authorization` header, no Context7 URL, no API key). `docs_query` with `name = "llms"` searches an `llms.txt` already inside a workspace root and downloads nothing. Why: crate documentation is the same shape as memory — useful, not core, found through `tool_search`. Effect: T65.1. `cox-tools` depends on `sha2` (already a workspace dependency) and `zstd` 0.13 (already in the lockfile via wasmtime) to digest and decompress that download. No §0 decision changes. The card exceeds the 200-line guide because the query, the fetch and the `llms.txt` path share one cache format; splitting them would leave a reader with nothing to read.

- A146 §1.7, §1.12, T64.24 — quarantine untrusted MCP tool definitions. A server's tool description and `readOnlyHint` are untrusted input. `contract_hash` is the sha256 hex of `name|description|canonical input schema` (object keys sorted; annotations are not an input). Migration `00000000000008_mcp_tool_trust` stores the approved hash (`status` is only `approved`). A missing row is `Pending` for a server from project `.mcp.json` or a plugin, and an auto-baselined `Approved` insert for the user layer (`config` and `~/.claude.json`). A stored hash that differs is `Changed` and is not overwritten. Until `Approved`, `McpTool::spec` uses the fixed sentence `pending trust for mcp server '<name>'; run: cox mcp trust <name>`, forces `Risk::Write` (so `readOnlyHint` cannot skip approval) and keeps `deferred: true`; `call` returns that sentence as an error and does not call the transport. `tool_search` already indexes `spec().description`, so there is no second filter. `cox mcp trust <server>` connects and writes every current hash; `cox mcp trust` lists pending and changed tools. Why: a project or plugin server can put instructions in a tool description, or set `readOnlyHint`, and both were reaching the model and the permission engine. Effect: `cox-mcp` links `sha2`, already a workspace dependency. T64.7 and T64.10 stay open — a project `[mcp.servers]` entry is still source `config` until T64.7 reverts it, and an unsandboxed stdio server is still T64.10. No Bleve, no `cox-sandbox` change, no token-store rewrite, no config watcher, no JS code-execution tool.

- A147 §3 (new P66: T66.1–T66.14), by the creator (2026-10-09): prime-agent-derived improvements, from a study of PrimeIntellect-ai/prime-agent at `afe8d14c` (v0.9.8, 2026-10-08). Idea-only, clean-room. prime-agent is MIT ("Copyright (c) 2025-2026 Prime Intellect Ltd." and "Copyright (c) 2025 Mario Zechner"), which is compatible with cox's licence. Its Rust code, though, is a byte-level port of a TypeScript product that breaks cox's rules (camelCase JSON, `anyhow` outside `crates/cox`, `unwrap`, no Diesel), so nothing is copied and no notice is needed. A card that ever copies a substantial part adds prime-agent's full MIT text, both copyright lines and the URL to that file and to `THIRD-PARTY-NOTICES`, and the notice is never replaced by cox's header. Why: the study shows four gaps in cox. (1) Compaction does not tell the model which archived outputs still expand. (2) An over-cap subagent answer is summarised or cut with no archive row, against "Lossless by default", and a background answer cannot be collected later. (3) There is no reviewed, reversible way to adjust prompt notes, memory, skills and subagents. (4) `cox run -p` has no gate-driven loop with turn, token and time limits. Effect: fourteen cards in a new phase. `budget::decide` stays the USD cap; `prompt.md` and `prompt_minimal.md` stay immutable; a project config cannot set `[autonomous]`, because its gates are shell commands. T66.12 is a design gate: resident sessions and a supervisor change a crate boundary, so their implementation cards come in a later amendment after the creator approves `docs/design/serve.md`, and T66.13 and T66.14 wait for them. No new dependency; cox-ext links the workspace `sha2`. Overflow recovery already exists (`retried_after_too_long`), so it gets no card. Not taken, with reasons in P66: peer agent sockets, the Python kernel, state factories, per-model prompt blocks in the cached prefix, Prime's prompt prose, and the `HarnessEntry` and `GoalState` schemas. No §0 decision changes.

## 7. Risk register

| # | Risk | Signal | Mitigation | Task |
|---|------|--------|------------|------|
| R1 | Anthropic wire format changes (beta headers, `fallbacks`, effort names) break T1.1 | contract tests fail after re-recording | own provider layer isolates it to one file; cassettes re-recorded with `cox record`; prices/features carry `verified_on` | T1.1, T1.5, T1.7 |
| R2 | Cache hit rate stays low because instruction files or tool lists change mid-session | `cox stats --cache` shows repeated misses | breakpoint layout §1.9; discovered tools appended not reordered; diagnostics name the byte | T2.3, T8.3 |
| R3 | Sandbox blocks legitimate builds (network for `cargo fetch`, writes to `~/.cargo`) | users switch to `danger-full-access` | `writable` extras and `network` per project; `on-failure` policy asks instead of failing; doctor explains | T4.1–T4.3 |
| R4 | tree-sitter grammar/version churn (0.25 vs 0.27) | build breaks on update | pin grammars to a tested set; outline has a regex fallback | T3.2, T3.7 |
| R5 | ratatui inline viewport glitches on some terminals (tmux, Windows Terminal) | scrollback corruption reports | `tui.inline = false` falls back to alternate screen; PTY e2e covers both | T5.1, T5.8 |
| R6 | Hook or MCP server hangs the turn | turns stall | timeouts, process-group kill, fail open | T7.4, T7.6 |
| R7 | Bash classifier misses a destructive command | a destructive command runs without asking | classifier is an allowlist for `ReadOnly` (unknown → `Exec` → ask); sandbox is the second guard; fuzz the parser | T3.7, T12.4 |
| R8 | Task size limits force half-finished features | many §6 amendments | split by design at planning time; a phase gate reviews before the next phase starts | §2 |
| R9 | Third-party prices and thresholds in research were unverifiable | ledger cost wrong | `prices.toml` verified from official pages before the ledger goes live; doctor warns when stale | T1.7 |

---

## Note 2026-09-17 — testing library candidates

Shared catalog: [`listepo/rust.md`](../../rust.md) → *Testing candidates*.
Do not auto-add to workspace `Cargo.toml`.

Fits for cox (1–3):

1. `mockall` — Provider / Tool / MCP trait unit mocks (HTTP stays on `wiremock`).
2. `tokio-test` — async helpers for core loop / provider stream unit tests.
3. `serial_test` — only if P16 concurrent-session or global-env tests cannot
   isolate with temp roots (prefer isolation first).

Already covered: `assert_cmd`, `assert_fs`, `insta`, `predicates`, `pretty_assertions`, `trycmd` (P48),
`proptest`, `rstest`, `tempfile`, `wiremock`, `libfuzzer-sys` (`fuzz/`). Skip
`bolero`/`honggfuzz` unless fuzz gaps beyond libfuzzer; `vfs` optional for
tools FS unit tests (compare with rtok T56 pattern); `testcontainers` YAGNI
unless Docker e2e is required.

### T62. Audit fixes (2026-10-07)

Findings from a code audit on 2026-10-07. Verified-clean worth noting: zero non-test `unwrap/expect/panic!` across ~150k LOC, parameterized SQL, hardened plugin install path (https-only, sha256-gated, tar ToC refusal), correct flock session lock. T62.1 (self-update 404) is closed in `done.md`.
