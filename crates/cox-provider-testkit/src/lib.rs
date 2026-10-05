// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Pure data types and helpers shared by the `Scripted` and `Replay`
//! test-double providers (plan.md T32.11): scenario parsing, event
//! building, cassette hashing and secret redaction. Depends only on
//! `cox-protocol`, so any crate's tests can parse a scenario or hash a
//! cassette without pulling in `cox-provider`'s real network wire
//! (reqwest, async-openai, tiktoken) — reuse (d).
//!
//! The `Scripted` and `Replay` structs themselves — the `Provider` trait
//! impls that actually stream events or replay a cassette over
//! server-sent events — stay in `cox-provider`. They call
//! `cox-provider`-internal helpers (`tokens::estimate`,
//! `anthropic::stream::AnthropicStream`, `sse::parse_sse_str`) that have
//! not split into their own crates yet (T32.10, T32.12, T32.13); moving
//! the structs here today would make `cox-provider` depend on this crate
//! for its `pub use` re-export while this crate depended back on
//! `cox-provider` for those helpers — a cycle. `cox_provider::scripted`
//! and `cox_provider::replay` re-export what moved here.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod replay;
pub mod scripted;
