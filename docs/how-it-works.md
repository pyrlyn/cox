# How cox works, with examples

One idea explains cox: a `Submission` goes into a pure core state machine,
a sequence of `Event`s comes out, and every surface renders that same
sequence. The TUI, headless `cox run -p`, the ACP editor server, the MCP
server, the JSONL rollout on disk, and the test suite are all consumers of
one event stream (`cox-protocol::types::{Submission, Event}`).

```text
                ┌────────────────────────────────────────┐
  you ──Submission──▶│ cox-core: Session state machine    │──▶ Event stream ──▶ TUI
script ──UserTurn───▶│  assemble → route → stream → tools │──▶ stream-json      ACP
editor ──Approve────▶│  (no network / fs / process here)  │──▶ rollout.jsonl    tests
                └────────────────────────────────────────┘
```

Everything the core needs from the outside world (models, files, shells,
stored sessions) arrives through traits in `cox-protocol`
(`Provider`, `Tool`, `Store`, `Hook`, `Archive`). That is what makes the
loop testable without a model: a scripted provider plus a golden event log.

For the full contract see `plan.md` §1.2–§1.3; for per-component rationale
see `docs/design/`. This file is the walkthrough.

## Example 1: one turn in the TUI

```bash
./target/debug/cox
# > create hello.txt containing hi
```

What happens inside `cox-core` (`plan.md` §1.3), simplified:

1. `Submission::UserTurn { text: "create hello.txt containing hi", .. }`
   enters the session. `UserPromptSubmit` hooks may rewrite or block it.
2. The core assembles a provider-neutral `Request` (system prompt, tool
   schemas, instruction files, history), picks the `code` tier
   (`claude-sonnet-5`), and streams the model.
3. The model emits a `write` tool use. The core emits, in order:

```json
{"type":"tool_call_requested","call":{"name":"write","input":{"path":"hello.txt","content":"hi\n"}}}
{"type":"tool_call_done","call_id":"…","result":{"ok":true,"visible":"wrote 3 bytes to hello.txt","bytes":3}}
{"type":"text_delta","text":"Created hello.txt."}
{"type":"turn_done","stop":"end_turn"}
```

4. All tool results for that assistant message go back to the model in
   **one** user message, in emission order — even when the calls ran in
   parallel. No `Event` is ever emitted after `TurnDone` for that turn.

Interrupt (`Esc`) cancels the provider stream and every running tool
through one shared token, then emits the partial assistant text and
`TurnDone{Interrupted}`.

## Example 2: the same turn, headless

```bash
export ANTHROPIC_API_KEY=sk-...
./target/debug/cox run -p "create hello.txt containing hi"
./target/debug/cox run -p "summarise the diff" --output-format stream-json | head -n 5
```

`stream-json` prints the *same* `Event` JSON the TUI renders, one object
per line (Claude Code-compatible framing). Exit codes are scriptable:
`0` ok · `1` error · `2` denied · `3` budget · `4` interrupted.

Approval from a script: a `Write`/`Exec` call the policy would ask about
is denied instead under the headless default (`--approve never`), with
the reason in the tool result so the model can try another approach:

```bash
./target/debug/cox run -p "commit this" --approve never; echo "exit=$?"
# exit=2 when a call was denied
```

