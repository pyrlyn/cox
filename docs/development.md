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

## Testing plugin `cox_http`

The `cox-plugin` `hostfn::tests::http_*` tests run a tiny WAT relay plugin
that calls `cox_http` against a local `wiremock` server on `127.0.0.1`; they
never touch the real network.

Every guest call runs under the plugin's `call_ms` cap (200 ms by default).
The HTTP client behind `cox_http` is one process-wide `reqwest::Client`
(`net.rs`), built host-side as soon as a plugin is granted a `net:` host or
bound a provider section. Building it loads rustls' root store, which takes
roughly 400 ms in a debug build; when it was built lazily inside the first
`cox_http`, that cost landed on the guest's deadline and the call failed with
`Timeout`.

Run them with:

```sh
cargo test -p cox-plugin --lib hostfn::tests::http_
```

Limitations: the warm-up runs once per process, so the first plugin granted
network access pays the build cost at load time. A request that is itself
slower than the plugin's `call_ms` still times out, by design.
