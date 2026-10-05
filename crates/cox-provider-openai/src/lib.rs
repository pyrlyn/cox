//! The OpenAI-shaped providers. `responses` covers the Responses API
//! (`POST /v1/responses`), which is what OpenAI's own models use; `chat`
//! covers the Chat Completions subset that local servers speak (Ollama,
//! vLLM, LM Studio, llama.cpp, OpenRouter).
//!
//! Both are separate from `anthropic` (and from each other) rather than
//! generalised behind one translator: the wire formats disagree about where
//! system text, reasoning and tool results live, and a shared abstraction
//! would have to be re-specialised at every one of those points.
//!
//! Stripped out of `cox-provider` (T32.14; `docs/design/crates.md`
//! dependencies (a) and size (c)) so `async-openai` is a dependency of this
//! crate alone. `cox-provider` re-exports it at the old `openai` path, so
//! `cox_provider::openai::chat::OpenAiChatProvider` and
//! `cox_provider::openai::responses::OpenAiResponsesProvider` keep working.

// Imported at the crate root so the moved wires' `crate::http`,
// `crate::retry` and `crate::sse` paths resolve exactly as they did inside
// `cox-provider`, which re-exports the same modules (T32.12).
use cox_provider_http::{http, retry, sse};

pub mod chat;
pub mod responses;
mod wire;
