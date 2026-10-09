# Compatibility notes

Manual smoke checks that no automated test covers. Each entry says what was
run, on what, and when.

## `cox mcp` as an MCP server (T6.2)

Claude Code / any MCP client `.mcp.json` entry:

```json
{ "mcpServers": { "cox": { "command": "cox", "args": ["mcp"] } } }
```

Add `"--allow-write"` to also serve `edit`, `write` and `apply_patch`, or
`"--tools", "bash,read"` to name the exact set (the only way to get `bash`).
Calls go through the permission engine with the approval policy forced to
`never`: anything that would ask in the TUI is denied with the reason in the
tool result. Paths are confined to the workspace roots.

Smoke 2026-09-03, macOS, stdio JSON-RPC by hand: `initialize` reports
`cox 0.1.0` with the tools capability; `tools/list` returns `read`, `grep`,
`glob` by default; `read note.txt` returns the numbered file; `write` is
"unknown tool" without `--allow-write` and a denied result with it under the
default permission mode; `read ../note.txt` is rejected by path confinement.

## Claude Code / Codex compatibility (T12.3)

A checkout configured for Claude Code or Codex works with cox unchanged:

| Their setup | cox reads | Notes |
|---|---|---|
| `AGENTS.md` / `CLAUDE.md` hierarchy | yes | same lookup order |
| `SKILL.md` agent skills | yes | same frontmatter |
| `.claude/settings.json` permissions, hooks, env | yes, read-only | one config layer; never written back. A repository's hook commands are reverted (T64.1); `~/.claude` hooks still run |
| `.claude/commands/*.md`, `.claude/agents/*.md` | yes | same palette, `explore`/`shell` presets ship embedded |
| `.mcp.json` servers | yes | stdio + Streamable HTTP; `${ENV}` expansion; a stdio server runs under the sandbox by default (T33.42) |
| Codex `apply_patch` (V4A) | yes | Add/Update/Delete/Move, `@@` context |
| `--output-format stream-json` | yes | same event shapes for scripts |
| `~/.codex/config.toml` | no | Codex config is not imported |
| MCP OAuth | yes | authorization code + PKCE, token in the keyring, `cox mcp login <name>` |
| MCP elicitation | yes (TUI, `--plain`) | form mode through the question modal; `cox run -p` declares none and declines |
| Worktree isolation | yes | `--worktree <name>` and `agent(isolation: "worktree")`; `_worktrees/<repo>-<name>`, branch `<name>`, locked for its owner |
| MCP resources/prompts, image input | no | deferred to v0.2 |
| MCP Apps (`_meta.ui.resourceUri`) | no | UI resource ignored; a tool's text and structured result are kept unchanged (T55.1, P55 option (a)) |

## Gemini over its OpenAI-compatible endpoint (P39)

`[providers.gemini]` is a built-in type-2 preset: Google's
OpenAI-compatible endpoint
(`https://generativelanguage.googleapis.com/v1beta/openai`, bearer
`GEMINI_API_KEY`), spoken by the shared Chat client. Point a tier at it with
`tiers.code.provider = "gemini"`. Vertex AI is not covered.

