# Development

Toolchain, checks and the crate layout. The short checklist is [CONTRIBUTING.md](../CONTRIBUTING.md).

## Development

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the short checklist.
Rust is pinned through [mise](https://mise.jdx.dev/). Run Cargo through mise rather than a global toolchain:

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo test --workspace
```

## Project layout

| Area | Responsibility |
| --- | --- |
| `crates/cox` | CLI surface and dispatch |
| `crates/cox-protocol` | Shared types and traits |
| `crates/cox-core` | Agent state machine, context, permissions, budgets |
| `crates/cox-provider` | Provider adapters and replay fixtures |
| `crates/cox-tools` | Built-in tools, path confinement, sandbox |
| `crates/cox-store` | SQLite sessions, archives, usage ledger |
| `crates/cox-ext` | Instructions, skills, commands, hooks |
| `crates/cox-tui` | Terminal UI |
| `crates/cox-acp` / `crates/cox-mcp` | Editor and MCP integrations |

Cutting a release, and recovering from a failed one: [`release.md`](release.md).

For design decisions, milestones, and task checks, see [`plan.md`](../plan.md). For evidence behind the design, see [`research.md`](../research.md).

## Prompt-cache and tool-serving tests

Two contracts have dedicated tests:

- **Prompt cache (`cache_control`).** `cox-core/src/context.rs` checks the
  breakpoint plan (prefix breakpoint after `system[2]`, then the previous and
  newest message, at most three) and that a growing history keeps the cached
  system prefix byte-identical. `cox-provider-anthropic/src/request.rs` checks
  where markers land (cached system blocks and the last block of a message,
  never tools or the top level), the four-breakpoint clamp, the `ttl` field
  (`5m`/`1h`), byte-identical tools/system across turns (cache hit), and that
  changing a cached system block or the tool set changes the prefix (miss).
  `cox-provider-openai` (`chat.rs`, `responses.rs`) checks that breakpoints
  never emit `cache_control` for APIs that cache automatically.
- **`cox mcp` tool serving.** `crates/cox-mcp/tests/serve_contract.rs` drives
  `ToolServer` over an in-process duplex with an rmcp client: `tools/list`
  names, descriptions and schemas; success result shape; unknown tool as a
  protocol error that leaves the connection usable; invalid arguments and
  `ToolError`s as `is_error` results; eight concurrent calls on one
  connection, each with its own answer and overlapping execution.

Run them with:

```sh
cargo test -p cox-mcp --test serve_contract
cargo test -p cox-provider-anthropic -p cox-provider-openai cache_control
cargo test -p cox-core --lib context::tests
```
