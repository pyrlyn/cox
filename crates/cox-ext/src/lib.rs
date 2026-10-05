// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Instruction files (`AGENTS.md`/`CLAUDE.md` hierarchy), skills
//! (`SKILL.md`), slash commands, subagent definitions, and hook config.
//! Separate because these are user- and repo-supplied extension points, not
//! core agent logic.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod agents;
pub mod claude_settings;
pub mod commands;
pub mod frontmatter;
pub mod hooks;
pub mod instructions;
pub mod memory;
pub mod presence;
pub mod skills;
