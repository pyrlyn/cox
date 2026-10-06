# Design: cox for macOS — a native desktop client over the cox crates (A67, phase P37)

Status: **proposal, not approved.** Nothing here is in `plan.md` yet; §12 is the
draft amendment for the creator. Evidence: `research.md` §9 (cited R9.n).
Section references inside this file are DT§n.

## 0. Decisions at a glance

| # | Decision | Why |
| --- | --- | --- |
| DT-1 | macOS only, SwiftUI with AppKit where SwiftUI is too slow, minimum macOS 26, Apple Silicon only (creator, 2026-09-28) | Creator's scope. macOS 26 brings Liquid Glass, SwiftUI `WebView`, `TextEditor` over `AttributedString` (R9.3.11) — no back-deployment shims |
| DT-2 | Rust is linked **in-process** as a static library through **UniFFI** | Only maintained binding with async fn ↔ Swift `async` and Swift-implemented traits, proven by Element X, Bitwarden and Firefox (R9.3.1–9.3.6). An XPC helper or an app-server subprocess adds an IPC hop on the hottest path for no security gain: cox already sandboxes every shell command itself (R9.4.10) |
| DT-3 | **Rust owns the view model.** Swift renders and integrates with the OS; it never decides | "Separate view from business logic" taken literally: folding events into a transcript, costs, approvals, command parsing, markdown parsing and syntax highlighting are Rust, shared with the TUI. Swift gets ready-to-draw blocks and applies keyed patches (the matrix-rust-sdk timeline model, R9.3.8) |
| DT-4 | Three new crates: `cox-session` (assembly), `cox-app` (UI-agnostic app core), `cox-ffi` (UniFFI, the only crate that knows Swift exists) | Session assembly is stuck in the binary with `anyhow`, `&Cli` and `eprintln!` (R9.4.8); the TUI's fold logic is tied to ratatui. Both move to crates every surface can share |
| DT-5 | Same `~/.cox` home, same `cox.db`, same rollouts as the CLI | A session started in the terminal opens in the app and back. No second store |
| DT-6 | Developer ID + notarization + Hardened Runtime, **no App Sandbox**, Sparkle 2 updates, the `cox` CLI bundled inside the app | An App-Sandboxed host cannot nest `sandbox-exec` (R9.4.10) or read arbitrary repositories; Mac App Store is out of scope |
| DT-7 | The app is a sixth consumer of the one `Event` stream (D2) | No new core protocol. The gaps the GUI hits (DT§4.7) are fixed in `cox-protocol` for every surface |

## 1. Problem, goals, falsifiers

**Problem.** cox has four surfaces (TUI, `run -p`, ACP, MCP) and no GUI. Every
serious competitor now ships a desktop client (R9.1), and nearly all of them
are Electron or VS Code forks (R9.2). The creator wants a macOS client that
uses part of cox as a library and is better than the others.

**Goals.**

1. Everything the TUI can do, with no logic re-implemented in Swift.
2. Win on what the field is weak at (R9.2): responsiveness and memory,
   cost you can see, approvals you do not rubber-stamp, work you cannot lose,
   worktrees that do not rot on disk.
3. Keep the core's rules: pure state machine, lossless, cache-stable prefix,
   cost ledger, fail-open extensions.

