# cox — research: how the terminal coding agents are built, and what cox takes from each

Date: 2026-09-02. Method: six parallel low-cost research agents (Haiku) with web access, one adversarial fact-check agent, plus direct verification of crate versions (crates.io API) and Codex's workspace manifest by the author. Every claim carries a confidence tag; §6 is the fact-check ledger; §7 lists what is still unverified. Cross-references from `plan.md` decisions (D1–D16) point here as R§n.

## 1. OpenAI Codex CLI (`codex-rs`) — the closest existing Rust TUI agent

### 1.1 Shape
Rust workspace with 160+ crates, edition 2024, release `rust-v0.152.0` (2026-09-01). Domains: core/protocol/app-server, TUI, exec (headless), MCP client + server, sandbox (`linux-sandbox`, Seatbelt), providers (OpenAI, Bedrock, Ollama, LM Studio), extensions (`ext/{agent,mcp,skills,memories,web-search}`), 20+ `utils/*`. [high — repo tree]

### 1.2 Protocol: Submission Queue / Event Queue
The core is driven by a submission queue and emits an event queue; the TUI, `codex exec`, the app-server (JSON-RPC 2.0 over stdio/WebSocket/Unix socket for IDE extensions) and the MCP server are all consumers. Hierarchy `Thread → Turn → Item`, item deltas streamed, bounded queues with an overload error (`-32001`), queued submissions with stable ids that auto-start when the thread is idle. [high — app-server README] → cox D2, D11.

Design response: `docs/design/protocol.md`.

### 1.3 Dependencies (read from `codex-rs/Cargo.toml` on 2026-09-02, not from memory)
| Concern | Codex uses | cox verdict |
|---|---|---|
| TUI | ratatui 0.30.2 (default-features off), crossterm 0.29 (OpenAI fork), ratatui-macros 0.7, pulldown-cmark 0.10, syntect 5, image 0.25, arboard 3 | same, unforked crossterm |
| async / net | tokio 1, tokio-util, tokio-stream, reqwest 0.12, **eventsource-stream 0.2.3**, tokio-tungstenite 0.28, axum 0.8, tonic 0.14 | same minus axum/tonic (no app-server in v0.1) |
| MCP | rmcp =3.1.3 | rmcp 3.2 |
| storage | sqlx 0.9 (SQLite), JSONL rollouts in `~/.codex/sessions` | Diesel 2.2 on bundled SQLite (sync ORM; hooks and tests need no runtime; plan A1) + JSONL |
| diff / patch | diffy 0.4.2, similar 2.7 | diffy 0.5, similar 3.2 |
| code | tree-sitter 0.25.10, tree-sitter-bash 0.25, tree-sitter-powershell (command classification), nucleo (git), ignore 0.4.23 | same idea; tree-sitter-bash for the permission engine |
| sandbox | landlock 0.4.4, seccompiler 0.5, bubblewrap wrapper, Seatbelt via `sandbox-exec` | same |
| pty | portable-pty 0.9, vt100 0.16 (tests) | same |
| config | toml 0.9.5, toml_edit 0.24, dirs 6, keyring 3.6, schemars 0.8 | toml 1.1, toml_edit 0.25, figment, directories 6, keyring 4, schemars 1 |
| observability | tracing 0.1.44, tracing-subscriber 0.3.22, opentelemetry 0.31 (+otlp, semconv) | tracing; OTel behind a feature |
| tests | insta 1.46, pretty_assertions 1.4, wiremock 0.6, assert_cmd 2, predicates 3, tempfile 3.23, tokio-test | same |

Takeaway: the "eventsource-stream is stale" verdict from the crate survey (§4.5) is wrong in practice — the crate is small, finished, and shipped by Codex; cox uses it (D3).

