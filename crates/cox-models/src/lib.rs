// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The model catalog (plan.md T30.24; `docs/design/providers.md` §
//! "Target shape" item 3): one pure crate mapping a model id to its
//! context window, max output, efforts, capabilities and price, merged
//! from three layers, each overriding the last by id — built-in rows
//! (embedded, written only by `cox-vendor models`, AGENTS.md A48) < a
//! `Config`'s `[providers.*].models` entries < a user-supplied
//! `prices.toml` — and the one per-wire effort map over it (T30.26).
//!
//! Depends only on `cox-protocol` (AGENTS.md layout table;
//! `crates/cox/tests/deps.rs` enforces it) and does no I/O beyond parsing
//! an embedded or caller-supplied string — a file on disk is the caller's
//! job (`cox-provider::usage::load_price_table`).

mod catalog;
mod effort;
mod price;

pub use catalog::{
    Capabilities, Catalog, CatalogError, ModelRow, PluginModels, RowSource,
    supports_adaptive_thinking,
};
pub use effort::{Api, WireEffort, effort_for};
pub use price::{Price, PriceError, PriceTable};
