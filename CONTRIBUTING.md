# Contributing

Repository prose is English. Read [`AGENTS.md`](AGENTS.md) before changing
anything.

## Checks

```bash
mise exec -- cargo fmt --check
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo test --workspace
```

User docs live in [`docs/`](docs/); the sync-docs workflow publishes them to the pyrlyn site.
Start from [`docs/getting-started.md`](docs/getting-started.md). Keep claims
aligned with what the crates actually expose — do not invent surfaces or metrics.
