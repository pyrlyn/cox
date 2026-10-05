// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Shared HTTP plumbing every network provider wire needs on top of a plain
//! `reqwest` call: connection-pooled client construction and credential
//! resolution (env var, then platform keyring), auth headers, and non-2xx →
//! `ProviderError` mapping ([`http`]); generic Server-Sent-Events framing
//! ([`sse`]); and the retry/backoff policy wrapped around one provider
//! stream ([`retry`]).
//!
//! Stripped out of `cox-provider` (T32.12; `docs/design/crates.md` reuse
//! (d)) because Anthropic, OpenAI, Jev and any wire cox-provider gains next
//! all need these three without the rest of any one wire's own
//! dependencies. `cox-provider` re-exports `http`, `retry` and `sse` at
//! their old paths, so `cox_provider::http::resolve_key_with` and
//! `cox_provider::http::resolve_key` (named in AGENTS.md and tests) keep
//! working for existing callers.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod http;
pub mod retry;
pub mod sse;