- Google says its OpenAI compatibility is still in beta
  (https://ai.google.dev/gemini-api/docs/openai, last updated 2026-09-02).
- Gemini 3 sends a thought signature with each tool call, and it must go
  back exactly as received. cox keeps it in history and returns it as
  `extra_content.google.thought_signature` on that tool call. This field
  path is **unverified**: the page that documented it now only says it has
  moved. `crates/cox/tests/gemini_compat.rs` runs a two-round tool loop
  offline against fixtures in that shape. A live recording (T39.7) will
  confirm or refute it.

## Known leftovers (T22.7)

Every `Not done:` line in `done.md` that no done task closed and no open
`plan.md` §3 card owns, recorded deliberately with one sentence why it stays.
(`scripts/leftovers.sh` re-checks this table against `done.md`; fixing any
row is a card, not this audit.)

| task | leftover | why it stays |
|---|---|---|
| T1.6 | Mid-stream-close test uses a raw `TcpListener` instead of wiremock. | Wiremock cannot observe a client hang-up, so the raw listener is the only harness that can. |
| T2.2 | `allow_for_session_persists` is read but not acted on. | The key is parsed ahead of the session-persistence write path that would consume it. |
| T3.7 | `sandbox.env_passthrough` has no config key; the allowlist is fixed. | Passthrough needs its own config-key design, which P4 never scoped. |
| T3.8 | The `web_fetch` Anthropic server-tool passthrough (`web_fetch_20260209`) is not wired. | It needs `server_tool_use`/`web_fetch_tool_result` blocks in the SSE consumer the provider task did not own. |
| T3.9 | `crates/cox-tools/src/agent.rs` does not exist; the tool lives in `cox-core::subagent`. | The workspace dependency direction forbids `cox-tools` depending on `cox-core`. |
| T4.2 | Landlock cannot carve `.git` out of a writable root; the read-only test skips without bwrap. | Landlock only grants access, so the carve-out needs the bwrap backend. |
| T4.2 | Linux tests were not executed on the macOS authoring host. | `portable-pty`/Landlock runs happen in Linux CI instead. |
| T4.4 | The sandbox design doc was reviewed by its own author in the same session. | An independent second read is a phase-gate luxury, not a merge blocker. |
| T5.3 | No closed-block cache for very long streaming replies. | Finished cells leave the viewport for scrollback, so per-frame work is already bounded. |
| T5.3 | `ToolResult` carries no exit code; `bash` puts it in `visible` instead. | The protocol type has no such field, and duplicating it would split the source of truth. |
| T5.4 | Collapse is per session (`Ctrl+O`), not per file. | With the inline viewport a finished cell is already in scrollback, so there is nothing to select. |
| T5.5 | `/sandbox` forwards as a `Command` for lack of a `Submission` variant. | No `Submission` variant sets the sandbox, so the core routes it through the generic one. |
| T5.5 | `arboard` unused; `Cmd::Copy` was a no-op (now owned by open card T23.4). | Native clipboard crates were out of scope; the terminal OSC 52 path is T23.4. |
| T5.6 | The composer's own text is trusted input and is not sanitised. | Sanitising what the user just typed would mangle their own keystrokes. |
| T5.6 | `truncate` is used for the tool header only; everything else wraps. | Wrapping preserves content that truncation would silently drop. |
| T5.7 | `Esc` during a running turn interrupts rather than entering normal mode. | A running turn needs a stop key more than a mode key (`Ctrl+C` now shares the job). |
| T5.7 | `@`/`/` do not open pickers from normal mode. | Normal mode edits text; the pickers belong to the insert path. |
| T6.1 | `stop` serialises as the protocol's `{"type":"end_turn"}` object, not a bare string. | The stream-json surface follows the protocol shape, not the CLI shorthand. |
| T6.2 | Streamed tool output has no MCP channel (final text only). | The MCP surface ships complete results; streaming is a TUI/headless concern. |
| T6.2 | `--tools` does not validate names; unknown ones are silently absent. | Fail-open extensions skip unknown names instead of refusing to start. |
| T6.2 | MCP smoke was by hand over stdio, not from Claude Code itself. | No automated Claude Code harness exists to drive the server end to end. |
| T6.3 | No `Edit`/`AllowForSession` from the headless driver (approve/deny only). | Stdin approvals stay a two-verb protocol so a stray log line cannot grant anything. |
| T6.3 | The TUI approval fix has no automated test (PTY e2e uses a text-only scenario). | The text-only scenario cannot exercise the modal the fix touches. |
| T7.1 | Token cost stays the bytes/4 heuristic, not a provider count. | The core cannot depend on the provider crate that owns the real estimator. |
| T7.2 | A proprietary `anthropics/skills` sample (`pdf`) was deliberately not vendored. | Vendoring a proprietary sample would license-pollute the repo. |
| T7.3 | No loop test through the core for commands/subagent definitions. | The wiring test would cross the crate boundary the 3-file cap forbade. |
| T7.4 | `additionalContext` is ignored for `SessionStart` (no `HookOutcome` variant). | Carrying hook stdout back needs an outcome variant the hooks task did not own. |
| T7.4 | No rtok fixture for hooks (its hook subcommand contract was unverified). | Vendoring an unverified contract would invent behaviour the tool never promised. |
| T7.5 | `env` passthrough is parsed but dropped (no config key or tool wiring). | The key and its tool wiring were never scoped, so the layer drops it loudly. |
| T7.5 | A list fed by both `.cox` and Claude is labelled by the first layer. | Figment reports provenance per key, not per list element. |
| T7.5 | `prompt`/`agent` hook types are skipped. | The shell-hook runner implements lifecycle events, not prompt/agent injection. |
| T7.6 | `resources/read` (`read mcp://…`) and `prompts/list` are not exposed. | The MCP client surfaces tools first; resources/prompts are deferred to v0.2. |
| T7.6 | No wiremock test for the MCP client (no OAuth to mock then). | There was no auth flow worth a mock before T22.5 landed the real one. |
| T7.6 | No seatbelt/bwrap confinement wraps MCP stdio servers. | Server confinement needs a sandbox design the MCP task did not own. |
| T14.1 | The composer/picker `·` and the `-v` diagnostic markers stay Unicode. | The glyphs are in every font we care about, and the markers are deliberate diagnostics. |
| T14.1 | An override string leaks for the process lifetime. | That is what a config value costs; freeing it buys nothing. |
| T14.1 | No `unicode`/`ascii` snapshot pair with equal line widths. | ASCII `…`→`...` and `📎`→`@` change widths by design, so byte-equality was the wrong assertion. |
| T15.1 | Untracked files are outside the git counts and diff by design. | The status surface shows tracked change; untracked listing is `git status`'s job. |
| T15.1 | Ahead/behind is not collected. | Remote tracking needs a fetch the status path must never trigger. |
| T14.2 | A 16-colour terminal gets a hue-and-brightness approximation, not a CIE-nearest match. | The cheap approximation is indistinguishable at 16 colours; CIE would cost a dependency. |
| T14.2 | `Depth` is not exposed to the `stream-json` or ACP surfaces. | Those surfaces emit no colour, so a depth field would be dead schema. |
| T14.3 | The fold marker splits long tool output into two syntect runs. | The hidden middle lines are the reason the string state cannot carry over. |
| T14.3 | A diff hunk highlights as if its `+`/`-` lines were consecutive source. | That is what a diff shows, not what either file contains. |
| T14.3 | `bash` output stays plain because its subject is a command, not a path. | Extension-based highlighting has nothing to key on. |
| T14.3 | The theme list in the warning is unsorted (syntect's map order). | Sorting a borrowed map's keys would copy it for a warning nobody greps. |
| T18.1 | No PTY end-to-end of TUI resume. | The seeded-transcript unit tests cover the path the PTY run would repeat. |
| T23.2 | The `vt100` PTY fixture drops lines scrolled off a DECSTBM region, so the PTY tests model the save real terminals make; that save is not re-verified per terminal (Terminal.app, Windows Terminal). | `scrolling-regions` relies on it by design (ratatui#1341), and no emulator crate in the tree implements it. |
| T23.7 | A resize that lands halfway through a frame can misplace the rebuilt viewport by the rows that frame's cursor had travelled. | The terminal moves lines at the instant it resizes, and the app cannot learn where a half-written frame left the cursor. |
| T23.7 | tmux pane resizes and reflowing terminals (text rewrapped on a width change) are not reproduced; Linux CI was not run from the macOS host. | The `vt100` fixture truncates instead of reflowing, and tmux is not installed on the authoring host. |
