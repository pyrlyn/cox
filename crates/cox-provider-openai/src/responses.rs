// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Pure translation of a provider-neutral [`Request`] into an OpenAI
//! Responses body ([`build_body`]), and the SSE state machine that turns a
//! `POST /v1/responses` stream back into [`ProviderEvent`]s
//! ([`OpenAiResponsesStream`]). Same split as `anthropic::request`/
//! `anthropic::stream` (pure, no I/O) but combined into one file here since
//! T1.3's file list is just this module plus `super` (the client).
//!
//! **`input` shape.** Unlike Anthropic's per-message `content` array,
//! Responses' `input` is one flat list of items: a `Content::Text` becomes a
//! `message` item, a `Content::ToolUse` a `function_call` item, a
//! `Content::ToolResult` a `function_call_output` item — `message_items`
//! walks one `Message`'s content blocks into zero or more of these,
//! `build_body` concatenates every message's items into one flat `input`.
//!
//! **Ids.** Same reasoning as `anthropic::stream`: `call_id` on the wire is
//! an opaque provider string, never a valid ULID, so [`OpenAiResponsesStream`]
//! mints a fresh `CallId::new()` per `function_call` output item instead of
//! parsing it, and `message_items` round-trips that same cox-minted id back
//! out as both `function_call.call_id` and `function_call_output.call_id`.
//!
//! **Tool-call correlation.** `ProviderEvent::ToolUseInputDelta`/`ToolUseEnd`
//! carry no id (plan.md §1.2, same as Anthropic), and the Responses API opens
//! one `function_call` item fully (`output_item.added` … `arguments.done`)
//! before the next even for a parallel batch, so no per-item-id tracking is
//! needed here — unlike `anthropic::stream`, this state machine carries no
//! "current block" at all, because each event's own `type` (not a shared
//! `delta.type` field) already says what it is.
//!
//! **Thinking replay.** OpenAI reasoning items are opaque and provider-
//! specific like Anthropic's signed thinking blocks, but nothing in this
//! task wires up replaying them. A `Content::Thinking` with `signature: None`
//! is dropped silently (nothing to replay); one with `Some(_)` is a real
//! signed block from another provider that this translator cannot honour, so
//! `build_body` returns `ProviderError::Unsupported` rather than silently
//! dropping model-produced state — the one deliverable-mandated difference
//! from Anthropic's request builder, which stays infallible.
//! ponytail: reasoning-item replay unimplemented; add an `OpenAi`-specific
//! `Content` field (or a lookaside) when a task needs it end to end.
//!
//! **Wire types (T30.11 / A40 step 1).** Requests are built through
//! `wire::CreateResponse` and friends (`async-openai`'s typed Responses-API
//! structs) rather than a hand-written `json!` body, and known stream events
//! deserialize into the matching `wire::Response*Event` struct rather than a
//! `Value` field walk. `reorder_body` exists only because the typed structs'
//! derived field order doesn't match the wire order this file's snapshots
//! pin byte-for-byte — see its own doc comment. See `wire.rs`'s header for
//! why the crate's own top-level `ResponseStreamEvent` enum and the full
//! `Response` object are deliberately not used.

use async_trait::async_trait;
use cox_protocol::config::ProviderModel;
use cox_protocol::errors::ProviderError;
use cox_protocol::ids::CallId;
use cox_protocol::traits::Provider;
use cox_protocol::types::{
    Caps, Content, Effort, Message, ProviderEvent, ProviderId, Request, Role, StopReason, Usage,
};
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::wire;

