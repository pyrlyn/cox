// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The resident-session supervisor (T66.19–T66.22, `docs/design/serve.md`
//! S4–S7): the restart decisions, the worker processes, the owner-only
//! socket, the router that sends each client's calls to the right worker,
//! and the `attach --stdio` bridge behind `cox serve`. Its own crate because
//! it owns process supervision and a trust boundary (who may connect), which
//! must stay out of `cox-app` (linked into the macOS app through `cox-ffi`)
//! and out of `crates/cox` (clap and dispatch only). Depends on `cox-store`
//! alone among workspace crates, for the singleton lock.

#[cfg(unix)]
pub mod attach;
pub mod restart;
pub mod router;
#[cfg(unix)]
pub mod socket;
pub mod supervisor;
mod worker;

pub use restart::{Decision, Policy};
pub use router::Workspace;
pub use supervisor::{CAP, Options, Status, SuperviseError, Supervisor, terminated};
