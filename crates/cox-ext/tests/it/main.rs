// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One integration-test binary for cox-ext: every module below was its own
//! `tests/*.rs` binary, so nextest linked 6 of them (T61.5.3). A module keeps its
//! old name, so `-E 'test(/^skills::/)'` selects what `--test skills` did.
//! `fixtures/` stays in `tests/` because `cox-core` includes a skill from it.

mod agents;
mod claude_settings;
mod commands;
mod hooks;
mod instructions;
mod skills;
