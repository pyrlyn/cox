// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The MCP client over `rmcp`: `.mcp.json`/config discovery, OAuth, and
//! tool namespacing. Separate from `cox-tools` because MCP servers are
//! untrusted network peers, not built-in tools.
//! `server` is the other direction: cox's own tools offered to another
//! agent over stdio (`cox mcp`). `elicit` maps a server's
//! `elicitation/create` form onto the questions a person answers (T47.1).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod auth;
pub mod client;
pub mod discovery;
pub mod elicit;
pub mod server;
