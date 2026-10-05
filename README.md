# cox

[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=listepo_cox&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=listepo_cox) [![Coverage](https://sonarcloud.io/api/project_badges/measure?project=listepo_cox&metric=coverage)](https://sonarcloud.io/component_measures?id=listepo_cox&metric=coverage) [![Tests](https://img.shields.io/sonar/tests/listepo_cox?server=https%3A%2F%2Fsonarcloud.io&compact_message)](https://sonarcloud.io/component_measures?id=listepo_cox&metric=tests)

> A modular terminal coding agent in Rust.

**cox** is named for the coxswain: it steers the work while models, tools, and extensions row. It is being built as one reliable, testable agent core with several ways to use it: an interactive terminal UI, headless automation, editor integration through ACP, and MCP tools for other agents.

[Documentation](https://pyrlyn.github.io/cox/) · [Getting started](docs/getting-started.md) · [Architecture](https://pyrlyn.github.io/cox/docs/architecture/) · [Configuration](https://pyrlyn.github.io/cox/docs/configuration/)

## 60-second start

```bash
git clone https://github.com/pyrlyn/cox && cd cox
mise exec -- cargo build -p cox
export ANTHROPIC_API_KEY=sk-...   # or OPENAI_API_KEY
./target/debug/cox doctor          # green except prices? you are good
./target/debug/cox run -p "create hello.txt containing hi"
./target/debug/cox                 # interactive TUI: Enter sends, Esc interrupts
```

`y`/`s`/`n` answers approval prompts, `/model` switches tiers,
`/compact` compacts now, `cox run -p` drives headless scripts, `cox acp`
serves Zed and JetBrains. Costs land in `cox stats`; see
[`docs/config.md`](docs/config.md) for every key,
[`docs/tools.md`](docs/tools.md) for the tool catalogue and
[`docs/observability.md`](docs/observability.md) to send traces to Jaeger,
Grafana, SigNoz or any OTLP backend.

## Status

cox is under active development. APIs, configuration, and installation instructions are not yet stable. The Rust workspace already contains the protocol, core turn loop, provider adapters, tools, storage, extension loading, TUI, ACP, and MCP crates; features land incrementally against the project plan.

What the TUI looks like: [docs/screenshots](docs/screenshots) holds one SVG per screen state (a streaming reply, an approval prompt, the diff view, …). They are rendered from the same frames the snapshot tests compare, so `just screenshots` regenerates them after a change.

![A streamed reply after a read tool](docs/screenshots/streaming_reply.svg)

![A folded tool card with the head/tail output and the expand hint](docs/screenshots/tool_card_folded.svg)

## Design principles

- **One event stream.** `Submission` values enter a pure core state machine and typed `Event` values leave it. Every surface consumes the same events.
- **Safe by default.** Permission decisions are centralized, model paths stay inside the workspace, and shell commands run in a sandbox unless users deliberately choose otherwise.
- **Lossless context.** Full tool output is archived before it is shortened for model context, so it remains retrievable by ID.
- **Visible costs.** Provider usage is recorded per request; routing between cheap, code, and think tiers is explicit.

## Development

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the short checklist.
Rust is pinned through [mise](https://mise.jdx.dev/). Run Cargo through mise rather than a global toolchain:

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo test --workspace
```

## Documentation site

The public documentation site is a Hugo project in [`website/`](website/), styled with Tailwind CSS.

```bash
cd website
npm ci
npm run build:css
hugo --minify --destination public
```

GitHub Actions builds and deploys the site from `main` to [GitHub Pages](https://pyrlyn.github.io/cox/).

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

For design decisions, milestones, and task checks, see [`plan.md`](plan.md). For evidence behind the design, see [`research.md`](research.md).

## License

You can use this project under **any** of the following licenses, at your choice:

1. [GNU GPLv3](LICENSE): free for open source applications on any platform, including embedded systems.
2. [Royalty-free License](LICENSE-ROYALTY-FREE.md): free for proprietary desktop, mobile, and web applications, as long as you disclose that your application uses this project. Embedded systems are not covered.
3. [Commercial license](PRICING.md): for proprietary applications, including embedded systems, without the attribution requirement.

<!-- license-sync:start -->
Commercial use not covered by the GPLv3 or the Royalty-free License requires a separate paid
license — see [PRICING.md](PRICING.md).
<!-- license-sync:end -->
