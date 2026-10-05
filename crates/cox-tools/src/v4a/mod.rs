// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `apply_patch`: Codex's V4A patch grammar (plan.md D4/D8, T3.5). Parsing,
//! hunk matching and staging (`parse`, `stage`) moved to the pure
//! `cox-patch` crate (T32.6) — no filesystem, no `ToolCx`. [`tool`] is the
//! `Tool` impl: it resolves every path through `crate::path::confine` and
//! writes through `crate::write`, so it stays here rather than moving with
//! the rest of `v4a` — `confine` keeps one call site
//! (docs/design/crates.md, AGENTS.md trust boundaries).

pub mod tool;

pub use cox_patch::{Hunk, HunkLine, Op, Patch, parse};
pub use tool::ApplyPatchTool;
