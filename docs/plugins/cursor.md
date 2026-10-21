# Using the Cursor plugin

`plugins/cursor` drives Cursor's own official `agent` CLI as a subagent —
never the Cursor desktop session, never a reverse-engineered proxy
(`docs/design/external-agents.md`, EA§8). It needs the `agent` CLI on
`PATH` and a dashboard-issued `CURSOR_API_KEY`.

## Install, grant, set the key

```bash
cox plugin install plugins/cursor --yes
export CURSOR_API_KEY="<your dashboard-issued key>"
```

Install validates `plugin.toml`, copies the plugin into `COX_HOME`, and
grants the one capability it declares — printed for review before `--yes`
decides it:

```
Cursor (0.1.0)
  Drive Cursor's official `agent` CLI as an ACP or stream-json subagent (P35)
  asks to be able to:
    - agent:cursor agent acp key=CURSOR_API_KEY
plugin cursor enabled
```

Drop `--yes` to answer the `[y/N]` prompt yourself. The key is looked up
by name (`key_env`, D12/A49): an env var, then the platform keyring —
never typed into a config file. Only the dashboard-issued key works; a
desktop-session token is out of scope by design (EA§8).

## Dispatch it

Once granted, `cursor` is one more name `agent(preset: "cursor")` resolves,
alongside built-ins and `.cox/agents/*.md` definitions. The parent model
calls it like any other subagent; a follow-up addressed to it keeps
routing there.

`plugins/cursor/plugin.toml` ships `mode = "acp"` by default; the file
also documents the `stream-json` alternative in a comment. To switch, edit
that `[[external_agents]]` block and reinstall — both modes reach the same
`agent(preset: "cursor")` name.

## Limits, stated honestly

- An ACP `session/request_permission` that would need to ask you is
  **refused**, not queued: the driver has no relay to cox's own approval
  prompt yet, so it fails closed (EA§4). Grant the permissions Cursor needs
  ahead of time, or expect those actions to be declined.
- Each turn spawns a fresh `agent` process; nothing persists between turns
  beyond what Cursor's own CLI keeps on its side.
- No token usage is reported — cox records the turn's cost as $0,
  `billed_externally: true` (EA§6); real spend lands on your Cursor plan.
- The CLI runs under the same sandbox as `bash`, with a minimal env
  (`PATH`, `HOME`, `LANG`, `TERM`, …) plus `CURSOR_API_KEY`.

## When something is missing

`cox doctor` prints one row per granted external agent:

```
external agent cursor: ⚠ agent not found on PATH; key_env CURSOR_API_KEY not set
  fix: install the `agent` CLI or fix `command`
```

A missing CLI or unset key is always a warning, never a hard failure — the
preset is left out of `agent`'s names until both are in place (EA§7).

## Checking a real CLI release (T35.10)

cox's own tests drive a fake `agent` that replays Cursor's documented
messages. To check that a real CLI release still sends those shapes, the
creator records one live run of each mode with their own key:

```bash
COX_LIVE_CURSOR=1 CURSOR_API_KEY="<your key>" just vendor cursor-live-fixtures
mise exec -- cargo nextest run -p cox -E 'test(/external_agents_cursor/)'
```

The recorder refuses to start without `COX_LIVE_CURSOR=1`, reads the key
from the environment only (no keychain) and runs `agent` with a scratch
`HOME` in a scratch git repository, so neither `agent login` state nor the
desktop session is used. It refuses every permission request, redacts the
key, the session and request ids and the scratch paths, and writes
`crates/cox/tests/fixtures/cursor/live_stream_json.json` and
`live_acp.json`. An event type the docs do not list stops it before
anything is written — that is the drift this check exists to catch. The
e2e then replays those files through the same mapper and ACP client as the
documented ones. Nothing runs this in CI.