/// Translates a `Request` into the JSON body for `POST /v1/responses`,
/// built through `wire::CreateResponse` (see module header). Errors when
/// history carries a signed thinking block (see module header), or in the
/// unreachable-in-practice case that serializing the typed request fails —
/// every field this function sets is a plain owned `String`/number/enum/
/// `serde_json::Value`, none of which `serde_json::to_value` can reject.
pub fn build_body(req: &Request) -> Result<Value, ProviderError> {
    let mut items = Vec::new();
    for m in &req.messages {
        items.extend(message_items(m)?);
    }

    let tools: Vec<wire::Tool> = req
        .tools
        .iter()
        .map(|t| {
            wire::Tool::Function(wire::FunctionTool {
                name: t.name.clone(),
                description: Some(t.description.clone()),
                parameters: Some(t.input_schema.clone()),
                ..Default::default()
            })
        })
        .collect();

    let create = wire::CreateResponse {
        model: Some(req.model.0.clone()),
        input: wire::InputParam::Items(items),
        stream: Some(true),
        store: Some(false),
        max_output_tokens: Some(req.max_tokens),
        // Responses reads no row capability (`cox_models::effort_for`).
        reasoning: cox_models::effort_for(
            cox_models::Api::Responses,
            req.effort,
            &cox_models::Capabilities::default(),
        )
        .map(|w| wire::Reasoning {
            effort: Some(effort(w.effort)),
            ..Default::default()
        }),
        instructions: (!req.system.is_empty()).then(|| {
            req.system
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        }),
        tools: (!tools.is_empty()).then_some(tools),
        ..Default::default()
    };

    let body = serde_json::to_value(&create).map_err(|e| ProviderError::BadRequest {
        message: format!("serializing OpenAI Responses request: {e}"),
    })?;
    Ok(reorder_body(body))
}

/// `CreateResponse`'s derived field order (roughly alphabetical by field
/// name) doesn't match the wire order this file's snapshots pin — the order
/// the hand-written `json!` builder this task replaced produced. JSON object
/// key order carries no meaning to the API, but the snapshots are
/// byte-for-byte pins (T30.11), so this reassembles the top-level object
/// into that exact key order after building the body through the typed
/// request; the two nested item/tool shapes whose typed order also disagrees
/// (`function_call`, the `function` tool) get the same treatment first.
const BODY_KEY_ORDER: [&str; 8] = [
    "model",
    "input",
    "stream",
    "store",
    "max_output_tokens",
    "reasoning",
    "instructions",
    "tools",
];

fn reorder_body(mut body: Value) -> Value {
    if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            reorder_in_place(
                item,
                "function_call",
                &["type", "call_id", "name", "arguments"],
            );
        }
    }
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools.iter_mut() {
            reorder_in_place(
                tool,
                "function",
                &["type", "name", "description", "parameters"],
            );
        }
    }
    canonical_order(body, &BODY_KEY_ORDER)
}

/// Reassembles `value`'s keys into `order` when its `"type"` is `only_type`
/// — the one item/tool shape (of the ones this file emits) whose typed
/// field order disagrees with the pinned wire order. Every other shape
/// (`message`, `function_call_output`) already serializes in the order the
/// snapshots pin, so this leaves them untouched.
fn reorder_in_place(value: &mut Value, only_type: &str, order: &[&str]) {
    if value.get("type").and_then(Value::as_str) != Some(only_type) {
        return;
    }
    let owned = std::mem::take(value);
    *value = canonical_order(owned, order);
}

/// Rebuilds a JSON object's keys in `order`. Drops nothing: a key `order`
/// doesn't name is appended after (in its original relative order), so a
/// field this function's caller forgot to list is still visible rather than
/// silently lost.
fn canonical_order(value: Value, order: &[&str]) -> Value {
    let Value::Object(mut obj) = value else {
        return value;
    };
    let mut ordered = serde_json::Map::with_capacity(obj.len());
    for key in order {
        if let Some(v) = obj.remove(*key) {
            ordered.insert((*key).to_string(), v);
        }
    }
    ordered.extend(obj);
    Value::Object(ordered)
}

fn role(r: Role) -> wire::WireRole {
    match r {
        Role::User => wire::WireRole::User,
        Role::Assistant => wire::WireRole::Assistant,
    }
}

