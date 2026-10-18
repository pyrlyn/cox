// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One integration-test binary for cox-app: `intents` and `scenarios` were
//! their own `tests/*.rs` binaries (T61.5.3). A module keeps its old name, so
//! `-E 'test(/^intents::/)'` selects what `--test intents` did. `app`,
//! `best_of`, `external` and `mcp_login` stay beside it because they set
//! process environment variables (`HOME`, `COX_HOME`, `COX_PROVIDER`), which would
//! leak into the other tests under a runner that shares one process.

// cox-core's harness, shared by path as before; it stays in cox-core's `tests/`.
#[path = "../../../cox-core/tests/common/mod.rs"]
mod common;

mod intents;
mod scenarios;
