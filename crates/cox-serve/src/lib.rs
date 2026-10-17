// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The resident-session supervisor (T66.19, `docs/design/serve.md` S4): the
//! decisions and, in later cards, the socket and the worker processes behind
//! `cox serve`. Its own crate because it owns process supervision and a trust
//! boundary (who may connect), which must stay out of `cox-app` (linked into
//! the macOS app through `cox-ffi`) and out of `crates/cox` (clap and
//! dispatch only). Depends on `cox-store` alone, for the singleton lock.

pub mod restart;
#[cfg(unix)]
pub mod socket;

pub use restart::{Decision, Policy};