/// One message's content blocks as flat `input` items (see module header).
fn message_items(m: &Message) -> Result<Vec<wire::InputItem>, ProviderError> {
    let mut items = Vec::new();
    for c in &m.content {
        match c {
            Content::Text { text } => {
                items.push(wire::InputItem::EasyMessage(wire::EasyInputMessage {
                    role: role(m.role),
                    content: wire::EasyInputContent::Text(text.clone()),
                    ..Default::default()
                }))
            }
            Content::ToolUse { id, name, input } => items.push(wire::InputItem::Item(
                wire::Item::FunctionCall(wire::FunctionToolCall {
                    arguments: input.to_string(),
                    call_id: id.to_string(),
                    namespace: None,
                    name: name.clone(),
                    id: None,
                    status: None,
                    caller: None,
                    r#async: None,
                }),
            )),
            Content::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                // `function_call_output` has no dedicated error flag on the
                // wire (unlike Anthropic's `tool_result.is_error`); folding
                // it into the text is the only way to carry it through.
                let output = if *is_error {
                    format!("Error: {content}")
                } else {
                    content.clone()
                };
                items.push(wire::InputItem::Item(wire::Item::FunctionCallOutput(
                    wire::FunctionCallOutputItemParam {
                        call_id: Some(call_id.to_string()),
                        output: wire::FunctionCallOutput::Text(output),
                        id: None,
                        status: None,
                        name: None,
                        namespace: None,
                        caller: None,
                    },
                )));
            }
            Content::Image {
                media_type,
                data_b64,
            } => items.push(wire::InputItem::EasyMessage(wire::EasyInputMessage {
                role: role(m.role),
                // `InputImageContent::detail` has no `skip_serializing_if`
                // (only `#[serde(default)]`), so this also emits an explicit
                // `"detail":"auto"` the hand-written version never sent —
                // harmless (it's the documented API default);
                // `responses_request_user_image` pins it.
                content: wire::EasyInputContent::ContentList(vec![wire::InputContent::InputImage(
                    wire::InputImageContent {
                        image_url: Some(format!("data:{media_type};base64,{data_b64}")),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            })),
            // Microcompaction: same treatment as `anthropic::request`.
            Content::Pointer { archive, summary } => {
                items.push(wire::InputItem::EasyMessage(wire::EasyInputMessage {
                    role: role(m.role),
                    content: wire::EasyInputContent::Text(format!(
                        "[archived: {summary}; expand {}]",
                        archive.id
                    )),
                    ..Default::default()
                }))
            }
            Content::Thinking { signature, .. } => {
                if signature.is_some() {
                    return Err(ProviderError::Unsupported {
                        feature: "thinking replay".into(),
                    });
                }
                // No signature: nothing to replay, drop silently.
            }
        }
    }
    Ok(items)
}

/// Type conversion only: which level to send is `cox_models::effort_for`'s.
fn effort(e: Effort) -> wire::ReasoningEffort {
    match e {
        Effort::Low => wire::ReasoningEffort::Low,
        Effort::Medium => wire::ReasoningEffort::Medium,
        Effort::High => wire::ReasoningEffort::High,
        Effort::Xhigh => wire::ReasoningEffort::Xhigh,
    }
}

/// The state carried across one `POST /v1/responses` SSE body: just the
/// usage counters (see module header on why no "current block" is needed).
#[derive(Debug)]
pub struct OpenAiResponsesStream {
    usage: Usage,
    /// SSE frame ordinal, for `ProviderError::Parse { line }`.
    frame_no: u64,
}

impl Default for OpenAiResponsesStream {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiResponsesStream {
    /// Starts a fresh state machine for one streamed call.
    pub fn new() -> Self {
        Self {
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                estimated: false,
                cost_usd: 0.0,
                latency_ms: 0,
            },
            frame_no: 0,
        }
    }

    /// The usage accumulated so far (cost/latency filled in by the caller).
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// Feeds one SSE frame and returns the `ProviderEvent`s it produces.
    pub fn feed(
        &mut self,
        event: Option<&str>,
        data: &str,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.frame_no += 1;
        let value: Value = serde_json::from_str(data).map_err(|_| ProviderError::Parse {
            line: self.frame_no,
        })?;
        // The JSON body's own "type" mirrors the SSE `event:` name (same
        // fallback `anthropic::stream` uses), so a frame missing the SSE
        // field is still routed correctly.
        let kind = event
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        match kind.as_str() {
            "response.output_text.delta" => {
                let event: wire::ResponseTextDeltaEvent = self.typed(&value)?;
                Ok(vec![ProviderEvent::TextDelta { text: event.delta }])
            }
            "response.output_item.added" => self.on_output_item_added(&value),
            "response.function_call_arguments.delta" => {
                let event: wire::ResponseFunctionCallArgumentsDeltaEvent = self.typed(&value)?;
                Ok(vec![ProviderEvent::ToolUseInputDelta { text: event.delta }])
            }
            "response.function_call_arguments.done" => {
                // Nothing here carries data `ToolUseEnd` needs; deserializing
                // anyway validates the frame's shape like every other known
                // event, instead of trusting the SSE `event:` name alone.
                let _event: wire::ResponseFunctionCallArgumentsDoneEvent = self.typed(&value)?;
                Ok(vec![ProviderEvent::ToolUseEnd])
            }
            "response.completed" => self.on_completed(&value),
            "error" => self.on_error(&value).map(|e| vec![e]),
            // response.created/in_progress, content_part.*, output_text.done,
            // output_item.done, ping and anything unrecognised: no event, no
            // error — a stray frame must never fail the whole call.
            _ => Ok(vec![]),
        }
    }

    /// Deserializes `value` into a known event's typed payload; a shape
    /// mismatch (missing/wrong-typed field) is a `Parse` error, same
    /// precedent as the `item`/`response` container checks below — only an
    /// *unrecognised* `type` is ever silently ignored (`feed`'s `_` arm).
    fn typed<T: serde::de::DeserializeOwned>(&self, value: &Value) -> Result<T, ProviderError> {
        serde_json::from_value(value.clone()).map_err(|_| ProviderError::Parse {
            line: self.frame_no,
        })
    }

    fn on_output_item_added(&mut self, v: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        let item = v.get("item").ok_or(ProviderError::Parse {
            line: self.frame_no,
        })?;
        // A `message` item, any other known item type, or one
        // `wire::OutputItem` has no variant for (an `Err` here): none of
        // them start a tool call. Text arrives via `response.output_text.
        // delta`, so there is nothing to emit — same "ignore, don't fail
        // the call" fallback the hand-written `Value` check gave every
        // non-`function_call` item.
        match serde_json::from_value::<wire::OutputItem>(item.clone()) {
            Ok(wire::OutputItem::FunctionCall(call)) => Ok(vec![ProviderEvent::ToolUseStart {
                id: CallId::new(),
                name: call.name,
            }]),
            _ => Ok(vec![]),
        }
    }

    fn on_completed(&mut self, v: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        let response = v.get("response").ok_or(ProviderError::Parse {
            line: self.frame_no,
        })?;
        // Only `response.usage` is typed, not the whole `response` object
        // (see `wire.rs`'s header on why not); a shape that doesn't match
        // `wire::ResponseUsage` means "no usable usage in this frame", same
        // as the old `if let Some(usage) = response.get("usage")` skip.
        if let Some(usage) = response
            .get("usage")
            .and_then(|u| serde_json::from_value::<wire::ResponseUsage>(u.clone()).ok())
        {
            self.apply_usage(&usage);
        }
        Ok(vec![
            // §1.2 StopReason: a provider only ever emits EndTurn/Refusal/
            // Error; the Responses API surfaces refusal as a `refusal`
            // content part rather than a terminal status this task covers.
            ProviderEvent::Stop {
                stop: StopReason::EndTurn,
            },
            ProviderEvent::Usage { usage: self.usage },
        ])
    }

    fn on_error(&mut self, v: &Value) -> Result<ProviderEvent, ProviderError> {
        let event: wire::WireErrorEvent = self.typed(v)?;
        let code = event.code.unwrap_or_default();
        let mapped = if code.contains("rate_limit") {
            ProviderError::RateLimited { retry_after: None }
        } else if code.contains("auth") || code.contains("api_key") {
            ProviderError::Auth
        } else {
            ProviderError::BadRequest {
                message: event.message,
            }
        };
        Ok(ProviderEvent::Error { error: mapped })
    }

    /// `wire::ResponseUsage`'s fields are all required, so `on_completed`
    /// only calls this once the whole shape has already deserialized —
    /// unlike the hand-written version, a partially-shaped usage object
    /// (missing e.g. `output_tokens`) skips the update entirely rather than
    /// applying the fields that were present; real Responses usage objects
    /// always carry all of them together. `cache_write_tokens` stays 0:
    /// unlike Anthropic, OpenAI does not bill a separate cache-write cost,
    /// and `wire::ResponseUsage` carries no such field.
    fn apply_usage(&mut self, usage: &wire::ResponseUsage) {
        self.usage.input_tokens = usage.input_tokens;
        self.usage.output_tokens = usage.output_tokens;
        self.usage.cache_read_tokens = usage.input_tokens_details.cached_tokens;
    }
}

/// A configured Responses-API client (`POST /v1/responses`): what OpenAI's
/// own models use, and what any gateway passing `api = "responses"` speaks.
/// Same thin-client shape as `chat::OpenAiChatProvider`: [`build_body`]
/// translates, [`OpenAiResponsesStream`] parses, this sends one and drives
/// the other over the shared `sse` framing.
pub struct OpenAiResponsesProvider {
    /// `providers.openai.base_url`, without a trailing slash.
    pub base_url: String,
    /// `None` means no `Authorization` header at all; `Some` is a bearer token.
    pub api_key: Option<String>,
    /// Known models with their context windows; the roomiest bounds
    /// [`Caps::max_context`] (which drives the compaction trigger).
    pub models: Vec<ProviderModel>,
    /// Fallback context window when `models` names nothing.
    pub context_window: u32,
    /// The shared connection pool.
    pub http: reqwest::Client,
    /// Backoff for transient failures before the first byte.
    pub retry: crate::retry::Policy,
}

impl OpenAiResponsesProvider {
    /// Builds a client for any `api = "responses"` section — `[providers.
    /// openai]` or any compatible section passing that shape (T30.23:
    /// `openai_shaped` in `crates/cox-session/src/provider.rs` is the one
    /// production caller). `api_key` is already resolved by the caller
    /// (`None` means no `Authorization` header at all).
    pub fn new(
        transport: &cox_protocol::config::Transport,
        api_key: Option<String>,
        models: Vec<ProviderModel>,
        context_window: u32,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: transport.base_url.trim_end_matches('/').to_string(),
            api_key,
            models,
            context_window,
            http: crate::http::client_with_timeout(transport.timeout_s)?,
            retry: crate::retry::Policy {
                max_retries: transport.max_retries,
                ..Default::default()
            },
        })
    }

    /// The roomiest known context window: a listed model if the request
    /// names one, else the section fallback.
    pub fn context_for(&self, model: &str) -> u32 {
        self.models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.context_window)
            .filter(|c| *c > 0)
            .unwrap_or(self.context_window)
    }
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }

    /// Responses is OpenAI's own wire, whose models take `input_image`.
    fn accepts_images(&self, _model: &str) -> bool {
        true
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            // `reasoning.effort` is sent, but reasoning summaries are not
            // surfaced as `ThinkingDelta` (see the module header), so this
            // stays false until replay/surface lands.
            thinking: false,
            server_tools: false,
            count_tokens: false,
            max_context: self
                .models
                .iter()
                .map(|m| m.context_window)
                .max()
                .filter(|c| *c > 0)
                .unwrap_or(self.context_window),
        }
    }

    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        crate::retry::stream_with_retry(self.retry, sink, cancel, |sink, cancel| {
            self.stream_once(&req, sink, cancel)
        })
        .await
    }

    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Err(ProviderError::Unsupported {
            feature: "count_tokens".into(),
        })
    }
}

