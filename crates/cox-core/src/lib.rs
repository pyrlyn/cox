// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The agent loop as a state machine: turns, context assembly, compaction,
//! the permission engine, hooks, model routing, budget. No I/O except
//! through traits in `cox-protocol`, so the loop can be tested by replaying
//! events instead of calling a model.

mod advise;
mod budget;
pub mod cache_diag;
mod checkpoint;
mod compact;
mod context;
mod dedup;
pub mod external_agent;
mod hooks;
pub mod init;
pub mod memory_extract;
pub mod mode;
mod monotone;
mod plugin_model;
pub mod redact;
mod repomap;
mod rewind;
mod rollout;
pub mod router;
mod session;
mod side;
pub mod subagent;
pub mod tasks;
pub mod title;
mod truncate;
mod turn;

/// T32.8: the permission engine moved to its own crate (guard (b), pure —
/// `docs/design/crates.md` C8); re-exported here at the old path so
/// `cox_core::permission::Engine` keeps working for existing callers.
pub use cox_permission as permission;

pub use context::{assemble, assemble_with, microcompact};
pub use permission::{Engine, Outcome};
pub use rollout::{History, HistoryTurn};
pub use session::{MemoryStore, Session};
