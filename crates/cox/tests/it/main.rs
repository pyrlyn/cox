// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One integration-test binary for cox: every module below was its own
//! `tests/*.rs` binary, so nextest linked 16 of them (T61.5.2). A module keeps its
//! old name, so `-E 'test(/^lsp::/)'` selects what `--test lsp` did. Five binaries
//! stay beside it because a command names each by itself (`docs` and `ide` in
//! `just docs-check`, `deps` in `AGENTS.md`, `trycmd`, `tui_e2e`), and `remote`
//! stays because it sets process environment variables, which would leak into
//! the other tests under a runner that shares one process.

mod cloud_agents;
mod doc_examples;
mod external_agents_cursor;
mod gemini_compat;
mod lsp;
mod mcp_serve;
mod no_real_keychain_in_tests;
mod plain;
mod plugin_cli;
mod plugin_example_dart;
mod plugin_example_go;
mod plugin_example_kotlin;
mod plugin_new_cli;
mod plugins;
mod plugins_jev;
mod run_cli;
mod subagent_messaging;
#[cfg(feature = "plugins")]
mod wasm_example;