impl OpenAiResponsesProvider {
    /// One HTTP attempt; `stream` wraps it in the retry policy.
    async fn stream_once(
        &self,
        req: &Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let started = std::time::Instant::now();
        let body = build_body(req)?;

        let mut request = self
            .http
            // CodeQL cleartext-transmission: the key travels only in the
            // Authorization header; base_url is user-configured (https by default).
            .post(format!("{}/responses", self.base_url))
            .header("content-type", "application/json")
            .json(&body);
        if let Some(key) = &self.api_key {
            request = request.header("authorization", crate::http::bearer(key)?);
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout
            } else {
                ProviderError::Network
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::http::map_http_error(status, &body_text, retry_after));
        }

        let mut frames = std::pin::pin!(crate::sse::sse_stream(response.bytes_stream()));
        let mut machine = OpenAiResponsesStream::new();
        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                frame = frames.next() => frame,
            };
            let Some(frame) = next else {
                break;
            };
            let (event, data) = frame.map_err(|_| ProviderError::Network)?;
            for provider_event in machine.feed(event.as_deref(), &data)? {
                // The receiving end hung up: unwind as a cancellation
                // rather than silently dropping the rest of the call.
                if sink.send(provider_event).await.is_err() {
                    return Err(ProviderError::Cancelled);
                }
            }
        }

        let mut usage = machine.usage();
        usage.latency_ms = started.elapsed().as_millis() as u64;
        Ok(usage)
    }
}