Answer an interactive approval in the TUI with `y` (allow once),
`s` (allow for this session), `n` (deny), `e` (edit the call's input).
Programmatically that is `Submission::Approve { call_id, decision }`:

```rust
use cox_protocol::types::{Decision, Submission};
// `call_id` is the pending call from the `ApprovalRequired` event.
// Allow the pending call the engine escalated:
let answer = Submission::Approve { call_id, decision: Decision::Allow };
// …or let this tool+subject-prefix through for the rest of the session:
let answer = Submission::Approve { call_id, decision: Decision::AllowForSession };
```

## Example 3: permissions — deny beats allow

Every tool call carries a `risk` (`ReadOnly` | `Write` | `Exec` |
`Destructive`) and a `subject` (the confined path, command line, URL, or
`mcp__<server>__<tool>` name). `cox_core::permission::Engine` decides
each call, in order: `deny` rules → `bypass`/`plan` modes → `allow`
rules → `ask` rules → session grants → risk default → approval policy
(`plan.md` §1.8). Adding a `deny` rule can never turn a `Deny` into
anything else.

```toml
# ~/.cox/config.toml
[permissions]
allow = ["Bash(cargo test:*)"]   # this command runs without asking
ask   = ["Bash(git commit:*)"]   # this one always asks
deny  = ["Read(~/.ssh/**)"]      # this one never runs — even with an allow rule
```

Rule grammar (Claude Code's): `Tool` matches every call, `Tool(text)` the
exact subject, `Tool(prefix:*)` a subject that is `prefix` alone or
`prefix` followed by whitespace, `WebFetch(domain:host)` the host and its
subdomains, and a path glob for file tools.

`CloudAgent(<owner>/<repo>)` (T56.4) names a GitHub repository whose code a
cloud agent would read and edit off this machine. It asks in every
permission mode, `auto` and `bypass` included, and `plan` mode denies it.
Only an exact `allow = ["CloudAgent(owner/repo)"]` in your own config lifts
the question, for that one repository (names are matched ignoring case); a
bare `CloudAgent` allow, a session grant and a repository's own project
config do not, because a project config's `allow` is reverted. `deny` and
`ask` rules apply as for any tool. The approval names the repository, the
remote, the starting ref and that the code leaves the machine.

A `bash` command line is matched **command by command** (T36.1). The
tool splits it with its tree-sitter parse on `;`, `&&`, `||`, `|`, `&` and
newlines, including commands nested in a subshell, a `$(…)` or a loop
body, and hands the engine those strings next to the whole line:

- a `deny` or `ask` rule that matches **any** command applies, so
  `deny = ["Bash(rm:*)"]` denies `git status && rm -rf x`, and a leading
  `VAR=value` does not hide the `rm`. It also sees past a wrapper Claude
  Code itself strips before matching (`nohup`, `timeout 5`, `time`,
  `nice`, `stdbuf`, the builtins `command`/`builtin`, zsh's `noglob`, bare
  `xargs`), so `deny = ["Bash(rm:*)"]` denies `nohup rm -rf x` too
  (T36.2), and it re-parses an `eval …`/`sh -c '…'`/`bash -c "…"` string
  with the same walk, so it denies `sh -c 'rm -rf x'` — stricter than
  Claude Code here, whose own rules do not look inside such a string
  (research.md row 38);
- an `allow` prefix rule or a session grant allows the line only when
  **every** command is covered, so `allow = ["Bash(git:*)"]` runs
  `git status && git diff` without asking but asks for `git status; rm -rf x`.
  Different rules may cover different commands;
- a line the split cannot vouch for is never allowed by a prefix rule or a
  grant, whatever its first word: `$(…)` or backticks, `<(…)`, `eval`,
  `sh -c`/`bash -c`, an output redirect to a path (`2>&1` and `/dev/null`
  are fine) or a parse error. It takes the normal ask path; `cox run -p`
  turns that ask into a deny;
- `export`/`declare`/`unset` and a variable assignment (`PATH=… git`
  changes what `git` runs) work the same way, and are never rated
  `ReadOnly` either (T36.2 closed a gap where `classify` dropped the
  assignment as if it did not change what runs) — except a leading
  assignment of a pure locale/display variable (`LC_ALL`, `LANG`, `TZ`,
  `NO_COLOR`), which cannot change what a later command resolves to or
  does, so it stays `ReadOnly` and eligible for an allow rule or grant,
  same as if it were not there;
- a bare `Bash` rule or an exact rule (`Bash(make && make install)`) still
  matches the whole line as written, and the read-only auto-allow and
  `bypass` mode are unchanged.

"Always allow this session" on a compound line records one grant per
command, so approving `git status && npm test` later covers `npm test` on
its own and never `npm test; rm -rf ~`. An opaque line is granted only as
the exact line approved.

```rust
use std::path::Path;
use cox_core::permission::{Engine, Outcome};
use cox_protocol::{CallId, config::PermissionsConfig, types::*};
use serde_json::json;

let home = Path::new("/home/alice");
let engine = Engine::compile(&PermissionsConfig::default(), Some(home), Path::new("/repo"))
    .expect("default rules compile");
let ssh = ToolCall {
    id: CallId::new(), name: "read".into(),
    input: json!({"path": "/home/alice/.ssh/id_ed25519"}),
    risk: Risk::ReadOnly, subject: "/home/alice/.ssh/id_ed25519".into(),
    segments: None, // only `bash` fills it (T36.1)
};
// The default config denies this, despite the ReadOnly risk:
assert!(matches!(
    engine.decide(&ssh, PermissionMode::Default, ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite, &[]),
    Outcome::Deny { .. }
));
```

(The same snippet runs as a doctest on `Engine::decide`, so `cargo test`
keeps it compiling. A matching doctest on
`cox_protocol::types::Submission` covers the `UserTurn` JSON shape.)

`plan` permission mode (`Shift+Tab` in the TUI) denies every non-`ReadOnly`
call without prompting, so the model learns to describe the change
instead of making it.

### Modes: architect and editor

A mode is a named preset over two things only: the permission mode and
the tier main turns run on. `/mode architect|editor` in the TUI, `--mode`
on the command line and `core.mode` in config all set the same value.

| | architect | editor (default) |
| --- | --- | --- |
| permission mode | the narrower of `permissions.mode` and `plan` | `permissions.mode` |
| main tier | think, confirmed | the configured tier |
| writes and non-read-only `bash` | denied by `plan` | per policy |

A mode never widens permissions: architect over a `plan` config is still
`plan`, and `/mode editor` goes back to `permissions.mode`, never past it.
Tools are not filtered by mode, so the tool list and system blocks stay
byte-identical across a switch and the prompt cache survives; `plan` does
the narrowing through the same `cox_permission::Engine` as every other
call. The think tier still needs consent: the TUI asks its price once per
architect stretch, and in `cox run` only the explicit `--mode architect`
flag (or `--deep`) counts, so `core.mode = architect` from a config file
alone makes the run refuse with the flag to pass.

## Example 4: big output is lossless, not lost

Tools return their **full** output; the core archives it *before* the
model sees anything. The model sees head + tail lines plus a pointer:

```text
line 1
line 2
[… 84 KiB archived; expand #01J9… lines 3–8210]
line 8211
```

Read the rest any time — you see exactly what the model saw, plus more:

```bash
./target/debug/cox expand 01J9…              # full archived output
./target/debug/cox expand 01J9… --lines 60-90
```

Two refinements keep the window small without losing evidence:

- **Dedup:** an identical read-only call within
  `context.dedup_window_turns` (default 8) with no write to its subject
  since returns `"unchanged since #<id>"` instead of the bytes again.
- **Microcompaction:** tool results older than
  `context.microcompact_after_turns` (default 6) become
  `Pointer { archive, summary }` in new requests. The rollout on disk is
  untouched — only what the model is (re)sent shrinks.

## What the model sees: the cache-stable prefix

Every request is laid out so the stable bytes come first and the
volatile bytes last (`plan.md` §1.9). Anthropic caches the stable
prefix; OpenAI-compatible providers get the same order for free via
automatic prefix caching:

```text
system[0]  tool schemas, sorted by name ............ byte-stable ┐
system[1]  cox system prompt (versioned, no date) ... byte-stable │ breakpoint 1
system[2]  AGENTS.md / skills index ................ byte-stable ┘
system[3]  volatile: date, cwd, branch, memory ..... never cached
messages   summary (if compacted) + history ........ breakpoint 2 (end of last turn)
           this turn's messages ................... breakpoint 3 (moves)
```

The rule: touch `system[0..=2]` and you invalidate the cache for every
later call. Discovering a deferred tool via `tool_search` does exactly
that, once — the core emits a `Notice` explaining it. `cox stats --cache`
shows whether the prefix is actually hitting.

## When context runs out: compaction

After a turn, when the last call's context tokens reach
`context.compact_at` (default 75%) of the model's window — or on
`/compact [focus]`, or on a context-length error — the core summarises
every turn but the last `keep_turns` (default 2) with the cheap tier,
appends one `Summary` item, and emits:

```json
{"type":"compacted","summary":"…","dropped":["…"],"before_tokens":150000,"after_tokens":9000}
```

Append-only: the rollout keeps every original line; `dropped` ids are
just skipped when building future requests. Early turns keep their
verbatim text right up until they are summarised.

## Where it lands on disk

Under `~/.cox/` (`COX_HOME` overrides; never touch the real home in
tests — use `COX_HOME=/tmp/cox-scratch`):

```text
~/.cox/
  config.toml                 effective config (see cox config show --sources)
  cox.db                      sessions, per-request usage ledger, archive index, memory FTS
  sessions/<ulid>.jsonl       the rollout: one Event per line, resume + replay source
  archive/<ulid>              tool outputs over 16 KiB (smaller ones inline in the db)
  checkpoints/<hash>/         one private bare git repository per workspace root (below)
  logs/cox.log                tracing log
```

Every provider call writes one `usage` row (model, input/output, cache
read/write, cost). `cox stats --day`, `cox stats --month`, and
`cox stats --cache` read the ledger; session/monthly caps in
`[budget]` stop the turn with `TurnDone{Budget}` instead of a surprise.

## Checkpoints: every write has a pre-image

Before `edit`, `write` or `apply_patch` runs, cox reads the files the call
names and archives their bytes (`checkpoints` row `pre`; `created` when the
file did not exist). Around a call that names no path — `bash`, an MCP
tool — cox snapshots the workspace before and after and archives the
pre-image of every file that changed or disappeared (`deleted`). The row and
the archive exist *before* the model sees the result; only then does
`Event::Checkpoint { turn, call, files }` reach the surfaces. A marker row
per user turn gives `/rewind` (T26.2) its timeline.

The snapshot is `git add -A` + `write-tree` inside a private bare repository
under `~/.cox/checkpoints/<hash>` whose work tree is the workspace root:
your repository's index, hooks and `.git` are never touched, `.gitignore`
still keeps `target/` out, and `GIT_ALTERNATE_OBJECT_DIRECTORIES` points at
your repository's objects so unchanged blobs are never copied. The private
index doubles as the stat cache, so a warm snapshot is one stat pass. A
pre-image over 8 MiB is recorded without bytes. Without `git` on `PATH`,
the session warns once and runs without checkpoints — never a failed turn.

`/rewind` (or `Esc Esc` on an empty composer) lists the turns newest first
— `T7 · 3 files · "add the cache column"` — and asks what to restore: code,
conversation or both. Code walks the rows from the newest turn down to the
chosen one and writes each earliest pre-image back (created files are
removed, deleted ones return); every write is checkpointed first under a
new turn number, so a rewind is itself undoable. Conversation appends an
`Event::Rewound { to_turn }` marker: the in-memory history is cut there,
the rollout keeps every line, and resume stops reading at the marker. The
next turn keeps counting from where the session was (`T8` after a rewind
to `T7`), so a turn number never means two things.

## MCP servers that need a login

An HTTP MCP server may answer the handshake with `401` and a `WWW-Authenticate` challenge. cox then runs the standard flow (authorization code with PKCE, dynamic client registration when the server offers it) through rmcp: in the TUI the login URL is printed and the browser opened, a listener on `127.0.0.1` takes the redirect, and the token is filed in the OS keyring as `cox/mcp/<name>`. From then on the token is attached to every request and refreshed before it expires. Headless surfaces (`cox run`, `cox acp`) never wait for a browser: the server is skipped with the warning `run \`cox mcp login <name>\``, which runs the same flow outside a session. `cox mcp logout <name>` forgets the token, and `cox doctor` prints one `mcp auth <name>` row per HTTP server (`ok (expires in 3h)`, `expired`, `none`).

## Worktrees: a task that must not touch this checkout

`cox --worktree t42` runs the session in `_worktrees/<repo>-t42` on branch `t42`, creating both when they do not exist. The location and the name follow the workspace `worktrees` rule: the nearest ancestor of the repository that already holds `_worktrees/` (else a new one next to the repository, or `WT_ROOT`), a lower-case branch cut from a freshly fetched `origin/<default>` with no upstream, and a lock whose reason names the owner (`cox / pid 123 | t42 | 2026-09-22`). The main checkout stays a second workspace root, so the model can read it but every edit lands in the worktree; the status line shows `⎇ t42 +3 −1 · ⧉ t42`, and the presence record carries the worktree path. `/quit` on a clean worktree asks whether to remove it; a dirty one is kept and said so. The branch is never deleted — merging is the user's action. A subagent gets the same thing with `agent(isolation: "worktree")`: its worktree is named after the task id, its answer ends with `[worktree <path>, branch <name>]`, and the worktree outlives the task. Another owner's lock (`Cursor / grok | …`) is never reused or removed.

A worktree is one session's place. Starting a second session in a worktree another live session of the project already holds prints `cox: warning: session <id> (pid <n>) is already working in worktree <path>` and starts anyway — a warning, not a lock. Every session's model is told which worktree each other session works in, `/agents` shows `⧉ <worktree>` on those rows, `cox sessions` marks worktree sessions with `⧉`, and asking for `agent(isolation: "worktree")` is Destructive (it asks, and plan mode refuses it). `cox --resume <id>` of a worktree session reopens it in its worktree with the same roots as `--worktree`; if the worktree has been removed, cox refuses and points at `git worktree list` and `cox --worktree <name> --resume <id>` rather than creating it again.

## The four surfaces, and the macOS app (one stream each)

| Surface | Command | What it does with `Event`s |
|---|---|---|
| TUI | `cox [PROMPT]` | renders transcript, diffs, approval modal, status line |
| Headless | `cox run -p … --output-format text\|json\|stream-json` | prints the stream for scripts |
| Editor | `cox acp` | maps `Event` → ACP `session/update` (see `docs/ide.md`) |
| Other agents | `cox mcp [--allow-write] [--tools a,b]` | serves built-in tools, not the loop (see `docs/compat.md`) |
| macOS app | `Cox.app` (links `cox-ffi` as a static library) | `cox-app` folds them into keyed blocks; Swift pulls `TimelinePatch` batches through UniFFI (see `docs/design/desktop.md`) |
| Remote host | `cox app-server --stdio` (run by the app over your own `ssh`) | serves the same calls and `TimelinePatch` batches as JSON lines, so a host's sessions show in the app; keys and the ssh agent never cross (see `docs/app-server.md`) |

Useful companions: `cox sessions --grep <q>` (find a rollout),
`cox doctor` (keys, sandbox, stale price rows, a configured model with no
catalog price), `cox config show
--sources` (which file each key came from), `cox ext list` (which
instruction files, skills, commands, agents, hooks, MCP servers are in
effect).

## What leaves the session is redacted

Anything cox persists or prints outside the session passes through one
helper, `cox_core::redact::scrub`, which replaces five secret shapes with
`«redacted»`: `sk-…` API keys, `Bearer …` tokens, AWS `AKIA…` key ids,
GitHub `ghp_…` tokens, and PEM blocks (from `-----BEGIN …` through their
`-----END` line). Nothing the model needs for the task passes through it —
redacting model input would break tasks that legitimately handle keys.

Scrubbed: the rollout lines for `TextDelta`, `ToolCallOutput` and
`ToolCallDone` text (the copy `Store::rollout_append` receives; the
in-memory history keeps the original), and `cox run`'s `stream-json`,
`json` and `text` output. Not scrubbed: user text, tool-call input and
thinking (model input), the live TUI/ACP transcript (that *is* the
session), and the tool-output archive (lossless by default). When the
scrub changes a tool result, the session raises `Notice(Security, "tool
output contained a secret-shaped string; redacted in the rollout")` right
behind it.

Two known holes are deliberate: a secret split across streamed deltas is
redacted per delta only, and `resume` rebuilds history from the scrubbed
rollout — a resumed turn's model input is the redacted copy. The
remaining surfaces (`cox sessions --grep` lines, `cox expand` output,
`logs/cox.log` fields, `cox record`'s cassettes) follow in the T28.4b
follow-up card.

## Trust boundaries in one paragraph

Model output, tool results, MCP responses, hook stdout, skill files, and
repository instruction files are all untrusted. Four guards from
`AGENTS.md` cover them, and this doc's examples each touched one: the
**permission engine** authorises every call (Example 3); **path
confinement** (`cox_tools::path::confine`) rejects workspace escapes
before a file tool runs; the **sandbox** confines shell commands and every
stdio MCP server's process unless the session chose `danger-full-access`
or, per server, `sandbox = false` (T33.42); terminal **sanitisation**
strips escape sequences and bidi overrides before anything the model or
a tool wrote is displayed. A broken hook, skill, or MCP server is a
warning and an absence, never a fatal error.
