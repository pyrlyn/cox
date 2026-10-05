// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The built-in slash-command table (T37.10, DT§4.3): name, usage and one
//! line of help per command. Shared data rather than TUI code so every
//! surface — the TUI's `/` palette, `/help` and parser, the desktop's
//! completion — lists the same commands; `cox-tui` re-exports it at its old
//! `commands::COMMANDS` path.

/// `(name, usage, what it does)`; the palette lists the names in this order.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "model",
        "/model [cheap|code|think] [model]",
        "switch a tier's model",
    ),
    (
        "think",
        "/think <prompt>",
        "one turn on the think tier, price confirmed first",
    ),
    (
        "mode",
        "/mode architect|editor",
        "switch between planning and editing",
    ),
    (
        "effort",
        "/effort [low|medium|high|xhigh]",
        "effort for the rest of the session; bare restores the tier default",
    ),
    ("compact", "/compact [focus]", "compact the context now"),
    (
        "rewind",
        "/rewind",
        "go back to an earlier turn: code, conversation or both",
    ),
    ("undo", "/undo", "undo the last turn's file changes"),
    ("redo", "/redo", "redo what /undo took back"),
    ("cost", "/cost", "what this session has spent"),
    ("context", "/context", "where the next request's tokens go"),
    (
        "repomap",
        "/repomap [refresh]",
        "show or rebuild the repo map",
    ),
    (
        "autocompact",
        "/autocompact",
        "the compaction threshold and its config source",
    ),
    (
        "permissions",
        "/permissions [default|plan|auto|bypass]",
        "show or set the permission mode",
    ),
    (
        "sandbox",
        "/sandbox <read-only|workspace-write|danger-full-access>",
        "set the sandbox mode",
    ),
    ("resume", "/resume", "pick an earlier session to resume"),
    ("sessions", "/sessions", "this project's recent sessions"),
    ("rename", "/rename <title>", "name this session"),
    (
        "expand",
        "/expand <id>",
        "show an archived tool output in full",
    ),
    ("agents", "/agents", "live cox sessions in this workspace"),
    (
        "loop",
        "/loop <interval> <prompt> [--budget usd] | /loop stop",
        "repeat a prompt on a timer with its own budget cap",
    ),
    ("skills", "/skills", "list skills"),
    ("hooks", "/hooks", "list hooks"),
    ("mcp", "/mcp", "MCP servers and their tools"),
    (
        "plugin",
        "/plugin <new|update|remove|list|reload> ...",
        "manage plugins (PL§13)",
    ),
    ("doctor", "/doctor", "check the install"),
    ("clear", "/clear", "new session, same directory"),
    (
        "fork",
        "/fork [turn]",
        "new child session with the history up to a turn (default: all)",
    ),
    (
        "handoff",
        "/handoff <objective>",
        "new child session seeded with a cheap summary and the objective",
    ),
    (
        "init",
        "/init [--force]",
        "scaffold AGENTS.md for this repo",
    ),
    ("todo", "/todo", "toggle the todo panel"),
    ("tasks", "/tasks", "list running background tasks"),
    ("vim", "/vim", "toggle vim keys"),
    (
        "theme",
        "/theme [name]",
        "pick a colour theme, previewed live",
    ),
    ("help", "/help", "this list"),
    ("quit", "/quit", "exit"),
];
