// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The pure `grep`/`glob` search engine: gitignore-aware directory walking
//! (`ignore::WalkBuilder`), regex content search (`grep-regex` +
//! `grep-searcher`) and fuzzy path ranking (`nucleo`) — split out of
//! `cox-tools` (T32.5) because these are dependencies (a): each is a heavy,
//! narrowly-used crate that only a rebuild of this leaf needs to recompile.
//!
//! No filesystem writes, no `ToolCx`. The `grep`/`glob` `Tool` impls stay in
//! `cox-tools`: they resolve paths through `cox_tools::path::confine` (the
//! workspace-escape guard) and, for `grep`, archive over-cap results through
//! `cx.archive`, so they need `cox-tools` underneath them. Splitting the
//! pure engine out keeps this crate a leaf while `confine` stays a single
//! call site (docs/design/crates.md, AGENTS.md trust boundaries).
//! `cox-tools` re-exports the `Tool` types at the old `grep`/`glob` paths.

pub mod fuzzy;
pub mod glob;
pub mod grep;
