# cox — examples

Recipes for the command line. The ones marked *offline* were run against cox 0.1.2 built from
`main` with a scratch `COX_HOME`; the ones that call a model need a provider key and are checked
against `cox <command> --help` only. Start with [getting-started.md](getting-started.md) for the
TUI; every config key is in [config.md](config.md).

```bash
export COX_HOME=$(mktemp -d)      # offline: a throwaway home, nothing of yours is read or written
```

## Config *(offline)*

```bash
cox config set tiers.code.model sonnet   # writes <COX_HOME>/config.toml, keeps comments
cox config get tiers.code.model          # sonnet
cox config path                          # where the user config file is
cox config show                          # every effective key, e.g. budget.session_usd = 5.0
```

`config set` writes to the user config file only; a project `.cox/config.toml` cannot set keys that would run a program (see [config.md](config.md)).

## One-off overrides

Flags override the config for a single invocation and are accepted by every subcommand:

```bash
cox --tier code=sonnet --budget 2          # TUI with a $2 session cap
cox --worktree fix-login                   # work in _worktrees/<repo>-fix-login on branch fix-login
cox --cwd ~/code/app --add-dir ~/code/lib  # run from another directory, with a second workspace root
```

## Headless runs

```bash
export ANTHROPIC_API_KEY=sk-...
cox run -p "summarise the diff" --output-format json
cox run -p "list the TODOs" --allowed-tools read,grep,glob --max-turns 5
cox run --loop 10m --max-iterations 3 -p "check whether CI is green and say so"
```

`--output-format` is `text`, `json` or `stream-json`; `--allowed-tools` restricts the run to a
comma-separated list; `--loop` needs `--max-iterations`. Exit codes: `0` ok, `1` error,
`2` denied, `3` budget, `4` interrupted.

## Look back at sessions and cost *(offline: empty on a fresh home)*

```bash
cox sessions                       # No sessions found, until you have run something
cox sessions --grep "migration"    # full-text search over indexed session text
cox sessions <ID>                  # one record: times, provider, model, tokens
cox stats --day                    # usage by day, by tier and job
cox stats --json                   # machine-readable; --csv also works
cox expand <ID> --lines 120-180    # archived tool output, by the id printed next to a truncated result
```

## What is in effect *(offline)*

```bash
cox ext            # instruction files, skills, commands, agents, hooks and MCP servers cox picked up
cox ext list --json
cox plugin         # plugins: none (until you install one)
```

## Use cox's tools from another agent *(offline)*

`cox mcp` serves the built-in tools over MCP on stdio, read-only unless you ask otherwise:

```bash
cox mcp --cwd ~/code/app                 # read, grep, glob
cox mcp --cwd ~/code/app --allow-write   # also edit, write, apply_patch
```

A client speaks JSON-RPC lines. Initialize, then call a tool:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read","arguments":{"path":"a.txt"}}}' \
  | cox mcp --cwd /tmp/demo
```

```
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18", ... "serverInfo":{"name":"cox","version":"0.1.2"} ...}}
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"1\talpha beta\n\n[1 lines total]"}],"isError":false}}
```

The tools are confined to the workspace given by `--cwd`.
