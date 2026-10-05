// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Agent Client Protocol adapter over the same `Event` stream used by
//! the TUI. Separate crate because it is an alternate surface (Zed,
//! JetBrains), not a variation of the terminal UI. The same crate also
//! holds the reverse role, cox as the ACP client of an external agent
//! (`client`, T35.3), since both sides share one protocol dependency.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod client;
pub mod client_events;
pub mod client_tools;
pub mod map;
pub mod server;
mod terminal;

pub use client::{Approver, ClientHost, connect, initialize_request};
pub use client_events::{AgentInfo, UpdateFold};
pub use client_tools::ClientLink;
pub use server::{FactoryRequest, ServerState, SessionFactory, serve_channel, serve_stdio};
