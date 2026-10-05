// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The WASM plugin host (A52, `docs/design/plugins.md`): loads a plugin
//! module with extism and runs each plugin on a worker thread of its own. A
//! crate of its own because it is the only one that links extism and
//! wasmtime (plan.md §1.1), so nothing else pays for them.
//!
//! - [`host`] — `PluginHost`: load from bytes, the two queues, per-call
//!   deadlines and the memory cap (PL§4).
//! - [`error`] — `PluginError`, mapped from `extism::Error`.
//! - [`discover`] — finds user and project plugins, validates each
//!   manifest and computes its package digest (PL§1, T33.4).
//! - [`grant`] — the pure grant check and the granted-capability list
//!   (PL§3, T33.6).
//! - [`install`] — the user plugin's on-disk layout: staging a package into
//!   `versions/<digest12>/` and the `current`/`previous` swap (PL§1b, T33.31).
//! - [`hostfn`] — the `cox:host/v1` host functions, each checked against
//!   the grant and the calling export, and `cox_init`'s input (PL§4, T33.9).
//! - [`hooks`] — `PluginHooks`: one plugin as a `Hook` source through
//!   `cox_hook` (PL§6, T33.11).
//! - [`context`] — the event-folded, redacted snapshot `cox_context`
//!   returns (PL§5, T33.9).
//! - [`provider`] — merges a granted plugin's declarative `[[provider]]`
//!   rows into `providers.custom` (PL§7a, T33.17).
//! - [`events`] — the session's `EventTap`: per-plugin drop-oldest rings
//!   delivered to `cox_on_event` in batches (PL§5, T33.10).
//! - [`external_agent`] — a granted `[[external_agents]]` entry resolved
//!   to the sandbox-wrapped command the host spawns, and the in-package
//!   program resolution `[[mcp]]` shares (EA§2, T35.2).
//! - [`live`] — a session's live plugins: one instance per granted plugin,
//!   `cox_init` once, shared by hooks, the tap and tools (T33.44).
//! - [`advisor`] — a plugin's granted decision points behind the protocol's
//!   `Advisor` trait, calling `cox_decide` (PL§4, T33.20).
//! - [`tool`] — `WasmTool`: a plugin's granted tool as a deferred `Tool`
//!   named `wasm__<id>__<tool>`, its spec frozen at `cox_init` (PL§7, T33.12).
//! - [`net`] — `cox_http`: the allow-list, the body cap, and the PL§7d
//!   refusal of a `net` entry that covers a provider host (T33.14.1).

pub mod advisor;
pub mod context;
pub mod discover;
pub mod error;
pub mod events;
pub mod external_agent;
pub mod grant;
pub mod hooks;
pub mod host;
pub mod hostfn;
pub mod install;
pub mod live;
pub mod net;
pub mod provider;
pub mod tool;

pub use advisor::PluginAdvisor;
pub use context::Context;
pub use discover::{Discovered, Plugin, Source, State, package_digest};
pub use error::PluginError;
pub use events::{PluginTap, Redraw, subscriptions};
pub use grant::Verdict;
pub use hooks::PluginHooks;
pub use host::{CONTROL_DEPTH, EVENT_DEPTH, Lane, PluginHost};
pub use hostfn::{HostEnv, init_input};
pub use live::{Live, LivePlugins, Notices, SessionTap};
pub use tool::WasmTool;
