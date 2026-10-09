# Design: extensions (T7.7)

## Problem

A coding agent is judged by what it can be taught: repository rules, house
skills, review commands, background agents, guard hooks, and whatever tools a
team already runs behind MCP. cox has to reach all of that in v0.1 without
committing to an in-process plugin ABI it would have to keep forever (D1). The
measurable question: is there an extension users need that cannot be expressed
as a markdown file, a subprocess or an MCP server?

## The field

**Claude Code (R§2.1, ledger #12).** The largest extension surface of any agent,
and none of it is in-process: `CLAUDE.md`, `SKILL.md`, `.claude/commands/*.md`,
`.claude/agents/*.md`, hooks (31 events, JSON on stdin/stdout, exit 2 = block),
MCP (stdio, HTTP, OAuth), and "plugins" that are bundles of exactly those files.
The rtok token-saving stack is built entirely on the hook protocol (R§4.1).

**Codex (R§1.2).** `AGENTS.md`, MCP, `apply_patch`. No hooks, no plugin host.

**Gemini CLI, Copilot (R§2.3).** `.agent.md` / `GEMINI.md`, MCP, an LSP bridge.
Copilot's "extensions" are remote services, not code loaded into the CLI.

**Zed, Cursor.** Zed loads WASM extensions (language servers, themes) through
extism-like hosts; Cursor loads nothing in-process. Neither exposes an agent
tool ABI to extensions; both reach agent tools through MCP.

The pattern: every agent that shipped an ecosystem did it with *data and
processes*. The one product that loads code in-process (Zed) does so for a
different problem (editor features), behind a component-model boundary, and
still puts agent tools behind MCP.

## cox

v0.1 adopts the formats verbatim (D4) and reads them from `.claude/` and `.cox/`
twins with the same schemas:

| Extension | File / protocol | cox module | What it can do |
| --- | --- | --- | --- |
| instruction files | `AGENTS.md`, `CLAUDE.md` hierarchy, `@include` | `cox_ext::instructions` (T7.1) | shape the system prompt, within `instruction_budget_tokens` |
| skills | `SKILL.md` frontmatter + body | `cox_ext::skills` + the deferred `skill` tool (T7.2) | load instructions on demand, declare `allowed-tools` |
| commands | `commands/*.md` with `$ARGUMENTS`, `!`cmd``, `@file` | `cox_ext::commands` (T7.3) | expand a prompt; shell and file inclusion go through the caller's `Includes`, never a raw spawn |
| subagent definitions | `agents/*.md` | `cox_ext::agents` (T7.3) | narrow a child's tools and pick its tier; never widen |
| hooks | Claude JSON protocol over `sh -c` | `cox_ext::hooks::ShellHooks` + `cox_core::hooks` (T7.4) | block or rewrite a prompt or tool input, observe results; fail open |
| Claude settings | `.claude/settings.json` | `cox_ext::claude_settings` (T7.5) | permission rules imported read-only; `~/.claude` hooks too; a repository's hook commands are reverted (T64.1) |
| MCP | `.mcp.json`, `[mcp.servers]`, `~/.claude.json` | `cox_mcp::{discovery, client}` (T7.6) | tools as `mcp__<server>__<tool>`, deferred, gated like built-ins |

Three properties hold across the table and are the reason this is enough:

1. **Every extension crosses a trust boundary the core already guards.** An
   extension's output is model-visible text or a tool call; both pass
   `permission::Engine`, `path::confine`, the sandbox and `text::sanitize`
   (AGENTS.md → Trust boundaries). A hook can *lower* what the model may do and
   a subagent file can *narrow* a tool list; nothing in the table can widen a
   permission, because the rules are compiled from config the extension does
   not own.
2. **Every extension fails open (D14).** A malformed skill, a crashing hook, a
   server that will not start, a broken `settings.json` is a `Notice(Warn)` and
   an absence, never a fatal. The tests that prove it are named in `done.md`
   (`broken_hook_is_skipped_not_fatal`, `client_server_crash_does_not_end_session`,
   `skills_malformed_or_misnamed_are_skipped_with_a_notice`).
3. **Nothing is loaded into the process.** The core's contract is `Submission`
   in, `Event` out; extensions are inputs to it. A user with a Claude Code setup
   gets cox for free, and cox owes no ABI to anyone.

What a process boundary costs: a hook pays a fork per event (tens of
milliseconds), an MCP server pays a handshake per session, and neither can see
the context window (why token economy is core, not a plugin — D6). Those are the
same costs Claude Code pays, and its ecosystem grew anyway.

## v0.2: the WASM contract sketch

If the falsifier below fires, the host is extism 1.30 (plan §1.1), not a dylib
(`abi_stable` was rejected for ABI fragility) and not a scripting runtime. The
contract is the existing `Tool` trait, serialised:

```text
guest exports
  spec()                       -> ToolSpec (JSON)            the same struct built-ins return
  subject(input: JSON)         -> String
  risk(input: JSON)            -> Risk
  call(input: JSON, cx: JSON)  -> Result<ToolOutput, ToolError>

host imports (all go through the core's guards)
  read(path)                   confined by cox_tools::path::confine
  archive_put(bytes)           the lossless archive, so `expand <id>` works
  output(line)                 the streamed-output channel
  cancelled() -> bool          the call's CancellationToken
```

A WASM tool would be registered next to MCP tools as `wasm__<plugin>__<tool>`,
deferred by default, with `Risk` from its `spec()` and the same engine in front
of it. It gains nothing an MCP server cannot do today except latency and a
single-file distribution; that is why it waits for evidence.

## Falsifiers

1. **An extension users need that is not markdown, a subprocess or MCP.** The
   concrete test: a feature request that needs to *read or edit the context
   window*, run *inside* a tool call, or hold state across calls faster than a
   stdio round trip. If three such requests arrive that MCP cannot serve, T7.7
   is wrong and the v0.2 host moves up.
2. **Fail-open hides real breakage.** If users report "my hook stopped working"
   and the only trace is a warn-level notice they never saw, the notice level or
   `cox ext` reporting is wrong, not the fail-open rule.
3. **The Claude formats drift.** If Claude Code changes the hook protocol or
   settings schema so that verbatim adoption breaks, `cox-ext` gains a version
   switch; D4 stands until then.

## Review

- `cox ext` on a fixture tree lists instruction files, skills, commands, agents
  and MCP servers with their source (T7.3/T7.6 e2e tests).
- The real binary was run against a scratch `COX_HOME` with a blocking hook
  (T7.4), an imported `settings.json` (T7.5) and a `.mcp.json` pointing at
  `cox mcp` itself (T7.6); each extension took effect and each unreachable one
  was reported and skipped.
- Not verified: an OAuth-protected HTTP server (no `auth.rs` yet), and the
  palette wiring for custom commands (cox-tui cannot depend on cox-ext; tracked
  under P9).
