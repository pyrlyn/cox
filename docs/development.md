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