**Non-goals (v1).** Windows/Linux GUI; Mac App Store; a code editor (the app
opens files in the user's editor); cloud execution; iOS.
Windows is now planned (`plan.md` A127, P57 and P58: M1 parity, WinUI 3 over `cox-ffi`; its design doc comes from T58.2).

**Budgets — the falsifiers.** If the shipped M1 misses any of these on an
M1 MacBook Air with 8 GB, the native-first argument failed and the design is
reopened:

| Metric | Budget | How measured |
| --- | --- | --- |
| Cold launch to an interactive window | ≤ 400 ms | `XCTApplicationLaunchMetric` |
| Idle memory, one open session of 2 000 blocks | ≤ 150 MB RSS | `XCTMemoryMetric` |
| Streaming 200 tokens/s | main thread busy ≤ 25 %, no hitch > 16 ms | signposts + SwiftUI Instruments template |
| Scrolling a 10 000-block transcript | ≤ 1 % hitch time | `XCTOSSignpostMetric.scrollDecelerationMetric` |
| Keypress to glyph in the composer | ≤ 16 ms | Instruments |
| Core never stalls on the UI | a UI that stops reading for 10 s does not delay a turn | Rust test in `cox-app` (DT§4.5) |

## 2. Where cox wins

The field converged (R9.2): session board, worktrees, diff review, plan mode,
MCP. Those are table stakes and are all in M1. The client wins on six things
that already exist in cox's core and that the competitors lack or hide:

| Edge | What the user sees | cox building block |
| --- | --- | --- |
| **Native speed** | Launches instantly, stays small, scrolls a 10k-block session smoothly | SwiftUI/AppKit, Rust in-process (DT-2) |
| **Cost you can see** | Live `$` and cache-hit % per turn, session, project; a budget cap that stops a turn before it overspends | `Event::Usage`, the cost ledger, `budget::decide` |
| **Approvals that carry information** | Every prompt says *why* (the rule or risk), shows the exact grant "allow for session" will add, lets you *edit* the command before it runs; one inbox for all sessions; allow/deny from a notification | `Why`, `why_text`, `grants_for`, `Decision::Edit`, `ApprovalRequired.source` |
| **Nothing is lost** | A truncated output expands in place; every tool call is a checkpoint; a timeline rewinds code, conversation, or both | `ArchiveRef` + `cox expand`, `Checkpoint`, `Submission::Rewind{code, conversation}` |
| **Any model** | Anthropic, OpenAI, OpenRouter, Ollama, LM Studio, vLLM; tier routing visible per turn | `cox-provider`, `cox-models`, `TurnStarted{tier, model}` |
| **Worktrees that clean up** | Each session's worktree with its disk size, merged/stale state and one-click prune | `Worktrees` trait, `GitWorktrees` |

Later (M2/M3) edges from R9.2: an ACP host that renders Claude Agent, Codex and
Cursor (P35) sessions in the same UI (Zed's and JetBrains Air's model), a
browser pane the agent drives, best-of-n across models, plugin panels drawn
natively from the WASM widget tree.

## 3. Feature set

M1 is the first release; M2 and M3 follow in order. Each row names the cox
piece it stands on; "new" means a gap closed in DT§4.7.

### 3.1 M1 — a complete, native daily driver

| Feature | Behaviour | Stands on |
| --- | --- | --- |
| Projects and sessions sidebar | Projects (git roots) with their sessions, newest first; "Needs you" and "Running" sections on top | `list_sessions`, `latest_session_for_cwd`, `project_slugs` |
| Several live sessions at once | Each session runs independently; switching never stops one | one `Session` per handle in `cox-app` |
| Worktree per session (optional) | New session: "in place" or "new worktree"; the toolbar shows the branch | `Worktrees`, `enter_worktree` |
| Streaming transcript | Blocks for user, assistant (markdown), thinking, tool calls, approvals, questions, notices, compaction, subagents | Event fold in `cox-app` |
| Composer | Multi-line; `@file` mentions with fuzzy match; `/commands` with completion; `!` shell mode; paste/drag images and files; queue while busy | `commands::parse`, `nucleo` matcher, `UserShell`, attachments (new) |
| Approvals | Inline card + pinned copy above the composer; allow once, allow for session (shows the grant), deny with reason, edit then run | `ApprovalRequired`, `Decision` |
| Questions (`ask_user`) | Card with the options as buttons and a free-text field | `QuestionAsked` (new) |
| Permission mode and model | Toolbar controls; the change is echoed as typed state, not a notice | `SetPermissionMode`, `SwitchModel`, `SetEffort`, `StateChanged` (new) |
| Review | Changed files for the session, unified or side-by-side diff, revert a file to any checkpoint, comment on a line to send it to the agent | `ToolResult.diff`, `Checkpoint`, `Rewind` |
| Rewind timeline | Gutter marks per turn; "restore code", "restore conversation", or both; edit-and-resend a past prompt | `Rewind`, `Redo` |
| Inspector | Tabs: Changes, Plan (live todo), Context & Cost, Tasks (subagents/background), Info | `ToolResult.structured` (new), `Usage`, `TaskCreated/Completed` |
| Search | ⌘K palette over actions, sessions, files; full-text over every past session | `rollout_search` (FTS) |
| Settings | Generated from `docs/config.jsonschema`; every field shows where its value comes from; API keys go to the Keychain | `cox-config` `source_of`, `set` |
| MCP servers | Status per server, OAuth login in the browser | `cox_mcp::auth::login` with a GUI `Prompt` |
| Notifications | Turn done (with cost), approval needed (Allow/Deny buttons), budget warning; Dock badge = items waiting for you | `TurnDone`, `ApprovalRequired`, `Level::Budget` |
| Resume and fork | Open any past session, fork at a turn, hand off with an objective | `History::from_rollout`, `fork`, `handoff` |
| Onboarding and doctor | Pick a folder; checks provider keys, git, sandbox, shell env; imports Claude settings | `cox doctor` checks, claude layer |

### 3.2 M2 — the rest of the terminal, and verification

Integrated terminal pane (SwiftTerm, R9.3.15) in the session's cwd; browser
preview pane (SwiftUI `WebView`, R9.3.11) the agent can screenshot and read;
pop a session out into its own window and native window tabs; menu-bar extra
listing running sessions and waiting approvals; Spotlight indexing of session
titles and App Intents ("Ask cox in <project>") for Shortcuts; per-hunk revert.

### 3.3 M3 — beyond a single agent

ACP host: Claude Agent (Claude Code's ACP adapter), Codex, Gemini CLI and
Cursor (P35) sessions in the same sidebar and transcript (reusing `crates/cox-acp` and T35.3's client adapter);
best-of-n (one prompt, several models, each in a worktree, compared diffs);
plugin panels drawn from the `Widget` tree (R9.4.12); remote sessions over SSH
through a `cox app-server` that speaks the same patch protocol (DT§4.4).

#### 3.3.1 ACP host: a top-level session driven by an external agent (T52.1)

Status: **approved by the creator on 2026-09-29**, with decisions 5 and 7 of
the list at the end settled as written there. Evidence: R9.6.
Guards and fail-open rules are EA§2, §4 and §7 (`docs/design/external-agents.md`),
unchanged; this section only adds what a *top-level* session needs beyond the
subagent path T35.13 built.

**Driving a session.** `OpenRequest { agent: Some(name), .. }` resolves `name`
to one `ExternalAgentCommand` through its only constructor, from either source:
a `[external_agents.<name>]` user-config entry (T52.2) or a granted plugin's
`[[external_agents]]` entry (T35.6). Both come back already wrapped in the
session's `sandbox::Policy`, so no path spawns an unwrapped agent. `cox-app`
then (T52.4):

1. spawns **one process per session**, not per turn as the subagent driver
   does (R9.6.2.7). It runs in the session cwd or its worktree, in its own
   process group, with `CHILD_ENV_ALLOWLIST` plus `key_env` and nothing else
   from cox's environment. Its sandbox is the session's `[sandbox]` with
   network always on and the entry's `writable` state directories added
   (`agent_policy`, T52.2). A program on no `PATH` directory, or a key that
   does not resolve, is one warning, and the session does not open (EA§7);
2. sends `initialize` (`initialize_request(sandboxed)`) and then `session/new`
   with the cwd and no MCP servers (the agent keeps its own MCP config). On
   reopen it sends `session/load` instead when the agent advertises
   `loadSession` (T52.6);
3. maps `Send` to `session/prompt`. ACP allows one prompt in flight, so
   `Queue` holds the next one locally. `Interrupt` sends `session/cancel` and
   waits for the `cancelled` stop reason. Closing the session, or a quit,
   kills the process group (`cox_tools::bash::kill_group`);
4. folds every `session/update` through T52.3's mapper into the same
   `Timeline` and patch coalescer a cox session uses (DT-7), so there is no
   second fold. All text passes `cox_sanitize::sanitize` first.

**Event mapping (T52.3).** A pure fold. Kinds are from R9.6.2.1, stop reasons
from R9.6.2.3.

| ACP | cox |
| --- | --- |
| `session/prompt` sent | `TurnStarted`, then the user item (cox-app emits these; the agent does not echo them) |
| `agent_message_chunk` (text) | `ItemStarted(AssistantMessage)` on the first chunk, `TextDelta` after it, and `ItemDone` when another kind arrives or the turn ends |
| `agent_thought_chunk` | `ThinkingDelta`; `ThinkingDone` when another kind arrives |
| `tool_call` | `ItemStarted(ToolCall { name: title, input: rawInput })` + `ToolCallRequested`, shown as "run by <agent>". The request is informational only: it never becomes an approval (see approvals below) |
| `tool_call_update` with `content` | `ToolCallOutput`; output over the cap is archived before it is shortened (lossless rule) |
| `tool_call_update` with `diff` | `ToolCallDone` carrying the diff, so Changes and Review list the file |
| `tool_call_update` with `terminal` | the output of the cox-run terminal (T35.11) with that id |
| `tool_call_update` status `completed` / `failed` | `ToolCallDone` / `ToolCallDone` with the error text, then `ItemDone` |
| `plan` | `ToolResult.structured` (the Inspector's Plan tab, G3) |
| `session_info_update` (title) | `TitleSet` |
| `usage_update` | the context ring only (`used` / `size`). `cost` is dropped, and no `Usage` event is made (see "What is stored" below) |
| `available_commands_update` | the composer's `/` completion list for this session; no `Event` |
| `current_mode_update`, `config_option_update` | kept as session info for the Inspector's Info tab; no `Event` |
| `user_message_chunk` | user items, only during a `session/load` replay |
| any other or unknown kind, or a non-text content block | one sanitized `Notice(Info)`, never a failure (fail open; R9.6.3) |
| stop `end_turn` / `cancelled` | `TurnDone` / `TurnDone` (interrupted) |
| stop `max_tokens` / `max_turn_requests` / `refusal` | `Notice(Warn)` naming the reason, then `TurnDone` |
| JSON-RPC error or process exit mid-turn | `Error(CoreError::ExternalAgent { agent, message })` (T35.12) with the sanitized stderr tail |

**Intents.** An external session accepts `Send`, `Queue`, `Interrupt`,
`Approve` (for its own inbox items) and `Rename`. Every other intent returns
`AppError::Unsupported { agent, intent }`, because the cox state behind it
does not exist: the model, mode, history and checkpoints all belong to the
agent's process.

| Intent | Why it is refused | What the user sees |
| --- | --- | --- |
| `SetMode` | The Engine that judges the agent's requests is compiled once, at open, from `[permissions]` and the mode picked in the New-session sheet. ACP's own `session/set_mode` is the agent's mode, not cox's, so it is not offered in M3 | The mode control is replaced by the "<agent> · ACP" chip |
| `SwitchModel`, `SetEffort` | The model is the agent's own. `session/set_config_option` is not driven in M3 | The model chip reads "<agent> · ACP" and has no menu (mockup 27) |
| `Rewind`, `Redo`, `RevertFile`, `RevertHunk` | cox took no checkpoint: the agent wrote the files itself | No rewind gutter marks. Review shows the diffs without revert buttons |
| `Compact` | The agent compacts its own context | The menu item is disabled. `/compact` typed in the composer goes to the agent as text if the agent advertised it |
| `Fork`, `Handoff` | There is no cox history to fork, and ACP's `session/fork` is unstable (R9.6.2.5) | The menu items are disabled |
| `Background`, `Shell` (`!`), `Answer` | They need cox's task list, `UserShell` or `ask_user`. The agent has none of them | The controls are hidden. `!` mode is off |
| `Command` (cox slash commands) | The agent's commands are the ones that apply | `/…` is sent verbatim, and completion lists the agent's commands |

A disabled control carries the same sentence as its help tag: "Not available
in <agent> sessions (Agent Client Protocol)". The error itself is reached only
through a shortcut, an App Intent or the remote wire. It then shows the same
way as any other failed intent, with the agent named.

**Approvals.** The agent's own tool calls run inside its process. They are
guarded by the process sandbox, not judged per call (EA§2). Only what the
agent asks cox for meets cox's guards:

- `session/request_permission` goes to `cox_permission::Engine` through the
  existing `judge()` (T35.3's kind→tool map). `Allow` and `Deny` answer at
  once. `Ask` goes to a new `Approver` in `cox-app` (T52.5), which replaces
  `RefuseAsk` for top-level sessions. It becomes an `ApprovalRequired` block
  and an inbox item, with the agent as `source`. Allow once → `allow_once`.
  Allow for session → the engine's grant, recorded for this connection, and
  `allow_always`. Deny → `reject_once`. A closed session or a timeout
  answers deny;
- `fs/read_text_file` and `fs/write_text_file` go through `path::confine`
  against the session's roots, and writes also through the sandbox policy.
  They never prompt: an agent asks with `request_permission` before an edit;
- `terminal/create` is judged by the Engine as a `bash` call (its `Ask` takes
  the same inbox path). It runs only under the session's sandbox grant, and
  otherwise it is refused with the reason named (T35.11).

**What is stored.**

- The session row gets its `agent` name (T52.6), plus the agent's ACP
  `sessionId` so that `session/load` can find it.
- The rollout gets the prompts and the mapped events, sanitized.
  Over-cap tool output goes to the archive as usual.
- The ledger gets **no row**. An external agent's usage is its own billing:
  cox made no model request, and a zero or estimated row would be invented.
  The subagent path's `$0`/`billed_externally` row (EA§6, T35.5) stays as it
  is: there the external turn is a request inside a cox session.
- The cost pill, the notification's cost and the sidebar show "—".
  `[budget]` does not apply. A best-of-n group (T52.9) sums only its cox
  candidates' rows and marks the total as partial when an external candidate
  is in it.

**Launch table** (the documented examples T52.2 ships, not defaults; R9.6.1;
`writable` is the entry's state directory, from the creator's decision 7):

| Agent (display name) | `command` | `args` | `key_env` | `writable` | Install (by the user; cox never installs) |
| --- | --- | --- | --- | --- | --- |
| Claude Code's ACP adapter ("Claude Agent", R9.6.1.6) | `claude-agent-acp` | `["--hide-claude-auth"]` | `ANTHROPIC_API_KEY` | `["~/.claude"]` | `npm install -g @agentclientprotocol/claude-agent-acp` (0.84.0, Node ≥ 22; brings the Claude Code binary) |
| Codex's ACP adapter ("Codex") | `codex-acp` | `[]` | `CODEX_API_KEY` | `["~/.codex"]` | `npm install -g @agentclientprotocol/codex-acp` (2.0.0; brings `@openai/codex`) |
| Gemini CLI's ACP mode ("Gemini CLI") | `gemini` | `["--acp"]` | `GEMINI_API_KEY` | `["~/.gemini"]` | `npm install -g @google/gemini-cli` (0.61.0, Node ≥ 20) or `brew install gemini-cli` |
| Cursor ("Cursor") | `agent` | `["acp"]` | `CURSOR_API_KEY` | — (a plugin entry has no `writable`) | `curl https://cursor.com/install -fsS \| bash`; the entry comes from the granted Cursor plugin's `[[external_agents]]` (T35.6), not from user config |

The `@zed-industries/*` package names are deprecated (R9.6.1.2, R9.6.1.8), so
the table uses the `@agentclientprotocol/*` names.

**Risks found here, for T52.4's live check.**

- The host wrap copied `[sandbox] network`, whose default is `false`
  (R9.6.2.8), so under the default none of the four agents could reach its
  vendor's API. Settled by decision 7: an external agent's wrap always has
  network (`agent_policy`, T52.2).
- The agents may need to write their own state directories under `$HOME`.
  The sandbox denies those writes (R9.6.3, unverified). Settled by decision
  7 for user-config entries (`writable`); the Cursor plugin's entry has no
  such list yet, so `~/.cursor` stays read-only for it.

**Approved by the creator on 2026-09-29** (all eight, as written, with 5 and
7 settled as below)

1. Top-level ACP sessions accept only `Send`, `Queue`, `Interrupt`, `Approve`
   and `Rename`. Everything else is `AppError::Unsupported`, with the UI
   above. ACP's `session/set_mode`, `session/set_config_option` and
   `session/fork` are not driven in M3.
2. One agent process per session, kept alive across turns. The mode is fixed
   at open.
3. No ledger row and "—" for cost, and `[budget]` does not apply, while the
   subagent path keeps its EA§6 `$0` row. The agent's reported `usage_update`
   cost is dropped, not shown.
4. The launch table above, including the `@agentclientprotocol/*` names,
   installed programs only (no `npx -y`, which would fetch unpinned code on
   every launch), and API keys only.
5. Claude: it runs as `claude-agent-acp --hide-claude-auth`, with
   `ANTHROPIC_API_KEY` only, so cox never uses a claude.ai subscription or
   login (Anthropic's rule, R9.6.1.6), and it is labelled "Claude Agent" in
   the UI, never "Claude Code". Mockup 27 now says "Claude Agent".
6. Store the agent's ACP `sessionId` next to `sessions.agent`. T52.6 names
   only the `agent` column.
7. Network and state directories: an external agent always gets network
   inside its sandbox, whatever `[sandbox] network` says; its file limits
   stay. An `[external_agents.<name>]` entry may add `writable` directories
   for the agent's own state (`~/.claude`, `~/.codex`, `~/.gemini`,
   `~/.cursor`). They are confined to the user's home, and a project config
   may not set them (its own guard and reason, `external_agents.*.writable`).
   The Agents list's launch line shows both. T52.2's schema carries
   `writable`.
8. cox's own slash commands are off in external sessions; `/…` goes to the
   agent verbatim.

#### 3.3.2 Best of: a cox candidate needs a usable provider (T60.2, A138)

`App::best_of` asks `App::readiness(project)` once for the group. When it is
not `Ready`, every cox candidate is refused before its worktree is made:
`Launched.failed` carries `Readiness::message()` (for example "No API key for
anthropic. Add one in Settings, or pick another provider."), `worktree` and
`session` stay `None`, and nothing is created for the compare view to clean
up. An agent candidate (`Candidate::Agent`) brings its own provider and is not
gated. The rule is the composer's (DT§5.3), so a client shows the same text
in the launch sheet and may disable Best of with it.

## 4. Architecture

### 4.1 Layers

```
┌──────────────────────── Cox.app (Swift) ─────────────────────────┐
│ CoxTranscript The transcript: CoxUI cards in the TextKit 2 view,  │
│              bound to a SessionStore's patches.                   │
│ CoxUI        SwiftUI views, design tokens. No other cox package.  │
│ CoxPlatform  Notifications, Keychain bridge, Sparkle, OAuth,      │
│              NSWorkspace, SwiftTerm, WebView. No business rules.  │
│ CoxModel     @Observable stores; apply patches; send intents.     │
│              Depends on the CoreClient protocol, not on FFI.      │
│ CoxCore      UniFFI-generated Swift + CoreClient implementation.  │
└───────────────▲──────────────────────────────┬────────────────────┘
      patches   │ async pull                    │ intents (sync, non-blocking)
┌───────────────┴──────────────────────────────▼────────────────────┐
│ cox-ffi      UniFFI exports, one tokio runtime, foreign traits.   │  staticlib
├────────────────────────────────────────────────────────────────────┤
│ cox-app      Workspace, SessionController, Timeline fold, Patch   │
│              coalescer, Inbox, Status, Commands, Completion.      │
├────────────────────────────────────────────────────────────────────┤
│ cox-session  Builds a Session from config: provider, tools, MCP,  │
│              skills, hooks, plugins, checkpointer, worktrees.     │
├────────────────────────────────────────────────────────────────────┤
│ cox-core · cox-protocol · cox-store · cox-config · cox-render ·   │
│ cox-tools · cox-mcp · cox-ext · cox-plugin · … (unchanged roles)  │
└────────────────────────────────────────────────────────────────────┘
```

The TUI, `run -p` and ACP move onto `cox-session` too (one builder, R9.4.8),
and the TUI may later move its fold onto `cox-app`; the desktop does not wait
for that.

### 4.2 New crates and their dependency rules

| Crate | Owns | May depend on | Must not |
| --- | --- | --- | --- |
| `cox-session` | `open(SessionSpec) -> Result<Opened, SessionError>`: config → provider, tools, MCP, skills, agents, hooks, plugins, checkpointer, worktrees; `fork`, `handoff`, `resume`; login-shell environment resolution (DT§4.8). Warnings return as data, never printed | core, protocol, config, provider, tools, mcp, ext, store, plugin, sandbox | clap, anyhow, any `print`; cox-tui |
| `cox-app` | The UI-agnostic application core (DT§4.3). Pure logic over events; the only async parts are the per-session drain task and the controller. Owns the live sessions (T37.39): opens, resumes, forks and hands them off through `cox-session`, with host-supplied keys through a plain `Host` trait | session, core, protocol, store, config, render (neutral part), search, ext, tools | ratatui, crossterm, uniffi anywhere in its tree; clap, anyhow directly |
| `cox-ffi` | UniFFI records/enums mirroring `cox-app` types, the exported objects, the runtime, foreign traits (DT§4.4). `crate-type = ["staticlib", "lib"]` | app, protocol | anything else directly |

`crates/cox/tests/deps.rs` gets one rule per crate in the same change
(crates.md step 3), plus: `uniffi` only in `cox-ffi`; `cox-app` does not pull
ratatui; `cox-ffi`'s direct workspace dependencies are exactly `cox-app` and
`cox-protocol`. The slim build of the `cox` binary does not link `cox-ffi`.

`cox-render` today emits ratatui `Line`s. Its markdown and syntect
highlighting gain a neutral output — `StyledDoc` (blocks of `StyledSpan{text,
token: StyleToken, bold, italic, link}`) — with the ratatui conversion behind
a `ratatui` feature the TUI enables. One highlighter and one theme for both
surfaces; the desktop needs no Swift markdown or tree-sitter library.

### 4.3 `cox-app`: the application core

```
Workspace                       one per process
 ├─ projects(), sessions(project, limit), search(q)         → rows
 ├─ open(OpenRequest{cwd, resume, worktree, model}) → SessionController
 ├─ inbox: Inbox                approvals + questions across all sessions
 └─ worktrees(project) → [WorktreeRow{path, branch, bytes, merged, stale}]

SessionController               one per open session
 ├─ timeline: Timeline          Event → Block fold (+ replay from rollout)
 ├─ status: Status              model, tier, effort, mode, context %, cost, cache %
 ├─ drain task                  always reads Session::events(); never blocks the core
 ├─ coalescer                   batches patches per frame (16 ms or 64 patches)
 └─ send(Intent)                maps to Submission; spawns UserTurn
```

**Block model.** A block has a stable `BlockId`, a `turn`, and a kind:

| Kind | Built from | Carries |
| --- | --- | --- |
| `User` | `ItemStarted{UserMessage}` | text, attachments |
| `Assistant` | `ItemStarted{AssistantMessage}` + `TextDelta`s | `StyledDoc` (parsed in Rust) |
| `Thinking` | `ItemStarted{Thinking}` + `ThinkingDelta`s → `ThinkingDone` (A91) | text, seconds (`None` while streaming), collapsed |
| `Tool` | `ToolCallRequested` → `ToolCallOutput` → `ToolCallDone` | tool, one-line summary, icon key, risk, state, output tail (last 5 lines), full-output `ArchiveRef`, `DiffModel`, duration |
| `ToolGroup` | consecutive read/grep/glob/outline calls | "Explored 7 files", children |
| `Approval` | `ApprovalRequired` / `ApprovalDecided` | call, why text, grant preview, source (subagent), state |
| `Question` | `QuestionAsked` (new) | question, options, state |
| `Task` | `TaskCreated` / `TaskCompleted` / `TaskMessage` | label, tier, cost, status, kind (subagent or shell); the child session or archived output is read back by `open_task` (T37.29.6) |
| `Compaction` | `Compacted` | before → after tokens, reason, summary |
| `Checkpoint` | `Checkpoint` | turn, files |
| `Notice`, `Error` | `Notice`, `Error` | level, text, retryable |
| `TurnMeta` | `TurnStarted` + `Usage` + `TurnDone` | model, tokens in/out, cache read/write, cost, duration, stop reason |
| `Plugin` | `Advised`, plugin renderers | `Widget` tree (M3) |

Summaries ("Ran `cargo test` — exit 0 · 4.2 s", "Edited `crates/x.rs` +12 −3")
are produced in Rust so the TUI, ACP titles and the app say the same thing.

**Patches.** Swift never sees `Event`. It sees:

```
enum TimelinePatch {
  Reset { blocks: Vec<Block> }                 // open, resume, rewind
  Upsert { block: Block, after: Option<BlockId> }
  AppendText { id: BlockId, text: String }      // thinking, tool output tail
  DocTail { id: BlockId, from: u32, blocks: Vec<DocBlock> }  // markdown: closed blocks are frozen, only the tail is re-sent
  Remove { id: BlockId }
  Status { status: Status }                     // beside the list: `queued`, the turns waiting behind the running one (T37.24.8); `mode`, the `next_mode` ⇧⇥ asks for, and the main turn's `model` and `effort`, seeded from config and kept by `StateChanged`/`TurnStarted`/`ModelSwitched` (T37.24.7), with the catalog's `model_name` for that model (A111, T37.22.7); a queue keeps only the latest
  Usage { usage: UsageView }                    // token meter (DS§7): ledger totals, tok/s, TTFT, and `text` (MeterText, T37.25): every figure formatted, with the window share and the system/tools/instructions/history parts from the core's `ContextBreakdown` scaled to the last call's context (A98, T37.25.1), what the window has left, and the turn's cache hit (the Context tab, T37.29.3.1); a queue keeps only the latest
}
```

Keyed by id, not index, so SwiftUI identity is stable and a dropped patch can
be healed by `Reset`. Streaming markdown re-parses only the open tail block.

**Inbox.** Every pending approval and question from every session, oldest
first, with `session`, `source` and an expiry flag. Drives the "Needs you"
section, the Dock badge and notifications.

**Intents.** `Send{text, attachments, confirm_think}`,
`Approve{call, decision}`, `Answer{question, text}`, `Interrupt`,
`Queue{text, attachments, confirm_think}`, `Compact`, `SetMode`, `SwitchModel`,
`SetEffort`, `Rewind`, `Redo`, `Fork{turn}`, `Handoff`,
`Background{call}`, `Shell{command, share}`, `Command{line}` (parsed by the
shared command table). `send` never awaits a turn: `UserTurn` is spawned, as
`run.rs` already does (R9.4.3).

### 4.4 The FFI surface

A sketch; names are indicative, the shape is the decision.

```rust
#[derive(uniffi::Object)]
pub struct App { /* Workspace, runtime handle */ }

#[uniffi::export(async_runtime = "tokio")]
impl App {
    #[uniffi::constructor]
    pub fn new(home: String, host: Arc<dyn Host>) -> Result<Arc<Self>, AppError>;
    pub fn projects(&self) -> Vec<ProjectRow>;
    pub fn sessions(&self, project: Option<String>, limit: u32) -> Vec<SessionRow>;
    pub async fn search(&self, query: String, limit: u32) -> Vec<SearchHit>;
    pub async fn open(&self, req: OpenRequest) -> Result<Arc<SessionHandle>, AppError>;
    pub async fn next_app_patches(&self) -> Vec<AppPatch>;   // sidebar, inbox, badge
    pub fn settings(&self, cwd: String) -> Result<SettingsView, AppError>; // values + provenance
    pub fn set_setting(&self, cwd: String, key: String, json: String) -> Result<SettingsView, AppError>;
}

#[uniffi::export(async_runtime = "tokio")]
impl SessionHandle {
    pub fn snapshot(&self) -> Vec<Block>;
    pub async fn next_patches(&self) -> Option<Vec<TimelinePatch>>; // None: closed
    pub fn send(&self, intent: Intent) -> Result<(), AppError>;      // never blocks
    pub async fn expand(&self, archive: String) -> Result<String, AppError>;
    pub fn complete(&self, prefix: String, kind: CompletionKind) -> Vec<Completion>;
    pub async fn changes(&self) -> Result<Changes, AppError>;   // the Changes tab (T37.29.1)
    pub fn plan(&self) -> Vec<TodoItem>;                      // the Plan tab (T37.29.2)
    pub fn close(&self);
}

#[uniffi::export(with_foreign)]
pub trait Host: Send + Sync {          // implemented in Swift (CoxPlatform)
    fn open_url(&self, url: String);    // MCP OAuth, links
    fn notify(&self, note: Note);       // optional; the app also reads the inbox
}
```

Rules:

- **Pull, not push.** `next_patches` is an async pull (R9.3.2): backpressure
  and cancellation come free with Swift `Task` cancellation. The only foreign
  trait is `Host`, for things Rust must ask the OS to do.
- **Records are plain data**, generated as Swift structs and enums; errors are
  one `AppError` enum mapped from the crates' `thiserror` enums.
- **The patch types derive `Serialize` and `JsonSchema` too.** The same stream
  can later go over a socket (`cox app-server`, M3 remote sessions) or be
  recorded as a fixture for Swift tests (DT§8) without a second protocol.
- Secrets (T37.30): Rust asks `Host::secret(section)`; an env var still
  wins. CoxPlatform's `KeychainSecretStore` is the one write path and the
  reader behind `secret`: generic-password items `cox/<section>`, the item
  the CLI's keyring entry uses, so one key serves both. Tests use an
  in-memory store, never the real Keychain (A49).
- The host (T37.30.2): CoxPlatform's `MacHost` implements CoxClient's
  `PlatformHost` — `secret` over `KeychainSecretStore`, `notify` through
  `UNUserNotificationCenter`, `badge` when the count falls with no new item
  (an approval or question answered, its session closed; T37.27), `open`
  through `NSWorkspace` for `http(s)` links only — and CoxCore's `HostBridge` adapts it to the generated
  `AppHost`, so CoxPlatform tests without the XCFramework and CoxCore never
  links AppKit. The app passes `HostBridge(MacHost())` to `LiveCoreClient`.

### 4.5 Threads, runtime, backpressure, cancellation

- **One tokio runtime per process**, created by `cox-ffi` on first use
  (UniFFI's tokio feature, R9.3.2). Nothing calls `block_on` inside it.
- **The core is never blocked by the UI.** `Session::events()` is a bounded
  channel of 256 and `emit` awaits (R9.4.2). The drain task in `cox-app`
  reads it continuously, folds into the timeline and pushes patches into an
  unbounded but *coalescing* buffer: consecutive `AppendText`/`DocTail` for
  the same block merge, so a stalled UI costs memory proportional to the
  number of changed blocks, not to the number of tokens. Budget test: DT§1.
- **Swift side.** One `Task` per open session awaits `next_patches()` and
  applies the batch on the `MainActor` in one transaction. Batching at 16 ms
  keeps main-actor hops to at most one per frame.
- **Cancellation.** Closing a session cancels its Swift task and calls
  `close()`; the agent turn keeps running unless the user interrupts
  (`Intent::Interrupt` → `Session::interrupt`). Quitting the app with running
  turns asks first.
- **Sessions open elsewhere.** The TUI and the app share `cox.db`; `Presence`
  (already in the protocol) marks a session open in another surface, and the
  app shows it read-only with a "take over" action. One process drives a
  session (T37.34, DT§11 Q6): it holds an OS lock on `sessions/<id>.lock`
  (`cox_store::lock`), and any other opener gets `SessionBusy { holder }`
  and may follow it read-only (`cox_session::Follow` tails the rollout) or
  fork it; "take over" is only named in the notice so far.

### 4.6 Swift side

| Package | Contents | Depends on |
| --- | --- | --- |
| `CoxCore` | `binaryTarget` `CoxFFI.xcframework`; generated `cox_ffi.swift` (target `CoxFFIBindings`, Swift 5 mode, a symlink into `build/bindings/`); `LiveCoreClient` converting its values to `CoxClient`'s | `CoxModel`'s `CoxClient` |
| `CoxModel` | Target `CoxClient`: the timeline and intent values, the `CoreClient` protocol and `FixtureCoreClient` — here, not in `CoxCore`, because a package declaring the binary target does not load before the XCFramework is built (T37.16). Target `CoxModel`: `@Observable @MainActor` stores: `AppStore` (projects, sessions, inbox, badge), `SessionStore` (ordered blocks by id, status), `ComposerStore` (the draft, shell mode, picked `@` files, attachments, the rows `SessionClient.complete` returns — the fixture client answers from a fixed list — and the count of prompts queued while a turn runs, read from the core's `status` patch, T37.24, T37.24.8), `SettingsStore`. `apply(_ patches:)` and `send(_ intent:)` only | swift-collections |
| `CoxUI` | Views and the design system (DT§5.9); imports no other cox package, so a card is built from plain values | — |
| `CoxTranscriptText` | `TranscriptTextView`: the transcript as one TextKit 2 `NSTextView`, every timeline block a tracked text range (`BlockRanges`: id → range, location → id), styled by a `TranscriptStyle` the caller builds from tokens (each Rust `StyleToken` maps to a style colour, never a literal); a reply's text is built from its `StyledDoc` spans, and `apply` splices each timeline patch into its own block's range instead of rebuilding the text (`AppendText`, `DocTail`, upsert, remove); tool, approval, question and subagent cards are view-backed attachments (one character each) hosting the SwiftUI views the caller passes as `TranscriptCards`, so it depends on no CoxUI (T37.40, T37.41, T37.43, DT§5.2, `research.md` §9.5.13) | `CoxModel`'s `CoxClient` |
| `CoxTranscript` | `TranscriptView`: a `SessionStore`'s timeline in `CoxTranscriptText`'s view, a tool, tool-group or task block as CoxUI's `ToolCard`, an approval or question in a caller's slot, with `TranscriptStyle.cox` built from CoxUI's tokens; it follows the store through `SessionStore.didApply`, so each patch batch the store applies is spliced into the text. The one place the three meet, so CoxUI and `CoxTranscriptText` stay independent (T37.23). Also `SessionComposer`: CoxUI's `Composer` over CoxModel's `ComposerStore` (T37.24) | `CoxModel`, `CoxTranscriptText`, `CoxUI` |
| `CoxPlatform` | `Host` implementation, notifications with actions, Sparkle, OAuth handoff, `NSWorkspace` "open in editor", SwiftTerm and `WebView` panes (M2) | `CoxModel` |
| App target | `@main`, scenes, menus, entitlements, Info.plist, assets; `project.yml` for XcodeGen, `just desktop-app` builds an ad-hoc signed Debug `Cox.app` (T37.32.1) | all |

**What Swift may do:** lay out, animate, localize dates and numbers, map a
`StyleToken` to a color, keep UI-only state (scroll position, which blocks
are expanded, window frames), talk to the OS.
**What Swift may not do:** decide a permission, compute a cost, parse a
command or markdown, compute a diff, choose a model, touch `~/.cox` or git.
A review rule: any `if` in Swift that inspects a tool name or an event kind
to decide behaviour is a bug — the kind comes from Rust already decided.

Strict concurrency is on (Swift 6 language mode). Generated UniFFI code is
wrapped so its partial `Sendable` coverage (R9.3.4) stays inside `CoxCore`.

### 4.7 Core changes the GUI needs (fixed for every surface)

| # | Gap (R9.4) | Change | Who else benefits |
| --- | --- | --- | --- |
| G1 | Assembly in the binary; ACP bypasses it (9.4.8) | `cox-session` crate; TUI, `run`, ACP call it | ACP gets MCP, skills, hooks, checkpoints |
| G2 | `UserTurn.attachments` ignored (9.4.4) | Images and file attachments reach the request (per provider capability) | TUI paste, ACP |
| G3 | Todo only as text (9.4.5) | `ToolResult.structured: Option<Value>`; TUI and ACP drop their re-parsers | TUI, ACP |
| G4 | `ask_user` side channel (9.4.6) | `Event::QuestionAsked{id, call, question, options, source}` + `Submission::Answer{id, text}` | rollout replay, ACP, `run -p` |
| G5 | Mode/effort echoed as a string (9.4.7) | `Event::StateChanged{mode, effort}` | TUI status line |
| G6 | No session title | `Event::TitleSet{title}` from a low-cost `Job::Title` call after the first turn | TUI, sessions list |
| G7 | Stale counts in `protocol.md` (9.4.1) | Fix the doc | — |
| G8 | Instruction files never reach the prompt (9.4.9) | Wire the `AGENTS.md` chain (spun off as its own task) | every surface |
| G9 | Plugin UI channel uses TUI types | Neutral `PluginRequest` in `cox-app` (M3) | — |

G2–G6 change `cox-protocol`, so the schema test regenerates
`docs/protocol.jsonschema` and each lands as its own ≤ 200-LOC card.

### 4.8 The login-shell environment

An app launched from Finder or the Dock does not get the user's shell `PATH`,
so `bash` would not find `cargo`, `mise` or `node`, and env-var API keys are
invisible. `cox-session` resolves the environment once at startup: run the
user's login shell (`$SHELL -l -i -c` printing `env -0`) with a 10 s timeout,
parse, and use it as the base environment for tools; on timeout fall back to
`launchd`'s environment and show a notice. The CLI keeps its inherited
environment. Keys the app itself stores live in the Keychain.

## 5. Interface design

### 5.1 Window anatomy

Default window 1 440 × 900, minimum 900 × 600. A three-column
`NavigationSplitView`:

```
┌ toolbar ─────────────────────────────────────────────────────────────────────┐
│ ◧  cox › main ⎇ wt/fix-login     [Sonnet 5 · high ▾] [Ask|Plan|Auto]  $0.42 · ctx 38% ■ ◨ │
├──────────────┬──────────────────────────────────────────────┬─────────────────┤
│ SIDEBAR 250  │ TRANSCRIPT  (reading column ≤ 760 pt)        │ INSPECTOR 320   │
│ ⌕ Filter     │  12 │ You: fix the login redirect …          │ Changes · Plan  │
│ NEEDS YOU 2  │     │ Assistant text …                       │ Context · Tasks │
│ RUNNING 1    │     │ ▸ Explored 6 files                     │ · Info          │
│ ▾ cox        │     │ ✎ Edited src/auth.rs  +12 −3           │                 │
│   ● Fix log… │     │ ⚠ Run `git push`?  [Allow] [Session] … │                 │
│   ○ Bench …  │                                              │                 │
│ ▸ other-proj │ ┌ composer ────────────────────────────────┐ │                 │
│ ＋ New ⌘N    │ │ Ask cox…  @file  /cmd  !shell     ⏎      │ │                 │
│              │ └ 📎  Plan ⇧⇥   Sonnet 5 · high   think ─────┘ │                 │
└──────────────┴──────────────────────────────────────────────┴─────────────────┘
```

- **Toolbar** (Liquid Glass): sidebar toggle (⌃⌘S); breadcrumb *project › branch ›
  worktree* (click: switch branch/worktree); model chip (tier · model · effort,
  menu); permission-mode segmented control; cost pill (`$` for the session and
  context-window fill; click opens Context & Cost); Stop button while a turn
  runs (⌘.); Appearance (⌘⌥A); inspector toggle (⌃⌘I). **Bypass mode** paints
  a thin red strip under the whole toolbar for as long as it is on.
- **Sidebar**: filter field; "Needs you" (sessions with a pending approval or
  question, orange count); "Running"; then projects as disclosure groups.
  A row: status glyph (● running, ◐ waiting for you, ○ idle, ✕ error),
  title, one line of last activity, cost at the right on hover. Context menu:
  rename, fork, open worktree in Finder/editor, archive, delete (with
  confirm). Footer: New session (⌘N), provider-health dot.
- **Transcript**: one centered reading column, max 760 pt, with a 36 pt left
  gutter for turn numbers and checkpoint marks. The composer is docked at
  the bottom of the column and grows up to 40 % of the height.
- **Inspector** (⌃⌘I), tabs:
  - *Changes* — files touched this session with +/− and the tool call that
    touched them; click opens Review.
  - *Plan* — the live todo list with statuses.
  - *Context & Cost* — a stacked bar of the window (system, tools,
    instructions, history, free), cache-hit %, "Compact now"; a per-turn cost
    table (input, output, cache read, cache write, `$`), session and project
    totals, the budget cap and how close it is.
  - *Tasks* — subagents and background calls: label, tier, state, cost; click
    opens the child transcript in the inspector.
  - *Info* — session id, cwd, worktree, config provenance, rollout path.

### 5.2 Transcript blocks

- **User message** — full-width block with a subtle filled background
  (quaternary fill, 10 pt radius), attachments as thumbnails under the text.
  Hover: "Edit and resend" (rewinds the conversation to before this turn and
  prefills the composer), "Copy".
- **Assistant message** — plain text on the window background, no bubble,
  markdown drawn from the Rust `StyledDoc`. Code blocks: header with language,
  Copy and "Open in editor" (when the block names a path); monospaced body;
  long blocks (> 40 lines) fold with "Show 120 more lines".
- **Thinking** — one secondary-color row "Thought for 12 s ▸"; expands to
  the reasoning in secondary text. Streaming: the row shows a live timer.
- **Tool call** — one row: SF Symbol per tool (`doc.text` read, `pencil`
  edit/write, `terminal` bash, `magnifyingglass` grep/glob, `globe` web,
  `checklist` todo, `person.2` agent), the Rust summary, and a trailing state
  (spinner, ✓, ✕ with exit code, duration). While running, a monospaced
  five-line live tail of the output sits under the row. Expanded: the input
  (command or pretty JSON), the output (truncated outputs end with "Show full
  output · 84 KB", which calls `expand` and never refetches from the model),
  and the diff for edits. Consecutive read/grep/glob/outline calls fold into
  one "Explored 6 files" row. High-risk calls carry a risk badge.
- **Approval** — a card with an orange edge, in the transcript where it
  happened and pinned above the composer while pending. Content: *what* (the
  command, highlighted; for an edit, the diff), *why* (from `why_text`:
  "matches ask rule `Bash(git push:*)`", "risk high: writes outside the
  workspace", "sandbox denied network"), *who* (a subagent label when
  `source` is set). Buttons: **Allow** (⏎), **Allow for session** (⌘⏎, with
  the exact grant it adds, e.g. "`git push *` until this session ends"),
  **Edit…** (opens the command in an editable field; runs as
  `Decision::Edit`), **Deny** (⎋, optional reason field). After a decision
  the card shrinks to one line: "Allowed by you · for session".
- **Question** — a card with the question, one button per option and a text
  field; answered cards collapse to "You answered: …".
- **Subagent task** — a nested card with label, tier, live status and cost;
  "Open" shows its transcript in the inspector.
- **Compaction** — a centered divider: "Context compacted · 142k → 31k
  tokens ▸ summary".
- **Checkpoint** — a dot in the gutter; hover: "Restore code to here".
- **Notice / budget / security** — slim single lines, color by level; a
  budget stop adds "Raise cap and continue".
- **Error** — a red-edged block with the message and "Retry".
- **Turn meta** — on hover over a turn, a secondary line: model, tokens in /
  out, cache %, `$`, duration, stop reason.

Selection: text selection is enabled per block; "Copy as Markdown" on every
block and on a multi-block selection made with ⇧-click in the gutter.
Cross-block drag selection (on by default, `cross_block_selection = false`
clamps it to one block) runs on our own TextKit 2 view, package
`CoxTranscriptText`, chosen by spike T37.37 (`research.md` §9.5.13).
Copy writes the selection twice, blocks in order: Markdown
(`net.daringfireball.markdown`; a whole reply as its source, a card as its
summary line, a code block cut short still fenced) and plain text; the view
takes the setting as `crossBlockSelection` and never reads config (T37.42).
A user prompt and a thought stay text in that view, so a drag can start
partway through a prompt (T37.23.4): a layout fragment draws the bubble or the
thought's rule behind their paragraphs, an attachment's tile and the thought's
fold header are small view-backed attachments, and a thought folds by
dropping the text after its header, its open state the view's own. They copy
as the text shown: a prompt without its tiles, a folded thought as nothing.

### 5.3 Composer

- `TextEditor` over `AttributedString` (R9.3.11). ⏎ sends, ⇧⏎ new line.
- `@` opens a file picker ranked by the Rust fuzzy matcher; the chosen file
  becomes a pill. `/` lists commands from the shared command table with
  their help text. A leading `!` switches to shell mode: monospaced font,
  terminal icon, "share output with the agent" toggle (`UserShell{share}`).
- Paste or drop images and files; they show as removable chips.
- While a turn runs, ⏎ queues the message (the send button shows "Queued ·
  1"); ⌘⏎ interrupts and sends now; ⌘. interrupts.
- Sending is disabled until the session can answer (T60.2, A138). A client
  asks `App::readiness(cwd)` (async: it re-probes, so a key added in Settings
  or a server just started counts at once) and, unless it returns `Ready`,
  disables Send, ⏎ and Best of and shows `Readiness::message()` beside the
  composer. The four outcomes and their texts:

  | `Readiness` | Meaning | Text |
  | --- | --- | --- |
  | `Ready` | `tiers.code.provider` is a configured section and is in `usable_providers` (A110) | none |
  | `NoProvider` | the provider is empty or names no `[providers.<name>]` section | "No provider is set for the code tier. Choose one in Settings." |
  | `NoKey { provider }` | a keyed provider whose key is in neither its env var nor the host's store | "No API key for `<provider>`. Add one in Settings, or pick another provider." |
  | `Unreachable { provider }` | a provider on loopback whose server does not accept connections | "`<provider>` is not running on this machine. Start its server, or pick another provider." |

  The texts are English in `cox-app` for now (it has no `cox-i18n`
  dependency); a client localizes by matching the variant. The gate is also
  enforced in the core: `LiveSession::send` of `Intent::Send` or
  `Intent::Queue` (and a plugin's prompt) returns `AppError::NotReady`
  without starting a turn, so a client that forgets it still cannot send. A
  key the server rejects counts as usable until a turn fails. Under a test
  double (`COX_PROVIDER`) the answer is always `Ready`.
- ↑ in an empty composer walks the prompt history (`user_prompts`).
- The chips row below: attachment button, permission mode (⇧⇥ cycles), model
  and effort, "think" toggle.
- Provider pick (T60.3, A138): the model menu also lists the models of the
  other configured provider sections. Picking one sends
  `Intent::SwitchProvider { provider, model, make_default }` through
  `LiveSession::send` (`SessionHandle.send` over the FFI). A model of the
  section the session already runs on is a plain code-tier model switch,
  allowed at any time; it returns no session. Another section is allowed
  only while the session has no turn yet (no turn spawned, empty history):
  the core reopens the session in place — the same session id, cwd, row and
  rollout, with `tiers.code.provider` and `tiers.code.model` overridden for
  this session — ends the old one and returns the reopened session, which
  the client puts in the window's slot instead of the old handle (whose
  patch stream then closes). The composer keeps its draft and attachments:
  they live in the client until sent. If the new provider cannot open (no
  key, a broken section) the error comes back and the old session runs on.
  After the first turn the pick fails with `AppError::ProviderLocked`, "start
  a new session to change provider"; the client offers a new session.
  `make_default` also writes both keys to the user `config.toml` through the
  Settings setter (refused like Settings when a project layer pins them), so
  the next session and a later resume of this one open on it; without it a
  resumed session opens on the configured default, as after `/model`.
  The send gate reads the session's own provider: an open session asks
  `LiveSession::readiness()`, which is `App::readiness(cwd)` with the
  session's picked provider in place of the configured one, so a reopened
  session may send on its pick while `App::readiness(cwd)` still reports the
  default. Switching provider mid-session is not offered (`roadmap.md`).

### 5.4 Review

⌘⇧R, or "Review" in the Changes tab, replaces the transcript column with a
split: file list (grouped by turn, +/− counts) and the diff (unified or side
by side, toggle ⌘⌥D), syntax colored by the same Rust highlighter. Per file:
"Revert to before turn N" (checkpoint restore), "Open in editor". Click a
line number to add a comment; comments collect into a draft and "Send to
agent" posts one message with file:line anchors. "Rewind to here" on any
turn in the list. Per-hunk revert is M2.

### 5.5 Command palette and keyboard

⌘K opens one palette over actions, sessions (fuzzy on title), files in the
current project and slash commands (mockup 12, T37.44.13). `cox_app::palette::rank`
orders it: actions, then sessions, then commands and files, best match first
within each group and at most five of each, with the matched characters for the
row to draw bold; commands and files join only once something is typed. ⏎ runs
the selected row: an action runs, a session shows in the window, a command or
file lands at the end of the composer's draft. Esc or a click on the scrim
closes it. The whole app is keyboard-drivable:

| Key | Action |
| --- | --- |
| ⌘N / ⌘⇧N | New session / new session in a worktree |
| ⌘1…⌘9 | Jump to the n-th session in the sidebar |
| ⌘K | Palette |
| ⌘L | Focus composer |
| ⌘. | Interrupt |
| ⏎ / ⌘⏎ / ⎋ | On a focused pending approval card: allow / allow for session / deny |
| ⌘⏎ / ⌘⌫ | In the composer while the pinned approval bar shows: allow / deny (plain ⏎ still sends; T37.27.5) |
| ⌘⇧R | Review |
| ⌃⌘I / ⌃⌘S | Inspector / sidebar: the defaults of the system `InspectorCommands` and `SidebarCommands` (A89) |
| ⌘⌥A | Appearance popover |
| ⌃` | Terminal pane under the transcript: show / hide; the first show opens the session's shell (T51.6) |
| ⌘⇧B | Browser pane beside the transcript: show / hide; the page the agent's `browser_*` tools drive (T51.10) |
| ⌘⇧F | Search all sessions |
| ⌘[ / ⌘] | Previous / next turn in the transcript |
| ⌘+ / ⌘− | Text size |

### 5.6 Notifications, Dock, menu bar

Notifications only when the app is not frontmost or the session is not
visible: "Turn finished · $0.18", "Approval needed: `git push`" with
**Allow once**, **Deny** and **Open** actions (mockup 23, A126): Allow once is
the one-call approval, never a standing rule; allow-for-session and edit need
the app, by design; Open, like a click on the notification, brings cox
forward on the session's window, where the pending approval waits (T37.44.15).
"Budget reached". A question takes a typed **Answer**. CoxPlatform's
`NotificationActions` maps a note to its content and an action back to the
session and `Intent`, or Open to the session it shows, as plain functions,
tested without posting; the app sets `NotificationResponder` as the centre's
delegate and sends the intent to that session (T37.27). Dock badge = pending approvals + questions across
all sessions. The M2 menu-bar extra lists running sessions and the inbox.

### 5.7 Settings

A separate Settings window (⌘,) generated from `docs/config.jsonschema` with
hand-tuned grouping: General, Models & Providers, Permissions, Sandbox,
Budget, MCP Servers, Plugins, Appearance, Advanced. Each field shows a
provenance badge — *default*, *user*, *project*, *env*, *flag* — from
`source_of`; a field overridden by the project config shows the project file
and is read-only here (project guard keys stay guarded). A project value the
guard list threw out is listed on its page with the guard's reason and the
value that holds instead. Edits go through
`cox-config`'s comment-preserving `set` (user file only). Provider keys are
entered in a secure field and go to the Keychain through `SecretStore`. MCP:
status per server, Log in / Log out; login opens the browser via `Host`.
The status badge (connected, needs login, failed, disabled, unknown) comes
from `cox-app`'s `mcp_status`: the config, the token store and what the
last session opened in the project made of each server; Show log opens why
a server failed, sanitized and capped in Rust (T37.45.4).

### 5.8 Onboarding and empty states

First launch: "Open a project" (folder picker or drop a folder), then a
checklist from the doctor checks — provider key found, git available,
sandbox works, shell environment resolved — each with a fix button. The
rows come from `cox-app`'s `checklist`, which runs `cox doctor`'s own checks
(`cox_session::doctor`), so the app and the CLI never disagree. Offer to
import Claude Code settings. Empty transcript: three example prompts built
from the project (e.g. "Explain the architecture of <name>"). An error state
always names the cause and one action.

### 5.9 Typography, color, motion, accessibility

- **Type.** Body SF Pro Text 13 pt, line height 1.45; meta 11 pt secondary;
  markdown headings 17 / 15 / 13 pt semibold; code SF Mono 12 pt (any
  installed monospaced font can be chosen). Text size scales 85–150 % (⌘+/⌘−).
- **Color.** Semantic tokens only, one-to-one with `cox-render`'s
  `StyleToken` (Text, Dim, Accent, Success, Warning, Error, DiffAdd, DiffDel,
  Border, Selection, syntax roles), each an asset color with light, dark and
  increased-contrast variants. The accent follows the system accent. Syntax
  colors come from the same theme as the TUI, so a screenshot of either looks
  like the same product. Mode colors: Plan blue, Auto accent, Bypass red.
- **Motion.** No per-token animation; new blocks fade in over 120 ms; state
  changes (spinner → ✓) cross-fade; everything respects Reduce Motion.
- **Accessibility.** Each block is one accessibility element with a Rust
  summary as its label; the finished assistant message is announced once, not
  per token; VoiceOver rotors for Approvals, Tool calls and Errors; every
  control reachable by keyboard; contrast checked in both themes.
- **Language.** English first, String Catalogs from day one; Russian next.

## 6. Where things live

```
apps/cox/
├─ crates/
│  ├─ cox-session/            NEW  session assembly (from crates/cox/src/session.rs)
│  ├─ cox-app/                NEW  workspace, timeline fold, patches, inbox, status
│  │  └─ src/{lib,workspace,controller,timeline,patch,inbox,status,intent,complete}.rs
│  ├─ cox-ffi/                NEW  UniFFI surface, runtime, foreign traits
│  │  └─ src/{lib,types,app,session,host,error}.rs
│  ├─ cox-render/                  + neutral StyledDoc; ratatui behind a feature
│  └─ cox/                         TUI, run, ACP call cox-session; deps.rs rules
├─ desktop/macos/             NEW
│  ├─ project.yml                  XcodeGen spec of the thin Cox.xcodeproj (generated, gitignored): the app target only
│  ├─ App/                         CoxApp.swift (@main, scenes, commands), entitlements, Info.plist, assets
│  ├─ Packages/
│  │  ├─ CoxCore/                  Package.swift (binaryTarget ../../build/CoxFFI.xcframework)
│  │  ├─ CoxModel/                 stores + Tests/
│  │  ├─ CoxUI/                    views, DesignSystem/ + Tests/ (snapshots)
│  │  ├─ CoxTranscriptText/        TextKit 2 transcript view + Tests/
│  │  ├─ CoxTranscript/            TranscriptView: CoxUI cards in the text view + Tests/ (snapshots, DT§9 gate)
│  │  └─ CoxPlatform/              Host, notifications, Sparkle, terminal, web
│  ├─ Fixtures/                    patch streams recorded from scripted scenarios
│  └─ UITests/
├─ scripts/desktop/xcframework.sh  NEW  cargo (aarch64) → xcodebuild -create-xcframework
├─ scripts/desktop/app.sh          NEW  xcodegen → xcodebuild: build/Cox.app, Debug, ad-hoc signed
└─ justfile                        + desktop-xcframework, desktop-app, desktop-test, desktop-bench
```

The app keeps all Swift code in local Swift packages so the `.xcodeproj`
stays small and merge-friendly; only the app target lives in it. XcodeGen
generates it from `project.yml`, so only that spec is in git (T37.32.1).

## 7. Build, packaging, distribution

- **Rust → XCFramework.** `scripts/desktop/xcframework.sh` builds
  `cox-ffi` for `aarch64-apple-darwin` only (no Intel, DT§11 Q2) in release with split debug info, sets
  `MACOSX_DEPLOYMENT_TARGET=26.0` explicitly (R9.3.7), runs
  `uniffi-bindgen-swift`, and packs `build/CoxFFI.xcframework` + the
  generated Swift into `Packages/CoxCore`. Pattern from Firefox's script
  (R9.3.6). Run through `mise exec --` like every cargo command.
- **Dev loop.** An Xcode "Run Script" phase calls the script only when a Rust
  input changed; CI builds from scratch. Rust never compiles inside every
  Swift build.
- **Bundled CLI.** The same build puts the `cox` binary in
  `Cox.app/Contents/Helpers/cox`; the app offers "Install command-line tool"
  (a symlink in `/usr/local/bin` or `~/.local/bin`), so app and CLI never
  drift in version or database schema.
- **Signing.** Developer ID Application, Hardened Runtime, notarized with
  `notarytool`, stapled. No App Sandbox (DT-6). Entitlements: none beyond the
  defaults unless a plugin runtime needs JIT (wasmtime: `allow-jit` — to be
  confirmed by the first signed build, DT§11 Q5).
- **Updates.** Sparkle 2 (R9.3.17) with an EdDSA-signed appcast on GitHub
  Releases; updates never kill running turns: "Install on quit", or after
  asking when sessions are running (the Codex SIGKILL-on-update complaint,
  R9.2).
- **Other channels.** A Homebrew cask pointing at the same notarized DMG.
- **CI.** A macOS job: build the XCFramework, `swift test` for the packages,
  UI tests on the scripted scenarios, a notarization dry run on tags.

## 8. Testing

- **Rust first.** `cox-app` is where the logic is, so it carries the tests:
  `insta` snapshots of the block list for every scripted scenario in
  `tests/` (the same scenarios the TUI and e2e runs use); a test that a
  consumer which stops reading for 10 s does not delay a turn; patch
  coalescing; replay-from-rollout equals live fold; inbox across two
  sessions. No network, no API key, no real Keychain.
- **Fixtures across the boundary.** A `cox-ffi` test binary records the
  patch stream of each scenario to `desktop/macos/Fixtures/*.json`
  (the patch types are serde, DT§4.4). Swift tests and SwiftUI previews
  replay them through a `FixtureCoreClient`, so previews show real
  transcripts with no running core.
- **Swift.** Swift Testing for the stores (patch application, intents
  emitted); swift-snapshot-testing for views in light, dark and
  increased-contrast (R9.3.19); XCUITest for the smoke path: open a project,
  send a prompt, approve, review, rewind — against `cox-ffi`'s
  `Scripted` provider build flag.
- **Performance.** The DT§1 budgets as XCTest metrics, run by
  `just desktop-bench`, and recorded in `research.md` §4.x like the other
  measurements.
- **Real runs.** Manual checks use a scratch `COX_HOME`, like every other
  surface.

## 9. Transcript rendering: the one open technical bet

SwiftUI `LazyVStack` in a `ScrollView` is the simplest way to draw
variable-height rich blocks but is known to degrade on very long lists
(**unverified** community reports, R9.3); an `NSTableView` with
`NSHostingView` cells gives true reuse. The decision is a measured gate, not
a guess: card T37.23 builds the transcript on `LazyVStack` with stable ids
and a realized window (turns older than the last 30 collapse to one-line turn
summaries until scrolled to), runs the DT§1 scroll and streaming budgets on
the 10k-block fixture, and switches to the `NSTableView` host if it misses.
Cross-block text selection (DT§11 Q7, on by default) needs a single TextKit 2
document (`NSTextView`); spike T37.37 measured it within the scroll budget at
2 000 and 10 000 blocks (`research.md` §9.5.13), so the benchmark includes it.

## 10. Trust boundaries

Nothing new is trusted. Every model- and tool-originated string the app
shows went through `cox_sanitize::sanitize` in Rust before it became a block;
Swift renders text only (no HTML from the model, links shown with their
target and opened through `Host` after a confirmation for non-`https`
schemes). Tool calls are still decided by `cox_permission::Engine`; the app
only answers the question the engine asked. Paths still go through
`cox_sandbox::path::confine`; shell commands still run under
`cox_sandbox::sandbox::Policy` — the desktop adds no "run in the app's own
context" path. The M2 browser pane is a separate `WebPage` with no
JavaScript bridge to the app.

## 11. Open questions for the creator

1. ~~**Plan decisions.** Amend D1, D2, D11?~~ Resolved 2026-09-28: option A,
   in-process static library; `plan.md` §0 and A67 carry the new wording.
2. ~~**Hardware floor.**~~ Resolved 2026-09-28: macOS 26+ on Apple Silicon
   only, no Intel.
3. ~~**Process model.**~~ Resolved 2026-09-28 with Q1: in-process UniFFI. The
   patch types stay serde so a socket server can be added later for remote
   sessions without redesign.
4. ~~**License and repo.**~~ Resolved 2026-09-28: this repository, `desktop/macos/`, under the repository's licence.
5. ~~**Swift dependencies**~~ Resolved 2026-09-28: `research.md` §9.5 and A67;
   cross-block selection engine: our own TextKit 2 view (`CoxTranscriptText`),
   decided by spike T37.37 (`research.md` §9.5.13); Textual was rejected.
6. ~~**Concurrent processes on `cox.db`.**~~ Resolved 2026-09-28: one shared
   database (WAL and busy timeout already on, `cox-store/src/lib.rs:190`);
   one process drives a session under an OS file lock, others follow or fork
   (T37.34); IMMEDIATE write transactions and a `data_version` change feed
   (T37.35); an older binary refuses a newer schema (T37.36).
7. ~~**Cross-block text selection**~~ Resolved 2026-09-28: cross-block selection, on by default, with a setting to turn it off.
8. ~~**External agents (ACP host)**~~ Resolved 2026-09-28: stays M3; moves into the plan when a planned card is blocked by it.

## 12. Plan amendment (applied as A67, phase P37)

Applied to `plan.md` on 2026-09-28: A67 changes §0 D1, D2 and D11 (option A,
in-process static library) and adds phase P37 with cards T37.0–T37.36. The
cards, their order and their checks live in `plan.md` only; M2 and M3 are in
`roadmap.md`. The view-layer guide and tokens are `desktop/design/DESIGN.md`.
