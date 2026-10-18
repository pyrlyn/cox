# Getting started with cox

cox is a modular terminal coding agent in Rust. One core state machine turns
submissions into typed events; the same stream powers the TUI, headless runs,
editor clients (ACP), and MCP.

## Build and first run

Rust is pinned with [mise](https://mise.jdx.dev/). Prefer `mise exec -- cargo …`
over a global toolchain.

```bash
git clone https://github.com/pyrlyn/cox && cd cox
mise exec -- cargo build -p cox
export ANTHROPIC_API_KEY=sk-...   # or OPENAI_API_KEY
./target/debug/cox doctor         # green except prices? you are good
./target/debug/cox -p "create hello.txt containing hi"
./target/debug/cox                # interactive TUI: Enter sends, Esc interrupts
```

In the TUI: `y` / `s` / `n` answer approval prompts, `/model` switches tiers,
`/compact` compacts context now. `!cmd` runs a shell line through the same
sandbox and rules as the model's `bash`, kept out of the conversation; `!!cmd`
also hands its output to the model. Headless scripts use `cox run -p`; editors use
`cox acp`; other agents can call `cox mcp`. `cox init` scaffolds an `AGENTS.md` for a repository
that has none.

`cox run -p "what is wrong here?" --image shot.png --image log.jpg` attaches
images to the first turn; repeat `--image` for more. Each must be a PNG, JPEG,
GIF or WebP of at most 3.75 MB. A missing file, a non-image or an oversized
image exits 2 with the reason before any request is made.

## Keys

`?` on an empty composer shows this table over the transcript; `/help`
prints it with the slash commands. The composer's empty line shows the first
five rows for where you are (idle, a running turn, a modal, an overlay).
`Ctrl+C` twice quits when idle. A running turn falls back to the idle keys.
`plugin.leader` (`Ctrl+K` by default) arms the next key for a plugin's own
binding; a plugin's slash commands show up in the `/` palette as
`/<id>:<name>`, after the built-ins.

| Key | Action | Context |
| --- | --- | --- |
| `Enter` | send | idle |
| `Shift+Tab` | mode.cycle | idle |
| `@` | file | idle |
| `/` | command | idle |
| `?` | help | idle |
| `Shift+Enter` | newline | idle |
| `Alt+Enter` | newline | idle |
| `Ctrl+Enter` | newline | idle |
| `Ctrl+R` | history | idle |
| `Ctrl+T` | thinking | idle |
| `Ctrl+O` | transcript | idle |
| `Ctrl+E` | expand | idle |
| `Ctrl+G` | diff | idle |
| `Alt+V` | voice | idle |
| `Ctrl+K` | plugin.leader | idle |
| `Ctrl+C` | quit | idle |
| `Ctrl+D` | quit | idle |
| `y` | copy | idle |
| `Shift+Y` | copy.all | idle |
| `Esc` | interrupt | running |
| `Ctrl+C` | interrupt | running |
| `Ctrl+B` | background | running |
| `Ctrl+O` | transcript | running |
| `Alt+Enter` | send.now | running |
| `Ctrl+Enter` | send.now | running |
| `Ctrl+U` | unqueue | running |
| `Enter` | choose | modal |
| `Esc` | close | modal |
| `Up` | previous | modal |
| `Down` | next | modal |
| `Ctrl+E` | theme.edit | modal |
| `Esc` | close | overlay |
| `?` | close | overlay |
| `PageUp` | scroll.up | overlay |
| `PageDown` | scroll.down | overlay |

## Vim keys

Set `tui.vim = true` or type `/vim` to toggle vim keys in the composer. The
status line shows `-- NORMAL --`, `-- INSERT --`, `-- VISUAL --` or
`-- VISUAL LINE --`. `Enter` still sends from any mode.

| Keys | Mode | Effect |
| --- | --- | --- |
| `i` `a` `o` | normal | Insert before / after the cursor, on a new line below |
| `Esc` | insert, visual | Back to normal mode; in insert mode while a turn runs it interrupts the turn instead. In normal mode it only drops a pending command and never interrupts |
| `h j k l` `w b e` `0 ^ $` `gg G` | normal, visual | Motions; a count repeats them (`3w`, `2j`), `5G` goes to line 5 |
| `d` `c` `y` + motion | normal | Delete, change, yank over the motion (`d2w`, `c$`, `yG`) |
| `dd` `cc` `yy` | normal | Whole line; a count takes more lines (`3dd`) |
| `iw aw` `i" a"` `i' a'` `i( a(` `i[ a[` `i{ a{` | after `d c y`, visual | Text objects: inner / around a word, quotes or brackets (`ci"`, `da(`) |
| `x` `X` | normal | Delete the character under / before the cursor |
| `p` `P` | normal | Put after / before the cursor (linewise yanks put whole lines) |
| `u` `Ctrl+R` | normal | Undo / redo; one insert session is one undo step |
| `v` `V` | normal | Characterwise / linewise visual mode; `d` `c` `y` `x` act on the selection |
| `Ctrl+C` | any | Interrupts the running turn |

Not supported: `.` repeat, macros and registers.

## Loop

`/loop <interval> <prompt> [--budget <usd>]` repeats `prompt` on `interval`
(`30s`, `5m`, `1h`, or a bare number of seconds) whenever the session is
idle, until `/loop stop` or `Esc` on an empty composer ends it. `--budget`
is this loop's own spend cap; leave it out and it defaults to the session
cap (`budget.session_usd`), which still applies underneath either way.

The headless counterpart is `cox run --loop <interval> -p "…"
--max-iterations N`: same interval grammar, no `--budget` of its own —
it stops after `N` turns or once the session hits `budget.session_usd`,
whichever comes first. `Ctrl+C` between iterations exits cleanly with
whatever already ran.

## Refine

`/refine <text> [--global]` asks the cheap model to propose a change to
prompt notes, memory, skills or subagent definitions from `text` and the
session so far. The proposal opens as a diff and nothing is written until
you press `y` (or `Enter`); `n` or `Esc` drops it. The change is local to
this session unless you pass `--global`, which writes what every session
reads. Skills and subagents are read when a session opens, so those edits
show up in the next one.

`/refine list` shows this session's refines, newest first. `/refine undo
[<event id>]` reverses the given one, or the newest applied local one; a
refine that changed the global entries is undone only with `--global`.

The same history is available outside the TUI: `cox refine list [--session
<id>]` (default: the newest session in this directory) and `cox refine undo
<event id> [--global]`. Proposing and applying stay in the TUI, where the
diff is reviewed.
## Autonomous runs

`cox run -p "…" --autonomous` runs the shell commands in `[autonomous] gates`
(your own config; a project config cannot set them) after each turn. While one
fails, the run starts another turn with the end of its output, within
`max_continuations`, `max_turns`, `max_tokens`, `timeout_secs` and
`gate_retries`; a run with no gates is refused. Every gate runs in the same
sandbox as the `bash` tool, its full output is archived (`cox expand`), and a
gate over `gate_timeout_secs` counts as failed. Under `--output-format
stream-json` each decision is one `{"type":"autonomous","decision":…}` record,
and the stop reason is printed on stderr and kept in the `json` summary.
`gates_passed` means only that every gate exited 0, not that the task is
done. A run that stops for any other reason of its own exits 1. Not
combinable with `--loop`.

## What to read next

| Doc | Contents |
| --- | --- |
| [config.md](config.md) | Every configuration key |
| [examples.md](examples.md) | Command-line recipes: config, headless runs, sessions and stats, the MCP server |
| [tools.md](tools.md) | Built-in tools, risk, subjects |
| [observability.md](observability.md) | Traces, metrics, OTLP backends |
| [ide.md](ide.md) | Zed, JetBrains, Neovim via ACP |
| [how-it-works.md](how-it-works.md) | One user turn on the event stream |
| [compat.md](compat.md) | What cox reads from `.claude/` / Codex setups |

Costs land in `cox stats`. Screenshots of TUI states live in
[screenshots/](screenshots/).

## Status line

One row under the composer, e.g.
`sonnet-5 · ctx ▰▰▰▱▱ 41% · $0.83/5 · workspace-write · 0 tasks · [plan]`:

- `ctx` is a five-cell mini bar with the context share and percent; the
  filled cells mark the cached share of the last call.
- `$` is the session spend over the session cap (`budget.session_usd`); it
  warns once past `budget.warn_at`.
- `[plan]` is the permission mode badge; `effort:xhigh` appears only when
  `/effort` overrode the tier default.
- `↻ 4m12s` appears only while a `/loop` is running: the time left until its
  next turn. It disappears the moment no loop is active.
- The session's title sits before the mode badge once it has one: generated
  after the first turn (`[session] auto_title`) or set with `/rename <title>`,
  which a generated title never replaces.
- `cox --plain` prints the same segments as one `status: …` line per turn.

Narrow terminals drop segments from the right in this order: loop countdown →
title → git counts → cache → tasks → effort → sandbox → model → cost → ctx (the `ctx`
bar and the mode badge never drop).

### Your own status row

`[tui.status_line]` adds one row above the status line, drawn from the first
line your command prints. It reads the same JSON on stdin as a Claude Code
statusline script, so one you already have runs unchanged:

```toml
# ~/.cox/config.toml (a project config cannot set this)
[tui.status_line]
command = "~/.cox/status.sh"
```

```sh
#!/bin/sh
# ~/.cox/status.sh: model, context share and branch
input=$(cat)
model=$(printf '%s' "$input" | jq -r '.model.display_name')
ctx=$(printf '%s' "$input" | jq -r '.context_window.used_percentage')
branch=$(printf '%s' "$input" | jq -r '.git.branch // "-"')
printf '%s · %s%% · %s\n' "$model" "$ctx" "$branch"
```

It runs sandboxed, read-only and offline, 300 ms after the status changes;
colours are stripped. [config.md](config.md) lists every field and rule.

## Status

cox is under active development. APIs, configuration, and install paths are not
yet stable. Treat this tree as the current manual, not a frozen release surface.