### 1.4 Sandbox
Linux: bubblewrap if on `PATH` (`--unshare-user --unshare-pid`, read-only root, network namespace when restricted), `PR_SET_NO_NEW_PRIVS`, seccomp; fallback Landlock + mounts (`features.use_legacy_landlock`). `.git`, resolved `gitdir:` and `.codex` are re-applied read-only inside writable roots. macOS: `sandbox-exec` with generated Seatbelt profiles, `SandboxPolicy.network_access`. Sandbox modes read-only / workspace-write / danger-full-access; approval policies untrusted / on-request / on-failure / never. [high — linux-sandbox README, issues #11210, #6828] → cox D7.

### 1.5 Storage, config, context
`~/.codex/config.toml` with profiles, `[features]` gates (hooks, memories), `[sandbox]`, `[hooks]` (PreToolUse, PostToolUse, PreCompact, SessionStart, UserPromptSubmit, Stop), `model_context_window` / `model_max_output_tokens` (a known bug: ignored on some models, issue #19185). Sessions as JSONL rollouts plus SQLx index. Compaction and prompt-caching strategy are not documented. [med] → cox D9, D13; cox documents its compaction (D6f).

### 1.6 TUI and tests
Ratatui history cells, streaming markdown (pulldown-cmark → spans, syntect), diff renderer, composer with `@` file mentions and slash commands, transcript overlay, vim search (`/`, `?`, `n`/`N`) added in 0.152. Tests: `insta` snapshots through `TestBackend`; PTY tests with `portable-pty` + `vt100`; `wiremock` for the API. [high for crates; med for structure] → cox D10, D12.

### 1.7 Known weaknesses
`apply_patch` ENOENT on Windows (#17240); context-window config ignored (#19185); Desktop SIGKILL on update (#30359); "hangs and ignores instructions" (#38124); compaction opaque; `config.toml` silent failures. [high — issue tracker]

## 2. Competitors

### 2.1 Claude Code and GitHub Copilot CLI
| | Claude Code | Copilot CLI |
|---|---|---|
| loop | classic `while tool_use`, parallel tool calls, foreground/background subagents, `/loop`, task chips | turn-based with preview-before-execute approval gates [med] |
| tools | Read/Edit/Write, Bash, Grep/Glob, WebFetch/WebSearch, Agent, Todo, ToolSearch (deferred tool schemas) | repo browse, shell, LSP hover/goto-def, GitHub API; 128-tool cap per request |
| permissions | modes default/auto/plan/bypass; `allow`/`ask`/`deny` rules `Tool(pattern)` in `settings.json`, deny wins; sandbox = Seatbelt (macOS), bubblewrap + socat (Linux/WSL2) [high, ledger #11] | org-admin policies; no per-tool modes documented |
| extensibility | hooks (31 events, ledger #12), MCP (stdio, HTTP, OAuth), slash commands, skills (`SKILL.md`), plugins/marketplaces, subagent files, output styles | `.agent.md` custom agents, MCP (stdio/HTTP), `/lsp`, `/experimental` |
| memory / context | `CLAUDE.md` hierarchy, auto-memory dir per project, `--continue`/`--resume`, `/compact [focus]`, instructions reloaded after compaction | `copilot-instructions.md`; memory undocumented |
| UI | React + Ink; vim mode; `keybindings.json`; transcript viewer `Ctrl+O`; status line | new TUI GA June 2026: themes, narrow-terminal layout |
| headless | `-p`, `--output-format text/json/stream-json`; Agent SDK spawns the CLI | `-p`; `--headless --port` server + TypeScript SDK |
| routing / cost | delegates to Haiku for cheap jobs (the most-cited complaint: silent, only visible in verbose logs); `/cost`; `/model` | auto model selection by task/health/cost, 10 % credit discount for auto — praised because explicit |
| top complaints | April-2026 quality regression (three overlapping bugs); silent Haiku delegation; co-author trailer; over-engineered output | Node OOM after ~37 min (leaked libuv handles); auth/SSO failures; PowerShell constrained mode; wrong model id sent to custom endpoints |

Sources: code.claude.com docs (permissions, sandboxing, hooks, memory, interactive-mode), github.com/github/copilot-cli, GitHub changelog 2026-01-14 / 2026-02-25 / 2026-06-23 / 2026-07-01. Report A's "default model Claude 3.5 Sonnet" and "MCP 1.0" lines were stale and are dropped.

### 2.2 Pi, OpenCode, Crush, Goose, Gemini CLI, aider, and the rest
| Agent | Stack | What is distinctive | Weak spot |
|---|---|---|---|
| Pi (badlogic/pi-mono) | TypeScript monorepo: `pi-coding-agent`, `pi-agent-core`, `pi-ai`, custom `pi-tui` (differential rendering) | deliberately minimal: read/write/edit/bash only, no MCP, extensions as TypeScript; unified multi-provider API; sessions shareable to Hugging Face; isolation by container (Docker, micro-VM) rather than a permission model | no built-in access control; OpenRouter cost tracking, image rendering, Windows install issues [high — repo] |
| OpenCode (sst → anomalyco) | TypeScript client/server, TUI + desktop + web | Build (read-write) and Plan (read-only) agents switched with Tab; general subagent; LSP integration; MCP servers; share links; 200 k+ stars | 4 k+ open issues: desktop GPU crashes, timeout config ignored, per-subdirectory project sprawl, subagent progress invisible to integrations [high — issues; MCP/LSP from docs] |
| Crush (charmbracelet) | Go, Charm libs | MCP with stdio/http/sse, per-server timeouts and disabled tools, dynamic OAuth client registration; LSP; multi-session per project; XDG config | provider config gotchas, hard-coded timeouts, silent model fallback in headless [high — README/issues] |
| Goose (block) | Rust core, CLI + desktop + API | 15+ providers, 70+ MCP "extensions", recipes, ACP client; Linux Foundation (AAIF); evals under `evals/harbor`, `deny.toml` | session-state bugs after editing history, UI freezes; desktop is Electron [high — repo] |
| Gemini CLI (google) | TypeScript, React/Ink | checkpointing, policy engine, hooks, extensions, `GEMINI.md`, Google Search grounding, `-p` with json/ndjson output | model picker gaps, auth hangs from subdirectories, 590+ open issues [high — repo] |
| aider | Python | repo map (tree-sitter tags + PageRank, token-budgeted), edit formats (whole/diff/udiff), architect/editor two-model split, `--weak-model` for commits and summaries, auto-commit, polyglot benchmark | credential exposure in child commands, markdown fence parsing bugs, maintenance-status questions [high — repo/issues] |
| Cline | TS | Kanban task board running agents in parallel with auto-commit | — |
| Warp | Rust (closed) | "Oz" agents triage → spec → implement → review | — |
| Qwen Code / Kimi Code / Mistral Vibe | TS / TS / Python | multi-protocol provider switching at runtime; open-model first | — |
| Kilo / Roo | TS | "team of agents" modes; Kilo #1 on OpenRouter by volume | — |
| Amp, Cursor CLI, Factory Droid | closed | minimal terminal agent; IDE-bound CLI; enterprise droids | no public source |

### 2.3 Best-of-breed, and the gaps
| Feature | Who does it best | Why | cox |
|---|---|---|---|
| decoupled core / many surfaces | Codex | SQ/EQ protocol, one core → TUI, exec, app-server, MCP | D2, D11 |
| permission rules + sandbox | Claude Code + Codex | rule syntax that is readable (`Bash(npm run test:*)`) plus a real kernel sandbox with a small mode vocabulary | D7, T2.2 |
| extensibility without a plugin ABI | Claude Code | hooks (31 events), skills, commands, subagent files, MCP, plugins as bundles of those | D1, D4, P7 |
| explicit model routing | Copilot CLI (auto), aider (`weak_model`) | routing is visible and priced; Claude Code's hidden Haiku delegation is the counter-example | D5 |
| token-frugal tools | Claude Code (ToolSearch, line-range Read), aider (repo map) | deferred schemas; outlines instead of files | D6c–d |
| diff-shaped edits | Codex (V4A `apply_patch`), Claude Code (`str_replace`) | both trained-in formats | D8 |
| minimalism / hackability | Pi | four tools, extensions in the host language, session sharing | keeps the core eight tools small |
| plan vs build modes | OpenCode | one key toggles a read-only agent | permission mode `plan` (T2.2) |
| MCP client depth | Crush, Goose | OAuth registration, per-server timeouts/disabled tools; 70+ extensions | T7.6 |
| testing an agent | Codex, Goose | insta + TestBackend + vt100 PTY; `evals/` directory in-repo | D12, P12 |
| headless / SDK | Claude Code (`stream-json`), Copilot (`--headless --port`) | scriptable event stream | T6.1 |
| editor integration | ACP (Zed, JetBrains, neovim), Goose as ACP client | one protocol instead of one extension per IDE | T11.1 |

Nobody does well: (1) showing cache hit/miss and *why* a cache broke (D6, T8.3); (2) lossless truncation with a retrieval handle instead of silent cuts (D6a, T2.5); (3) refusing identical re-reads (D6b, T2.6); (4) a documented, append-only compaction that keeps the cache (D6f, T8.1); (5) a budget that stops the session (T2.7); (6) an offline, model-free regression suite that replays the agent's own event log (D12). The competitor-survey agent (C, 15 tools, ~75 lookups) adds three gaps cox should also close: no agent shows *why* it chose a tool and what context it passed (cox: the rollout JSONL plus `cox stats --turn`); none falls back to a local model when offline (cox: `cheap` tier may be Ollama, T9.1); none offers undo without git (cox: the archive keeps pre-edit file contents, T3.4). Its aggregate figures ("83 % MCP adoption", "30–50 % savings from architect/editor") are unsourced and not used.

## 3. Specifications cox implements

| Spec | Version / date | What it costs to implement | Verdict |
|---|---|---|---|
| MCP | revision 2026-07-28 (HTTP+SSE deprecated since 2025-03-26, reclassified Deprecated in 2026-07-28; Streamable HTTP + stdio current; OAuth with Client ID Metadata Documents; registry at registry.modelcontextprotocol.io) [high, ledger #3–4] | client via rmcp 3.2 (`client`, `server`, `auth` features); `.mcp.json` discovery; tool namespacing | must (T7.6, T6.2) |
| ACP (Agent Client Protocol) | JSON-RPC over stdio; Zed since Aug 2025, JetBrains since 2025-10-06 (ledger #14), public agent registry; crate `agent-client-protocol` 2.0.0 (2026-07-23) [high] | ~300 LOC over the event stream | must for IDE reach (T11.1) |
| AGENTS.md | plain markdown at repo root and above; no frontmatter; read by 20+ agents; AAIF/Linux Foundation [high] | trivial | must (T7.1) |
| Agent Skills | `SKILL.md` frontmatter `name`, `description`, optional `license`, `allowed-tools`, `metadata`, `compatibility`; progressive disclosure; opened Dec 2025 (agentskills.io) [high] | frontmatter parser + lazy body | must (T7.2) |
| Claude Code surfaces | `settings.json` permission rules, hooks JSON protocol (stdin JSON, stdout JSON, exit 2 blocks), `.claude/commands/*.md`, `.claude/agents/*.md`, `stream-json` [high — code.claude.com] | import layer | must (T7.3–T7.5, T6.1) |
| Codex surfaces | `~/.codex/config.toml`; V4A patch grammar (`*** Begin Patch`, Add/Update/Delete/Move, `@@` context, progressive matching) [med — community write-up + repo] | V4A parser only; config not imported | V4A yes (T3.5), config no |
| Anthropic Messages API | streaming, tool use, `cache_control` (min cacheable prefix 512 tokens on Fable 5.1/Opus 5/Sonnet 5, 4 096 on Haiku 4.5; ledger #21), adaptive thinking, `output_config.effort`, `fallbacks`, `count_tokens`, server tools `web_search_20260209`/`web_fetch_20260209`, mid-conversation `system` messages (Opus 5/Fable) [high — Claude API reference] | own client (D3) | must (T1.1–T1.2) |
| OpenAI Responses + Chat Completions | Responses for OpenAI models; Chat Completions is the common subset for Ollama, vLLM, LM Studio, llama.cpp, OpenRouter, DeepSeek [high] | own client | must (T1.3–T1.4) |
| OpenTelemetry GenAI semconv | `gen_ai.operation.name`, `gen_ai.provider.name`, `gen_ai.request.model`, `gen_ai.usage.input_tokens/output_tokens`; experimental [high] | tracing layer + feature flag | should (D16) |
| A2A | v1.0.1 (May 2026), agent cards | — | later |
| Benchmarks | SWE-bench Verified; Terminal-Bench (site now shows 4.0; 2.0 task count unverified, ledger #16); aider polyglot (225 exercises) | adapter | T12.1 |

### ACP stdio smoke (T11.2, 2026-09-04)

Recorded against the debug binary with a scratch `COX_HOME` (no network, dummy key only for `session/new`):

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,…},"authMethods":[]}}
→ {"jsonrpc":"2.0","id":2,"method":"authenticate","params":{"methodId":"none"}}
← {"jsonrpc":"2.0","id":2,"result":{}}
→ {"jsonrpc":"2.0","id":3,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}
← {"jsonrpc":"2.0","id":3,"result":{"sessionId":"01M1MJRKRG6GPHGPWYRX5QAT4H"}}
→ {"jsonrpc":"2.0","id":4,"method":"session/load","params":{"cwd":"/tmp","sessionId":"01ARZ3NDEKTSV4RRFFQ69G5FAA"}}
← {"jsonrpc":"2.0","id":4,"error":{"code":-32602,"message":"Invalid params",…}}
```

Notes: `session/new` without `mcpServers` is rejected by schema validation
(`DefaultOnError` does not apply to that field); unknown `session/load`
ids are explicit errors, never empty sessions. Full prompt/permission
round-trips run in-process in `crates/cox-acp/tests/conformance.rs`.

## 4. Token economy, routing, provider layer, crates

### 4.1 Prompt caching (the largest lever)
Prefix match over tools → system → messages; any byte change invalidates everything after it. Anthropic: up to 4 breakpoints, 5-minute default TTL, 1-hour TTL option, cache write 1.25× input, cache read 0.1× (Fable 5.1 cache read $0.25/MTok); model-scoped, so a routing cascade forfeits reuse across models. OpenAI: automatic prefix caching. Anthropic's own guidance: measure the capable model at lower `effort` before building a cascade. [high — Claude API reference + prompt-caching docs; ledger #21] → D5, D6e.

### 4.2 Compaction and truncation
Claude Code auto-compacts near the window and reloads instruction files after; `/compact [focus]`; older tool results are "microcompacted". Codex compaction undocumented. Third-party numbers ("Claude Code auto at 250–300 k", "500–2 000 tokens per tool result", "335 k → 169 k") were **not verifiable** (ledger #9–10) and are not used as design inputs; cox measures its own (T8.5). Techniques that are verifiable by construction: head/tail truncation with the full output on disk (cox D6a), replacing old tool results with pointers (D6f microcompact), search-before-read and line-range reads, structural outlines (tree-sitter), deferred tool schemas (Claude Code ToolSearch; Anthropic `tool_search_tool_*` server tools), diff-only edits. [high for mechanisms, low for third-party effect sizes]

### 4.3 Why an own provider layer (D3)
Candidate crates: rig-core 0.42 (multi-provider, opinionated), genai 0.7-beta, async-openai 0.41, community `anthropic` 0.0.8 (2024, unofficial, stale — ledger #20). What decides cost and correctness in 2026 is wire-level: `cache_control` placement, thinking blocks replayed unchanged on the same model, `effort`, `fallbacks`, `stop_details`, server tools, per-message system blocks. None of the frameworks track all of these, and each provider is ~500 LOC. Codex hand-rolls its client and uses `eventsource-stream` for SSE (§1.3). Verdict: own layer, `eventsource-stream` for SSE, `wiremock` + recorded `.sse` fixtures for tests.

### 4.3.1 SDKs, specs and logins for the Claude Code and Codex providers (checked 2026-09-25)
Question: before hand-writing a provider, is there (1) a maintained Rust SDK, else (2) a machine-readable spec to generate from? And can cox log in the way Claude Code and Codex do (subscription OAuth)? Every row was checked against the primary source on 2026-09-25 unless marked.

| Item | Finding | Source |
|---|---|---|
| Official Rust SDK, Anthropic | none; official SDKs are Python, TypeScript, Go, Java, Ruby, C#, PHP | https://github.com/anthropics (org repo list) |
| Official Rust SDK, OpenAI | none | https://github.com/openai (org repo list) |
| `async-openai` | 0.42.0, released 2026-09-09; hand-written "based on OpenAI OpenAPI spec", struct and field names copied from it; typed Responses API incl. streaming events (`responses` feature → `response-types`); custom base URL; raw JSON escape hatches (`extra_body`, BYOT, `serde_json::Value`) | https://crates.io/api/v1/crates/async-openai ; https://github.com/64bit/async-openai (README, CONTRIBUTING.md) |
| Community Anthropic crates | none maintained with cache_control, thinking replay and server tools; `anthropic` 0.0.8 is stale (ledger #20) | crates.io search; §4.3 |
| Codex model client (`codex-rs`) | crates `codex-api` (Responses request + SSE), `codex-client` (transport, retry), `codex-protocol` (wire types, Rust → TS via `ts-rs`), `codex-model-provider-info`, `codex-login`; hand-written; none published on crates.io; Apache-2.0; 180+ workspace crates, no stability contract | https://github.com/openai/codex `codex-rs/Cargo.toml`, `codex-rs/codex-api/src/endpoint/responses.rs`; https://crates.io/api/v1/crates?q=codex |
| Codex endpoints | `https://api.openai.com/v1/responses` (API key) and `https://chatgpt.com/backend-api/codex/responses` (ChatGPT login), same request code, SSE only | `codex-rs/codex-api/src/provider.rs`; `codex-rs/model-provider-info/src/lib.rs:77` |
| Codex ChatGPT login | OAuth PKCE (S256), token endpoint `https://auth.openai.com/oauth/token`, Codex's own client id, local callback on port 1455 (fallback 1457), tokens in `$CODEX_HOME/auth.json`, requests carry `Authorization: Bearer` + `ChatGPT-Account-ID` | `codex-rs/login/src/auth/manager.rs`, `login/src/oauth/authorization.rs`, `login/src/server.rs`, `login/src/auth/storage.rs`, `model-provider/src/bearer_auth_provider.rs` |
| Third-party use of the ChatGPT login | no OpenAI document permits or forbids it: **not found** | OpenAI docs and terms searched; nothing primary |
| Claude Code auth modes | Console API key; Claude Pro/Max/Team/Enterprise login; Bedrock; Vertex; Microsoft Foundry; gateway | https://code.claude.com/docs/en/authentication |
| Third-party use of a Claude subscription | **forbidden**: "Anthropic does not permit third-party developers to offer Claude.ai login into their own applications" | https://code.claude.com/docs/en/legal-and-compliance |
| Claude Code OAuth endpoints, client id, beta header | not published by Anthropic; only community reverse-engineering: **unverified** | https://code.claude.com/docs/en/authentication (absent) |
| Claude Code SDK dependency | the npm package ships a native binary with no declared `dependencies`; which SDK it bundles is not visible | https://registry.npmjs.org/@anthropic-ai/claude-code/latest (2.1.282) |
| Anthropic OpenAPI spec | `openapi_spec_url` removed from `anthropic-sdk-python/.stats.yml` in commit `f9b0cf28` (2026-09-03); the last linked Stainless URL still answers: OpenAPI 3.1.0, contains `MessageStreamEvent` and `content_block_delta`. A snapshot, not a maintained pointer | https://github.com/anthropics/anthropic-sdk-python/commit/f9b0cf28 ; https://storage.googleapis.com/stainless-sdk-openapi-specs/anthropic/anthropic-465bff21a179090915396565d1ae8f705cf8596e2ec920eb121072f25b8a7d68.yml |
| OpenAI OpenAPI spec | `openai/openai-openapi`, `main`, `openapi.yaml`/`openapi.json`, OpenAPI 3.1.0, MIT, ~3.7 MB, contains `ResponseStreamEvent` | https://github.com/openai/openai-openapi |
| progenitor | 0.15.0 (2026-09-10); OpenAPI 3.0.x only (via `openapiv3` 2.2); no SSE support documented | https://crates.io/api/v1/crates/progenitor ; README |
| typify | 0.8.0 (2026-09-09); JSON Schema → serde types; used in cox since T30.10 | https://crates.io/api/v1/crates/typify ; https://github.com/oxidecomputer/typify |
| OpenAPI 3.1 + SSE Rust generator | none maintained found: **not found** (README-level search only) | progenitor, openapi-generator READMEs |

Reading: no vendor ships Rust. For OpenAI a maintained typed crate exists (`async-openai`); for Anthropic only the spec exists, and only as an unlinked snapshot, so generation is from a vendored copy (T30.10 does this for the stream types). Subscription login: Anthropic forbids it in writing; OpenAI is silent.

### 4.3.2 LM Studio's native API as a cox provider (checked 2026-09-25)

LM Studio serves three API families on one port (default 1234). Facts are
from LM Studio's docs and from the running server (LM Studio CLI commit
`07b7252`, MLX runtime `mlx-llm-mac-arm64-apple-metal-advsimd` 1.11.0,
model `prism-ml/bonsai-27b`).

| Fact | Source |
|---|---|
| Native REST API v1 at `/api/v1/*` is an "official release" in LM Studio 0.4.0; the legacy `/api/v0/*` stays | https://lmstudio.ai/docs/developer/api-changelog |
| Anthropic-compatible `POST /v1/messages` arrived in 0.4.1; OpenAI-compatible `/v1/chat/completions` tool calling since 0.3.6 | https://lmstudio.ai/docs/developer/api-changelog |
| v1 endpoints: `GET /api/v1/models`, `POST /api/v1/models/load` (`context_length`, `eval_batch_size`, `flash_attention`, `num_experts`, `offload_kv_cache_to_gpu`), `POST /api/v1/models/unload`, `POST /api/v1/models/download`, `POST /api/v1/chat` | https://lmstudio.ai/docs/developer/rest, https://lmstudio.ai/docs/developer/rest/load |
| `GET /api/v1/models` returns per model `max_context_length`, `loaded_instances[].config.context_length`, `capabilities.trained_for_tool_use`, `capabilities.reasoning.allowed_options`, `capabilities.vision`, quantization and format | live `curl localhost:1234/api/v1/models` |
| `POST /api/v1/chat` rejects a `tools` array (`"Unrecognized key(s) in object: 'tools'"`); its only tool mechanism is MCP servers passed as `integrations` | live call; https://lmstudio.ai/docs/developer/core/mcp |
| v1 chat streams named SSE events (`chat.start`, `prompt_processing.*`, `reasoning.start/delta/end`, message deltas, `chat.end` with the full result) and reports `stats` (`input_tokens`, `total_output_tokens`, `reasoning_output_tokens`, `tokens_per_second`, `time_to_first_token_seconds`); `previous_response_id` chains stateful chats | live streaming call; https://lmstudio.ai/docs/developer/rest |
| One auth scheme for all three families when "Require Authentication" is on: `Authorization: Bearer <token>` (Anthropic path also `x-api-key`) | https://lmstudio.ai/docs/developer/core/authentication |
| No Rust SDK: crates.io has no `lmstudio`, `lm-studio`, `lmstudio-rs`, `lmstudio_rs` (`lms` is an unrelated rsync tool). Official SDKs are TypeScript and Python only | https://crates.io/api/v1/crates/lmstudio (404), https://github.com/lmstudio-ai/lmstudio-js, https://github.com/lmstudio-ai/lmstudio-python |
| No published OpenAPI or JSON Schema for the native API found; lmstudio-js keeps TS/zod types in `packages/lms-shared-types/src` | https://github.com/lmstudio-ai/lmstudio-js (tree checked; deeper listing **unverified**) |
| The SDKs talk to LM Studio over a WebSocket RPC protocol | **unverified** (secondary summaries only; no LM Studio protocol doc) |
| cox's Anthropic Messages path works against LM Studio as is: one-tool task finished, `cost_usd` 0 | live run, T30.14 |
| `POST /api/v1/models/load` answers `type`, `instance_id`, `load_time_seconds`, `status: "loaded"` and, only when the request sets `echo_load_config: true`, `load_config` (`context_length`, …); cox did not call it against the live server (T30.16 tests it on wiremock) | https://lmstudio.ai/docs/developer/rest/load (checked 2026-09-26) |
| `GET /api/v1/models` shape re-checked for T30.16: `prism-ml/bonsai-27b` loaded at `context_length` 251648 of `max_context_length` 262144, `trained_for_tool_use: true`; saved as `fixtures/lmstudio/models.json` | live `curl localhost:1234/api/v1/models`, 2026-09-26 |
| `lms load prism-ml/bonsai-27b --context-length 65536 -y` left the model loaded with `context_length` 251648 and `parallel` 4 (`lms ps`, `/api/v1/models`): the loaded context must be read back, not assumed | live, 2026-09-25 |

What follows for cox. Wire types fall to step 3 of D3/A40 (no Rust SDK, no
spec), and the native surface cox needs is small: `models` and
`models/load`. The chat loop cannot move to `/api/v1/chat`, because it takes
no custom tool schemas, so it stays on `/v1/messages` through the existing
Anthropic provider. OpenAI Chat is not an option either while `chat.rs` drops
tool calls (ideas.md). The native API earns its place for what the
compatibility endpoints lack: the loaded context length (the context window
cox needs for compaction, today a hand-set `context_window`), tool-use and
reasoning capabilities, load state, and loading a model with an explicit
context length before a session starts.

### 4.3.3 Providers, models, prices and effort as they stand (T30.17, repo at db1f313, checked 2026-09-25)
The source for every row is the repository itself, at the commit named in the heading.

| Concern | Where | What it does today | Divergence |
|---|---|---|---|
| Provider sections | `cox-protocol/src/config.rs:280-480` | Named sections `anthropic`, `openai`, `local` and `typesafe` (Jev), plus flattened `[providers.<name>]` `CompatibleProviderConfig`. `models_for` (310-325) matches the four names by hand, then falls through to `custom`. | Knobs differ by section. `timeout_s`/`max_retries` exist only on Anthropic and Jev. `cache_ttl` exists only on Anthropic. `context_window` exists only on Local and Compatible. `api` exists on OpenAI, Local and Compatible. A pinned `model` exists on Local, Jev and Compatible. |
| Construction | `cox/src/session.rs:833-934` | `backend_for` has one match arm per family. `openai_shaped` (909) is shared by the OpenAI-wire arms only. | Local takes the whole config struct (861). Anthropic and Jev have bespoke constructors. |
| Keys | `anthropic/mod.rs:170-171`; `session.rs:855,875,896` | Anthropic always uses `ANTHROPIC_API_KEY` or keyring `cox/anthropic`. Jev resolves its section's `api_key_env`, then the keyring. OpenAI and Compatible read `std::env::var(api_key_env)` only. | `providers.anthropic.api_key_env` is never read. OpenAI and Compatible have no keyring fallback (A9 deferred it). |
| Retry, timeout | `AnthropicProvider::new` (`anthropic/mod.rs:86`), Jev | Anthropic and Jev take a `retry::Policy` from config. Chat and Responses wrap `stream_with_retry` with `Policy::default()`. | Retry is configurable for two of five families. |
| Context window | `anthropic/mod.rs:198` (200 000), `jev.rs:381` (128 000), `session.rs:857` (400 000 OpenAI default), `responses.rs:531` (from config) | `Caps.max_context` is a literal per provider unless Responses finds a configured model. | A second source of truth next to `ProviderModel.context_window`. A larger configured window on Anthropic is ignored. |
| Thinking capability | `anthropic/request.rs:40,403` `ADAPTIVE_THINKING_PREFIXES` | A model-name prefix list decides `thinking: adaptive`. | A third model table, disjoint from `ProviderModel` and `prices.toml`. |
| Effort | `types.rs:111` `Effort {Low, High, Xhigh}`; `router.rs:164` `clamp_effort`; `request.rs:140,394`; `responses.rs:96,287`; `config.rs:263` | The router clamps the level to `ProviderModel.efforts`. Anthropic sends `output_config.effort` plus adaptive thinking. Responses sends `reasoning.effort`. | Chat, Local and Compatible send no effort. Jev accepts the level but does not map it. The models.dev `medium` collapses into `High`. Each wire has its own `effort()` fn. |
| Prices | `cox-provider/src/usage.rs:36-233` | `PriceTable` comes from `prices.toml` and is keyed by the bare model id. `Priced` wraps any `Provider` and prices each `Usage`. | This path is already unified. A model in `providers.*.models` can have no price row: it is costed at 0 with `estimated=true` and warned about once. Only a unit test (`usage.rs:255-291`) checks catalog/price sync. |
| Usage | `types.rs` `Usage`; `anthropic/stream.rs:255,316`; `chat.rs:406`; `responses.rs:454` | There is one struct. | Only Anthropic fills `cache_write_tokens`. The Chat and Responses APIs do not bill cache writes, so 0 is correct there. |
| Model id and routing | `types.rs:979` `ModelId(String)`; `cli.rs:28-36`; `config_load.rs:113-117,186-196`; `router.rs:88-157` | `Router::pick` resolves tier → provider → model → effort in one place. | `--provider`/`--model` retarget only the `code` tier. Nothing parses a `vendor/model` id, although `ProviderModel.id` documents the form for gateways. |
| Chat `reasoning_effort` — OpenAI (T30.26, checked 2026-09-26) | OpenAI API reference, Create chat completion: https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create | Documents `reasoning_effort` as an optional top-level field, one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, and notes that not every reasoning model supports every value. | Accepted, but model-dependent: cox sends it only for a `models` entry that declares `reasoning_effort = true`. |
| Chat `reasoning_effort` — LM Studio (T30.26, checked 2026-09-26) | LM Studio docs, OpenAI Compatibility → Chat Completions, "Supported payload parameters": https://lmstudio.ai/docs/developer/openai-compat/chat-completions | Lists `model`, `top_p`, `top_k`, `messages`, `temperature`, `max_tokens`, `stream`, `stop`, `presence_penalty`, `frequency_penalty`, `logit_bias`, `repeat_penalty`, `seed`; no `reasoning_effort`. Its Responses page (https://lmstudio.ai/docs/developer/openai-compat/responses) shows `"reasoning": {"effort": "low"}` in an example. | Not documented on Chat, so cox's Chat wire does not send it by default; the per-model opt-in stays off for LM Studio rows unless a user sets it. |

**models.dev registry (T30.20, checked 2026-09-25).** `GET https://models.dev/api.json` returns JSON with no key and no pagination: 223 providers. The default urllib User-Agent gets HTTP 403, so the script sends its own. Shape: `{provider_id: {models: {model_id: {limit: {context, output}, cost: {input, output, cache_read, cache_write} (USD/MTok, absent = 0), reasoning_options: [{type: "effort", values: [...]} | {type: "toggle"} | …]}}}}`. Two cox section names differ from models.dev ids: `moonshot` → `moonshotai` and `z-ai` → `zai`. `cox-vendor models` (`scripts/vendor/src/cox_vendor/models.py`) regenerates `prices.toml` rows and `default.toml` `models` arrays from it.

Conclusions for the design (`docs/design/providers.md` § Target shape):

1. Two pieces are already unified and stay as they are: the router's resolution and the `Priced` cost path.
2. The splits are at construction time: which knobs a section has, how its key resolves, and where the model facts live.
3. One descriptor per section and one model catalog remove the literals and the prefix table. `Caps` then comes from the catalog.

### 4.3.4 Crate split: measurements (T30.18, repo at db1f313, checked 2026-09-25)
The source is the repository at the commit named in the heading. LOC counts come from `wc -l` over each crate's `*.rs` files, tests included. The internal graph comes from `use crate::…` lines. The workspace lists `members = ["crates/*"]` (`Cargo.toml:3`), so a new crate directory is picked up without editing the manifest.

| Crate | LOC | Largest modules | Heavy or platform deps |
|---|---|---|---|
| `cox` | 8 066 | `session.rs` 1311, `config_load.rs` 804, `plain.rs` 651, `doctor.rs` 650, `stats.rs` 597, `telemetry.rs` 287 | clap, figment, toml_edit; opentelemetry ×5 (feature `otel`, on by default, `crates/cox/Cargo.toml:10-16`) |
| `cox-protocol` | 3 331 | `types.rs`, `config.rs`, `traits.rs` | serde, schemars |
| `cox-core` | 12 451 | `session.rs` 1741, `init.rs` 667, `subagent.rs` 651, `turn.rs` 624, `context.rs` 602, `permission/*` 448, `rollout.rs` 446, `compact.rs` 435, `router.rs` 352 | globset |
| `cox-provider` | 6 941 (src) | `openai/chat.rs` 1042, `openai/responses.rs` 986, `anthropic/*` 2070, `jev.rs` 554, `usage.rs` 473, `scripted.rs` 451, `tokens.rs` 360, `replay.rs` 302, `retry.rs` 248, `http.rs` 159, `sse.rs` 88 | reqwest, tiktoken-rs, async-openai, typify (build) |
| `cox-tools` | 9 201 | `bash/*` 893, `v4a/*` 987, `sandbox/*` 714, `memory.rs` 610, `git.rs` 600, `grep.rs` 507, `checkpoint.rs` 416, `glob.rs` 366, `web_fetch.rs` 365, `path.rs` 209, `outline.rs` 195 | tree-sitter + 5 grammars (`outline.rs`, `bash/classify.rs:8,84`), ignore / grep-searcher / grep-regex (`grep.rs:15-17`), nucleo (`glob.rs:16`), reqwest (`web_fetch` only, `Cargo.toml:31-32`), nix, landlock and seccompiler (Linux) |
| `cox-mcp` | 1 717 | client, server | rmcp, reqwest |
| `cox-store` | 1 930 | | diesel, libsqlite3-sys |
| `cox-ext` | 2 816 | instructions, skills, commands, subagents, hooks | serde_yaml |
| `cox-tui` | 14 508 | `state.rs` 2548, `theme.rs` 724, `term.rs` 589, `diff.rs` 569, `markdown.rs` 538, `keymap.rs` 514 | ratatui, crossterm, syntect + two-face, pulldown-cmark, terminal-colorsaurus |
| `cox-acp` | 1 390 | one adapter | agent-client-protocol |

Findings:

- **The core's cycle.** `cox-core`: `session.rs` ↔ `turn.rs` import each other, and `init`, `subagent`, `compact`, `memory_extract` and `rewind` hang off `session`. This mass cannot be split without redesigning the loop.
  - Leaves with no `use crate::`: `permission/*`, `router.rs`, `rollout.rs`, `budget.rs` (55), `cache_diag.rs` (172), `dedup.rs` (209), `redact.rs` (261), `truncate.rs` (119).
- **The TUI.** `cox-tui`: `state.rs` is the TEA hub, with 15 internal imports.
  - Leaves with no `use crate::`: `theme`, `color`, `svg`, `term`, `text`, `vim`, `link`, `tasks`.
  - `markdown` and `diff` import two crate modules each.
- **A guard reached from outside the TUI.** The headless surface imports the TUI crate for one function: `crates/cox/src/plain.rs:21` `use cox_tui::text::sanitize`. `cox-tools/src/git.rs:8` routes git output through the same guard.
  - The two other `sanitize` names are unrelated helpers, not copies of the guard: `cox-ext/src/memory.rs:193` sanitizes a file name, and `cox-store/src/fts.rs:191` sanitizes an FTS query.
- **Syntax parsing.** `cox-tools`: tree-sitter serves both `outline.rs` and the bash classifier (`bash/classify.rs:84`). A syntax crate must take both, or the grammars stay in `cox-tools`.
- **The patch engine.** `v4a/apply.rs` does file I/O, so the patch engine is an adapter, not a pure crate.
- **The dependency test.** `crates/cox/tests/deps.rs` (152 lines) enforces the DAG through `cargo metadata`:
  - `cox-protocol` depends on nothing;
  - `cox-core` depends only on `cox-protocol`;
  - `cox-tui` and `cox-acp` depend only on `cox-core` and `cox-protocol`;
  - the adapters never depend on `cox-core`;
  - only `cox-store` may depend on diesel.

  Any new crate needs a rule there. D1 (`plan.md:25`) fixes "ten in-tree crates".
- **Shared packages.** `packages/`: only `packages/crates/file-backup` exists, and cox has no matching code to replace with it.
- **Build time (T32.2, C2 `cox-render`, measured 2026-09-26).** Setup: macOS arm64, 16 cores, `dev` profile, Rust 1.97.1, worktree on `main` at `855fe68`, default `-j`. Command: `cargo build -p cox-tui --timings`, run 5 times per row. "Clean" means `cargo clean -p cox-tui [-p cox-render]` first, with every other dependency already built. "Incremental" means `touch crates/cox-tui/src/state.rs` first. "Before" and "after" ran back to back on the same tree (the move stashed, then restored), at load average 12–15. Unit times come from the `--timings` report; figures are medians.

  | Build | Before: `cox-tui` unit | After: `cox-tui` unit | After: `cox-render` unit | Wall before → after |
  | --- | --- | --- | --- | --- |
  | clean | 1.25 s | 0.98 s | 0.50 s | 2.19 s → 2.20 s |
  | incremental, `state.rs` touched | 0.42 s (0.40–0.43) | 0.39 s (0.37–0.39) | 0 (not rebuilt) | 1.40 s → 1.34 s |

  **Result.** The split gains about 0.03 s of `cox-tui` compile time per `state.rs` edit (about −7 % of the unit, about −4 % of wall time). The ranges do not overlap, so the gain is real, but it is negligible. Explanation: dependency rule (a) moves heavy *dependencies*, and cargo never rebuilds those on an edit either way. Incremental compilation already skipped most of the moved 2.6k lines. The clean total grows by about 0.2 CPU-s (1.25 s → 0.98 s + 0.50 s), with no wall-time change because the two units overlap. So rule (a) buys almost nothing for edit-compile time. What remains is the dependency guard (`deps.rs` `only_render_depends_on_the_highlighters`) and letting a surface render without the TUI. A first unloaded run on the same day (load 28–36) gave the same picture: 0.41–0.47 s before, 0.35–0.39 s after.

### 4.3.5 WASM plugin host: extism and the precedents (A52, `docs/design/plugins.md`, checked 2026-09-26)

Primary sources only. Crate facts come from the crates.io API (`https://crates.io/api/v1/crates/<name>` and `/<version>/dependencies`) and from the published crate sources (`https://static.crates.io/crates/<name>/<name>-<version>.crate`), read at the version named. Lines cited as `src/…:N` are in that crate's published source.

| # | Fact | Source (primary) | Checked |
|---|---|---|---|
| P1 | `extism` latest stable is 1.30.0, published 2026-06-04, BSD-3-Clause. `extism-manifest` and `extism-convert` are 1.30.0, same day. | https://crates.io/api/v1/crates/extism, `/extism-manifest`, `/extism-convert` | 2026-09-26 |
| P2 | `extism-pdk` (guest SDK) latest stable is 1.4.1, published 2025-05-19. It depends on `extism-convert ^1.10`, `extism-pdk-derive ^1.4.1`, `serde`, `serde_json`, `base64`, `anyhow`. | https://crates.io/api/v1/crates/extism-pdk, `/extism-pdk/1.4.1/dependencies` | 2026-09-26 |
| P3 | `extism` 1.30.0 depends on `wasmtime ^43`, `wasi-common ^43` and `wiggle ^43`. The wasmtime features it enables are `cache`, `gc`, `gc-drc`, `cranelift`, `coredump`, `wat`, `parallel-compilation`, `pooling-allocator`, `demangle`. The latest `wasmtime` is 49.0.1 (2026-09-24), so cox would get the wasmtime extism pins, six majors behind. | https://crates.io/api/v1/crates/extism/1.30.0/dependencies; https://crates.io/api/v1/crates/wasmtime | 2026-09-26 |
| P4 | Other normal dependencies of `extism` 1.30.0: `anyhow 1`, `tracing 0.1`, `tracing-subscriber ^0.3.23` (`std`, `env-filter`, `fmt`), `toml ^0.9`, `serde_json`, `sha2 ^0.10`, `glob`, `url`, `uuid` (`v4`), `libc`, `async-trait`; `ureq ^3.0` is optional. `cbindgen` is a build dependency. | same as P3 | 2026-09-26 |
| P5 | Default features are `http`, `register-http`, `register-filesystem` and `wasmtime-default-features`. `http` and `register-http` pull `ureq`. With `default-features = false`, the plugin HTTP host function refuses every request (`src/pdk.rs:180-192`), URL-sourced modules are refused (`src/manifest.rs:81-84`), and file-sourced modules are refused (`src/manifest.rs:42-44`). Bytes passed in by the host still load. | https://crates.io/api/v1/crates/extism/1.30.0 (`features`); `extism-1.30.0/src/pdk.rs`, `src/manifest.rs` | 2026-09-26 |
| P6 | `extism::Error` is "a wrapper around a dynamic error type" (anyhow). A host crate therefore has to map it into its own `thiserror` enum at the boundary. | https://docs.rs/extism/1.30.0/extism/ | 2026-09-26 |
| P7 | `PluginBuilder` methods: `with_wasi(bool)`, `with_function(name, args, returns, UserData<T>, f)`, `with_function_in_namespace`, `with_functions`, `with_fuel_limit(u64)`, `with_cache_config(dir)`, `with_cache_disabled()`, `with_wasmtime_config(Config)`, `with_debug_options`, `build() -> Result<Plugin, Error>`, `compile() -> Result<CompiledPlugin, Error>`. `Plugin::new_from_compiled(&CompiledPlugin)` builds an instance from a compiled module. | https://docs.rs/extism/1.30.0/extism/struct.PluginBuilder.html, `/struct.Plugin.html` | 2026-09-26 |
| P8 | `Plugin` is `Send` and `Sync` (`unsafe impl`, `src/plugin.rs:182-183`). `Plugin::call` takes `&'b mut self` (`src/plugin.rs:1124`), so one instance runs one call at a time. Other methods: `function_exists`, `cancel_handle`, `fuel_consumed`, `reset`, `call_with_host_context`. | https://docs.rs/extism/1.30.0/extism/struct.Plugin.html; `extism-1.30.0/src/plugin.rs` | 2026-09-26 |
| P9 | `CancelHandle` is `Clone + Send + Sync`, and `cancel()` stops a running call from another thread. | https://docs.rs/extism/1.30.0/extism/struct.CancelHandle.html | 2026-09-26 |
| P10 | `Pool::get(timeout)` returns `Ok(None)` when no instance frees up in time. The maximum instance count is set through `PoolBuilder`. | https://docs.rs/extism/1.30.0/extism/struct.Pool.html | 2026-09-26 |
| P11 | `Manifest` fields: `wasm`, `memory: MemoryOptions`, `config: BTreeMap<String,String>` (read by the guest's `config::get`), `allowed_hosts: Option<Vec<String>>` (empty = no host; wildcards allowed), `allowed_paths: Option<BTreeMap<String, PathBuf>>` (WASI preopens; a `ro:` key prefix mounts read-only, `src/current_plugin.rs:354`), `timeout_ms: Option<u64>`. | https://docs.rs/extism-manifest/1.30.0/extism_manifest/struct.Manifest.html; `extism-1.30.0/src/current_plugin.rs` | 2026-09-26 |
| P12 | `MemoryOptions` fields: `max_pages: Option<u32>` (64 KiB WASM pages), `max_http_response_bytes: Option<u64>`, `max_var_bytes: Option<u64>` ("default value is 1mb"; `0` disables vars). | https://docs.rs/extism-manifest/1.30.0/extism_manifest/struct.MemoryOptions.html | 2026-09-26 |
| P13 | `timeout_ms` works through wasmtime epoch interruption driven by a timer thread (`src/plugin.rs:56`, `src/timer.rs`). A timed-out call returns `Error("timeout")` (`src/plugin.rs:1081`). | `extism-1.30.0/src/plugin.rs`, `src/timer.rs` | 2026-09-26 |
| P14 | A `Wasm` source may carry a `hash`. extism checks it as SHA-256 and refuses a mismatch (`src/manifest.rs:18-30`). | `extism-1.30.0/src/manifest.rs` | 2026-09-26 |
| P15 | A module given as bytes may be WAT text as well as a binary: the loader accepts input that starts with `(module` (`src/manifest.rs:125-135`, wasmtime `wat` feature from P3). This lets host tests use inline WAT with no build step. | `extism-1.30.0/src/manifest.rs` | 2026-09-26 |
| P16 | `with_wasi(true)` builds a `wasi-common` preview-1 `WasiCtx` whose only directories are the `allowed_paths` preopens (`src/current_plugin.rs:345-365`). | `extism-1.30.0/src/current_plugin.rs` | 2026-09-26 |
| P17 | The Rust PDK README uses the `wasm32-unknown-unknown` target with `crate-type = ["cdylib"]`, and names `wasm32-wasip1` as the alternative when WASI is needed (README lines 31-78). Host functions are imported with `#[host_fn] extern "ExtismHost" { … }` (README line 322). Guest helpers: `input`, `output`, `config::get`, `var::{get,set,remove}`. | `extism-pdk-1.4.1/README.md`, `src/lib.rs`, `src/config.rs`, `src/var.rs` | 2026-09-26 |
| P18 | Zellij's `ZellijPlugin` has `load(&mut self, BTreeMap<String,String>)`, `update(&mut self, Event) -> bool` (returning `true` asks for a `render`), `pipe(&mut self, PipeMessage) -> bool` and `render(&mut self, rows, cols)`, which is called only after an update asks for it or on resize. | `zellij-tile-0.45.1/src/lib.rs:33-48` (crates.io 0.45.1, 2026-08-28) | 2026-09-26 |
| P19 | Zellij plugins `subscribe` to `EventType`s and call `request_permission` over 14 permission types (`ReadApplicationState`, `RunCommands`, `FullHdAccess`, `InterceptInput`, …). The answer comes back as the `PermissionRequestResult` event. | https://zellij.dev/documentation/plugin-api-events.html, https://zellij.dev/documentation/plugin-api-permissions.html | 2026-09-26 |
| P20 | Zed extensions: an `extension.toml` manifest, Rust compiled to `wasm32-wasip2` against `zed_extension_api` (latest 0.7.0, 2025-09-12). Capabilities are declared in `extension.toml` (`process:exec`, `download_file`, `npm:install`, with wildcard patterns). Users narrow them with the `granted_extension_capabilities` setting. | https://zed.dev/docs/extensions/developing-extensions, https://zed.dev/docs/extensions/capabilities, https://crates.io/api/v1/crates/zed_extension_api | 2026-09-26 |
| P21 | Claude Code plugins are directory bundles with `.claude-plugin/plugin.json` plus `skills/`, `commands/`, `agents/`, `hooks/hooks.json`, `monitors/`, `output-styles/`, `themes/`, `bin/`, `.mcp.json` and `.lsp.json`. No component is loaded into the process. | https://code.claude.com/docs/en/plugins-reference ("Standard layout") | 2026-09-26 |
| P22 | mise's `rust` tool takes a `targets` option (array or comma list) and adds missing targets even when the toolchain is already installed. | https://mise.jdx.dev/lang/rust.html | 2026-09-26 |
| P23 | `dtolnay/rust-toolchain` takes a `targets` input ("Comma-separated string of additional targets to install e.g. wasm32-unknown-unknown"). cox CI uses `dtolnay/rust-toolchain@1.97.1` (`.github/workflows/ci.yml:23`). | https://github.com/dtolnay/rust-toolchain; repo file | 2026-09-26 |
| P24 | The repo pins `rust = "1.97.1"` with no `targets` (`mise.toml`). On the author's machine, 1.97.1 has `wasm32-unknown-unknown` installed and not `wasm32-wasip1` (`rustup target list --installed`). This is machine state, not a repo guarantee. | `mise.toml`; local `rustup` | 2026-09-26 |
| P25 | Measured under load (2026-09-26; macOS arm64, 16 cores, load average 36–47 from other agents; `release` profile; clean `target/`). The cox release binary grows from 53 713 168 B (51.2 MiB) to 71 277 728 B (68.0 MiB): **+17 564 560 B (+16.8 MiB)**, with `crates/cox` linking `cox-plugin` (extism 1.30.0 with default features off; wasmtime 43.0.2 with extism's features plus `anyhow`, P38). Nothing in `cox` depends on `cox-plugin` yet, so the "after" build used a temporary call to `PluginHost::load` in `main`, reverted after the measurement. Clean `cargo build --release -p cox --timings`: wall time 159.0 s before and 138.8 s after, so load noise is larger than the difference. Summed unit time went from 1 015 s to 1 478 s (+463 CPU-s, 581 → 770 units). The largest new units are cranelift-codegen (74 s), wasmtime (42 s), zstd-sys (30 s), wasmtime-environ (26 s), wasmparser (24 s), wast (24 s), wasmtime-internal-cranelift (24 s) and extism (22.5 s). Binary size is the deterministic number; wall-clock build growth cannot be measured on this machine under this load. Result: PL§12 falsifier 1 fired (budget 10 MiB). The creator's decision (2026-09-26): a default-on cargo feature `plugins` on `crates/cox` gates the optional `cox-plugin` dependency, so `--no-default-features --features otel` is the slim build without extism or wasmtime (`deps.rs` `slim_build_has_no_wasm_runtime`), and the binary budget becomes 20 MiB (PL§11, PL§12). | `scripts/footprint.sh` before and after; `cargo build --timings` HTML reports; T33.3 | 2026-09-26 |
| P26 | `github.com/extism/go-pdk` is official. Its latest release is v1.1.3 (2025-03-18) and the repo was last pushed 2026-01-22. The README recommends TinyGo (`tinygo build -o plugin.wasm -target wasip1 -buildmode=c-shared main.go`), says TinyGo ≥ 0.34 supports reactor modules natively, and also documents standard Go (`GOOS="wasip1" GOARCH="wasm" go build -buildmode=c-shared`). Exports use `//go:wasmexport`. | https://github.com/extism/go-pdk; https://api.github.com/repos/extism/go-pdk/releases | 2026-09-26 |
| P27 | Go 1.24 added the `go:wasmexport` directive, and `-buildmode=c-shared` builds a reactor/library on `GOOS=wasip1`. The latest Go is 1.27.1. | https://go.dev/doc/go1.24 (WebAssembly section); https://go.dev/dl/?mode=json | 2026-09-26 |
| P28 | The latest TinyGo is v0.42.0 (2026-09-01). | https://api.github.com/repos/tinygo-org/tinygo/releases/latest | 2026-09-26 |
| P29 | The extism GitHub organisation has PDKs for Rust, Go, C, C++, AssemblyScript, Haskell, .NET, Zig, JS, Python and MoonBit, and none for Kotlin or Dart. | https://api.github.com/orgs/extism/repos (repos with `pdk` in the name) | 2026-09-26 |
| P30 | The only Kotlin PDK found is the community repo `LizAinslie/extism-kotlin-pdk`, last pushed 2023-11-30 (5 stars): dead by the workspace rule. No Dart PDK was found; the only Dart extism repos are *host* SDKs (`AmiK2001/extism-dart-sdk`). | https://api.github.com/search/repositories?q=extism+kotlin, `?q=extism+dart`, `?q=extism+pdk+dart` | 2026-09-26 |
| P31 | Kotlin/Wasm is Beta. The `wasmWasi` target "supports WASI 0.1, also known as Preview 1", with 0.2 planned, and names Node.js, Wasmtime, Deno and WasmEdge as runtimes. Browsers need "garbage collection and legacy exception handling". The page does not say which exception-handling encoding `wasmWasi` emits. | https://kotlinlang.org/docs/wasm-overview.html, https://kotlinlang.org/docs/wasm-wasi.html | 2026-09-26 |
| P32 | `kotlin.wasm.WasmExport` and `WasmImport` exist since Kotlin 1.8 and are experimental (opt-in `ExperimentalWasmInterop`). The latest Kotlin is v2.4.20 (2026-09-07). | https://kotlinlang.org/api/core/kotlin-stdlib/kotlin.wasm/; https://api.github.com/repos/JetBrains/kotlin/releases/latest | 2026-09-26 |
| P33 | The extism 1.30.0 engine config turns on `wasm_tail_call`, `wasm_function_references` and `wasm_gc`. It turns on `wasm_exceptions` only under its non-default `wasmtime-exceptions` feature (`src/plugin.rs:60-66`). | `extism-1.30.0/src/plugin.rs`; https://crates.io/api/v1/crates/extism/1.30.0 (`features`) | 2026-09-26 |
| P34 | The current wasmtime docs list `gc`, `function-references`, `exception-handling` and `tail-call` as Tier 1. The page names no version and does not say whether "legacy" exception handling is supported. **unverified** for wasmtime 43, the version extism pins. | https://docs.wasmtime.dev/stability-tiers.html | 2026-09-26 |
| P35 | dart2wasm output "currently targets JavaScript environments … and thus currently doesn't support execution in standard Wasm run-times like wasmtime and wasmer". It needs WasmGC and a JS bootstrap. The WASI/component-model proposal dart-lang/sdk#56366 is open. The latest Dart stable is 3.13.4 (2026-09-15). | https://dart.dev/web/wasm; https://github.com/dart-lang/sdk/issues/56366; https://storage.googleapis.com/dart-archive/channels/stable/release/latest/VERSION | 2026-09-26 |
| P36 | `dart_mcp` (the Dart team, `dart-lang/ai`) latest is 0.5.2 (2026-06-29). | https://pub.dev/api/packages/dart_mcp | 2026-09-26 |
| P37 | The mise registry has `go` (core), `tinygo` (aqua), `kotlin` (github:JetBrains/kotlin), `java` (core), `gradle` (aqua) and `dart` (http). | `mise registry` (local mise, registry as of 2026-09-26) | 2026-09-26 |
| P38 | `extism` 1.30.0 does not compile with `default-features = false` alone: 20+ `E0277` errors ("`?` couldn't convert the error: `wasmtime::Error: std::error::Error` is not satisfied", `src/plugin.rs`, `src/manifest.rs`, `src/current_plugin.rs`). It depends on wasmtime 43 with `default-features = false` and a list that omits wasmtime's `anyhow` feature (P3), which only its `wasmtime-default-features` feature brings, via the whole wasmtime default set. Declaring `wasmtime = { version = "43", default-features = false, features = ["anyhow"] }` beside it fixes the build without the rest of that set. | `extism-1.30.0/Cargo.toml` (`[dependencies.wasmtime]`, `[features]`); `wasmtime-43.0.2/Cargo.toml` (`[features] default`, `anyhow`); local `cargo build -p cox-plugin` | 2026-09-26 |
| P39 | wasmtime 43.0.2, the newest version extism 1.30.0 allows (`^43`), is affected by two advisories with no fix in the 43 line: RUSTSEC-2026-0222 / GHSA-hgjw-h833-99q9 "Stores can mix up type indices between engines" (2026-07-31; patched in >=24.0.12 <25, >=36.0.13 <37, >=46.0.2 <47, >=47.0.3) and RUSTSEC-2026-0269 / GHSA-vqjp-4c8c-hfgg "Filesystem sandbox escape when paths or symlinks contain trailing slashes" (2026-08-20; patched in >=24.0.13 <25, >=36.0.14 <37, >=46.0.3 <47, >=47.0.4). No extism release after 1.30.0 (2026-06-04) exists; extism `main` already pins wasmtime 48 (`runtime/Cargo.toml`), which neither advisory affects. `cargo deny check advisories` fails on both. 0222 is not guest-triggerable: it needs the embedder to put an object from one `Engine` into another `Engine`'s `Store` through specific APIs (`Store::debug_register_module`, `StructRefPre`/`ArrayRefPre`/`ExnRefPre::new`, `Tag::new`, breakpoint edits); single-engine embedders and embedders not calling them are not affected. extism 1.30.0 calls none of them, and it cannot share one engine: `CompiledPlugin::new` always builds its own (`src/plugin.rs:74`) and takes no `Engine`. 0269 is a WASI filesystem sandbox escape (cap-std, macOS and Linux without `openat2`). The creator's decision (2026-09-26): ignore both in `deny.toml` with a reason and a review date of 2026-12-31, to be removed when extism ships wasmtime >= 48; keep WASI off and block filesystem preopens (T33.14) until then; cox never moves wasmtime objects between plugins. | https://rustsec.org/advisories/RUSTSEC-2026-0222, https://rustsec.org/advisories/RUSTSEC-2026-0269 (local advisory-db copy); https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-hgjw-h833-99q9, `/GHSA-vqjp-4c8c-hfgg`; https://crates.io/api/v1/crates/extism; https://github.com/extism/extism/blob/main/runtime/Cargo.toml; `extism-1.30.0/src/plugin.rs` | 2026-09-26 |
| P40 | T33.35 spike toolchain, installed locally via `plugins/spikes/kotlin/mise.toml` (not the root or `plugins/mise.toml`): `kotlinc-jvm 2.4.20` (matches P32's latest release), Temurin OpenJDK `21.0.12+8-LTS`, Gradle `9.8.0`. | local machine, `mise exec -- kotlinc -version` / `java -version` / `gradle -version` | 2026-09-26 |
| P41 | The spike module (`plugins/spikes/kotlin`, throwaway, not merged): a Kotlin Multiplatform `wasmWasi` target (`binaries.executable()`) with one `@WasmExport("cox_init")` function that calls one `@WasmImport("extism:host/env", "alloc")` import and returns `Int`. `./gradlew compileProductionExecutableKotlinWasmWasi` produced a 6 165-byte `kotlin-spike.wasm`. A direct `wasmtime::Module::new` parse (bypassing extism, with `wasm_exceptions`/`wasm_gc`/`wasm_function_references`/`wasm_tail_call` all on) lists exactly one import, `extism:host/env::alloc`, and three exports: `cox_init` (`() -> i32`), `memory`, `_start`. No WASI import appears anywhere in this module — Kotlin/Wasm's `wasmWasi` runtime bootstrap does not pull in `wasi_snapshot_preview1` unless the program actually performs I/O. | local build; `wasmtime::Module::imports()`/`exports()` dump, scratch `crates/cox-plugin/examples/kotlin_spike.rs`, not committed | 2026-09-26 |
| P42 | Loading that module in extism 1.30.0 with the workspace's current, unmodified pins (`default-features = false`, no `wasmtime-exceptions`) fails to *parse*, before WASI or any host-function question arises: `failed to parse WebAssembly module: exceptions proposal not enabled`. This reproduces identically with `with_wasi(false)` (`cox_plugin::host::PluginHost::load`, the real host exactly as committed, A55) and with `with_wasi(true)` (raw `extism::PluginBuilder`) — the failure has nothing to do with WASI. Confirms P33/P34 empirically for wasmtime 43.0.2: Kotlin 2.4.20's `wasmWasi` compiler output unconditionally emits the WebAssembly exception-handling proposal, and extism enables it only under its non-default `wasmtime-exceptions` feature. | local, scratch `crates/cox-plugin/examples/kotlin_spike.rs`, not committed | 2026-09-26 |
| P43 | With `wasmtime-exceptions` turned on (a temporary, reverted edit to the workspace root `Cargo.toml`'s `extism` line, never committed): (a) the module parses and instantiates through `cox_plugin::host::PluginHost::load` with WASI **off**, exactly as the real host runs today (P41 — this module needs no WASI, so A55/T33.43 is not what blocks it); `cox_init` is found and a raw `extism::Plugin::call("cox_init", …)` succeeds. (b) The full cox `host.init()` round-trip (JSON `InitIn` → `InitOut`) fails with `plugin payload is not the expected JSON: EOF while parsing a value` — expected: this probe never calls extism's `input_*`/`output_set` host functions, so it implements none of the real PDK I/O contract a T33.36 SDK would need to. (c) Instantiating and calling succeed identically with WASI on (`extism::PluginBuilder::with_wasi(true)`, scratch only). | local, same scratch example | 2026-09-26 |
| P44 | **Dart WASI re-check spike (T33.37, falsifier for P35/PL§13).** dart-lang/sdk#56366 is still `state: open` (8 comments, last 2026-06-21). Installed the latest stable Dart SDK, 3.13.4 (macOS arm64, from the official archive, throwaway dir, not the pinned toolchain). `dart compile wasm --help` has no flag for a non-JS/WASI target. `dart compile wasm -o main.wasm main.dart` on a one-line `print` program emits `main.wasm` plus a JS bootstrap `main.mjs` (and a source map); the `.mjs` calls `WebAssembly.compileStreaming(source, {builtins: ['js-string']})` and builds a `dart2wasm` import object of JS functions, with a `jsStringPolyfill` registered under import module `"wasm:js-string"` as fallback. Loading `main.wasm` through the workspace's pinned `extism` 1.30.0 / `wasmtime` 43.0.2 (`extism::PluginBuilder::new(manifest).with_wasi(true).build()`, a `#[ignore]` test in `cox-plugin`, not committed) fails to parse: `"failed to parse WebAssembly module: exceptions proposal not enabled (at offset 0x276)"` — extism/wasmtime 43 only turn on `wasm_exceptions` behind the non-default `wasmtime-exceptions` feature (P33), and even with that feature on, the module would still need imports extism's ABI does not supply (`wasm:js-string`, the `dart2wasm` JS function table). **Falsifier not met: refuted, `dart compile wasm` still needs a JS bootstrap.** The one new lead since P35: a third-party package outside the Dart SDK, `simolus3/wasm.dart`'s `wasm_tools` (first published 2026-06-21, per the issue thread), wraps `dart2wasm` with a hand-built WASI/component-model shim; not tried here — it is not an SDK-level fix and the issue itself is still open. Re-run this spike when #56366 closes or a `dart compile wasm` release note says it targets non-JS embedders directly. | https://api.github.com/repos/dart-lang/sdk/issues/56366 (state, comments); https://storage.googleapis.com/dart-archive/channels/stable/release/latest/VERSION (3.13.4); local `dart --version`, `dart compile wasm --help`, `dart compile wasm`, and `extism::PluginBuilder::build()` against the workspace's pinned extism/wasmtime | 2026-09-26 |
| P45 | **Kotlin needs WASI after all (T33.36 draft).** Any Kotlin/Wasm `wasmWasi` module that does real work imports `wasi_snapshot_preview1.random_get`: Kotlin's stdlib reaches it through `Any.hashCode`/`Any.toString` → `identityHashCode` → `Random.Default` → `defaultPlatformRandom` → `wasiRandomGet` (traced in the compiler's `-Xwasm-generate-wat` output). A 10-line probe with no libraries (a string, `encodeToByteArray`, one throw/catch) imports it too; the P41 spike module had none only because it did nothing. `cox_plugin::host::PluginHost::load` (WASI off) refuses both modules: `unknown import: wasi_snapshot_preview1::random_get has not been defined`, matching extism 1.30.0 `src/plugin.rs:395-399`, which links WASI only when `with_wasi` is true. | local, Kotlin 2.4.20, Gradle 9.8.0, OpenJDK 27.0.0; draft on branch `wip/t33.36-kotlin`; `extism-1.30.0/src/plugin.rs` | 2026-09-26 |

**T33.35 spike result (2026-09-26).** The card's falsifier ("the module fails to instantiate under the wasmtime 43 extism pins, or needs a feature cox will not enable") fires, but not for the reason A55 predicted going in. Kotlin/Wasm `wasmWasi` 2.4.20 needs no WASI at all for a minimal `cox_init` (P41) — it fails to *parse* under extism's default engine config because its compiled output unconditionally uses the WebAssembly exception-handling proposal, which extism 1.30.0 enables only behind its non-default `wasmtime-exceptions` feature (P33, P42). With that feature on, the module instantiates and round-trips a raw `cox_init` call cleanly, with WASI off exactly as the real host runs today (P43) — so lifting A55 (T33.43) would not by itself unblock Kotlin. **Verdict: refuted as shipped.** Per the falsifier's second clause, cox does not turn on `wasmtime-exceptions` without the creator's sign-off: it is a workspace-wide engine feature that would apply to every plugin (including T33.40's Jev plugin), not only a future Kotlin one, and this spike did not evaluate its maturity or security posture on wasmtime 43. Kotlin stays out of `--lang`; T33.36 does not proceed unless the creator turns `wasmtime-exceptions` on for the workspace. **Resolved 2026-09-26:** the creator approved `wasmtime-exceptions` for the workspace (plan.md A61), so T33.36 proceeds.

### 4.3.6 Jev as the first cox plugin (T33.40, A25/A52, checked 2026-09-26)

Input for the T33.40 cards (`cards.md`). Cited in the cards as J§n and Jn. Primary sources are TypeSafe's own docs and posts, plus this repository. No request went to the Jev API and none went to a model API. Jev's behaviour is therefore not measured in this note. Anything only the vendor claims says **vendor claim**, and anything this note could not confirm says **unverified**.

Creator decisions from 2026-09-26 are applied throughout:

- **C1.** Once the plugin exists, the built-in `typesafe` section and `jev.rs` leave the core. An old config still loads, with a notice and a pointer to `cox plugin install` (D14).
- **C2.** At `route`, a plugin may only downgrade the tier.
- **C3.** In v1, plugins install only from a local folder.
- **C4.** SDK publishing waits for a stable ABI. The Jev plugin builds from `plugins/` in the repository.

#### J1. Sources

| # | Fact | Source (primary) | Checked |
|---|---|---|---|
| J1 | The endpoint is `POST https://api.typesafe.ai/v1/systemone` with `Authorization: Bearer <key>` and `Content-Type: application/json`. The request has `state`, `model` and `questions` (a map keyed by the caller's ids). The response has `model`, `answers` (the same keys) and `usage {input_tokens, output_tokens}`. | https://docs.typesafe.ai/api.md | 2026-09-26 |
| J2 | Question types: `noul` returns a P(yes) between 0 and 1, with an optional `criteria` that defines true and false. `choice` has at most 255 options and returns `choice`, `probabilities` and `confidence`. `score` takes an ordered rubric of 2 to 10 levels and returns `score`, `legend`, `probabilities` and `confidence`. Every type has `type` and `instructions`, which may be a string, an object or an array. | https://docs.typesafe.ai/api.md; https://docs.typesafe.ai/primitives/choice.md | 2026-09-26 |
| J3 | Choice `criteria` is a map from option to description. For options that are easy to confuse, the description can be an object with `what`, `not_for` and `examples`. The docs recommend an `other` or "none of the above" option when the list may not be complete. | https://docs.typesafe.ai/primitives/choice.md | 2026-09-26 |
| J4 | Errors: 401 means an invalid key, 422 an invalid body, and 429 or 529 a rate limit or overload, "use exponential backoff". | https://docs.typesafe.ai/api.md | 2026-09-26 |
| J5 | Models: `jev-1.13.0` is the current stable model. The aliases `jev-latest` and `jev-preview` both point to it. | https://docs.typesafe.ai/models.md | 2026-09-26 |
| J6 | Limits: "64k tokens per request; 32k tokens for `state` plus the longest question". Input is text only. | https://docs.typesafe.ai/models.md | 2026-09-26 |
| J7 | Price: input $0.042/MTok, output free. | https://docs.typesafe.ai/models.md; https://typesafe.ai/blog/introducing-system-one-models-and-jev | 2026-09-26 |
| J8 | Rate limits: "250,000 tokens per second / 1,200 requests per minute", and they "are adjusting dynamically". | https://docs.typesafe.ai/models.md | 2026-09-26 |
| J9 | Latency: "End-to-end response time is 70ms-500ms" (**vendor claim**; cox has not measured it, and the network path from the user is not included). The post's own date reads 2026-09-15 in the body and 2026-09-25 in the header (**unverified** which is right). | https://typesafe.ai/blog/introducing-system-one-models-and-jev | 2026-09-26 |
| J10 | Batching: 13 questions (8 Noul, 2 Choice, 3 Score) over a document of about 54 000 characters cost $0.000497 and took 0.27 s as one call. As 13 separate calls they cost $0.006090 and took 2.71 s (the sum of sequential calls). Model `jev-1.12`, 5 repeats. | https://docs.typesafe.ai/cookbooks/parallel_questions.md | 2026-09-26 |
| J11 | Confidence is one number in [0, 1] derived from the probability distribution. **Noul answers carry no confidence.** Thresholds should scale with the stakes. The examples use a floor of 0.5 or 0.6 and require more than 0.85 or 0.9 for high-stakes automatic actions. Confidence-gated routing: below the floor, fall back to "a different system". | https://docs.typesafe.ai/confidence.md; https://docs.typesafe.ai/patterns/confidence-routing.md | 2026-09-26 |
| J12 | Documented weaknesses of Jev 1.13: literal reading; poor counting, arithmetic and date comparison; "Accuracy falls as the state grows with content unrelated to the decision"; it "does not treat [adversarial content] as hostile by default" and can be steered by injected instructions; it is not built to generate text. | https://docs.typesafe.ai/model-jaggedness/jev-1.13.md | 2026-09-26 |
| J13 | Jev inside coding agents: "Jev is not a drop-in replacement" for the agent's LLM, and "There is no `model: 'jev-latest'` setting". Jev is meant as a decision call inside the agent's code (routing, scoring, verification). | https://docs.typesafe.ai/introduction/coding-agents.md | 2026-09-26 |
| J14 | Skill suggestion: one Choice over 182 skills plus Noul gates, then a rerank of the top three. Over 488 requests with Claude Haiku 4.5, wrong loads fell from 16.8 % to 7.3 % and needless loads from 9.8 % to 4.0 %. Gate and fit thresholds are 0.30. The suggestion fixed 37 requests and broke 7. | https://docs.typesafe.ai/cookbooks/skill_suggestion.md | 2026-09-26 |
| J15 | Guardrails: a Noul per hazard plus a Score for severity. Review threshold 0.35; action threshold 0.70 (strict) or 0.85 (permissive); severity block at 2.0. The recipe publishes no measured precision or recall. | https://docs.typesafe.ai/cookbooks/llm_guardrails.md | 2026-09-26 |
| J16 | `state` may be a string, a JSON object or an array. An object is recommended so that each part has a name. Keep content in `state` and judgments in the questions. | https://docs.typesafe.ai/concepts/state.md | 2026-09-26 |
| J17 | Data: TypeSafe says it does not train on user data. Zero data retention is offered to enterprise customers only, through sales. The docs index names no processing location and no SLA (**unverified** in the DPA itself). | https://docs.typesafe.ai/legal.md | 2026-09-26 |
| J18 | The cox code as of `22a1138` plus the working tree: `crates/cox-provider/src/jev.rs` (558 lines); `[providers.typesafe]` in `crates/cox-protocol/default.toml:375-381` with `context_window=128000`; `JevProviderConfig` at `config.rs:520-552`; the router arms at `router.rs:131,155`; `ProviderId::Jev` at `types.rs:997`; `provider_name` at `cox-core/src/session.rs:1457`; the session arm at `crates/cox/src/session.rs:900`; the doctor arm at `doctor.rs:215`; the catalog and price special cases at `cox-models/src/catalog.rs:184` and `price.rs:173-179`; the price row at `prices.toml:197` ($0.042 input). No code path calls Jev (plan.md P21: "no call sites yet"). | repository | 2026-09-26 |
| J19 | Prices in cox's catalog: `claude-sonnet-5` costs $2 input, $10 output, $2.50 cache write and $0.20 cache read. `claude-haiku-4-5` costs $1, $5, $1.25 and $0.10. | `crates/cox-provider/prices.toml:26-50` (source platform.claude.com/docs/pricing, verified_on 2026-09-02) | 2026-09-26 |
| J20 | A model switch today rewrites history: `Session` runs `inner.history = strip_thinking(&inner.history)` (`cox-core/src/session.rs:672`). With `sandbox` confining it, `Exec` is auto-allowed (`permission/mod.rs:187`), and so is `Write` under `Auto` (`:186`). | repository | 2026-09-26 |

#### J2. What Jev is, as cox uses it

Jev is a "System One" decision model. A request carries one `state` (a string or a JSON object) and a map of typed questions. Choice picks from ≤255 described options. Score rates on an ordered rubric of 2–10 levels. Noul gives a yes-probability. The response answers every question in the same call, with a probability for each option or level, and a `confidence` for Choice and Score. Jev generates no text (J1–J3, J12, J13). Questions in one request are independent, so all the questions cox has at one moment belong in one call (J10).

For cox this means three things:

1. **Jev is an `Advisor`, not a tier.** J13 says so outright, and it matches the code: a tier routed to `typesafe` today would get a JSON decision where it expects a title, a summary or a turn. The provider form exists only as the transport that carries the ledger row, the budget gate and the key. No tier should name it.
2. **State must be small and focused.** Accuracy falls with irrelevant state (J12), and the hard cap is 32k tokens of state plus the longest question (J6). Each decision point builds its own state object of about 0.3–2k tokens. The transcript is never sent.
3. **Jev can be steered by injection (J12).** The state includes model- and tool-written text: commands, file names, tool output. Only a monotone rule makes an answer safe to use. `risk` may only raise, and `route` may only lower, which spends less and never loosens a permission. Jev can never be the reason something becomes *allowed*.

**Where the current code disagrees with the docs** (fix it in the plugin, not in `jev.rs`, which is going away):

- **Context window.** `context_window = 128000` (`default.toml:381`) contradicts the 64k-per-request limit (J6). The plugin's `[[models]]` row uses 64 000, and the state builders cap state at 32k tokens minus the question.
- **Noul confidence.** `parse_response` sets a Noul's confidence to `noul` itself (`jev.rs:162`). The API returns no Noul confidence (J11), and P(yes) = 0.05 is a *confident no*, not a low-confidence answer. The plugin uses `|2p − 1|` as the Noul certainty and puts its thresholds on `p` directly.
- **The request body.** `build_body` collapses a whole `Request` into `state` with one fixed Noul (`jev.rs:48-86`). The plugin builds the questions for each point itself. The lossy mapping stays only as the fallback for a tier that names the provider.
- **The model id.** Evals pin `jev-1.13.0`, so a result names the model that produced it. Users keep `jev-latest` (J5).

#### J3. Limits that shape the design

| Limit | Value | Consequence |
|---|---|---|
| Latency | 70–500 ms end to end (J9, vendor claim) | This is above the design's default `risk` budget of 200 ms at the upper end. Ask only when the answer could change the outcome (J6.1), batch all calls of one tool batch into one request, and measure the late-fallback rate (E1). |
| Tokens per request | 64k, of which 32k for state plus the longest question (J6) | A per-point state builder with a hard cap. |
| Options per Choice | ≤255 (J2) | `rank` over `tool_search` candidates fits (BM25 top 20). |
| Rate | 1 200 rpm, 250k tok/s (J8) | Not binding for one user. The host's retry reuses the 429/529 backoff (J4). |
| Price | $0.042/MTok input, output free (J7) | Negligible next to a coding turn (J7 below). Latency, not money, is the cost. |
| Data | No training on user data; zero retention only for enterprise (J17) | Every point is opt-in (`[plugins.decide]`). The grant dialog and the user guide list, for each point, exactly what leaves the machine. State is built from the scrubbed event stream, the same redaction as the rollout (PL§5). |
| Early access | The wire may change (v0.2-jev falsifier 3) | This is the reason the wire belongs in a plugin: when it changes, the fix is a plugin update, not a cox release. |

#### J4. ABI gaps found (PL§12 falsifier 3 applies)

Writing the Jev plugin against the design as it stands exposes four gaps. All four are fixed before `api = 1` freezes (card T33.40.1).

1. **Deadlock or ledger bypass inside `cox_decide`.** The plugin must reach its own provider while answering a question. There are two ways today, and both are wrong:
   - `cox_model_call` inside `cox_decide` routes by *tier*, so it cannot reach the `typesafe` section. If it could, the host would dispatch to the same plugin's `cox_provider_stream`. That plugin's only worker is blocked in `cox_decide` (`call` is `&mut self`, P8), so the call deadlocks.
   - `cox_http` to the provider's `base_url` host is allowed "all but render" (PL§4), so the guest could POST to Jev directly with the host-injected key. That request gets no budget gate and no `usage` row, which breaks invariant 8.
   
   **Fix, a two-phase decide.** `cox_decide(Question) -> DecideOut::{ Advice(Option<Advice>) | Call(ModelCall) }`. The host runs the `ModelCall` against the plugin's own provider section, through budget gate → `PluginProvider` → `Priced` → one `usage` row with `job = plugin:<id>`. The worker is free by then, so there is no re-entry. The host then calls `cox_decide_resume(DecideResume { question, events }) -> Option<Advice>`. `ModelCall` gains `target: Tier(t) | OwnProvider { name, model }`, and `OwnProvider` may name only a `[[provider]]` of the same plugin. The whole exchange shares the point's latency budget.
2. **`cox_http` to a provider host outside `cox_provider_stream`.** Allow it only inside `cox_provider_stream`, and return `NotInThisContext` everywhere else. With that rule, raw HTTP to a paid endpoint never escapes the ledger.
3. **Batching.** Give `Question` `items: Vec<…>`, so that one `risk` question covers every call in a tool batch. That is one Jev request instead of N (J10: 12× cheaper and 10× faster for 13 questions).
4. **Registry and ids for plugin ABI providers.**
   - `Router::pick` and `backend_for_with` know only the fixed names plus `providers.custom` (`router.rs:125-143`). T33.18 must register ABI sections by name so a tier (or a legacy `typesafe` tier) resolves to them.
   - `ProviderId` is a closed enum (`types.rs:990`). T33.18 must choose the ledger id for plugin providers. The proposal is a `ProviderId::Plugin` bucket with the section name as the ledger's provider string, as the `Local` bucket works for compatible providers. `provider_name` (`session.rs:1457`) then returns the section name.
   - The e2e harness sets `COX_PROVIDER=scripted`, which short-circuits provider construction. It must still build plugin providers, so the main turns are scripted while Jev is served by wiremock.

One more requirement lands on T33.20. A turn routed down must strip thinking blocks **in its own `Request` only**. It must never rewrite `inner.history` or emit `ModelSwitched` (J20). Otherwise the next `code` turn loses every cache read after the first thinking block.

#### J5. Use cases, ranked

Legend:

- **Point** is the decision point from PL§4 or the hook or event it hangs on.
- **Rule** is the monotone rule the core enforces.
- Every use case fails open (D14): with no key, a network error, a timeout past the point budget, a parse error, low confidence or three strikes, the built-in behaviour runs unchanged and one `Event::Advised { applied: false }` or `Notice(Warn)` records why.

| Rank | Use case | Point | Verdict |
|---|---|---|---|
| 1 | Bash/tool risk escalation | `risk` | **Do first.** It can only make cox safer, it can be measured for pennies (E1), and it involves no D5 question. |
| 2 | Tier downgrade of a main turn | `route` | **Do second, measured.** It is the only money lever, but at default prices the saving is uncertain (J5.2). It stays off by default until E2 shows a saving at no loss in pass rate. |
| 3 | Deferred-tool ranking | `rank` (`tool_search`) | **Later.** It can be measured only with a large MCP tool set, and cox has no such eval corpus. It is a card after E1 and E2. |
| 4 | Earlier compaction at a task boundary | `compact` | **Later.** The benefit (context-token-turns, `stats.rs`) is small with cache reads at 0.1× input, and it is hard to separate from noise. It is an `ideas.md` line. |
| 5 | Memory salience filter | `salience` | **Later.** It needs T33.21's optional wiring first. The rule "may drop a candidate, never add one" is sound. |
| — | Tool-output truncation choice (which lines survive) | none (new point) | **Drop for now.** It changes model-visible text from untrusted input that Jev can be steered by (J12), and it needs a new decision point. It is an idea. |
| — | "Looks safe" approve hint | `approve_hint` | **Drop.** An injected command can make Jev say "safe", and the badge would then nudge the user to approve. The only safe shape is warning-only ("Jev: likely irreversible"), which `risk` already covers by raising the prompt. The proposal is to remove `approve_hint` from PL§4, or make it warning-only (question 3). |
| — | Skill ranking | `rank` (skills) | **Drop from v1.** The skills index sits in the cache-stable prefix (`context.rs:82-131`), so reordering it per turn breaks invariant 1. Adding a hint message is not "reorder or filter". J14's gains are real, but they need a hint channel after the last breakpoint, which is a design change. |
| — | Subagent dispatch | none | **Drop.** The model already decides this, and `explore` already runs on `cheap`. There is no fixed-option judgment for cox to own. |
| — | Stop/continue ("claimed done without running tests?") | `Stop` hook | **Drop from v1.** Acting on it means injecting text into the turn, which no monotone rule allows. It stays an idea for a hook-based verifier. |
| — | Commit message or plan triage | `Job::Commit`/`Plan` | **Drop.** Commit messages are text generation, which Jev does not do (J12). Plan triage would route *up* to `think`, which D5 and C2 forbid. At most it could be a display-only hint, which is not worth a call. |

##### J5.1 `risk`: raise the risk of a call the engine would auto-allow

- **Hangs on.** `gate` (`cox-core/src/turn.rs`), after `PreToolUse` and before `Engine::decide`. That is T33.21's `risk` point.
- **Asked only when it matters.** The core asks only if raising the call to `Destructive` would change the engine's outcome from `Allow` to `Ask` or `Deny`. That covers `Exec` under a confining sandbox, `Write` under `Auto`, and rule matches that allow by risk (J20). ReadOnly calls, calls already `Ask`, and `Bypass` mode never ask. This filter belongs in T33.21 (a plan amendment to that card). It removes most calls, so latency hits only the calls whose outcome is actually in play.
- **Question** (one request for the whole tool batch, J4.3). State is an object with the fields below. The tool output is never sent.
  - `task`: the last user message, trimmed to 1k tokens;
  - `cwd`, the workspace roots and the sandbox mode;
  - per call: `tool`, `subject`, and `input` (for bash, the command; for edits, the path plus a diff summary with line counts);
  - `classifier_risk`.
  
  Questions per call:
  - `severity`: a Score over 4 levels whose descriptions mirror `Risk` (read-only / writes inside the workspace / runs a process / destroys data or reaches beyond the subject);
  - `irreversible`: a Noul ("cannot be undone by git or the checkpoint");
  - `external_effect`: a Noul ("pushes, publishes, deploys, sends, or deletes outside the workspace");
  - `exfiltration`: a Noul ("sends workspace content or credentials to a network destination").
- **What cox does.** The risk rises to `Destructive` when any of these holds:
  - `irreversible ≥ 0.7`;
  - `external_effect ≥ 0.7`;
  - `exfiltration ≥ 0.5`;
  - `severity ≥ 2.5` with confidence ≥ `min_confidence` (default 0.6, J11).
  
  The thresholds live in the plugin's config table (`[plugins.jev.risk]`), and the core clamps the result to `max(builtin, advised)`. The engine then asks in the TUI and denies in headless mode. `Event::Advised { point: risk, applied: true }` carries the probabilities.
- **Benefit.** A command the classifier misses stops auto-running: `git push --force`, `curl … | sh` when the network is allowed, `terraform apply`, `kubectl delete`, `npm publish`, `aws s3 rm`, `DROP TABLE` through `psql -c`.
- **Measure (E1, card T33.40.7).**
  - A labelled corpus of about 300 commands in two sets: must-ask, and benign.
  - The main provider is scripted, so it costs $0. Jev is live.
  - Metrics:
    - the recall gain on must-ask commands that the baseline auto-allows;
    - the false-raise rate on benign commands;
    - the late-fallback rate at a 200 ms budget;
    - p50 and p95 latency;
    - $ from the ledger (`job = plugin:jev`).
  - Budget cap: $0.10.
- **Cost per session.** About 20–60 asked calls per 100 tool calls, batched to about 10–30 requests of 0.6–1.5k tokens each. That is ≤ 45k tokens, or **≤ $0.002**. The added wall time is 1–15 s at 70–500 ms per request (J9, vendor claim).
- **Failure mode.** On a late or missing answer the built-in classifier's risk stands. False positives cost the user an extra prompt. The falsifier: if the false-raise rate exceeds 10 %, or more than 20 % of answers arrive late at p95, `risk` is not recommended by default, and the guide says so.

##### J5.2 `route`: downgrade a main turn from `code` to `cheap` (C2)

- **Hangs on.** `Router::pick` for `Job::Main` only, once per `UserTurn`. The choice sticks for every provider call in that turn, so a tool loop never flips back and forth. It is never asked for `Job::Plan` (the user explicitly asked for `think`), for subagents, or after `/model` pinned a tier.
- **What the core offers.** Only tiers at or below the static pick (T33.20), and never `think` (D5, C2). A new **cache-aware filter** (card T33.40.8) offers `cheap` only when the predicted cost of the turn on `cheap` is at most `(1 − margin)` of its predicted cost on `code`. The prediction uses catalog prices and the last request's prefix size.

  The prices make this necessary (J19). Haiku 4.5 is only 2× cheaper than Sonnet 5, and a switch forfeits the code tier's cache reads. For one turn with prefix P, k provider calls and output O (thinking included):

  ```text
  code  ≈ 0.20·P·k            + 10·O_code
  cheap ≈ 1.25·P + 0.10·P·(k−1) + 5·O_cheap          ($/MTok)
  ```

  At P = 30k, k = 5 and O = 3k on both tiers, `cheap` costs $0.0645 and `code` $0.060, so the downgrade loses money. It wins early in a session (small P), or when the `code` turn's adaptive thinking multiplies O_code. Only a measurement settles it (E2), and the filter keeps a downgrade from making things worse.
- **Question.**
  - State:
    - `prompt`: the user message, ≤ 2k tokens;
    - `todo`: the todo list;
    - `last_turn`: tool names, errors, and whether it ended in an error;
    - `files_touched_count`.
  - Questions:
    - `tier`: a Choice over the offered tiers. The descriptions come from the plugin: `cheap` is "mechanical: read, print, list, rename, run a named command, answer from context"; `code` is "reasoning over code: multi-file edits, debugging, design". An explicit `other` → `code`;
    - `wants_depth`: a Noul, "the user asks for careful or deep work".
- **What cox does.** It applies `cheap` only when:
  - `choice == cheap`;
  - `confidence ≥ 0.8` (a high-stakes action, J11);
  - `wants_depth < 0.3`;
  - the last turn did not end in an error.
  
  Everything else uses the static pick. `Event::Advised { point: route }` records the choice and the probabilities.
- **Measure (E2, card T33.40.10).** The `evals/tasks` suite plus four tasks that need the code tier, run once with and once without the plugin, on real Anthropic with live Jev. Metrics:
  - pass rate;
  - $ per task from the ledger, grouped by (tier, job);
  - the downgrade rate;
  - Jev latency.
  
  Caps: `--budget` per run, $3 in total.
- **Cost per session.** One request of about 1.5k tokens per user turn: 20 turns is 30k tokens, or **$0.0013**, plus 70–500 ms per user turn (not per tool call).
- **Failure mode.** On silence or a late answer, the static pick runs. The worst case of a wrong downgrade is a weaker turn. The user sees `↓cheap` in the status and can `/model code`. That override stops the point for the session.

##### J5.3 Later: `rank`, `compact`, `salience`

- **`rank`.** A Choice over BM25's top 20 in `tool_search`, with an `other` option (J3), may reorder or filter the list. Measure it by hit@1 against a labelled query set over a large MCP tool list. About 3k tokens per search.
- **`compact`.** A Noul "the current task is finished" after `TurnDone`, asked when the context is ≥ 40 % of the window. It may compact earlier than the threshold, never later. Measure it by `context_token_turns` and $ per session.
- **`salience`.** A Score per extracted memory item, which may drop an item and never add one.

All three wait for the E1/E2 results. A plugin update can add them with no cox release.

#### J6. Cost per session (all points on)

| Point | Requests | Tokens | $ |
|---|---|---|---|
| risk | 10–30 | ≤ 45k | ≤ 0.0019 |
| route | 20 | 30k | 0.0013 |
| total | ≤ 50 | ≤ 75k | **≤ $0.004** |

Jev's price (J7) is under 0.1 % of a typical coding session ($0.5–5 at the J19 prices), so the budget gate never blocks it in practice. It still runs, because invariant 8 applies to every request. The real cost is wall time: every request is on the critical path of a turn or a tool call.

#### J7. What moves, what stays, migration

**Into the plugin** (`plugins/jev/`, guest crate `cox-plugin-jev`, built from the repository workspace per C4):

- **The System One wire.** Request types for the three question kinds and a response parser. The Noul-confidence fix and the 64k/32k limits come with them (J2). The status codes from J4 map to the ABI's error kinds, so the host's retry treats 429 and 529 as transient.
- **`cox_provider_stream`.** A request marked as a decision call (`Job::Plugin("jev")` plus a JSON body in its one user message) is sent verbatim. A request from a tier that names the provider gets the old lossy mapping and a `Notice(Warn)` once: "typesafe is a decision model; no tier should route to it".
- **The manifest.**

  ```toml
  [[provider]]
  name = "typesafe"
  api = "plugin"
  base_url = "https://api.typesafe.ai"
  api_key_env = "TYPESAFE_API_KEY"
  auth = "bearer"
  ```

  The name is kept on purpose. The env var, the keyring entry `cox/typesafe`, the ledger's provider string and any `tiers.*.provider = "typesafe"` keep working without edits. PL§2's example says `name = "jev"` and should change.
- **`[[models]]`.** `jev-1.13.0` and `jev-latest`, with `context_window = 64000` and price `{input = 0.042, output = 0.0}`. After removal (C1) these are the only rows, so the price comes from the plugin layer, and `cox doctor` shows `source = plugin:jev` (T33.16, T33.39).
- **`cox_decide` and `cox_decide_resume`.** The questions, state builders and thresholds for `risk` and `route`, in one reviewable module per point, as v0.2-jev's review guidance asks.
- **Capabilities.** `decide = ["risk", "route"]`, `context = true`, and nothing else: no `net` (the provider section covers the host), no `fs`, no `kv`, no tools. The call-out to its own provider is implied by `decide` plus `[[provider]]`, and the grant dialog shows it as "sends decision questions to api.typesafe.ai; costs appear as plugin:jev".

**Stays in the host.** It all stays behind the four guards, and all of it is generic:

- `resolve_key` and the keyring;
- auth-header injection;
- retry and backoff around `PluginProvider` (`retry::Policy` from the section's `max_retries`);
- the budget gate, `Priced` and the `usage` row;
- catalog layering;
- the monotone rules and `Event::Advised`;
- the cache-aware route filter;
- the "ask only when it could change the outcome" filter for `risk`;
- `Engine`.

**Removed from the core after parity (C1).** The removal cards T33.40.12–T33.40.16 take out:

- `jev.rs` and its `mod`;
- `JevProviderConfig` (it becomes a tombstone, below);
- the `typesafe` arms in `router.rs`, `session.rs` and `doctor.rs`;
- `ProviderId::Jev`, which never appears in a serialized event or rollout (it is only mapped to the string `"typesafe"`), so removing it breaks no stored data;
- the `default.toml` section;
- the price row and the catalog and price special cases;
- the vendor script's `typesafe` exception;
- T32.15 (`cox-provider-jev`), which becomes moot.

**Back-compat for existing configs (the design's open question 8):**

| Old config | During parity (T33.40.5) | After removal (T33.40.12–14) |
|---|---|---|
| `[providers.typesafe]` table | With the plugin loaded, its knobs (`base_url`, `api_key_env`, `timeout_s`, `max_retries`, `model`) configure the plugin's `typesafe` section. The user wins, as for declarative sections. Without the plugin, the built-in client runs as today. | It still parses. It becomes a **tombstone** type `LegacyTypesafe` with the same keys (`deny_unknown_fields` kept) and feeds the plugin section's knobs. Without the plugin: one `Notice(Warn)` "Jev moved to a plugin: build `plugins/jev` and run `cox plugin install <dir>`", plus a `cox doctor` row. **Why a named tombstone:** removing the field would let the table fall into the `#[serde(flatten)] custom` map as a `CompatibleProviderConfig` with the default `api = "chat"`. That would silently POST chat bodies to `api.typesafe.ai`. |
| `models = [...]` in that table | Filled into the catalog as today | Accepted by the tombstone. Its rows still go through the config catalog layer (config beats plugin). |
| `tiers.<t>.provider = "typesafe"` | Resolves to the plugin section when loaded, else to the built-in | Resolves to the plugin section when it is loaded. Otherwise **fail open** (D14): `Notice(Warn)` with the install pointer, and that tier uses its `default.toml` provider and model for the session. |
| A tier model `jev-*` under another provider | unchanged | unchanged. The router does not look at the model id. |
| `[plugins.decide] risk = "jev"` with the plugin absent | Point off, plus `Notice(Warn)` | same |
| `TYPESAFE_API_KEY` and keyring `cox/typesafe` | used by the host for the section `typesafe` | same, because the name is unchanged |
| Ledger rows with provider `"typesafe"` | unchanged | unchanged. New rows carry the same provider string and `job = plugin:jev`. |

#### J8. How tests stay offline

There is no key and no network, and the keychain is never touched (A49, A51).

- **Guest unit tests (host target).** The wire, the state builders, the question sets, the thresholds and the Noul certainty are pure Rust in `plugins/jev/src/*.rs`. extism-pdk glue sits behind `cfg(target_arch = "wasm32")`. `cargo test --manifest-path plugins/Cargo.toml -p cox-plugin-jev` runs in the `plugins` CI job. The fixtures are the documented response shapes (J1, J2, and the ones `jev.rs`'s tests already use), inline as in `jev.rs`.
- **Host e2e (`tests/plugins_jev.rs`).**
  - `crates/cox-plugin-fixtures/build.rs` builds the plugin, as T33.28 builds the example.
  - The real binary runs against a scratch `COX_HOME`. Main turns come from `COX_PROVIDER=scripted`.
  - Jev is a wiremock server on 127.0.0.1 whose `/v1/systemone` returns fixture bodies, reached through `[providers.typesafe] base_url` in the scratch config.
  - The key is an env var, `TYPESAFE_API_KEY=test-key`, so `resolve_key` never reaches the keyring. Cargo also sets `COX_KEYRING=off`.
  - The asserts cover:
    - the bearer header seen by wiremock and absent from the guest (the T33.18 pattern);
    - one `usage` row per request with `job = plugin:jev`;
    - the budget-gate block;
    - `Advised` in the rollout;
    - 401 → one notice and fail-open;
    - 529 → the retry count;
    - a delay past the budget → fallback;
    - three strikes → the export disabled.
- **Replay.** `Replay` cassettes are keyed by the `Request` hash (`replay.rs:57`). A call-out is an ordinary `Request`, so a recorded cassette can back it later. That needs a key to record (optional card T33.40.17, run only by the creator). Until then, wiremock fixtures are the offline source.
- **Evals** (`evals/`, per the eval-tooling rule). E1 and E2 are `cox_evals` modules with pytest tests that run offline: corpus loading, metric maths, budget stop and the command line. Only the live run needs `TYPESAFE_API_KEY` (and `ANTHROPIC_API_KEY` for E2), set by the creator.

### 4.3.7 Subagent messaging: Claude Code, Codex CLI, OpenCode (input for P34, checked 2026-09-26)

Input for the P34 cards (T34.0–T34.9). Primary sources are each vendor's own docs, read directly on the date below; one fact is marked secondary because the primary page did not state it. cox's own subagent inventory (`crates/cox-core/src/subagent.rs`, `tasks.rs`, `crates/cox-ext/src/agents.rs`) has a one-shot `agent` tool, two hardcoded presets, a structurally depth-1 nesting limit (a child session never gets its own `AgentTool`), and no messaging beyond the one-shot approval relay (`relay_approval`) — T34.0–T34.9 close the messaging gap narrowly, staying short of Claude Code's `SendMessage`/agent-teams roster (`ideas.md`).

| # | Fact | Source (primary) | Checked |
|---|---|---|---|
| M1 | Claude Code subagent frontmatter fields: `name`, `description`, `tools`, `disallowedTools`, `model`, `permissionMode`, `maxTurns`, `skills`, `mcpServers`, `hooks`, `memory`, `background`, `omitClaudeMd`, `effort`, `isolation`, `color`, `initialPrompt`, `experimental`. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M2 | Claude Code states multiple subagents cannot run in parallel within a single turn; the documented workaround is multiple turns or backgrounding. cox already exceeds this: `agent`'s `Concurrency::Parallel` plus the turn's own parallel tool dispatch let several `agent` calls run concurrently in one turn. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M3 | Claude Code's `SendMessage` tool, addressed by the agent's id or name, resumes a **finished** subagent with its full history intact. Built-in one-shot agents (Explore, Plan) explicitly cannot be resumed even when agent teams are on. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M4 | **Secondary, unverified**: `SendMessage` is reported gated behind `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` and disabled by default. This came from a GitHub issue, not vendor docs, so it is a lead, not a confirmed fact. | https://github.com/anthropics/claude-code/issues/35240 | 2026-09-26 |
| M5 | Claude Code's inter-agent communication is the `SendMessage` tool (progress, questions, delegation) plus a separate `SubagentHandoff` tool; with agent teams on, structured messages (`shutdown_request`, `plan_approval_response`) and a sibling roster injected as a system reminder once `SendMessage` is available and at least one agent is named. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M6 | **Unverified precise defaults**: `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` (reported default 20) and `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH` (reported default 3, vs. cox's structural depth-1). Both numbers came through an automated fetch-summary of the page rather than a direct manual read; treat as directional only pending a manual recheck. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M7 | Claude Code scans a subagent's final report for instruction-shaped/prompt-injection content and marks it, but never strips it — the same class of problem cox's D14 ("everything not written by cox is untrusted") already targets generically. | https://code.claude.com/docs/en/sub-agents | 2026-09-26 |
| M8 | Codex CLI custom agents are TOML files under `~/.codex/agents/` (personal) or `.codex/agents/` (project); required fields `name`, `description`, `developer_instructions`; optional `model`, `model_reasoning_effort`, `sandbox_mode`. | https://learn.chatgpt.com/docs/agent-configuration/subagents | 2026-09-26 |
| M9 | Codex CLI runs true parallel agents across a turn ("Codex runs parallel agents and combines their results"), capped by `agents.max_concurrent_threads_per_session`; background threads switch with the `/agent` command. | https://learn.chatgpt.com/docs/agent-configuration/subagents | 2026-09-26 |
| M10 | Codex CLI has no resumption after completion — a finished subagent moves to a "Done" list for inspection only — and messaging is one-directional: the main thread waits until all requested results are available, then gets one consolidated response. No progress streaming or two-way dialogue is documented. | https://learn.chatgpt.com/docs/agent-configuration/subagents | 2026-09-26 |
| M11 | OpenCode subagents are configured via `opencode.json`'s `"agent"` key or markdown files under `~/.config/opencode/agents/` / `.opencode/agents/` (`description`, `mode`, `model`, `permission`, `prompt`, `temperature`/`top_p`, `steps`); invoked through a Task tool or directly by `@mention` (e.g. `"@general help me search for this function"`); setting a subagent's `permission` (Task) to `"deny"` removes it from the Task tool's description entirely. | https://opencode.ai/docs/agents/ | 2026-09-26 |
| M12 | OpenCode's docs do not specify parallel execution, background execution, resumption or inter-agent messaging for subagents. **Unverified/unknown** — not inferred either way. | https://opencode.ai/docs/agents/ | 2026-09-26 |

### 4.3.8 Cursor as a cox provider (checked 2026-09-26)

Question: does Cursor expose anything cox could wire up the way T30.15 wired
LM Studio — a wire format one of the three existing `Provider` clients
already speaks, or at least a maintained SDK/spec to generate types from?
Method: unauthenticated GETs of Cursor's own docs, the published OpenAPI
file, npm/PyPI registry metadata and the GitHub API. No Cursor endpoint was
called with credentials.

| Fact | Source |
|---|---|
| Cursor publishes eight documented programmatic surfaces: Cloud Agents API (beta, all plans), Admin API (enterprise), Analytics API (enterprise), AI Code Tracking API (enterprise), Bugbot API (enterprise), TypeScript SDK, Python SDK, SDK Bridge (Connect/protobuf), plus an early-beta Origin API (repos/PRs, separate auth) | https://cursor.com/docs/api (checked 2026-09-26) |
| **Cloud Agents API is agent-shaped, not chat-shaped.** Every endpoint creates, runs, streams or manages a durable "Cloud Agent" (formerly Background Agent) that clones a repo, works in its own branch and returns a run result; there is no endpoint that takes a message list + tool schema and returns a completion the way Anthropic Messages or OpenAI Chat/Responses do | https://cursor.com/docs/cloud-agent/api/endpoints ; OpenAPI spec below |
| Endpoints (v1 unless noted): `POST/GET /v1/agents`, `GET /v1/agents/{id}`, `POST/GET /v1/agents/{id}/runs`, `GET /v1/agents/{id}/runs/{runId}`, `GET /v1/agents/{id}/runs/{runId}/stream` (SSE), `POST .../cancel`, `GET /v1/agents/{id}/usage`, `GET/POST /v1/agents/{id}/artifacts[/download]`, `POST /v1/agents/{id}/archive\|unarchive`, `DELETE /v1/agents/{id}`, `POST /v1/sub-tokens`, `GET /v1/me`, `GET /v1/models` (recommended model ids for the `model.id` field only — not a completions call), `GET /v1/repositories`, plus `/v0/private-workers*` for self-hosted worker pools | published spec, `https://cursor.com/docs-static/cloud-agents-openapi.yaml`, fetched 2026-09-26, HTTP 200, 59 113 bytes, OpenAPI 3.0.3 |
| Auth: HTTP Basic (API key as username, empty password) or HTTP Bearer, both described as equivalent, key issued from `https://cursor.com/dashboard/api` (user or service-account key); server is `https://api.cursor.com` | same OpenAPI file, `components.securitySchemes` |
| Rate limits: no blanket number published; the one documented case is `GET /v1/repositories` at "1 per user per minute, 30 per user per hour" because it lists GitHub repos through Cursor's GitHub App | same OpenAPI file, endpoint description |
| Billing: no per-request price list found for the Cloud Agents API itself; usage is reported per agent via `GET /v1/agents/{id}/usage` (token counts) and drawn from the caller's Cursor plan/credit pool, the same pool the desktop app and CLI draw from. Third-party ("Other") models are billed at "public list API price + a $0.25/M-token Cursor Token Rate"; Cursor's own models (Composer, Grok) are exempt from that surcharge | https://cursor.com/help/models-and-usage/token-rate ; https://cursor.com/docs/models-and-pricing (secondary aggregation, not fully re-verified line by line: **med**) |
| ToS: no clause found that names the Cloud Agents API, the CLI or the SDKs specifically. The general prohibitions (no reverse engineering, no training a competing model, no scraping/harvesting, no renting/sublicensing "the Service") do not obviously reach "build a documented-API client called cox"; nothing in the ToS text affirmatively grants third-party client use either. Verdict: **not settled by the primary text — flag for the creator**, unlike Anthropic's explicit written prohibition on third-party Claude.ai login (R§4.3.1) | https://cursor.com/terms-of-service, fetched 2026-09-26 |
| Admin API: team/member/usage/spend/audit/model-access management, Basic auth with a team-scoped key, 20 req/min on most endpoints (60 for usage events, 50 for member removal), 429 + `Retry-After: 60` over the limit. No inference endpoint. Not what cox needs (it manages Cursor seats, not model calls) | https://cursor.com/docs/account/teams/admin-api |
| Official TypeScript SDK: npm `@cursor/sdk`, latest **1.0.32** (2026-09-22), repo `cursor/cursor` (monorepo, not independently browsable), license field `SEE LICENSE IN LICENSE.md` (proprietary, not OSS), requires Node ≥ 22.13, ships per-platform native binaries (`@cursor/sdk-<os>-<arch>`). Wraps the same agent runtime as the IDE/CLI/web app; local mode runs the loop in-process, cloud mode calls the Cloud Agents API. Not a raw-completions client — only full agent runs | `https://registry.npmjs.org/@cursor/sdk` fetched 2026-09-26 ; https://cursor.com/docs/sdk/typescript |
| Official Python SDK: PyPI `cursor-sdk`, latest **1.0.32**, summary "Python client for the Cursor SDK bridge", license "Proprietary", requires Python ≥ 3.10. It talks to the local SDK Bridge process, not the TS SDK directly | `https://pypi.org/pypi/cursor-sdk/json` fetched 2026-09-26 |
| SDK Bridge: a small local server (spawned by the Python/other-language client) that embeds `@cursor/sdk` and re-exposes it over a Connect (gRPC-Web-style)/protobuf contract (`sdk.v1`), for languages with no first-party SDK. Repo `cursor/sdk-bridge`: **MIT licensed**, not archived, last push 2026-09-22, latest release `v1.0.32` (same day/version as the npm and PyPI packages) — actively maintained in lockstep with the SDKs | `https://api.github.com/repos/cursor/sdk-bridge` fetched 2026-09-26 ; https://cursor.com/docs/sdk/bridge |
| No Rust SDK, official or community, was found for either the Cloud Agents API or the SDK Bridge protocol (crates.io has nothing under `cursor-sdk`/`cursor-agent`/`cursor-bridge`; the one `cursor-agent` npm package that exists, latest 1.0.3, is an unrelated third-party "task sequence creator" by `zalab-inc`, not Cursor's CLI) | crates.io search (empty result pages, not individually re-fetched: **med**); `https://registry.npmjs.org/cursor-agent` fetched 2026-09-26 |
| **Machine-readable spec: yes, one exists and is fetchable unauthenticated** — `https://cursor.com/docs-static/cloud-agents-openapi.yaml`, OpenAPI **3.0.3** (matches the `openapiv3`-based `progenitor` crate's supported version, R§4.3.1), served straight from the docs site with no separate license grant visible in the file or the page around it: treat as "Cursor's own", not redistributable-by-default the way `openai/openai-openapi` (MIT) is | fetched 2026-09-26, HTTP 200 |
| No spec at all exists for the SDK Bridge's protobuf/Connect contract beyond what `cursor/sdk-bridge`'s own README documents (handshake: bridge writes `cursor-sdk-bridge ready` + JSON with `schemaVersion`/`transport`/`protocol` to stderr) — no `.proto` file location confirmed by this pass | https://cursor.com/docs/sdk/bridge (secondary summary of the repo, README not fetched directly: **med**) |
| **CLI**: official binary is `agent`, marketed as "Cursor CLI"/"cursor-agent"; official install is `curl https://cursor.com/install -fsSL \| bash` (or a PowerShell one-liner), **not** an npm package | https://cursor.com/docs/cli/installation ; https://cursor.com/cli |
| CLI headless mode: `agent -p "<prompt>"` (`--print`), `--output-format text\|json\|stream-json`, `--force`/`--yolo` for auto-approval, `--stream-partial-output` for token-level assistant deltas. Auth via `CURSOR_API_KEY` env var (or prior `agent login`) | https://cursor.com/docs/cli/headless ; https://cursor.com/docs/cli/reference/output-format |
| `stream-json` event shapes (fetched verbatim): `{"type":"system","subtype":"init","apiKeySource","cwd","session_id","model","permissionMode"}`; `{"type":"user","message":{role,content[{type:"text",text}]},"session_id"}`; `{"type":"assistant","message":{role,content[{type:"text",text}]},"session_id","timestamp_ms"?,"model_call_id"?}`; `{"type":"tool_call","subtype":"started"\|"completed","call_id","tool_call":{"<toolName>ToolCall":{"args",...,"result"?}},"session_id"}`; `{"type":"result","subtype":"success","duration_ms","duration_api_ms","is_error","result","session_id","request_id"?}` — shape resembles cox's own `Event` enum (D2) more than a wire-protocol frame; it is the CLI's own agent-turn log, not a model-completions stream | https://cursor.com/docs/cli/reference/output-format, fetched 2026-09-26 |
| CLI as **ACP server**: `agent acp` runs Cursor CLI as an ACP agent over stdio, JSON-RPC 2.0, newline-delimited, documented protocol version 1; pre-authenticate with `agent login` or `--api-key`/`CURSOR_API_KEY` (or `--auth-token`/`CURSOR_AUTH_TOKEN`) before invoking `acp` | https://cursor.com/docs/cli/acp |
| CLI as **MCP**: MCP support is client-only — the CLI reads `.cursor/mcp.json` and calls out to MCP servers the same way the editor does. There is no `agent mcp` server subcommand exposing the CLI's own tools as an MCP server (unlike Claude Code's `claude mcp serve` or cox's own `cox mcp`) | https://cursor.com/docs/cli/mcp |
| Wire compatibility: **no.** The Cloud Agents API's request/response shapes (`prompt`, `model.id`, `repos`, `mcpServers`, `customSubagents`, SSE agent-turn events) do not match OpenAI Chat/Responses or Anthropic Messages; `/v1/models` returns a curated id list for agent creation, not model capability rows in either vendor's shape. Reusing `crates/cox-provider-openai` or `crates/cox-provider-anthropic` as-is is not possible; at best a `[providers.cursor]` `CompatibleProviderConfig` could point at `api.cursor.com`, but the request/response bodies still would not parse — worse than LM Studio, whose native chat endpoint at least speaks the Anthropic Messages shape verbatim (R§4.3.2) | direct comparison of the OpenAPI schemas above against `crates/cox-protocol`'s `Request`/`ProviderEvent` (plan.md §1.2) and the vendored Anthropic/OpenAI schemas (T30.10) |
| Unofficial/community routes exist and are numerous: GitHub topic `cursor-api`, and named repos `cursor-api-proxy`, `curapi`, `cursor-agent-api-proxy`, `cursoride2api`, `Cursor-To-OpenAI` — all wrap the desktop app's or the CLI's private/internal traffic (or shell out to `agent`) to fake an OpenAI-compatible `/v1/chat/completions` endpoint, letting a Cursor subscription serve non-Cursor clients. None is a Cursor-published surface; none was inspected for correctness here. ToS risk: these plausibly conflict with the ToS's "no reverse engineering", "no reselling/sublicensing the Service", and "no scraping" clauses (R§ above), and separately several proxy variants launder the *subscription* (not an issued API key) into third-party traffic, which is the same category Anthropic explicitly forbids for Claude.ai logins (R§4.3.1) — **do not build on these; named for completeness only** | https://github.com/topics/cursor-api ; individual repo READMEs (not independently verified: **unverified**) |

**Reading.** Cursor's Cloud Agents API is the spiritual equivalent of what
cox's own `cox-core`/`Event` stream is for cox: an orchestration surface
around an agent loop, not a raw-inference endpoint. It is well-documented,
has a real OpenAPI 3.0.3 spec (`progenitor` territory, not `typify`+hand-SSE
territory — R§4.3.1), and a maintained TS/Python SDK plus an MIT-licensed
bridge protocol, but nothing in it lets cox send `Request { system, tools,
messages, effort, … }` and get back `ProviderEvent`s the way it does for
Anthropic, OpenAI or an OpenAI-compatible chat endpoint. The one thing that
*is* wire-compatible with something cox already drives as a subprocess is
the CLI's `stream-json` output and its `acp` mode — both are "drive Cursor
as an agent", the same relationship cox already has with Claude Code and
Codex (D4), not "drive Cursor as a model". The creator resolved the
resulting question by deciding Cursor is a **plugin** (P35, `plan.md` §6
A54), driving the CLI's two official headless modes, never the desktop
session and never one of the unofficial proxies named above.

### 4.4 Routing evidence (D5)
Copilot's auto model selection is praised because it is explicit, priced (10 % discount) and switchable; Claude Code's Haiku delegation is complained about because it is silent. aider's `--weak-model` (commits, summaries) and OpenCode's small model for titles are the same pattern. Jobs that tolerate a small model, by consensus of the surveyed tools: titles, summaries, commit messages, compaction, search/explore, tool-result summarisation, classification. Effect-size numbers from the survey ("4.2× savings", "Codex 3–4× fewer tokens than Claude Code") are unsourced and dropped. [med]

### 4.5 Crate survey (versions verified against crates.io on 2026-09-02)
| Area | Recommended | Alternatives considered | Note |
|---|---|---|---|
| TUI | ratatui 0.30.2 (2026-06-19), crossterm 0.29.0, ratatui-macros 0.7 | cursive 0.21 (retained, 2024), iocraft/r3bl (small) | Codex choice; TestBackend for tests |
| composer / text | tui-textarea 0.7.0 (2024-10; stable, low churn) | own widget (Codex) | start with tui-textarea, replace if it blocks |
| markdown | pulldown-cmark 0.10 → own spans | tui-markdown 0.3.9 (2026-07), termimad 0.35 | Codex renders its own; tui-markdown as fallback |
| highlighting | syntect 5.3 | tree-sitter-highlight 0.27 | syntect for display; tree-sitter for structure |
| images (v0.2) | ratatui-image 11.0.6 | — | — |
| async | tokio 1.53, tokio-util, tokio-stream, futures 0.3 | async-channel 2.5 | — |
| HTTP / SSE | reqwest 0.12 (rustls), eventsource-stream 0.2.3 | reqwest-eventsource 0.6 (2024) | see §1.3 |
| MCP / ACP | rmcp 3.2.0 (2026-08-31; stdio, Streamable HTTP, OAuth via `auth` — ledger #13), agent-client-protocol 2.0.0 | mcp-sdk, mcpr (unofficial) | — |
| search | ignore 0.4.33, grep-searcher 0.1.17, grep-regex, globset, nucleo 0.5 | — | ripgrep's own libs |
| diff / patch | similar 3.2.0, diffy 0.5.2 | imara-diff | diffy for unified apply, similar for display |
| code structure | tree-sitter 0.25 (Codex) / 0.27 (latest, MSRV 1.90) + grammars | — | pin to grammar compatibility |
| git | shell out to `git` | gix 0.87.1, git2 0.21 | gix is v0.2 |
| tokens | tiktoken-rs 0.12.0 | tokenizers 0.23 | Anthropic: `count_tokens` endpoint |
| store / config | diesel 2.2 (`sqlite`) + libsqlite3-sys 0.30 (`bundled`, FTS5) + diesel_migrations, toml 1.1, toml_edit 0.25, figment, directories 6, keyring 4.2 | rusqlite 0.40.2 (plan v1), sqlx 0.9 (Codex), SeaORM (async) | sync ORM as in rtok D13: typed models, no runtime in hooks/tests; FTS5 via `sql_query` |
| plugins (v0.2) | extism 1.30.0 | wasmtime 48 (component model), rhai 1.26, mlua 0.12, dylib (`abi_stable`: rejected, ABI fragility) | — |
| process / sandbox | portable-pty 0.9.0, shlex 2, landlock 0.4.7, seccompiler 0.5.0, nix; `sandbox-exec` via `Command` | birdcage | — |
| observability / CLI / errors | tracing 0.1.44, tracing-subscriber 0.3.23, tracing-appender, opentelemetry 0.31 (feature), clap 4.6, thiserror 2.0, anyhow 1 | miette, color-eyre | — |
| tests | insta 1.48.0, proptest 1.11, wiremock 0.6.5, rstest 0.26, mockall 0.15, assert_cmd 2.2, predicates 3, assert_fs, tempfile 3.27, pretty_assertions, vt100 0.16.2, cargo-nextest, cargo-mutants (optional), cargo-llvm-cov | httpmock 0.8, expectrl | — |
| misc / release | uuid 1.26, jiff or chrono 0.4.45, notify 8.2, which 8, semver 1, cargo-dist, cargo-deny 0.20, git-cliff | indicatif (headless progress) | — |

### 4.6 Measured savings (filled by T8.5, 2026-09-03, `just bench`)
| Mechanism | Sessions | Context-token-turns before | after | Δ |
|---|---|---|---|---|
| archive (D6a truncation) | 5 | 393337 | 148662 | 62.2 % |
| dedup (D6b re-read) | 5 | 157588 | 148662 | 5.7 % |
| outline (D6c) | 5 | 158291 | 148662 | 6.1 % |
| deferred tools (D6d) | 5 | 189854 | 148662 | 21.7 % |
| compaction (D6f) | 5 | 161736 | 148662 | 8.1 % |
| prefix stability (D6e, emulated cache-write) | 5 | 244790 | 148662 | 39.3 % |

Method: 5 hand-written transcripts (`evals/token/sessions/*.jsonl`, 6 turns
each over the `evals/token/workspace` fixture) replayed through the real
`Session` loop with a `Scripted` provider and real `read`/`grep`/`glob`
tools; toggling is the real config flag per mechanism (see
`evals/token/README.md`). Totals are sums of `Usage::context_tokens` from
the ledger rows the loop wrote. Caveats: transcripts are built to exercise
each mechanism (big-file reads, repeated reads, outlines), so the shares
are ceiling-shaped, not field averages; `prefix` counts emulated
cache-write bytes, not billed tokens. No mechanism measured 0, so none is
flagged for removal.

### 4.7 Footprint (filled by T30.2, 2026-09-23, `just footprint`)

| Metric | Darwin-arm64 | How |
|---|---|---|
| cold start (`cox --version`, median of 5) | 11.0 ms | `date +%s%N` around the process |
| first frame (scripted `stream-json`, median of 3) | 40.0 ms | spawn to first event on stdout |
| replay RSS peak (30 turns, 60 provider calls) | 26.4 MiB | `/usr/bin/time -l`, max over turns |
| binary size (`target/release/cox`) | 43.9 MiB | `cargo build --release -p cox` |

Method: `scripts/footprint.sh`; baseline `scripts/footprint.json` (keyed by
OS-arch); CI runs `footprint.sh --check` and fails on a >20% regression of
any metric. Replay: every `evals/token/sessions/*.jsonl` line becomes a
`Scripted` scenario per user turn, run back to back through `--resume` with
real `read`/`grep`/`glob` over a workspace copy — the corpus is 30 user
turns / 60 provider calls, not 50 (the card's number predates the corpus;
context still grows across turns, which is what RSS measures). Caveats:
timings are machine- and load-dependent (CI compares per-runner, not against
this table); no network, no key, scripted provider only. Comparative numbers
for other agents are out of scope here — their footprint threads move weekly:
[Claude Code performance degradation #19452](https://github.com/anthropics/claude-code/issues/19452),
[Claude Code high memory usage #8836](https://github.com/anthropics/claude-code/issues/8836),
[Codex CLI memory leak #9345](https://github.com/openai/codex/issues/9345),
[Codex 12GB on startup (forum)](https://community.openai.com/t/codex-consuming-12gb-memory-for-5-minutes-on-startup-macos/1376282).

#### Plugin timings (filled by T33.28, 2026-09-26, `just bench`)

PL§11's per-plugin latencies over the Rust reference plugin
(`plugins/examples/rust`, 325 KiB release `wasm32-unknown-unknown` module),
from `crates/cox/examples/plugin_bench.rs` in the release profile. Primary
source: this run, on an Apple M3 Max (16 cores, 64 GiB), macOS 27.0,
rustc 1.97.1, extism 1.30 over wasmtime 43; three runs, range shown.

| Metric | Budget (PL§11) | Measured |
|---|---|---|
| session start per plugin: `LivePlugins::load` + `start` (compile, instantiate, `cox_init`), median of 20 | ≤ 50 ms warm (cache on); ≤ 500 ms cold for 1 MiB | 57–97 ms, cold (max 138–169 ms) |
| `cox_on_event`, batch of 16 `turn_started`, p50 of 1 000 | ≤ 1 ms | 0.12–0.17 ms |
| `cox_render` of the status segment, p95 of 1 000 | ≤ 5 ms | 0.09–0.51 ms |
| hook round trip (`PluginHooks::run`, `PostToolUseFailure` with a kv write and a notice), p95 of 1 000 | ≤ 5 ms | 0.38–0.72 ms |

Caveats: the host still builds with wasmtime's compilation cache off
(`PluginHost::load_with`, `with_cache_disabled`), so every start is a cold
compile and the warm-start row cannot be measured until the cache is wired
to `~/.cox/cache/wasmtime`; PL§12 falsifier 2 is therefore not judged by
these numbers. The module is 325 KiB, not the 1 MiB the cold-compile row
names. The machine ran four other agents' builds (load average 19–25 on
16 cores), and wasmtime compiles functions in parallel, so the start row is
the noisiest; the call rows are within budget by an order of magnitude.

### 4.8 `diagnostics` with a real rust-analyzer (T41.9, checked 2026-09-29)

Source: the ignored test `real_rust_analyzer_reports_a_type_error_under_the_sandbox`
in `crates/cox/tests/lsp.rs`, run with `mise exec -- cargo nextest run -p cox
--test lsp --run-ignored only --no-capture`, plus two manual `cox run -p`
runs against `COX_HOME=/tmp/cox-t41-9` with the server's stdin and stdout
tee'd to files. The host is macOS, so the sandbox backend is Seatbelt; bwrap
was not run. Server: `rust-analyzer 1.98.1 (48a229ce 2026-09-01)`, from the
mise-pinned toolchain, found on `PATH` through the rustup proxy with the
real `HOME`. Scratch crate: `src/main.rs` with `mod a;`, and `src/a.rs`
returning `"one"` from a `-> u32` fn.

| Measure | Result |
| --- | --- |
| Sandbox | no denial: rust-analyzer loaded the crate and its sysroot and ran `cargo check` (it wrote `Cargo.lock` and `target/flycheck0` in the workspace) |
| Push or pull | pull: the `initialize` result carries `diagnosticProvider = {identifier: "rust-analyzer", interFileDependencies: true, workspaceDiagnostics: false}` |
| First call, cold | 279 and 1,248 ms over two test runs (manual runs: 552 and 626 ms), result `no diagnostics`: the pull is answered with `items: []` before the crate is loaded |
| Server ready | first `workspace/diagnostic/refresh` request at 5.1 and 7.5 s, first `publishDiagnostics` push (the `cargo check` E0308) at 5.3 and 7.7 s after `cox` started |
| Call after a 20 s wait | 3 and 5 ms: `src/a.rs:2:5: error: expected u32, found &'static str [rust-analyzer E0308]` |

So the profile is not the problem; the cold start is. On a fresh server the
first call reports the file clean when it is not. In pull mode cox asks once
and returns what it gets. Two protocol facts explain why nothing else tells
it to wait (LSP 3.17,
https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/,
checked 2026-09-29). First, a server may start its own progress only if the
client sets `window.workDoneProgress`. cox's `initialize` does not set it, so
rust-analyzer sent no `$/progress`. Second, a server sends
`workspace/diagnostic/refresh` to ask the client to pull again. cox answers
it with "method not found" and does not pull again. Fixing that is a new
card, not part of T41.9.

## 5. Testability patterns adopted
1. `Provider` trait with `Scripted` and `Replay` (cassette) implementations; cassettes re-recorded on demand and redacted. Temperature 0 and seeds do not give bit-exact replay across providers; replaying the event log does. [high]
2. Golden `Event` JSONL for loop scenarios (`insta`); the rollout file and the fixture are the same format. [design]
3. ratatui `TestBackend` + `insta` per widget and per frame; `portable-pty` + `vt100` for the real binary (Codex practice). [high]
4. Tools in `tempfile` trees; `proptest` on `str_replace` and V4A (`parse(print(p)) == p`, edit-then-reverse identity); fuzz targets for SSE/V4A/frontmatter parsers. [design]
5. Evals separate from tests: Terminal-Bench 2.0 agent (Harbor) + 10 in-repo tasks, run on demand with the real provider, cost recorded in the ledger. §5.3 holds the recorded runs.

### 5.3 First eval run (T12.1, 2026-09-04)

Harness `evals/run.py` (`just eval`), 10 tasks in `evals/tasks/`. (The
terminal-bench 0.2.x adapter written here was replaced in T30.9 by a
Harbor agent, `evals/src/cox_evals/tbench.py`; see the Terminal-Bench
subsection below.)

Dry-run (`COX_PROVIDER=scripted just eval --dry-run`): **10/10 passed**,
$0.0000, no network, no key.

Live run: **blocked, $0 spent.** No Anthropic key in env; `OPENAI_API_KEY`
is set but the account is exhausted (`429 credit_balance_exhausted` on
`POST /v1/responses` with `gpt-4o-mini`, verified by direct curl the same
day — cox surfaces it as a fast turn error, exit 1). Reproduce when funded:

```bash
python3 evals/run.py --provider openai --model gpt-4o-mini
```

Related precise bug (not fixed here, provider owner's scope): neither
OpenAI module wraps its stream in `retry::stream_with_retry` (only the
Anthropic one does), so a retryable 429 fails on the first attempt instead
of backing off per §1.14. Worth a wiremock contract test (429, 429, 200)
when touched.

Two drive-by findings from building the harness (both fixed in T12.1):
empty `workspace_roots` reached the tools verbatim so every confined
tool failed outside `--cwd` (plan §1.6 says empty means git-root-else-cwd;
now resolved in `session::open`); eval runs pass `--no-hooks --no-mcp`
because ambient repo servers add startup noise to every task.

#### Live run with and without the verify preset (T30.3, 2026-09-25)

`claude-sonnet-5`, the 10 in-repo tasks, `--approve never
--permission-mode auto`. Cost and tokens are the per-task ledger rows
(`usage`, what `cox stats` reads), summed; the harness's own
per-task rounding gives $0.0516 / $0.0628.

| Configuration | Pass | Provider calls | Input | Output | Cache read | Cache write | Cost |
|---|---|---|---|---|---|---|---|
| baseline (`--no-hooks`) | 9/10 | 21 | 42 | 951 | 173 260 | 2 932 | $0.0515 |
| `--preset verify` | 9/10 | 25 | 50 | 1 273 | 208 382 | 3 300 | $0.0626 |

The one failure is the same task in both: `append-line` exits 2 because
the model first tried a writing `bash` command, which `--approve never`
denies ("Exec calls require approval"), then finished with `edit`; the
file is correct but the harness scores any denial as a failure. The
preset costs +22 % here and changes no outcome: nine of the ten tasks are
one tool call and one answer, so there is nothing for a test hook to
catch, and on `append-line` it doubled the calls (4 → 8). These tasks are
too small to show a verification benefit; Terminal-Bench is where it
would.

Getting here took three fixes the offline suite could not see: an
org-level key needs `anthropic-workspace-id` (T30.4); no production path
priced a call, so every ledger row was $0 (T30.5); the Anthropic stream
never emitted `ToolUseEnd`, so every tool call was dropped (T30.6). The
harness overrides `HOME`, which hides the macOS keychain, so the key has
to come from the environment:

```bash
ANTHROPIC_API_KEY="$(security find-generic-password -s cox -a anthropic -w)" \
  uv run --project evals cox-evals --provider anthropic --model claude-sonnet-5 [--preset verify]
```

#### Terminal-Bench 2.0 subset (T30.9, 2026-09-25)

Terminal-Bench 2.0 runs through Harbor, not the old `tb` CLI
(https://github.com/laude-institute/harbor, PyPI `harbor` 0.23.0,
https://pypi.org/project/harbor/0.23.0/, checked 2026-09-25). The agent is
`cox_evals.tbench:CoxAgent`, a `BaseInstalledAgent`: `install` uploads a
Linux cox build to `/installed-agent/cox`, `run` executes one
`cox run --output-format json` in the task container and copies the payload's
tokens and cost into Harbor's `AgentContext`. Tasks come from
https://github.com/laude-institute/terminal-bench-2 at commit
`69671fbaac6d67a7ef0dfec016cc38a64ef7a77c` (recorded by Harbor in each
trial's `config.json`).

Setup: cox from `b027a47`, cross-built with `cargo zigbuild --release
--target aarch64-unknown-linux-gnu.2.31` (cargo-zigbuild 0.23.4, zig
0.16.0); Docker in colima 0.10.3 (vz, arm64). `claude-sonnet-5`,
`--budget 0.25`, `--max-turns 40`, `--permission-mode bypass --sandbox
danger-full-access` (the container is the isolation boundary), two trials
at a time, three tasks picked for spread: git surgery, a COBOL-to-Python
port, a Coq proof.

| Task | Reward | Turns | Input | Output | Cache read | Cache write | Cost |
|---|---|---|---|---|---|---|---|
| `fix-git` | 1.0 | 9 | 17 | 2 691 | 102 496 | 7 533 | $0.0663 |
| `cobol-modernization` | 1.0 | 22 | 43 | 13 242 | 379 814 | 16 349 | $0.2493 |
| `prove-plus-comm` | 1.0 | 4 | 7 | 1 013 | 34 356 | 997 | $0.0195 |
| **total** | **3/3** | 35 | 67 | 16 946 | 516 666 | 24 879 | **$0.3351** |

Cost is cox's own ledger figure from the `cox run` payload; wall time
2 min 56 s. Three tasks out of 89 is a smoke test of the agent and the
pipeline, not a leaderboard score. `cobol-modernization` finished at
$0.249 of a $0.25 cap, so a larger subset needs a higher per-task budget.

A first attempt the same day spent about $0.51 on `cobol-modernization`
and `prove-plus-comm` (both stopped by the budget) plus an unrecorded
share of `fix-git` (at most $0.25, interrupted), and scored nothing, for
two reasons fixed in `db85e5f`: `--permission-mode auto --approve never`
denied 24-26 calls per task, which the model spent its budget retrying;
and the jobs dir sat in `/tmp`, which colima does not share with its VM
(only `$HOME`), so the verifier's reward file never reached the host.

Reproduce (Docker reachable, Linux cox built):

```bash
ANTHROPIC_API_KEY="$(security find-generic-password -s cox -a anthropic -w)" \
  uv run --project evals --extra tbench harbor run -d terminal-bench@2.0 \
  -a cox_evals.tbench:CoxAgent -m anthropic/claude-sonnet-5 --force-build -n 2 \
  -i fix-git -i cobol-modernization -i prove-plus-comm \
  --ak cox_bin=<linux cox> --ak budget_usd=0.25 -o ~/.cache/cox-evals/tb-jobs
```

The verify preset (T30.3) was not run on Terminal-Bench: the harness
system addendum and hook are wired for the in-repo harness only, and all
three tasks already passed without it.

## 6. Fact-check ledger
| # | Claim (report) | Verdict | Correction / source |
|---|---|---|---|
| 1 | ratatui 0.30.2 released 2026-06-19 (D) | confirmed | crates.io |
| 2 | rmcp 3.2.0 released 2026-08-31 (D) | confirmed | crates.io |
| 3 | MCP latest revision 2026-07-28 (E) | confirmed | modelcontextprotocol.io changelog |
| 4 | HTTP+SSE deprecated in 2026-07-28 (E) | confirmed, clarified | deprecated since 2025-03-26; reclassified 2026-07-28 |
| 5–7, 18 | Anthropic prices from finout.io (E) | unverifiable by the checker | plan uses the Claude API reference table cached 2026-06-24: Haiku 4.5 $1/$5, Sonnet 5 $2/$10, Opus 5 $5/$25, Fable 5.1 $10/$50; report E's "Sonnet promo ends Sept 1, then $3/$15" is unconfirmed → T1.7 re-verifies from the official pricing page |
| 28 | Anthropic prices re-verified 2026-09-02 (T1.7) | confirmed, Sonnet 5 promo extended | Haiku 4.5 $1/$5, Sonnet 5 $2/$10 (promo extended indefinitely), Opus 5 $5/$25, Fable 5.1 $10/$50; cache pricing verified; config/prices.toml carries verified_on dates — https://platform.claude.com/docs/pricing |
| 8 | eventsource-stream last release 2022-02-17 (D) | confirmed date, verdict rejected | Codex ships it (§1.3); small and finished |
| 9–10 | Claude Code compaction at 250–300 k; tool results 500–2 000 tokens (F) | unverifiable | dropped as design inputs |
| 11 | Claude Code sandbox = Seatbelt / bubblewrap (A) | confirmed | code.claude.com/docs/en/sandboxing |
| 12 | Claude Code hook events = 7 (A) | refuted | 31 events incl. SessionStart/End, UserPromptSubmit, Stop, StopFailure, PostToolUseFailure, PermissionRequest/Denied, Notification, PreCompact/PostCompact, PreModelSwitch/PostModelSwitch, Elicitation… (code.claude.com/docs/en/hooks) |
| 13 | rmcp has no OAuth (D) | refuted | OAuth 2.0 via `auth` feature (docs.rs/rmcp/3.2.0) |
| 14 | JetBrains adopted ACP Jan 2026 (E) | refuted | 2025-10-06 (zed.dev/acp) |
| 15 | tokio 1.53.1 on 2026-07-20 (D) | confirmed | crates.io |
| 16 | Terminal-Bench 2.0 has 89 tasks (E) | unverifiable | tbench.ai shows 4.0 |
| 17 | SWE-bench Verified is the de-facto benchmark (E) | unverifiable | site lists several variants |
| 19 | rig-core 0.42 multi-provider (D) | confirmed | crates.io |
| 20 | `anthropic` crate 0.0.8 unofficial, 2024 (D) | confirmed | crates.io |
| 21 | min cacheable prefix 1 024–4 096 tokens (F) | refuted | model-dependent: 512 (Fable 5.1, Opus 5, Sonnet 5), 4 096 (Haiku 4.5, Opus 4.5) — prompt-caching docs |
| 22 | Codex uses sqlx 0.9, ratatui 0.30.2, insta, wiremock (B) | confirmed by author | `codex-rs/Cargo.toml` read 2026-09-02 (§1.3) |
| 23 | Codex "tonic used for inter-service gRPC?" (B) | present in manifest, purpose unverified | tonic 0.14.3 listed |
| 24 | OpenCode has no MCP; aider supports "Claude 3.7" (C sub-agent) | refuted / stale | OpenCode docs list MCP + LSP; aider model list is generated from litellm and is current |
| 25 | Claude Code default model "3.5 Sonnet", "MCP 1.0" (A) | stale | dropped |
| 26 | OpenAI "GPT-5.6 Sol/Terra/Luna", Gemini "3.7 Flash", DeepSeek V3.2 prices (E) | unverified (third-party pricing sites only) | not used; T1.7 fills `config/prices.toml` from official pages |
| 27 | "RTK 60–90 %", "caveman 46 %", "engram +10.4 % at 8× fewer tokens" (F) | vendor claims, unverified | rtok's own measurements found 3–40 % for the hook stack; see `~/GitHub/rtok/research.md` |

## 7. Method and limits
- Agents: A (Claude Code/Copilot, 19 lookups), B (Codex, 22), C (competitors, 3 sub-agents, ~75 lookups), D (crates, 69), E (specs, 19), F (tokens/testing, **3 lookups** — largely written from the model's memory; treated as directional only), G (fact-check, 43). Total ≈ 550 k subagent tokens on Haiku, ≈ $1.
- Author verifications: `codex-rs/Cargo.toml` (deps), crates.io API (18 crates), crates.io name availability (`cox`, `coxswain`, `boatswain`, `mizzen`, `brigantine` free), the Claude API reference (models, prices, caching thresholds, thinking/effort rules).
- Still unverified: official prices for non-Anthropic providers; Claude Code's compaction thresholds and truncation limits; Terminal-Bench 2.x task counts; Copilot CLI internals (closed). Each has a task that replaces the guess with a measurement (T1.7, T8.5, T12.1).

## 8. Field survey 2026-09-22 — what terminal coding agents ship now, and where cox stands

Date: 2026-09-22. Method: four parallel research agents (Sonnet 5, web access, ~180 lookups, every bullet carries a URL in the agent transcripts) covering (A) Claude Code / Codex CLI / Gemini CLI→Antigravity / Copilot CLI, (B) OpenCode, Crush, Amp, Cursor CLI, Factory Droid, Goose, Pi, aider, Kilo, Mistral Vibe, Qwen Code, Kimi, Warp, (C) terminal capabilities and ratatui ecosystem, (D) capability trends and a matrix. The author verified the cox column against the code (not the plan), and the ratatui/crossterm claims against the vendored crate sources (ledger #29–31). Third-party claims about vendors keep the agents' confidence: [high] = official docs/changelog/repo, [med] = one secondary source, [unverified] = not confirmed. The improvement plan that follows from this section is `docs/design/improvement-plan-2026.md` (proposal, A26); nothing here changes §0 decisions.

### 8.1 What the field converged on (table stakes in 2026)

| Convention | Who ships it | Confidence | cox today |
|---|---|---|---|
| `Shift+Tab` cycles into a read-only *plan* mode | Claude Code, Codex (`/plan` too), Copilot CLI; OpenCode uses `Tab` | high | `Tab` cycles default→plan→auto (§1.13); no plan-specific view |
| Checkpoint before every edit, `/rewind` restores code, conversation or both | Claude Code (`Esc Esc`, ~30-day retention; bash-caused changes *not* tracked), Cursor CLI (`/rewind` timeline with per-turn diffs, branch-on-rewind), OpenCode (`/undo`, `/redo`, separate git object DB), Gemini CLI | high | implemented in T26.1–T26.2, including shell-caused changes |
| Queue messages while a turn runs; a *send-now* key interrupts and flushes | Claude Code (`Ctrl+Enter`, 2.1.275), Pi (queued messages pinned above the editor) | high | composer is blocked during a turn |
| Subagents run in the background by default, isolated in git worktrees on request | Claude Code (`isolation: "worktree"`, agents map with per-agent cards), Codex (up to 6, "Smart Approvals" label the source thread), Copilot (`/fleet`, live subagent timing) | high | `agent` tool with presets and worktree isolation (T27.3); `/agents` lists sessions; approvals are not labelled by source |
| `/fork` / `/branch` and `/handoff` | Codex (`/fork`, `/side`), Claude Code (`/branch`, `--fork-session`), Amp (`/handoff` seeds a new thread) | high | none (`parent_id` column exists) |
| `/context` token breakdown and a visible auto-compact threshold | Claude Code (`/context`, `/autocompact`), OpenCode v2 compacts *before* the call | high | `ctx %` and `cache %` in the status line; compaction after `TurnDone` only |
| Themes as files, `/theme` with live preview; syntax themes from `.tmTheme` | Codex (32 themes, `.tmTheme` drop-in), Crush (`Ctrl+P` palette, `Ctrl+E` live editor), OpenCode ("system" theme derived from the terminal background, `{dark,light}` per colour) | high | `tui.theme = auto|dark|light` picks one of two syntect base16 themes; `auto` is not detected |
| Custom keybindings file | Claude Code (`keybindings.json`), Codex (F13–F24) | high | none |
| Real vim: text objects, visual mode, undo/redo of drafts | Claude Code 2.1.118, Codex | high | vim-lite (`hjkl`, `i`, `x`, `dd`-less) |
| `!` drops into a shell line without leaving the session | Factory Droid, Claude Code | high | none |
| Desktop notification on turn end / approval (OSC 9/777 + BEL, hooks) | all four majors; Pi | high | none |
| OSC 8 hyperlinks on paths/URLs; clickable | Codex, Pi | high | none |
| Image paste (`Ctrl+V`), screenshots to the model | Codex (Windows/Linux too), Claude Code (macOS; Windows open issue), OpenCode (drag-drop), Pi (Kitty graphics inline) | high | refused with a hint (gate T19.4) |
| Screen-reader / plain mode, reduced motion, daltonized themes | Claude Code (`--ax-screen-reader`, `prefersReducedMotion`) | high | `NO_COLOR`, ASCII glyph fallback (T14.1/T14.2) |
| Voice input (`/voice`) | Claude Code, aider | high | none — out of scope (see plan §6) |
| Remote control from phone/web, session keeps running locally | Claude Code Remote Control, Codex Remote, Amp Orbs, Cursor background agents | high | none — out of scope for v0.2 |
| Scheduled agents / `/loop` | Claude Code Routines (cloud), Cursor CLI `/loop` (local) | high | none |
| MCP OAuth, elicitation, 2026-07-28 spec (MRTR, list TTLs, MCP Apps extension; sampling deprecated) | Claude Code CLI, Copilot, Codex | high | stdio + HTTP and OAuth implemented (T22.5); elicitation and MCP Apps remain open |
| ACP as an agent | OpenCode (Zed, JetBrains, Neovim), Goose, Amp via adapter; Claude Code/Codex/Cursor not listed as native ACP agents | med | `cox acp` (T11.1) — a real lead |
| `AGENTS.md` under the Agentic AI Foundation, 60 k+ repos; Claude Code reads it since 2.1.277 | everyone | high | yes (T7.1) |
| Native Windows sandbox | Codex only (restricted tokens/ACLs, "experimental") | high | none, loud warning (D7) |

### 8.2 What users complain about (the gaps a newcomer can win on)

- **Trust**: silent model downgrades and routing (Claude Code April 2026 incident, Gemini CLI Pro→Flash), expired credentials in the wider field, hallucinated tool results. cox's D5 ("never up, never silent"), keyring-backed MCP OAuth (T22.5), and the ledger are the answer; the remaining work is *showing* it (plan P28). [high]
- **Noise in multi-agent views**: Codex #12047 "raw scaffolding noise", approvals popping from unnamed threads. [high]
- **"It does not look good yet"** even for Codex (#2609, #21130: semantic colours beyond syntax, Plan/Build switcher). Crush is the reference for looks; OpenCode for "genuinely pleasant" clarity of tool calls and diffs. [high]
- **Footprint**: OpenCode ~1 GB RSS "for a TUI", Goose loads whole sessions into memory, Copilot CLI Node OOM. A Rust binary with a measured RSS is a marketing fact cox has not published. [high]
- **System-prompt tax**: Pi keeps the prompt under ~1 000 tokens by shipping nothing optional; `oh-my-pi` adds the rest as extensions. cox's deferred tools (D6d) are the same idea half-done: the skills index, memory index and instruction files still ride in every request. [high]
- **Copy fidelity**: Claude Code drops GFM features on copy (#26390); Codex falls back to key/value for cramped tables. [high]
- **Windows**: image paste, sandbox and MCP install remain the weakest area for everyone. [high]
- **Policy**: Anthropic forbids third-party apps from offering Claude.ai login or routing requests through Free/Pro/Max credentials (https://code.claude.com/docs/en/legal-and-compliance, checked 2026-09-25). cox does not implement Claude subscription OAuth; API keys and the keyring stay the path. OpenAI neither permits nor forbids third-party use of Codex's ChatGPT login in any document found (§4.3.1). [high for Anthropic, unverified for OpenAI]

### 8.3 Terminal capabilities (author-verified against vendored sources where marked ✔)

| Capability | Mechanism | Support | ratatui/crossterm 0.30.2/0.29 |
|---|---|---|---|
| Distinct `Shift+Enter`/`Ctrl+Enter` | Kitty keyboard protocol | Kitty, Ghostty, foot, Alacritty, iTerm2, WezTerm, Rio, Warp; not tmux | `PushKeyboardEnhancementFlags` ✔ (no-op where unsupported) |
| Flicker-free `insert_before` | scrolling regions | VT100-class terminals | ratatui feature `scrolling-regions` ✔ present, **enabled** in cox (T23.2, 2026-09-24): 40 → 0 full-viewport repaints for 40 inserted cells on the vt100 PTY fixture. Caveat: the `vt100` crate (0.16 `grid.rs` `scroll_up`) keeps no scrollback for lines scrolled off a DECSTBM region; the feature's premise (ratatui#1341) is that real terminals save lines leaving a region whose top is row 1, not re-verified per terminal here, so PTY tests that read scrollback must model that |
| Clipboard over SSH/tmux | OSC 52 | Alacritty, Ghostty, Kitty, WezTerm, tmux forwards | crossterm feature `osc52` → `CopyToClipboard` ✔ present, not enabled |
| Hyperlinks | OSC 8 | iTerm2, Terminal.app 13+, Ghostty, Kitty, WezTerm, Alacritty, VTE | no widget in ratatui; emit the sequence around a span (`hyperrat` exists) |
| Desktop notification | OSC 9 / OSC 777 / BEL | iTerm2, WezTerm, Ghostty, Kitty, Warp (OSC 9) | raw write |
| Tab/taskbar progress | OSC 9;4 | Windows Terminal, Konsole, foot, WezTerm, Kitty, Ghostty | raw write |
| Background colour → dark/light | OSC 11 | xterm, iTerm2, Kitty, Alacritty, WezTerm, foot, VTE, Windows Terminal ≥ 1.22 | `terminal-colorsaurus` / `termbg` crates (new dependency, needs approval) |
| Focus in/out (notify only when unfocused) | focus events | most | `EnableFocusChange` ✔ |
| Inline images | Kitty / iTerm2 / Sixel | Kitty ≥ 0.28, Ghostty, WezTerm, iTerm2; halfblock fallback | `ratatui-image` (v0.2 images gate) |
| Synchronised output | mode 2026 | Ghostty ≥ 1.0, Kitty, WezTerm | ratatui's crossterm backend already wraps frames |
| Languages beyond syntect's ~40 | `two-face` (bat's syntax set, ~250 languages, +0.6 MiB) | — | new dependency, needs approval |

Codex TUI structure worth copying (R§1.6 confirmed by two independent code readings): immutable committed `HistoryCell`s plus exactly one mutable active cell; a `BottomPane` stack of views (approval, pickers) that receives input first; `Ctrl+T` transcript overlay over the inline viewport. cox already has the first and third (T5.3, `Ctrl+O`); the modal stack is single-level.

### 8.4 cox versus the field — capability matrix (cox column verified in code on 2026-09-22)

`yes` shipped · `part` partial · `no` absent · `?` unverified for that vendor

| Capability | cox | Claude Code | Codex | OpenCode | Crush | Copilot | Cursor CLI | Pi | aider |
|---|---|---|---|---|---|---|---|---|---|
| Lossless tool-output archive + `expand` | **yes** | ? | ? | ? | ? | ? | ? | no | no |
| Explicit tiered routing, never up | **yes** | no (silent Haiku) | part (effort profiles) | part | part | part (auto, discounted) | no | no | part (architect/editor) |
| Per-request usage row with cache read/write | **yes** | part (`/cost`) | ? | ? | ? | ? | ? | ? | part |
| Deferred tool schemas + `tool_search` | **yes** | yes | ? | no | no | ? | ? | no | no |
| Dedup of repeated reads | **yes** | no | no | no | no | no | no | no | no |
| OS sandbox macOS + Linux | yes | yes | yes | ? | ? | yes | ? | no (containers) | no |
| Windows sandbox | no | no | yes (exp.) | no | no | part (proxies) | ? | no | no |
| Permission rules, deny wins | yes | yes | yes | ? | ? | yes | ? | no | no |
| Plan mode | part | yes | yes | yes | ? | yes | yes | no (`oh-my-pi`) | no |
| Checkpoints / rewind | **yes** | yes | ? | yes | ? | ? | yes | no | part (git commits) |
| Bash-caused changes in rewind | **yes** | no | ? | yes (worktree snapshots) | ? | ? | ? | no | yes (commits) |
| Queued messages + send-now | **no** | yes | ? | ? | ? | ? | ? | yes | no |
| Background subagents | part | yes | yes | yes | ? | yes | yes | no | no |
| Approval labelled by source agent | no | yes | yes | ? | ? | yes | ? | n/a | n/a |
| Worktree isolation | **yes** | yes | yes | part (community) | ? | yes | ? | no | no |
| `/fork`, `/handoff` | no | yes | yes | ? | ? | ? | yes (rewind branch) | no | no |
| `/loop` / scheduled | no | yes | part | ? | ? | ? | yes | no | no |
| Hooks (events) | yes (11 of 13 fire) | yes (30+) | part | yes (25+) | part | ? | ? | no | no |
| Skills (`SKILL.md`) | part (discovered, not in context) | yes | ? | ? | yes | ? | ? | yes | no |
| Custom slash commands from files | part (`cox ext list` only) | yes | yes | yes | ? | yes | yes | yes | no |
| MCP client | yes | yes | yes | yes | yes | yes | yes | no | part |
| MCP OAuth | **yes** | yes | yes | ? | yes | yes | ? | no | no |
| MCP elicitation | **yes** (TUI, plain; T47.3) | yes (CLI) | ? | ? | ? | ? | ? | no | no |
| ACP server | **yes** | ? | ? | yes | ? | ? | ? | ? | ? |
| `cox mcp` (tools as an MCP server) | **yes** | no | yes | no | no | no | no | no | no |
| Headless `stream-json` | yes | yes | yes | yes | yes | yes | yes | yes (RPC) | no |
| Multi-provider incl. local | **yes** | no | no | yes (75+) | yes | part | no | yes | yes |
| `ask_user` answered interactively | **no** (fixed answer) | yes | yes | yes | yes | yes | yes | no | n/a |
| Themes as files, `/theme` | no | part | yes | yes | yes | yes | ? | yes | no |
| Terminal background detection | no | ? | ? | yes | ? | ? | ? | ? | no |
| Word-level / side-by-side diff | no | no | part | yes (`diff_style`) | ? | ? | ? | ? | yes (best-rated) |
| Collapsible tool cards | part (fold + hint) | yes | yes | yes | yes | yes | ? | ? | no |
| Keybindings file | no | yes | part | ? | ? | ? | ? | ? | no |
| Vim mode | lite | full | full | ? | ? | ? | ? | ? | no |
| Mouse | no (config key is dead) | yes | ? | yes | yes | yes | ? | ? | no |
| Kitty keyboard protocol | no | ? | ? | ? | ? | ? | ? | yes | no |
| OSC 8 links | no | ? | yes | ? | ? | ? | yes (Jan 2026) | yes | no |
| OSC 52 clipboard | no | ? | ? | ? | ? | ? | ? | ? | no |
| Notifications (OSC 9 / bell) | no | yes | yes | ? | ? | yes | ? | ? | no |
| Image paste | no | yes | yes | yes | ? | ? | ? | yes | part |
| `/context` breakdown | no | yes | ? | ? | ? | ? | ? | ? | part (`/tokens`) |
| Pre-emptive compaction (before the call) | no (after turn) | yes | ? | yes | ? | ? | ? | ? | no |
| Auto-memory | part (opt-in extraction) | yes | yes | ? | ? | yes | ? | no | no |
| Screen-reader / plain mode | no | yes | ? | ? | ? | yes (a11y work) | ? | ? | part (plain) |
| Secret redaction of transcripts | part (`record --redact`) | part (community) | ? | ? | ? | yes | ? | ? | no |
| `doctor` | yes | yes | no | no (requested) | no | ? | ? | ? | no |
| Measured RSS / startup published | no | no | no | no (~1 GB reported) | no | no | no | no | no |
| Repo map | no (gate T19.6) | no | no | ? | ? | no | no | no | yes |
| LSP diagnostics after edit | no (gate T19.2) | part | part | yes | yes | ? | ? | part (ext) | no |

Reading: cox's core economics (archive, dedup, deferred tools, routing, ledger, `cox mcp`, ACP, multi-provider) are ahead of every vendor; checkpoints and rewind now close one major surface gap, while the remaining gaps include no queue, no themes, dead config keys, and a fixed-answer `ask_user`. The plan therefore spends P22–P26 on the surface and keeps the core decisions.

### 8.5 Fact-check ledger additions

| # | Claim | Verdict | Source |
|---|---|---|---|
| 29 | ratatui 0.30.2 has a `scrolling-regions` feature that makes `insert_before` scroll a region instead of repainting | confirmed by author | `~/.cargo/registry/.../ratatui-0.30.2/Cargo.toml` line 88; PR ratatui/ratatui#1341 |
| 30 | crossterm 0.29 ships `CopyToClipboard` behind an `osc52` feature | confirmed by author | `crossterm-0.29.0/Cargo.toml` line 66, `examples/copy-to-clipboard.rs` |
| 31 | crossterm 0.29 exposes `PushKeyboardEnhancementFlags` | confirmed by author | `crossterm-0.29.0/src/event.rs` |
| 32 | cox `tui.mouse`, `tui.theme = "auto"` are read but have no effect; `ask_user` in the TUI is `Answers::Fixed`; skills index and custom commands are only in `cox ext list`; `SessionStart` hook never fires | confirmed by author | `crates/cox/src/session.rs`, `crates/cox-tui/src/app.rs`, `crates/cox-core/src` grep on 2026-09-22 |
| 33 | Gemini CLI retired into a closed-source Antigravity CLI (June 2026) | [med] | developers.googleblog.com (agent A/D), not read by the author |
| 34 | Anthropic disabled Claude Pro/Max OAuth for third-party harnesses (2026-04-04) | confirmed in substance, date unverified | https://code.claude.com/docs/en/legal-and-compliance (checked 2026-09-25): third parties may not offer Claude.ai login or route requests through plan credentials; the enforcement date is from secondary press only |
| 35 | Vendor model names quoted by reviewers (e.g. "GPT-6 Astra") and star counts (OpenCode 140–172 k, Pi 104–140 k) | [unverified] | vary by source; directional only |
| 36 | Codex CLI checkpoints/rewind | [unverified] | no documentation found by agent D |
| 37 | Codex's ChatGPT-plan login is sanctioned for third-party clients | [unverified] | no OpenAI document found either way (2026-09-25); `codex-rs/login` shows the flow uses Codex's own OAuth client id |
| 38 | Claude Code matches a `Bash` rule per subcommand: separators `&&`, `||`, `;`, `|`, `|&`, `&` and newlines split the line; an allow rule must match each subcommand; deny and ask rules apply when any subcommand matches, including one nested in a subshell, a command substitution or a loop body; a dangling `&&`/`||` makes the line unparseable, so no allow rule approves it; deny matches past any leading `VAR=` assignment, allow only past known-safe ones; wrappers (`timeout`, `time`, `nice`, `nohup`, `stdbuf`, `command`, `builtin`, `noglob`, bare `xargs`) are stripped before matching; output redirect targets are checked against `Edit` rules; "don't ask again" on a compound line saves one rule per subcommand | confirmed | https://code.claude.com/docs/en/permissions (sections "Compound commands", "Wrappers", "Redirections"), checked 2026-09-26. cox (T36.1, T36.2) follows the split, the any/every rule, the substitution and parse-error cases and the per-command session grant, and (T36.2) now strips the same wrapper list before a deny/ask match and keeps a narrow known-safe assignment allow-list (`LC_ALL`, `LANG`, `TZ`, `NO_COLOR` — see row 39, the docs name no full list to match); cox still differs in being stricter: an output redirect to a path asks instead of consulting `Edit` rules, and (T36.2) a deny/ask rule now looks inside an `eval`/`sh -c`/`bash -c` string by re-parsing it, which Claude Code's own rules do not do (row 39) |
| 39 | Re-checking the "Wrappers" section for T36.2 (`plan.md` A64): the doc's exact wording is "Claude Code also strips a leading assignment of certain known-safe environment variables, so `Bash(npm test *)` matches `NODE_ENV=test npm test`. An allow rule won't match past an assignment of any other variable." — `NODE_ENV` is the only example given; the full known-safe list is not published, so cox does not try to match it and instead keeps its own short, deliberately narrower list (locale/display variables only, which cannot change what a later command resolves to). The doc also confirms: `command -v` (a query, not a run) and zsh's `nocorrect` are *not* stripped even though they look like wrapper forms; bare `xargs` is stripped only when it carries no flag of its own (`xargs -n1 …` is matched as `xargs`, not the inner command); and exec wrappers `watch`, `setsid`, `ionice`, `flock` are never stripped, so they always prompt in Manual mode. Its own worked table shows a `deny`/`ask` rule for `rm *` stopping `rm -rf build/` and `/bin/rm -rf build/` but explicitly *not* stopping `bash -c 'rm -rf build/'` — Claude Code's compound-command split does not look inside a shell string, unlike cox's `sh -c`/`eval` re-parse (T36.2) | confirmed | https://code.claude.com/docs/en/permissions (sections "Wrappers", "What a Bash rule doesn't match"), checked 2026-09-26 |
| 40 | Gemini through Chat Completions (P39, T39.5): the OpenAI-compatible endpoint is `https://generativelanguage.googleapis.com/v1beta/openai/` with `Authorization: Bearer $GEMINI_API_KEY`; streaming, tools, `image_url` data URIs and `reasoning_effort` are supported; reasoning cannot be turned off for Gemini 2.5 Pro or Gemini 3 models, so every `[providers.gemini]` model declares `reasoning_effort = true`; Google says OpenAI-library support is still in beta; thought signatures must be sent back exactly as received. models.dev lists the vendor as provider `google` with env `GEMINI_API_KEY`, hence `"gemini": "google"` in `cox_vendor/models.py`. The model ids `gemini-3.8-flash`, `gemini-3.1-pro-preview` and `gemini-3.5-flash-lite` come from the card; their context windows, efforts and prices are filled by `cox-vendor models`, not by hand | confirmed in the P39 gate check; not re-read in T39.5 (no network) | https://ai.google.dev/gemini-api/docs/openai (page "Last updated 2026-09-02 UTC"), https://ai.google.dev/gemini-api/docs/thinking (last updated 2026-09-25), both checked 2026-09-28 (`plan.md` P39); https://models.dev/api.json checked 2026-09-28 |
| 41 | "Windows App SDK latest stable: v1.8.12 (2026-09-24)" (A127 brief) | corrected | v1.8.12 is the last patch of the 1.8 line, whose servicing ended on 2026-09-24 (Maintenance); the current stable line is 2.x, latest 2.5.1 (2026-09-16, servicing to 2027-04-29) — https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/release-channels (ms.date 2026-09-24, checked 2026-09-29); https://github.com/microsoft/WindowsAppSDK/releases; NuGet `microsoft.windowsappsdk` stable versions end at 2.5.1 (R10.2.5) |
| 42 | D7 "Windows: no sandbox, loud warning, `on-request` forced" is implemented | refuted (not implemented) | `cox_sandbox::sandbox::backend` returns `None` off macOS/Linux and its doc comment says the surface "turns `None` into a security notice and forces `on-request`", but no caller does: the callers are `cox-session` doctor, external agents, MCP and plugins (which refuse to run unsandboxed), and the TUI status line; no code raises the approval policy (repo at `efd69625`, checked 2026-09-29; R10.1.9) |
| 43 | uniffi-bindgen-cs supports the uniffi 0.32 that cox-ffi pins | refuted | latest release v0.11.0+v0.31.0 (2026-06-23) and `main` pin uniffi 0.31.0; the 0.32 upgrade is open PR #176 (R10.2.1, R10.2.2) |
| 44 | Git Bash is required for Claude Code on native Windows | outdated | Git for Windows is optional; without it Claude Code runs commands through its PowerShell tool — https://code.claude.com/docs/en/setup "Set up on Windows" (checked 2026-09-29; R10.5.2) |

## 9. Desktop clients and the Rust↔Swift stack — survey 2026-09-28 (input for `docs/design/desktop.md`)

Scope: what a native macOS client for cox must match and where it can win, and which toolchain lets a SwiftUI app use cox crates as a library. Checked 2026-09-28. Rows marked **unverified** rest only on secondary sources (press, blogs, aggregators) or on a vendor page this survey could not fetch directly; do not cite them outward before re-checking the primary page. Cited below as R9.n.

### 9.1 Desktop and GUI coding-agent clients

| # | Product | Stack | What it ships (UX model, parallelism, review, approvals) | Source |
|---|---|---|---|---|
| 9.1.1 | Claude Code desktop ("Code" tab) | Electron (**unverified** — commentary, no Anthropic engineering statement found) | Sessions sidebar; git-worktree isolation per session at `<project>/.claude/worktrees/`, `.worktreeinclude`; diff viewer with line comments and a "Review code" pass; tabbed browser pane the agent drives to verify its work; terminal pane; cloud and SSH sessions; permission modes Manual / Accept edits / Plan / Auto / Bypass; context and plan usage ring; cross-session messaging | https://code.claude.com/docs/en/desktop ; stack lead: https://www.dbreunig.com/2026/02/21/why-is-claude-an-electron-app.html |
| 9.1.2 | OpenAI Codex app (macOS first) | not documented in the pages read | Threads per project; built-in worktrees; inline diff comments or open in own editor; skills; scheduled automations (local, run only while the laptop is on); review pass before a PR; cloud tasks | https://openai.com/index/introducing-the-codex-app/ ; https://developers.openai.com/codex/app/worktrees |
| 9.1.3 | Cursor 3.0 (2026-04-02) | VS Code fork (Electron) | "Agents Window": many agents across local, worktree, cloud and SSH; `/worktree`, `/best-of-n` (one task, several models, compared diffs); Design Mode (point at UI in a browser view); agent tabs in a grid; plans as files. **unverified** — cursor.com returned HTTP 403, facts come from cached quotes | https://cursor.com/changelog/3-0 |
| 9.1.4 | Zed agent panel | native Rust, GPUI | Threads sidebar; ACP external agents (Claude Code, Codex, Gemini CLI) render into the same UI; `AgentDiffPane` multi-buffer review with accept/reject per hunk; per-tool allow/deny/confirm profiles | https://zed.dev/docs/ai/agent-panel ; https://zed.dev/docs/ai/external-agents ; https://zed.dev/acp |
| 9.1.5 | Devin Desktop (ex-Windsurf, renamed 2026-06-02) | VS Code fork | "Agent Command Center" board/list of sessions, PRs, waiting tasks; "Spaces" share context and worktrees | https://cognition.com/blog/introducing-devin-desktop ; https://devin.ai/pricing |
| 9.1.6 | Google Antigravity 2.0 (I/O 2026-05-19) | VS Code fork | Manager view with up to 5 parallel agents; Artifacts (plans, screenshots, browser recordings) as the unit of review, with feedback attached to the artifact; scheduled background tasks | https://developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform/ ; https://antigravity.google/product/antigravity-ide/ ; https://antigravity.google/blog/google-io-2026 |
| 9.1.7 | JetBrains Air (2026-09-22) | IntelliJ platform, Fleet lineage (**unverified** framing) | One dashboard running Codex, Claude Agent, Gemini CLI and Junie at once, more over ACP; tasks local, in Docker or in worktrees | https://www.jetbrains.com/air/ides/ ; lead: https://www.theregister.com/software/2026/03/10/jetbrains-air-agentic-ide-built-on-abandoned-fleet/ |
| 9.1.8 | Warp | native Rust, GPU terminal; source public (MIT/AGPLv3) | Terminal-native agent mode; Claude Code, Codex, Gemini CLI, OpenCode as first-class agents; per-command approval, directory-scoped autonomy, secret redaction; "Oz" cloud orchestrator | https://www.warp.dev/ai ; https://www.warp.dev/agents |
| 9.1.9 | Kiro (AWS) | Code OSS fork | Spec-driven: `requirements.md` → `design.md` → `tasks.md` committed before code. GA date and pricing **unverified** | lead: https://www.developersdigest.tech/blog/aws-kiro-developer-guide-2026 |
| 9.1.10 | Goose desktop (Block, AAIF) | Electron over a Rust core | Chat UI, 70+ MCP extensions, many providers; already in R§1 row "Goose" | https://github.com/block/goose |
| 9.1.11 | Conductor | "native macOS app" per vendor; stack not documented | Many Claude Code / Codex sessions, each in its own worktree with branch, terminal, diff and review; bring-your-own subscription | https://docs.conductor.build/ |
| 9.1.12 | Nimbalyst (ex-Crystal, 2026-02) | desktop macOS/Windows/Linux, MIT | Parallel agents in worktrees; visual workspace (markdown, mockups, diagrams); task tracking; mobile companion | https://github.com/nimbalyst/nimbalyst ; https://github.com/stravu/crystal |
| 9.1.13 | Sculptor (Imbue) | open source; Apple Silicon macOS + Linux | One container per agent (moved off plain worktrees); Pairing Mode syncs an agent's live work into the user's IDE; "CI Babysitter" | https://imbue.com/blog/sculptor-announce ; https://github.com/imbue-ai/sculptor |
| 9.1.14 | Factory desktop | native desktop app, framework not documented | Sidebar of concurrent Droid sessions on persistent machines; inline preview of outputs with point-and-comment feedback; BYOK incl. Ollama | https://factory.ai/news/working-with-droid-in-the-desktop-app |
| 9.1.15 | Xcode 26.3 (2026-02) | native | Agentic coding with the Claude Agent SDK and Codex inside Xcode; Xcode exposes its own capabilities over MCP | https://www.apple.com/newsroom/2026/02/xcode-26-point-3-unlocks-the-power-of-agentic-coding/ |

Not desktop GUIs (checked, excluded from the design's comparison): GitHub Copilot agent mode and coding agent (IDE extension + cloud PRs, https://docs.github.com/copilot/concepts/agents/coding-agent/about-coding-agent), Jules (cloud web app, https://jules.google/), Amp, Cline (editor extensions). Roo Code archived 2026-05-15 (**unverified** date).

### 9.2 What the field converged on, where it is weak

- **Table stakes (≥ 4 clients):** a sidebar or board of concurrent sessions (9.1.1–9.1.7, 9.1.11, 9.1.14); worktree or container isolation per session (9.1.1–9.1.3, 9.1.5, 9.1.7, 9.1.11–9.1.13); diff review with per-file or per-hunk accept and line comments (9.1.1–9.1.4); a plan / read-only mode (9.1.1, 9.1.3, 9.1.9); MCP for tools (9.1.1, 9.1.4, 9.1.8, 9.1.10, 9.1.15); cloud or remote continuation (9.1.1–9.1.3, 9.1.6, 9.1.8).
- **Differentiators worth taking:** an agent-driven browser that verifies UI work (9.1.1, 9.1.3 Design Mode); ACP host that renders any agent in one native UI (9.1.4, 9.1.7); artifacts as the unit of review (9.1.6); spec files committed before code (9.1.9); comments on a rendered output rather than prose (9.1.14); best-of-n across models (9.1.3).
- **Stack:** of the fifteen clients, only Zed and Warp (Rust, own GPU UI) and Xcode are documented as native; the rest are Electron or VS Code forks, or undocumented. No surveyed agent client is documented as SwiftUI. Reference native macOS apps: Ghostty (Swift/AppKit shell over a Zig core, https://ghostty.org/docs/about), Tower (https://www.git-tower.com/blog/developing-for-the-desktop-tower), Nova (https://nova.app/).
- **Complaints (all leads, unverified):** approval fatigue — reflexive approval of nearly every prompt (https://www.developersdigest.tech/blog/approval-fatigue-agent-security-bug); stale worktrees filling disks, one report of 256 worktrees / 28 GB (https://www.gitkraken.com/blog/every-ai-agent-you-add-leaves-something-behind-to-clean-up); Electron/fork memory overhead (https://www.morphllm.com/comparisons/cursor-vs-vscode); billing-model churn (https://www.nxcode.io/resources/news/cursor-ai-pricing-plans-guide-2026). Already confirmed for Codex in R§1: "Desktop SIGKILL on update" (openai/codex#30359).

### 9.3 Rust↔Swift toolchain

| # | Fact | Source (checked 2026-09-28) |
|---|---|---|
| 9.3.1 | UniFFI newest 0.32.2, published 2026-09-23; proc-macro `#[uniffi::export]` is the maintained path, UDL legacy | https://crates.io/crates/uniffi ; https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md |
| 9.3.2 | UniFFI maps Rust `async fn` to Swift `async` with `async_runtime = "tokio"`; one lazily created global runtime serves every such future | https://mozilla.github.io/uniffi-rs/latest/swift/overview.html ; https://mozilla.github.io/uniffi-rs/latest/internals/async-overview.html |
| 9.3.3 | Foreign (Swift-implemented) traits, including async methods, are supported | https://mozilla.github.io/uniffi-rs/latest/foreign_traits.html |
| 9.3.4 | Swift 6 `Sendable` support in generated code is partial | https://github.com/mozilla/uniffi-rs/issues/2633 |
| 9.3.5 | swift-bridge newest 0.1.59, 2026-01-06 — no release in 8 months, no async foreign-trait story; BoltFFI is pre-1.0 and its speed claims are unbenchmarked (**unverified**) | https://crates.io/crates/swift-bridge ; https://github.com/boltffi/boltffi |
| 9.3.6 | Production pattern: Rust → UniFFI → prebuilt versioned XCFramework consumed as an SPM `binaryTarget`: Element X (`matrix-rust-components-swift`), Bitwarden (`sdk-internal/crates/bitwarden-uniffi/swift`), Firefox (`application-services` megazord) | https://github.com/element-hq/matrix-rust-components-swift ; https://github.com/bitwarden/sdk-internal/tree/main/crates/bitwarden-uniffi/swift ; https://github.com/mozilla/application-services/blob/main/megazords/ios-rust/build-xcframework.sh |
| 9.3.7 | Pitfall: an XCFramework built without an explicit macOS deployment target inherits the build machine's version | https://github.com/matrix-org/matrix-rust-sdk/issues/6750 |
| 9.3.8 | matrix-rust-sdk's timeline streams list changes as diffs (`VectorDiff`) from the `eyeball-im` crate — the model for a Rust-owned list with a Swift renderer. eyeball-im newest 0.9.1, 2026-09-14 | https://crates.io/crates/eyeball-im ; https://codeberg.org/jplatte/eyeball |
| 9.3.9 | Swift `AsyncStream` backpressure: SE-0406 | https://github.com/apple/swift-evolution/blob/main/proposals/0406-async-stream-backpressure.md |
| 9.3.10 | Xcode 26 new app targets default to `SWIFT_DEFAULT_ACTOR_ISOLATION=MainActor` and approachable concurrency (**unverified** — secondary summaries of the Swift 6.2 notes) | lead: https://www.massicotte.org/blog/mainactor-by-default/ |
| 9.3.11 | macOS 26 SwiftUI: Liquid Glass, floating `NavigationSplitView` sidebar, `.backgroundExtensionEffect()`; WWDC25 adds SwiftUI `WebView`/`WebPage`, `TextEditor` over `AttributedString`, a SwiftUI Instruments template | https://developer.apple.com/videos/play/wwdc2025/256/ ; https://developer.apple.com/videos/play/wwdc2025/323/ ; https://developer.apple.com/videos/play/wwdc2025/280/ |
| 9.3.12 | WWDC26 (macOS 27): `@State` becomes a macro (source-breaking for defaults set in `init`), `.reorderable()`, toolbar overflow APIs, revised Liquid Glass | https://developer.apple.com/videos/play/wwdc2026/269/ |
| 9.3.13 | MarkdownUI is in maintenance mode, successor Textual 0.5.0 (2026-06-15); swift-markdown is a parser only | https://github.com/gonzalezreal/swift-markdown-ui ; https://swiftpackageindex.com/gonzalezreal/textual ; https://github.com/swiftlang/swift-markdown |
| 9.3.14 | CodeEditSourceEditor states it is not ready for production; STTextView (TextKit 2) is the maintained AppKit text view | https://github.com/CodeEditApp/CodeEditSourceEditor ; https://github.com/krzyzanowskim/STTextView |
| 9.3.15 | SwiftTerm: pure-Swift VT100/xterm emulator used in shipped apps; libghostty embeddable only through young community wrappers | https://github.com/migueldeicaza/SwiftTerm ; https://github.com/termio-sh/libghostty-swift |
| 9.3.16 | Hardened Runtime is required for notarization; App Sandbox is optional outside the Mac App Store | https://developer.apple.com/documentation/security/hardened-runtime ; lead: https://lapcatsoftware.com/articles/hardened-runtime-sandboxing.html |
| 9.3.17 | Sparkle 2.10.0, MIT, EdDSA-signed appcasts | https://sparkle-project.org/ ; https://github.com/sparkle-project/Sparkle |
| 9.3.18 | `ASWebAuthenticationSession` for OAuth; macOS pitfall: `start()` returns true with no UI when the presentation anchor is wrong | https://developer.apple.com/documentation/authenticationservices/aswebauthenticationsession ; https://developer.apple.com/forums/thread/808304 |
| 9.3.19 | swift-snapshot-testing supports Swift Testing | https://github.com/pointfreeco/swift-snapshot-testing |

### 9.4 cox facts the desktop design rests on (verified in code 2026-09-28)

| # | Fact | Where |
|---|---|---|
| 9.4.1 | `Submission` has 15 variants and `Event` 23; `docs/design/protocol.md` still says 9 and 19 | `crates/cox-protocol/src/types.rs:840`, `:949` |
| 9.4.2 | `Session::events()` is a bounded channel of 256 and `emit` awaits `send`: a consumer that stops draining stalls the core | `crates/cox-core/src/session.rs:409`, `:536`, `:574` |
| 9.4.3 | `submit(UserTurn)` runs the whole turn; approvals and interrupts go through another clone | `crates/cox-core/src/session.rs:718-740`; `crates/cox/src/run.rs:310` |
| 9.4.4 | `UserTurn.attachments` is ignored by the core | `crates/cox-core/src/session.rs:739` |
| 9.4.5 | The todo list reaches surfaces only as text: `ToolResult` has no structured field, TUI and ACP each re-parse it | `crates/cox-tools/src/todo.rs:74-91`; `crates/cox-tui/src/status.rs:450`; `crates/cox-acp/src/map.rs:156` |
| 9.4.6 | `ask_user` bypasses `Event`/`Submission` through an mpsc side channel | `crates/cox-tools/src/ask_user.rs:14-35` |
| 9.4.7 | `SetEffort` and `SetPermissionMode` apply the change but report it only as a `Notice` string | `crates/cox-core/src/session.rs:761-780` |
| 9.4.8 | Session assembly lives in the binary (`session::open`, takes `&Cli`, returns `anyhow`, warns with `eprintln!`); `cox acp` bypasses it and so has no MCP, skills, hooks or checkpoints | `crates/cox/src/session.rs:54-282`; `crates/cox/src/acp_cmd.rs:22-60` |
| 9.4.9 | Instruction files are not in the prompt: `cox-core` sends a one-line stub; `cox_ext::instructions::load` is called only by `cox ext list` | `crates/cox-core/src/context.rs:15-16`, `:131-134`; `crates/cox/src/ext_cmd.rs:105` |
| 9.4.10 | macOS shell sandbox is `sandbox-exec` with a Seatbelt profile, which an App-Sandboxed host cannot nest | `crates/cox-sandbox/src/sandbox/seatbelt.rs:29` |
| 9.4.11 | MCP OAuth uses a loopback `127.0.0.1:0` redirect with a surface-supplied `Prompt` callback; tokens in the keyring | `crates/cox-mcp/src/auth.rs:179`, `crates/cox-mcp/src/client.rs:52-61` |
| 9.4.12 | Plugin UI is a closed serde `Widget` tree (Text, List, Table, KeyValue, Gauge, Stack, Block) with theme `StyleToken`s; only cox-tui draws it | `crates/cox-plugin-api/src/ui.rs:12-112`; `crates/cox-tui/src/plugin_ui.rs` |
| 9.4.13 | `cox-store` already answers a sessions sidebar: `list_sessions`, `session_info`, FTS `rollout_search`, `user_prompts`, `usage_for_session`, `sessions_tree`, `project_totals` | `crates/cox-store/src/fts.rs:105-163`; `crates/cox-store/src/queries.rs` |
| 9.4.14 | No workspace crate builds a `staticlib`/`cdylib`; no uniffi or Swift anywhere | workspace `Cargo.toml` files |

### 9.5 Swift dependencies for the macOS client (checked 2026-09-28, GitHub REST API and each repository's own README/LICENSE)

The creator's rule (A67): take the most used, best-maintained fit; if none fits, write our own as a separate package with its own card.

| # | Need | Candidate | Stars · latest release · last push · licence | Verdict |
| --- | --- | --- | --- | --- |
| 9.5.1 | Updates outside the App Store | https://github.com/sparkle-project/Sparkle | 9 773 · 2.10.0 (2026-09-13) · 2026-09-27 · MIT (API reports NOASSERTION; the `LICENSE` text is MIT) | Take (T37.32) |
| 9.5.2 | Terminal view (M2) | https://github.com/migueldeicaza/SwiftTerm | 1 709 · v1.19.0 (2026-08-18) · 2026-09-27 · MIT | Take in M2; next candidate hbang/NewTerm last pushed 2024-04-01 |
| 9.5.3 | View snapshots | https://github.com/pointfreeco/swift-snapshot-testing | 4 347 · 1.19.6 (2026-09-21) · 2026-09-21 · MIT | Take, tests only (T37.19) |
| 9.5.4 | Lint | https://github.com/realm/SwiftLint + https://github.com/SimplyDanny/SwiftLintPlugins | 19 751 / 85 · 0.65.1 (2026-08-21) both · MIT | Take; SwiftLint's README names SwiftLintPlugins as its build-tool plugin package (T37.18) |
| 9.5.5 | Format | `swift-format` (https://github.com/swiftlang/swift-format, 604.0.0, 2026-09-16, Apache-2.0) | ships in the Xcode toolchain: `xcrun --find swift-format` resolves inside `XcodeDefault.xctoolchain` (Swift 6.4, this machine) | Native, no dependency |
| 9.5.6 | Ordered keyed store | https://github.com/apple/swift-collections | 4 507 · 1.7.1 (2026-09-25) · 2026-09-25 · Apache-2.0 | Take, `OrderedDictionary` for the timeline store (T37.16) |
| 9.5.7 | Global hotkey | https://github.com/sindresorhus/KeyboardShortcuts | 2 716 · 3.1.0 (2026-09-11) · MIT | Not needed in M1; candidate for M2's menu-bar extra |
| 9.5.8 | Keychain | https://github.com/kishikawakatsumi/KeychainAccess | 8 253 · v4.2.2 (2021-03-01) · last push 2024-05-31 | Reject (unmaintained); Security framework directly (T37.30) |
| 9.5.9 | Cross-block selection | SwiftUI `textSelection(_:)` | Apple docs: "apply this method to an individual text view, or to a container to make each contained text view selectable" — each view on its own, not one drag across views | Not enough alone |
| 9.5.10 | Cross-block selection | https://github.com/gonzalezreal/textual | 889 · 0.5.0 · 2026-06-15 · MIT; README: "Native text selection with proper copy-paste support", selection per document | Rejected by spike T37.37 (§9.5.13): no drag across separate block views, copy is plain text + HTML, no clamp |
| 9.5.11 | Cross-block selection | https://github.com/krzyzanowskim/STTextView | 1 595 · 2.4.1 (2026-09-08) · 2026-09-26 · `LICENSE.md`: GPLv3 or a paid commercial licence | Reject: a GPL-only dependency would take the royalty-free option (A68) away from the app |
| 9.5.12 | Markdown view | https://github.com/gonzalezreal/swift-markdown-ui | 3 932 · 2.4.1 (2024-10-13) · description: "Maintenance mode — new development in Textual" | Reject; markdown comes from Rust (`StyledDoc`, T37.7) anyway |

#### 9.5.13 Cross-block selection spike (T37.37, measured 2026-09-28)

Candidates: Textual 0.5.0 (https://github.com/gonzalezreal/textual, tag `0.5.0` = commit `01b51875`, released 2026-06-15, `LICENSE` MIT; its dependencies resolve to swiftui-math 0.1.0 and swift-concurrency-extras 1.4.1, both MIT per the GitHub API; all checked 2026-09-28) in the two shapes it allows — one Textual view per block in a `LazyVStack` (the shape T37.23 needs, tool cards as SwiftUI views) and the whole transcript as one `StructuredText` (its only cross-block shape) — against our own TextKit 2 view: one `NSTextView` over the whole transcript, tool cards as view-backed `NSTextAttachment`s. Same 2 000-block fixture (40 % prose with inline Markdown, 20 % each code, diff, tool card). Code, fixture and the tests that print every number: `desktop/macos/Spikes/Selection/` (`swift test --no-parallel --package-path desktop/macos/Spikes/Selection 2>&1 | grep MEASURE`).

| Measurement (budget) | Textual, one view per block | Textual, one document | Own TextKit 2 view |
| --- | --- | --- | --- |
| One drag, prose → code → prose (blocks 0→2) | **Fails**: selection stays in block 0; each `StructuredText`/`InlineText` installs its own selection model and coordinator (`Sources/Textual/StructuredText/StructuredText.swift:121-122`) | Crosses blocks 0–6; code joins only with `.textual.overflowMode(.wrap)` — the default `.scroll` gives a code block its own selection and drops its text from the document's (`StructuredText/Style/Overflow.swift:73-76`); a tool card can only be Markdown, not a view | **Passes**: blocks [0, 1, 2]; drag diff → tool card → prose selects [3, 4, 5] |
| Copy keeps block order as Markdown | **Fails** (one block only) | **Fails**: order kept, but `copy(_:)` is private and writes plain text and HTML (`Internal/TextInteraction/AppKit/NSTextInteractionView.swift:277-290`); no fences, no `**` | **Passes**: a `diff` fence, then the tool card's `> **bash** …` line, then `Block 5 …`, via `writeSelection(to:types:)` |
| `cross_block_selection = false` clamps to one block | Always clamped; cannot be turned on | **Fails**: `.textual.textSelection(_:)` takes SwiftUI's `TextSelectability` (on/off); the selection model is internal | **Passes**: `setSelectedRanges(_:affinity:stillSelecting:)` clamps to the block the drag started in: 0→2 gives [0], 2→0 gives [2] |
| First frame, 2 000 blocks (inside DT§1's 400 ms cold launch) | 107 ms | **5 517–5 849 ms** | 131–133 ms, of which about 126 ms builds the attributed string from Markdown (the app gets spans from Rust, T37.7); layout + draw 6 ms |
| Scroll, 1 500 frames × 40 pt, 2 000 blocks: p50 / p99 / max, frames > 16.7 ms, hitch ratio (DT§1: ≤ 1 %) | 2.6 / 27.2–27.5 / 33.4–33.7 ms, 166–168, **3.4–3.5 %** | 123–125 / 130–136 / 175–179 ms, 1 500, **640–653 %** | 1.4 / 2.6–2.7 / 8.7–9.2 ms, 0, 0 % |
| Same at 10 000 blocks (DT§1 scroll budget) | 2.7 / 27.5–27.7 / 34.2–34.4 ms, 166, **3.4–3.6 %**; first frame 105 ms | not run (2 000 already misses) | 1.4 / 2.6 / 4.2–11.5 ms, 0, 0 %; first frame **629–633 ms**, 623–627 ms of it building the string |

How it was measured. Swift Testing on an Apple M3 Max (Mac15,9, 64 GB), macOS 27.0, Xcode 27.0 / Swift 6.4, on a shared machine (load average 6–13); two full runs, ranges above. The DT§1 budgets are for an M1 MacBook Air with 8 GB, so these numbers rank the engines; they are not the gate — T37.23 runs the gate with XCTest metrics.
- Automated, headless: a borderless window ordered in at (−20 000, −20 000). Each drag is real `NSEvent`s through `NSWindow.sendEvent` (mouse down, 8 drags, mouse up); both engines handle `mouseDragged` per event rather than in their own tracking loop, so this is the path a hand drag takes. Selection is read from `NSTextView.selectedRange()` and from Textual's internal model (`@testable import`). Copy is read from `writeSelection(to:types:)` into a private pasteboard, and for Textual from the internal `Formatter` its `copy(_:)` uses; the general pasteboard is untouched.
- Frame time is main-thread layout + draw + `CATransaction.flush()` per 40 pt step, timed with `CACurrentMediaTime`; render-server and GPU time and display sync are not in it. The hitch ratio is time over 16.7 ms divided by the frames' budgeted time, a stand-in for `XCTOSSignpostMetric.scrollDecelerationMetric`.
- Not measured headlessly: a hand drag with autoscroll past the viewport edge, trackpad momentum, Liquid Glass compositing, VoiceOver reading order, and memory (the process footprint delta depends on test order: 37–277 MB); these fall to T37.23's UI test and its `XCTMemoryMetric`.
- Risks for the TextKit 2 view: building the whole string at once misses the launch budget at 10 000 blocks, so it must build from Rust spans and append as the timeline grows; TextKit 2 estimates the height of text it has not laid out (139 885 pt for 2 000 blocks after 60 000 pt of scrolling), so the scroller can jump — not measured here.

Verdict: Textual fails the drag in the shape the transcript needs, the Markdown copy and the clamp, and its one cross-block shape misses the first-frame and scroll budgets by more than 10×. The TextKit 2 view passes all four, so it becomes its own package `desktop/macos/Packages/CoxTranscriptText` (T37.37's rule); Textual is not a dependency.

### 9.6 ACP host: the four agents' launch lines and the ACP surface cox consumes (checked 2026-09-29, input for DT§3.3.1)

Scope: what `cox-app` needs to spawn Claude Code's, Codex's, Gemini CLI's and Cursor's ACP agents as top-level sessions (P52, T52.1), and which ACP messages it must fold. Registry rows come from the npm registry API (`https://registry.npmjs.org/<package>`, `dist-tags.latest`, that version's `bin`, `engines`, `dependencies`, `deprecated`); repository rows from the tagged release's own files. The ACP schema rows are read from the crate source `Cargo.lock` resolves (`agent-client-protocol` 2.2.0, `agent-client-protocol-schema` 1.9.1). Cited below as R9.6.n.

#### 9.6.1 Launch facts per agent

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 9.6.1.1 | Claude Code's ACP adapter is npm `@agentclientprotocol/claude-agent-acp`, latest 0.84.0 (published 2026-09-28), bin `claude-agent-acp`, `engines.node` `>=22`, Apache-2.0; it depends on `@anthropic-ai/claude-agent-sdk` 0.3.284 and `@agentclientprotocol/sdk` 1.5.1 | https://registry.npmjs.org/@agentclientprotocol%2Fclaude-agent-acp ; release v0.84.0 at https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v0.84.0 |
| 9.6.1.2 | The older names are deprecated: `@zed-industries/claude-code-acp` (0.16.2) and `@zed-industries/claude-agent-acp` (0.23.1) both carry "This package has been renamed to @agentclientprotocol/claude-agent-acp" | https://registry.npmjs.org/@zed-industries%2Fclaude-code-acp ; https://registry.npmjs.org/@zed-industries%2Fclaude-agent-acp |
| 9.6.1.3 | `claude-agent-acp` with no arguments speaks ACP on stdio (stdout is the protocol; logs go to stderr, or to `$CLAUDE_AGENT_LOGS/agent.log`); `--cli` runs the wrapped Claude Code CLI instead; `--version` prints the adapter version | `src/index.ts` at v0.84.0: https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.84.0/src/index.ts |
| 9.6.1.4 | The Claude Code binary it drives is the Agent SDK's platform-specific optional dependency (or `$CLAUDE_CODE_EXECUTABLE`); nothing else to install | `src/acp-agent.ts` `claudeCliPath()` at v0.84.0: https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.84.0/src/acp-agent.ts |
| 9.6.1.5 | `--hide-claude-auth` hides the claude.ai login method from `initialize` and refuses any turn a claude.ai subscription would pay for; `ANTHROPIC_API_KEY`, an `apiKeyHelper` or a Console-issued key pass | `src/hide-claude-auth.ts` at v0.84.0: https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.84.0/src/hide-claude-auth.ts |
| 9.6.1.6 | Anthropic: "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK." Branding: "Claude Agent" is allowed; "Claude Code" or "Claude Code Agent" is not | https://code.claude.com/docs/en/agent-sdk/overview (Note and "Branding guidelines") |
| 9.6.1.7 | Codex's ACP adapter is npm `@agentclientprotocol/codex-acp`, latest 2.0.0 (published 2026-09-28), bin `codex-acp`, Apache-2.0; it depends on `@openai/codex` `^0.158.0`, so the Codex binary comes with it (`CODEX_PATH` overrides) | https://registry.npmjs.org/@agentclientprotocol%2Fcodex-acp ; README at https://github.com/agentclientprotocol/codex-acp (release v2.0.0) |
| 9.6.1.8 | `@zed-industries/codex-acp` (0.16.0) is deprecated: "replaced by @agentclientprotocol/codex-acp" | https://registry.npmjs.org/@zed-industries%2Fcodex-acp |
| 9.6.1.9 | `codex-acp` is "a stdio ACP agent server"; install `npm install -g @agentclientprotocol/codex-acp` or run `npx -y @agentclientprotocol/codex-acp`; API-key auth reads `CODEX_API_KEY`, then `OPENAI_API_KEY`; ChatGPT login is a browser method hidden by `NO_BROWSER=1`; `INITIAL_AGENT_MODE` is one of `read-only`, `workspace-write`, `agent`, `agent-full-access` | README "Installation", "Authentication", "Runtime options": https://github.com/agentclientprotocol/codex-acp#readme |
| 9.6.1.10 | Gemini CLI is npm `@google/gemini-cli`, latest 0.61.0 (published 2026-09-24), bin `gemini`, `engines.node` `>=20`; also `brew install gemini-cli` | https://registry.npmjs.org/@google%2Fgemini-cli ; README at v0.61.0: https://github.com/google-gemini/gemini-cli/blob/v0.61.0/README.md |
| 9.6.1.11 | Gemini CLI's ACP mode is `gemini --acp` (JSON-RPC 2.0 over stdio); it lists `loadSession`, `setSessionMode` and `unstable_setSessionModel`, and a file-system proxy through the client | `docs/cli/acp-mode.md` at v0.61.0: https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/cli/acp-mode.md |
| 9.6.1.12 | Gemini CLI's headless auth is the `GEMINI_API_KEY` environment variable (or Vertex AI) | `docs/get-started/authentication.mdx` at v0.61.0: https://github.com/google-gemini/gemini-cli/blob/v0.61.0/docs/get-started/authentication.mdx |
| 9.6.1.13 | Cursor's ACP mode is `agent acp` (protocol version 1); auth by `agent login`, `--api-key`/`CURSOR_API_KEY` or `--auth-token`/`CURSOR_AUTH_TOKEN`; methods `session/new`, `session/load`, `session/prompt`, `session/request_permission`, `session/cancel`; modes `agent`, `plan`, `ask` | https://cursor.com/docs/cli/acp (re-checked; R§4.3.8 first checked it 2026-09-26) |
| 9.6.1.14 | Cursor CLI installs with `curl https://cursor.com/install -fsS \| bash` (not npm), binary `agent`, "will try to auto-update by default" | https://cursor.com/docs/cli/installation |
| 9.6.1.15 | cox's Cursor plugin declares `command = "agent"`, `args = ["acp"]`, `mode = "acp"`, `key_env = "CURSOR_API_KEY"` | `plugins/cursor/plugin.toml` (T35.6, T35.13) |

#### 9.6.2 The ACP surface cox folds (crate source, `agent-client-protocol-schema` 1.9.1)

| # | Fact | Where |
|---|---|---|
| 9.6.2.1 | Stable `session/update` kinds: `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update`, `usage_update`. `plan_update`, `plan_removed`, `notice`, `compaction_update` and `compaction_summary_chunk` exist only behind `unstable_*` features; the enum is `#[non_exhaustive]` | `src/v1/client.rs:99-169`; https://crates.io/crates/agent-client-protocol-schema/1.9.1 ; https://agentclientprotocol.com/protocol/prompt-turn |
| 9.6.2.2 | `usage_update` carries `used` (tokens in context), `size` (window) and an optional cumulative `cost { amount, currency }`; per-turn `PromptResponse.usage` is unstable (`unstable_end_turn_token_usage`) | `src/v1/client.rs:609-629`, `:669-673`; `src/v1/agent.rs:3114-3137` |
| 9.6.2.3 | A prompt ends with `stopReason` `end_turn`, `max_tokens`, `max_turn_requests`, `refusal` or `cancelled` | `src/v1/agent.rs:3182-3201` |
| 9.6.2.4 | Tool-call content is `content`, `diff` or `terminal` | `src/v1/tool_call.rs:546-557` |
| 9.6.2.5 | Agent methods include `session/load` (only when `agentCapabilities.loadSession`), `session/set_mode`, `session/set_config_option`; `session/fork` is behind `unstable_session_fork` | `src/v1/agent.rs:3813-3818`, `:4766-4787`; crate `Cargo.toml` `[features]` |
| 9.6.2.6 | `agent-client-protocol` 2.2.0 has `default = []`, and cox enables no `unstable*` feature, so cox's client sees only the stable kinds in 9.6.2.1 | `agent-client-protocol-2.2.0/Cargo.toml` `[features]`; `Cargo.toml:145`; `crates/cox-acp/Cargo.toml:21`; `Cargo.lock:32-33` |
| 9.6.2.7 | cox's client already advertises `fs.readTextFile` and `fs.writeTextFile`, and `terminal` only when sandboxed; the subagent driver spawns one process and one `session/new` per turn and answers every `Ask` with a refusal (`RefuseAsk`) | `crates/cox-acp/src/client.rs:84-90`; `crates/cox-session/src/external_agents.rs:274-340` |
| 9.6.2.8 | The host wrap takes `[sandbox] network`, whose default is `false`, from the session config | `crates/cox-session/src/sandbox.rs:13-21`; `crates/cox-protocol/src/config.rs:858-872` |

#### 9.6.3 Unverified

- Each agent keeps state under the user's home: `~/.claude`, `~/.codex`, `~/.gemini`, `~/.cursor`. The sandbox denies writes outside the workspace (T35.2's check), so an agent may fail to save its session or login state there. **Unverified**: no vendor page or live run was checked for this. T52.4's live check settles it.
- An unknown `sessionUpdate` tag (a newer agent's kind, or an unstable one cox does not enable) may fail deserialization in the SDK instead of arriving as an unknown value. **Unverified**: T52.3's fixture test settles it.

## 10. Windows core build and desktop client — survey 2026-09-29 (input for P57, P58, A127)

Scope: what stops the workspace from building on Windows today, and the stack the creator chose for the Windows desktop client (A127: WinUI 3 + C# over `cox-ffi`, bindings from uniffi-bindgen-cs, M1 parity, no sandbox per D7). Repository rows are read at `efd69625`. Registry rows come from crates.io, the NuGet flat container (`https://api.nuget.org/v3-flatcontainer/<id>/index.json`) and the .NET release index. Cited below as R10.n.

### 10.1 cox on Windows today (repository at `efd69625`)

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.1.1 | CI drops Windows from the shared matrix on purpose: "cox is Unix-only (a pty through nix, std::os::unix in cox-tools; no Windows release target)"; the shared workflow already has a `windows-latest` / `x86_64-pc-windows-msvc` row | `.github/workflows/ci.yml:34`; `pyrlyn/ci/.github/workflows/ci-rust.yml@189816ac` line 128 |
| 10.1.2 | cargo-dist 0.32.0 builds four targets, none for Windows | `dist-workspace.toml:13` |
| 10.1.3 | `nix` 0.31 is an unconditional dependency of `cox-app`, `cox-ext`, `cox-session` and `cox-tools`; only `cox-sandbox` already gates it (`cfg(target_os = "linux")`, with landlock and seccompiler) | `Cargo.toml:107`; each crate's `Cargo.toml` |
| 10.1.4 | `bash` runs on a pty from `nix::pty::openpty`, starts the child with `setsid`, kills with `killpg`, and polls and sets termios through nix; it imports `std::os::unix::process::{CommandExt, ExitStatusExt}` | `crates/cox-tools/src/bash/mod.rs` |
| 10.1.5 | The shell is looked up only in `SHELL_DIRS` (`/bin`, `/usr/bin`, `/usr/local/bin`, `/opt/homebrew/bin`), not on `PATH`; the `Shell` enum already has a `Pwsh` variant; the risk classifier parses commands with the tree-sitter bash grammar | `crates/cox-tools/src/bash/mod.rs:92`; `crates/cox-tools/src/bash/classify.rs` |
| 10.1.6 | Process groups (`process_group(0)`, `killpg`) are the kill path in hooks, the login-shell probe, the lsp server, external agents, the app terminal and the status line | `crates/cox-ext/src/hooks.rs:163-183`; `crates/cox-session/src/env.rs`; `crates/cox-tools/src/lsp/server.rs:70`; `crates/cox-session/src/external_agents.rs:199-213`; `crates/cox-app/src/terminal.rs`; `crates/cox/src/status_line.rs:97-113` |
| 10.1.7 | The login-shell environment defaults to `/bin/zsh` (macOS) or `/bin/sh` and reads the output with `OsStringExt` | `crates/cox-session/src/env.rs:27-29` |
| 10.1.8 | `cox_home()` is `COX_HOME`, else `home_dir()/.cox`; `home_dir()` reads `HOME`, then `USERPROFILE`, then `.` | `crates/cox-config/src/load.rs:61-68` |
| 10.1.9 | `cox_sandbox::sandbox::backend` returns `None` off macOS and Linux; its doc says the session surface turns `None` into a notice and forces `on-request`, but no caller does: doctor reports it, the status line errors (`StatusLineError::NoSandbox`), external agents, MCP and plugins refuse to start unsandboxed, and nothing raises the approval policy | `crates/cox-sandbox/src/sandbox/mod.rs:51`; callers of `backend(` across `crates/` |
| 10.1.10 | Already portable: the session lock uses std `File::try_lock`; `cox-mcp` has Windows branches for opening the browser and the OAuth callback; `self_update.rs:116` and a dozen tests use `PermissionsExt` or `symlink` | `crates/cox-store/src/lock.rs`; `crates/cox-mcp/src/auth.rs:265`, `elicit.rs:658`; `crates/cox/src/self_update.rs:116` |
| 10.1.11 | Workspace dependencies that already have a Windows backend: keyring 4 (4.2.0 in `Cargo.lock`), portable-pty 0.9 (0.9.0), directories 6 | `Cargo.toml:119-120,186`; `Cargo.lock` |

### 10.2 Bindings, runtime and SDK versions

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.2.1 | uniffi-bindgen-cs (NordSecurity, MPL-2.0, not archived): latest release `v0.11.0+v0.31.0` (2026-06-23); its root `Cargo.toml` and `main` (e10ce410eb, 2026-06-23) pin uniffi 0.31.0; install is `cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.0+v0.31.0` | https://github.com/NordSecurity/uniffi-bindgen-cs/releases/tag/v0.11.0%2Bv0.31.0 ; repository `Cargo.toml` and README |
| 10.2.2 | The upgrade to uniffi 0.32 is open PR #176 (opened 2026-07-10, last updated 2026-09-04, mergeable); issue #183 "Update for 0.32 uniffi" (2026-08-28) is open. cox-ffi pins uniffi 0.32.2 | https://github.com/NordSecurity/uniffi-bindgen-cs/pull/176 ; https://github.com/NordSecurity/uniffi-bindgen-cs/issues/183 ; `Cargo.lock` |
| 10.2.3 | Issue #165 (open, 2026-02-24): an async callback interface generates synchronous return types instead of `Task<T>`; fix PR #166 is open. cox-ffi's foreign trait `AppHost` has async methods (`browser_load`, `browser_text`, `browser_snapshot`) | https://github.com/NordSecurity/uniffi-bindgen-cs/issues/165 ; https://github.com/NordSecurity/uniffi-bindgen-cs/pull/166 ; `crates/cox-ffi/src/host.rs` |
| 10.2.4 | Generated C# needs .NET 8+ (or net461) and `AllowUnsafeBlocks`; v0.11.0 generates PascalCase record properties, `LibraryImport` on .NET 8+, and async methods on records and enums | README and `CHANGELOG.md` at `v0.11.0+v0.31.0` |
| 10.2.5 | Windows App SDK: the 1.8 line is in Maintenance and its servicing ended 2026-09-24 (last patch 1.8.12); the current stable line is 2.x, 2.0 released 2026-04-29 and serviced to 2027-04-29, latest 2.5.1 (2026-09-16) | https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/release-channels (ms.date 2026-09-24); https://github.com/microsoft/WindowsAppSDK/releases ; `https://api.nuget.org/v3-flatcontainer/microsoft.windowsappsdk/index.json` |
| 10.2.6 | .NET 10 is LTS, latest 10.0.12 (2026-09-08), end of support 2028-11-14; .NET 11 is STS at RC1; .NET 8 LTS ends 2026-11-10 | https://dotnetcli.blob.core.windows.net/dotnet/release-metadata/releases-index.json |

### 10.3 Packaging, notifications, Mica

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.3.1 | WinUI 3 apps are packaged (MSIX) by default; package identity is needed for background tasks, push notifications, share targets and more; "packaged with external location" gives identity while keeping your own installer; unpackaged apps have no identity | https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/packaging/ (ms.date 2026-08-29) |
| 10.3.2 | MSIX apps typically run in a lightweight app container with file-system and registry virtualization, which the manifest can turn off | same page |
| 10.3.3 | Deployment is self-contained or framework-dependent; `PublishSingleFile` works only for unpackaged + self-contained; `dotnet publish -r win-x64` or `win-arm64 --self-contained` | https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/deploy-overview (ms.date 2026-09-10) |
| 10.3.4 | App notifications: `AppNotificationBuilder` with `AddButton(new AppNotificationButton(..).AddArgument(..))`, the `AppNotificationManager.Default.NotificationInvoked` event and `Register()`; not supported when the app runs elevated | https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/app-notifications-quickstart (ms.date 2026-09-10) |
| 10.3.5 | For unpackaged apps `Register()` registers the calling process as the COM server and takes the display name and icon from the shell; `Register(displayName, iconUri)` exists since SDK 1.2 | https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.windows.appnotifications.appnotificationmanager.register (updated 2026-07-28) |
| 10.3.6 | Mica needs Windows 11 (build 22000+); older builds get a solid fallback; High Contrast replaces it | https://learn.microsoft.com/en-us/windows/apps/design/style/mica (ms.date 2026-07-22) |

### 10.4 Processes, pty, credentials, paths

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.4.1 | portable-pty 0.9.0 (MIT, 2025-02-11) uses ConPTY on Windows: `#[cfg(windows)] pub type NativePtySystem = win::conpty::ConPtySystem;` | https://crates.io/crates/portable-pty/0.9.0 ; https://github.com/wezterm/wezterm/blob/main/pty/src/lib.rs |
| 10.4.2 | Job objects: child processes join the parent's job by default; `TerminateJobObject` ends every process in the job; `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills them when the last handle closes; nested jobs since Windows 8 | https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects (ms.date 2025-07-14) |
| 10.4.3 | process-wrap 10.0.1 (2026-09-23, Apache-2.0 OR MIT, watchexec) wraps `Command` with features `job-object`, `process-group`, `process-session`, `kill-on-drop`, `tokio1`, `std`; it is not in the workspace `rust.md` | https://crates.io/crates/process-wrap/10.0.1 ; https://github.com/watchexec/process-wrap |
| 10.4.4 | Alternatives: win32job 2.0.3 (2025-05-15, Windows only); windows-sys 0.61.2 / windows 0.62.2 (raw API); command-group 5.0.1 (2023) is superseded by process-wrap | https://crates.io/crates/win32job ; https://crates.io/crates/windows-sys ; https://crates.io/crates/command-group |
| 10.4.5 | keyring 4.2.0 (2026-08-29): the default `v1` feature pulls `windows-native-keyring-store` 1.1.0 (2026-05-24) on `cfg(target_os = "windows")`, the Windows Credential Manager store | https://crates.io/crates/keyring/4.2.0 ; https://crates.io/crates/windows-native-keyring-store/1.1.0 |
| 10.4.6 | Windows file names: case-insensitive names and drive letters; reserved names `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, also with an extension; the `\\?\` prefix turns off parsing (so `..` is not resolved); 8.3 short aliases; alternate data streams (`name:stream`) | https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file (ms.date 2024-08-28) |

### 10.5 The shell other agents use on Windows

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.5.1 | Codex's default user shell on Windows is PowerShell: `pwsh` first, then Windows PowerShell, with `cmd` as the last fallback | https://github.com/openai/codex/blob/c248f6d48b97/codex-rs/shell-command/src/shell_detect.rs (about lines 262-282, 350-352) |
| 10.5.2 | Claude Code on native Windows: Git for Windows is optional; with it the Bash tool runs Git Bash (`CLAUDE_CODE_GIT_BASH_PATH` overrides the path); without it commands run through a PowerShell tool, which also exists next to Bash; native Windows sandboxing is not supported, WSL 2 is | https://code.claude.com/docs/en/setup |

### 10.6 Release and CI

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.6.1 | cargo-dist supports `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, `powershell` and `msi` installers; its Windows code signing (SSL.com, Azure) covers x86_64 only | https://axodotdev.github.io/cargo-dist/book/reference/config.html |
| 10.6.2 | GitHub's `windows-latest` runner is Windows Server 2025 with Visual Studio 2026 | https://github.com/actions/runner-images (README) |

### 10.7 C# test and MVVM packages

| # | Fact | Source (checked 2026-09-29) |
|---|---|---|
| 10.7.1 | Latest stable on NuGet: xunit.v3 4.0.1, Verify.XunitV3 33.1.5, FlaUI.UIA3 5.0.0, MSTest 4.4.1, CommunityToolkit.Mvvm 8.4.2, Microsoft.Windows.CsWinRT 2.3.1 | `https://api.nuget.org/v3-flatcontainer/<id>/index.json` for each id |
| 10.7.2 | Verify (MIT) was last pushed 2026-09-28; FlaUI (MIT, UI Automation over UIA3) was last pushed 2026-08-13, its 5.0.0 package published 2025-02-25 | https://github.com/VerifyTests/Verify ; https://github.com/FlaUI/FlaUI |
| 10.7.3 | WinAppDriver's last release is v1.2.99 (2021-07-01): unmaintained, so it is not a candidate | https://github.com/microsoft/WinAppDriver/releases |

### 10.8 Unverified

- A GUI app started from the Start menu gets the user and system `PATH` from the registry, not a login shell, so `cox-session`'s login-shell probe has no Windows counterpart. **Unverified**: no vendor page was checked; T57.5 settles what the Windows session environment is.
- WinUI 3 controls can be rendered to PNG in a test through `RenderTargetBitmap` for snapshot tests. **Unverified**: not tried; T58.30 is the spike.
- `AppNotificationBuilder` supports a text box input for "deny with reason" from a notification. **Unverified**: not read in the API reference; T58.24 checks it.
- A taskbar badge (count of items waiting) is available to an unpackaged app. **Unverified**: T58.24 checks it with the packaging model T58.28 picks.
- mise and the repository's `.github/actions/rust` composite action run on a Windows runner. **Unverified**: T57.1's first run settles it.
