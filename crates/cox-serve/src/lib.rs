// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The resident-session supervisor (T66.19, T66.21, `docs/design/serve.md` S4,
//! S6): the restart decisions and the worker processes behind `cox serve`,
//! and in a later card its socket. Its own crate because it owns process
//! supervision and a trust boundary (who may connect), which must stay out of
//! `cox-app` (linked into the macOS app through `cox-ffi`) and out of
//! `crates/cox` (clap and dispatch only). No workspace dependency yet.

pub mod restart;
pub mod supervisor;
mod worker;

pub use restart::{Decision, Policy};
pub use supervisor::{CAP, Options, Status, SuperviseError, Supervisor, terminated};
