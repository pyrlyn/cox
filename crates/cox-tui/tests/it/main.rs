// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One integration-test binary for cox-tui: every module below was its own
//! `tests/*.rs` binary, so nextest linked 14 of them (T61.5.3). A module keeps its
//! old name, so `-E 'test(/^vim::/)'` selects what `--test vim` did. `screenshots`
//! stays beside it because `just screenshots` names it by `--test`.

// Shared with `screenshots`, which includes it by path; it stays outside `it/`.
#[path = "../common/mod.rs"]
mod common;

mod agents;
mod approval;
mod cells;
mod diff;
mod frames;
mod keys;
mod question;
mod rewind;
mod sanitize;
mod sessions;
mod shell;
mod status;
mod tasks;
mod vim;
