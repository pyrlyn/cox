// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `Provider` trait implementations: the Anthropic Messages API, the
//! OpenAI Responses/Chat APIs (also Ollama, vLLM, LM Studio, OpenRouter),
//! the TypeSafe Jev System One API, and the `Replay`/`Scripted` fakes used
//! by tests. Separate from `cox-core` so the agent loop never talks to the
//! network directly (AGENTS.md, D3).
//!
//! Each backend is split the same way: a *pure* request translator (a
//! `Request` in, a `serde_json::Value` body out — no I/O, so it snapshot
//! tests without a key) next to the streaming client that sends it. The
//! wire formats live here and nowhere else; nothing above this crate knows
//! what `cache_control` or `output_config` are.
//!
//! - [`anthropic`] — Messages API: cache breakpoints, adaptive thinking, effort, refusal fallbacks.
//! - [`http`] — credential lookup, auth headers and error mapping shared by every network backend.
//! - [`sse`] — generic Server-Sent-Events framing shared by every SSE-based provider.
//! - [`retry`] — backoff before the first byte, shared by every network backend.
//! - [`lmstudio`] — LM Studio's native `/api/v1` model list and load call (T30.16); not a chat wire.
//!
//! `http`, `retry` and `sse` are re-exports of `cox-provider-http` (T32.12:
//! reuse (d) — every wire needs them without the rest of this crate), kept
//! at these paths so `cox_provider::http::resolve_key_with` and every other
//! `crate::http`/`crate::retry`/`crate::sse` call in this crate's own wires
//! keep working unchanged.
//!
//! [`scripted`] and [`replay`] build on the pure scenario/cassette helpers
//! in `cox-provider-testkit` (T32.11), re-exported at their old paths.
//!
//! [`anthropic`] is a re-export of `cox-provider-anthropic` (T32.13:
//! dependencies (a), the typify build step, and size (c)), kept at this path
//! so `cox_provider::anthropic::*` keeps working.

pub use cox_provider_anthropic as anthropic;
pub use cox_provider_http::http;
pub mod jev;
pub mod lmstudio;
pub mod replay;
pub use cox_provider_http::retry;
pub mod scripted;
pub use cox_provider_http::sse;
pub mod usage;

/// T32.10: token estimation/counting moved to its own crate (dependency
/// (a): `tiktoken-rs` and its BPE data — `docs/design/crates.md`);
/// re-exported here at the old path so `cox_provider::tokens::*` keeps
/// working for existing callers.
pub use cox_tokens as tokens;

/// T32.14: the OpenAI Responses/Chat wires moved to their own crate
/// (dependency (a): `async-openai`, and size (c) — `docs/design/crates.md`);
/// re-exported here at the old path so `cox_provider::openai::*` keeps
/// working for existing callers.
pub use cox_provider_openai as openai;

use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;

/// Builds a test-double [`Provider`] from `COX_PROVIDER`.
///
/// `None` means construct a real provider. `scripted` needs `COX_SCENARIO`;
/// `replay` needs `COX_CASSETTES`.
pub fn from_env() -> Result<Option<Box<dyn Provider>>, ProviderError> {
    match std::env::var("COX_PROVIDER") {
        Ok(name) if name.eq_ignore_ascii_case("scripted") => {
            Ok(Some(Box::new(scripted::Scripted::from_env()?)))
        }
        Ok(name) if name.eq_ignore_ascii_case("replay") => {
            Ok(Some(Box::new(replay::Replay::from_env()?)))
        }
        Ok(name) if name.is_empty() => Ok(None),
        Ok(name) => Err(ProviderError::Unsupported {
            feature: format!("COX_PROVIDER={name}"),
        }),
        Err(_) => Ok(None),
    }
}