/// Test-only: normalizes freshly minted `ToolUseStart` ids into stable,
/// counter-derived ones, same purpose (and same reasoning) as
/// `anthropic::stream::normalize_tool_ids` — kept as its own copy rather than
/// reused across modules since it is `#[cfg(test)]`-only, one caller each.
#[cfg(test)]
fn normalize_tool_ids(events: Vec<ProviderEvent>) -> Vec<ProviderEvent> {
    let mut next = 0u32;
    events
        .into_iter()
        .map(|event| match event {
            ProviderEvent::ToolUseStart { name, .. } => {
                let id = format!("{next:026}")
                    .parse()
                    .expect("26 decimal digits is a valid ULID shape");
                next += 1;
                ProviderEvent::ToolUseStart { id, name }
            }
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;

    use cox_protocol::ids::{ArchiveId, CallId};
    use cox_protocol::types::{
        ArchiveRef, Concurrency, Job, ModelId, Risk, SystemBlock, Thinking, Tier, ToolSpec,
    };
    use serde_json::json;

    use super::*;
    use crate::sse::parse_sse_str;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/openai-responses")
            .join(format!("{name}.sse"));
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path:?}: {e}"))
    }

    fn run_fixture(name: &str) -> Vec<ProviderEvent> {
        let mut stream = OpenAiResponsesStream::new();
        let mut events = Vec::new();
        for (event, data) in parse_sse_str(&fixture(name)) {
            events.extend(
                stream
                    .feed(event.as_deref(), &data)
                    .expect("fixture is well-formed"),
            );
        }
        normalize_tool_ids(events)
    }

    #[test]
    fn responses_stream_text_only() {
        insta::assert_json_snapshot!("responses_stream_text_only", run_fixture("text_only"));
    }

    #[test]
    fn responses_stream_one_tool_call() {
        insta::assert_json_snapshot!(
            "responses_stream_one_tool_call",
            run_fixture("one_tool_call")
        );
    }

    #[test]
    fn responses_stream_parallel_tool_calls() {
        let events = run_fixture("parallel_tool_calls");
        let starts = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolUseStart { .. }))
            .count();
        assert_eq!(starts, 2, "expected two parallel function_call items");
        insta::assert_json_snapshot!("responses_stream_parallel_tool_calls", events);
    }

    #[test]
    fn responses_stream_usage_reads_cached_tokens() {
        let events = run_fixture("one_tool_call");
        let usage = events.iter().find_map(|e| match e {
            ProviderEvent::Usage { usage } => Some(*usage),
            _ => None,
        });
        assert_eq!(usage.map(|u| u.cache_read_tokens), Some(50));
    }

    #[test]
    fn responses_stream_malformed_json_is_a_parse_error_not_a_panic() {
        let mut stream = OpenAiResponsesStream::new();
        let err = stream
            .feed(Some("response.output_text.delta"), "{not json")
            .unwrap_err();
        assert!(matches!(err, ProviderError::Parse { line: 1 }));
    }

    #[test]
    fn responses_stream_unknown_event_is_ignored_not_fatal() {
        let mut stream = OpenAiResponsesStream::new();
        let events = stream
            .feed(Some("response.some_future_event"), "{}")
            .expect("ignored");
        assert!(events.is_empty());
    }

    /// `wire::ResponseTextDeltaEvent` (and every other typed event struct
    /// `feed` deserializes into) has no `#[serde(deny_unknown_fields)]`, so
    /// a field OpenAI adds tomorrow doesn't break a known event today.
    #[test]
    fn responses_stream_unknown_field_on_known_event_is_ignored() {
        let mut stream = OpenAiResponsesStream::new();
        let events = stream
            .feed(
                Some("response.output_text.delta"),
                r#"{"type":"response.output_text.delta","sequence_number":0,
                    "item_id":"msg_001","output_index":0,"content_index":0,
                    "delta":"hi","future_field":{"nested":true}}"#,
            )
            .expect("unknown fields on a known event are ignored, not fatal");
        assert_eq!(events, vec![ProviderEvent::TextDelta { text: "hi".into() }]);
    }

    fn call(n: u8) -> CallId {
        CallId::from_str(&format!("01ARZ3NDEKTSV4RRFFQ69G5FA{n}")).expect("valid ulid")
    }

    fn base(model: &str) -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId(model.into()),
            system: vec![
                SystemBlock {
                    text: "<tool specs>".into(),
                    cache: true,
                },
                SystemBlock {
                    text: "You are cox.".into(),
                    cache: true,
                },
            ],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
                risk: Risk::ReadOnly,
                concurrency: Concurrency::Parallel,
            }],
            messages: vec![],
            effort: Effort::High,
            max_tokens: 16384,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    fn user_text(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Content::Text { text: text.into() }],
        }
    }

    #[test]
    fn responses_request_plain_text() {
        let mut req = base("gpt-5.1");
        req.messages = vec![user_text("what does cox-provider own?")];

        let body = build_body(&req).expect("no thinking blocks, never fails");
        insta::assert_json_snapshot!(body);
    }

    /// T37.6 Check: an attached image reaches the wire as `input_image`.
    #[test]
    fn responses_request_user_image() {
        let mut req = base("gpt-5.1");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![
                Content::Text {
                    text: "what is in this screenshot?".into(),
                },
                Content::Image {
                    media_type: "image/png".into(),
                    data_b64: "iVBORw0KGgo=".into(),
                },
            ],
        }];

        let body = build_body(&req).expect("no thinking blocks, never fails");
        insta::assert_json_snapshot!(body);
    }

    #[test]
    fn responses_request_parallel_tool_calls() {
        let mut req = base("gpt-5.1");
        req.messages = vec![
            user_text("read both files"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Content::ToolUse {
                        id: call(1),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                    Content::ToolUse {
                        id: call(2),
                        name: "read".into(),
                        input: json!({"path": "b.rs"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![
                    Content::ToolResult {
                        call_id: call(1),
                        content: "fn a() {}".into(),
                        is_error: false,
                    },
                    Content::ToolResult {
                        call_id: call(2),
                        content: "no such file".into(),
                        is_error: true,
                    },
                ],
            },
        ];

        let body = build_body(&req).expect("no thinking blocks, never fails");
        let dumped = serde_json::to_string(&body).expect("serializes");
        assert!(dumped.contains("\"function_call\""));
        assert!(dumped.contains("\"function_call_output\""));
        assert!(dumped.contains("Error: no such file"));
        insta::assert_json_snapshot!(body);
    }

    #[test]
    fn responses_request_pointer_and_compaction_summary() {
        let mut req = base("gpt-5.1");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![
                Content::Pointer {
                    archive: ArchiveRef {
                        id: ArchiveId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FB0").expect("valid ulid"),
                        bytes: 91_000,
                    },
                    summary: "bash cargo test: 91000 bytes, exit 0".into(),
                },
                Content::Text {
                    text: "now fix the failing test".into(),
                },
            ],
        }];

        let body = build_body(&req).expect("no thinking blocks, never fails");
        insta::assert_json_snapshot!(body);
    }

    #[test]
    fn responses_request_signed_thinking_is_unsupported() {
        let mut req = base("gpt-5.1");
        req.messages = vec![Message {
            role: Role::Assistant,
            content: vec![Content::Thinking {
                text: "the parser is recursive descent".into(),
                signature: Some("sig-from-another-provider".into()),
            }],
        }];

        let err =
            build_body(&req).expect_err("a signed thinking block must not be dropped silently");
        assert!(matches!(err, ProviderError::Unsupported { .. }));
    }

    #[test]
    fn responses_request_unsigned_thinking_is_dropped_silently() {
        let mut req = base("gpt-5.1");
        req.messages = vec![Message {
            role: Role::Assistant,
            content: vec![Content::Thinking {
                text: "thought, never signed".into(),
                signature: None,
            }],
        }];

        let body = build_body(&req).expect("no signature: nothing to replay, not an error");
        assert_eq!(body["input"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn responses_request_effort_maps_to_reasoning() {
        let mut req = base("gpt-5.1");
        req.effort = Effort::Xhigh;
        let body = build_body(&req).expect("no thinking blocks, never fails");
        assert_eq!(body["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn responses_request_medium_effort_is_sent_as_medium() {
        let mut req = base("gpt-5.1");
        req.effort = Effort::Medium;
        let body = build_body(&req).expect("no thinking blocks, never fails");
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    /// "Done when": a wiremock shaped like `POST /responses` completes a
    /// tool-call turn end to end through the live client, with the bearer
    /// header the key implies.
    #[tokio::test]
    async fn responses_over_http() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/responses"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer sk-test",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(fixture("one_tool_call"), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = OpenAiResponsesProvider::new(
            &transport(&server.uri()),
            Some("sk-test".into()),
            vec![],
            400_000,
        )
        .expect("client builds");
        let mut req = base("gpt-5.1");
        req.messages = vec![user_text("read a.rs")];

        let (tx, mut rx) = mpsc::channel(64);
        let usage = client
            .stream(req, tx, CancellationToken::new())
            .await
            .expect("mock matched, so the header was sent");
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::ToolUseStart { .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Stop {
                stop: StopReason::EndTurn
            }
        )));
        assert_eq!(usage.input_tokens, 512);
        assert_eq!(usage.output_tokens, 24);
        assert_eq!(usage.cache_read_tokens, 50);
    }

    /// A section's `&Transport` with test-friendly defaults; individual
    /// tests override `max_retries` where the retry count is what's under
    /// test.
    fn transport(base_url: &str) -> cox_protocol::config::Transport {
        cox_protocol::config::Transport {
            base_url: base_url.to_string(),
            api_key_env: String::new(),
            timeout_s: 120,
            max_retries: 4,
        }
    }

    #[test]
    fn responses_context_for_prefers_listed_model() {
        use cox_protocol::config::ProviderModel;
        let client = OpenAiResponsesProvider::new(
            &transport("https://api.openai.com/v1"),
            None,
            vec![ProviderModel {
                id: "gpt-5.5".into(),
                context_window: 1_050_000,
                efforts: vec![],
                ..Default::default()
            }],
            400_000,
        )
        .expect("client builds");
        assert_eq!(client.context_for("gpt-5.5"), 1_050_000);
        assert_eq!(client.context_for("gpt-unknown"), 400_000);
        assert_eq!(client.capabilities().max_context, 1_050_000);
        let bare = OpenAiResponsesProvider::new(&transport("https://x"), None, vec![], 400_000)
            .expect("client builds");
        assert_eq!(bare.capabilities().max_context, 400_000);
    }

    #[test]
    fn responses_provider_defaults_retry_policy() {
        let provider = OpenAiResponsesProvider::new(
            &transport("https://api.openai.com/v1"),
            None,
            vec![],
            400_000,
        )
        .expect("client builds");
        assert_eq!(provider.retry.max_retries, 4);
    }

    /// T30.23 Check ("same for Responses if cheap"): a Responses section
    /// with `max_retries = 0` makes exactly one attempt on a 529.
    #[tokio::test]
    async fn responses_529_with_zero_max_retries_makes_one_attempt() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/responses"))
            .respond_with(wiremock::ResponseTemplate::new(529))
            .mount(&server)
            .await;

        let client = OpenAiResponsesProvider {
            base_url: server.uri(),
            api_key: None,
            models: vec![],
            context_window: 400_000,
            http: reqwest::Client::new(),
            retry: crate::retry::Policy {
                max_retries: 0,
                base: std::time::Duration::from_millis(1),
            },
        };
        let mut req = base("gpt-5.1");
        req.messages = vec![user_text("hi")];
        let (tx, _rx) = mpsc::channel(64);
        let err = client
            .stream(req, tx, CancellationToken::new())
            .await
            .expect_err("529 exhausts a zero-retry budget");
        assert!(matches!(err, ProviderError::Overloaded));
        assert_eq!(server.received_requests().await.map(|r| r.len()), Some(1));
    }
}
