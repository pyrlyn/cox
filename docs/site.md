---
title: cox
tagline: A modular terminal coding agent in Rust with a safe, event-driven core.
repo: https://github.com/pyrlyn/cox
homepage: https://pyrlyn.github.io/cox/
install: "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pyrlyn/cox/releases/latest/download/cox-installer.sh | sh"
version: "0.1.0"
accent: "#A8E06C"
accentLight: "#3D8B3A"
order: 3
---

<!-- Website copy for the listepo project site. The sync-docs workflow copies this file to
pyrlyn/landing (main) as content/projects/cox.md on every change to main and on every v*
tag; front matter follows CONTENT_CONTRACT.md in that repository.
Sources (checked 2026-09-27): README.md, docs/ and the clap CLI in crates/cox/src/cli.rs; version
from the latest GitHub release (v0.1.0); accent is the dark-theme --accent in
website/assets/css/main.css. -->

## Overview

**cox** is named for the coxswain: it steers the work while models, tools, and extensions row.
It is one reliable, testable agent core with several ways to use it: an interactive terminal
UI, headless automation, editor integration through ACP, and MCP tools for other agents.

cox is under active development. APIs, configuration, and installation instructions are not
yet stable.

## Features

- **One event stream.** Submissions enter a pure core state machine and typed events leave it.
  The TUI, headless runs, editor clients and MCP all consume the same events.
- **Safe by default.** Permission decisions are centralized, model paths stay inside the
  workspace, and shell commands run in a sandbox (seatbelt on macOS; bubblewrap or Landlock
  with seccomp on Linux) unless you deliberately choose otherwise.
- **Lossless context.** Full tool output is archived before it is shortened for the model, so
  it stays retrievable by id with `cox expand`.
- **Visible costs.** Provider usage is recorded per request in a local ledger; routing between
  the cheap, code and think tiers is explicit. `cox stats` reads it by session, day or month.
- **Many providers.** Anthropic and OpenAI, plus OpenAI-compatible endpoints such as DeepSeek,
  OpenRouter, Moonshot, Z.ai and local servers, configured per tier.
- **Terminal UI.** Streaming replies, approval prompts, a diff view, folded tool cards, vim
  keys and a screen-reader mode (`--plain`).
- **Editors and other agents.** `cox acp` serves Zed and JetBrains over the Agent Client
  Protocol; `cox mcp` serves the built-in tools to other agents (read-only unless you opt in).
- **Observability.** Traces go to Jaeger, Grafana, SigNoz or any OTLP backend.

## Install

Prebuilt binaries for macOS (Apple silicon) and Linux (x86-64 and arm64):

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pyrlyn/cox/releases/latest/download/cox-installer.sh | sh
```

From source (Rust is pinned with mise):

```bash
git clone https://github.com/pyrlyn/cox && cd cox
mise exec -- cargo build -p cox
```

Update an installed binary with `cox self update`.

## Usage examples

Check the machine, then run one prompt headlessly:

```bash
export ANTHROPIC_API_KEY=sk-...   # or OPENAI_API_KEY
cox doctor
cox run -p "create hello.txt containing hi"
```

Open the interactive TUI (Enter sends, Esc interrupts; `y`/`s`/`n` answer approval prompts,
`/model` switches tiers, `/compact` compacts context), optionally with a first prompt:

```bash
cox
cox "explain the layout of this repository"
```

Script a headless run and pick up where it stopped:

```bash
cox run -p "add a unit test for parse_args" --output-format json --max-turns 20
cox run --continue -p "now run the tests"
```

Find a past session, read archived output and costs:

```bash
cox sessions --grep "parse_args"
cox expand <id> --lines 60-90
cox stats --day
cox stats --cache
```

Inspect configuration and extensions:

```bash
cox config show --sources
cox config set tiers.code.model '"claude-sonnet-5"'
cox ext list
```

Serve cox to an editor or another agent:

```bash
cox acp
cox mcp --allow-write
```

## Links

- Repository: <https://github.com/pyrlyn/cox>
- Documentation: <https://pyrlyn.github.io/cox/>
- Getting started: <https://github.com/pyrlyn/cox/blob/main/docs/getting-started.md>
- Releases: <https://github.com/pyrlyn/cox/releases>
- Changelog: <https://github.com/pyrlyn/cox/blob/main/CHANGELOG.md>
- License: your choice of GNU GPLv3, a royalty-free license for proprietary desktop, mobile and web
  apps (with attribution), or a commercial license (see <https://github.com/pyrlyn/cox#license>)
