# Architecture

## One event stream

At the center of cox is a pure state machine in `cox-core`. A `Submission` enters the core; a sequence of typed `Event` values leaves it. The terminal UI, headless stream-JSON mode, ACP server, MCP server, session rollout, and tests all consume that same sequence.

This makes the hard part of an agent observable: a test can replay a scripted provider response and assert on the events without opening a network connection or terminal.

## Clear crate boundaries

| Crate | Responsibility |
| --- | --- |
| `cox` | CLI parsing and surface dispatch |
| `cox-protocol` | Cross-crate types and traits |
| `cox-core` | Turns, context, routing, budgets, permissions, compaction |
| `cox-provider` | Model-provider adapters and replay fixtures |
| `cox-tools` | Built-in tools |
| `cox-sandbox` | Path confinement and the platform sandbox |
| `cox-permission` | The permission engine |
| `cox-mcp` | MCP client and server support |
| `cox-store` | SQLite sessions, rollouts, archived output, and cost ledger |
| `cox-ext` | Instructions, skills, commands, agents, and hooks |
| `cox-tui` | Terminal presentation |
| `cox-acp` | Agent Client Protocol adapter |

These are the main crates; [`AGENTS.md`](../AGENTS.md) lists every crate in the workspace and what it owns.

The core owns no direct filesystem, process, or network I/O. Those operations are defined as traits in `cox-protocol` and implemented at the edge of the system.

## Trust boundaries

Everything produced outside cox is untrusted input: model output, tool results, MCP responses, hooks, skills, and repository instruction files. The important guards are centralized rather than copied into individual tools:

1. The permission engine authorizes, denies, or escalates every tool call.
2. Path confinement rejects workspace escapes.
3. The sandbox restricts shell commands unless a user selects full access.
4. Terminal rendering sanitizes escape sequences and bidi overrides.

## Lossless context management

Long tool output is archived *before* the model receives a shortened representation. A later `expand` action can retrieve the complete result by ID. This preserves evidence for users while avoiding needless context growth.
