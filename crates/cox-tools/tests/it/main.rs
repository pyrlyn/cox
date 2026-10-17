// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One integration-test binary for cox-tools: every module below was its own
//! `tests/*.rs` binary, so nextest linked 7 of them (T61.5.3). A module keeps its
//! old name, so `-E 'test(/^confine::/)'` selects what `--test confine` did. The
//! two sandbox modules compile only on their own OS, as before.

#[path = "../common/mod.rs"]
mod common;

mod bash;
mod confine;
mod edit;
mod sandbox_linux;
mod sandbox_macos;
mod v4a;
mod web_fetch;
