// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Turns Anthropic Messages SSE frames into [`ProviderEvent`]s.
//!
//! Pure and synchronous — no I/O, no `async`. The network side ([`super`])
//! pulls frames one at a time from [`crate::sse::sse_stream`] and calls
//! [`AnthropicStream::feed`] on each; a fixture test does the same over
//! [`crate::sse::parse_sse_str`]. Same code path either way, so a fixture
//! that passes here behaves identically against a live socket.
//!
//! **Block tracking.** The Messages API streams one content block fully
//! (`content_block_start` … `content_block_stop`) before starting the next,
//! even for parallel tool calls, so a single `current_block` field — not a
//! per-index map — is enough to route a `content_block_delta` to the right
//! `ProviderEvent`.
//!
//! **Ids.** `ProviderEvent::ToolUseStart.id` is a `CallId` (a ULID, per
//! `cox-protocol`), but Anthropic's wire `content_block.id` is an opaque
//! provider string (`toolu_…`) that is never a valid ULID. `stream.rs`
//! mints a fresh `CallId::new()` for every `tool_use` block instead of
//! parsing the wire id — cox is a stateless caller from Anthropic's point of
//! view (each request is self-consistent, not validated against ids the API
//! emitted in a prior call), and `anthropic::request::content_blocks`
//! already round-trips this same cox-minted id back out as both the replayed
//! `tool_use.id` and the matching `tool_result.tool_use_id` (plan.md §1.2;
//! see `crates/cox-provider-anthropic/src/request.rs`).
//!
//! **Thinking signatures.** `signature_delta` chunks are consumed and
//! dropped: `ProviderEvent` (plan.md §1.2, committed in `cox-protocol`
//! T0.2/T1.1) has no field to carry one out of the stream, and this task's
//! `Files:` line does not include `cox-protocol/src/types.rs`. T1.1 made the
//! same call for the request-building side (`docs/design/provider.md`: "no
//! `produced_by` added to `Content::Thinking`") — replay of thinking blocks
//! stays unimplemented until a task actually needs it end to end.
//! ponytail: signature dropped, not stored; add a `ProviderEvent` variant
//! (and the `Content::Thinking` plumbing to use it) when a task needs
//! thinking-block replay across turns.
//!
//! **`stop_reason`.** `StopReason` (`cox-protocol`) documents that "a
//! provider only ever emits `EndTurn`/`Refusal`/`Error`; the others are
//! added by `cox-core` once it has aggregated multiple calls in a turn."
//! So `end_turn`, `tool_use`, `max_tokens` and `stop_sequence` all collapse
//! to `StopReason::EndTurn` here — `cox-core` inspects the `ToolUseStart`/
//! `ToolUseEnd` events already forwarded to decide whether a tool round
//! follows, and decides `max_tokens` continuation the same way (T2.x).
//!
//! **`redacted_thinking`.** Named in the claude-api streaming reference but
//! not in plan.md's T1.2 step list (`content_block_start (text | thinking |
//! tool_use)`); an unrecognised block type is tracked as "no current block"
//! so its deltas are silently skipped rather than corrupting a sibling
//! block, and produces no `ProviderEvent`.
//! ponytail: redacted_thinking dropped silently; add a block kind + event
//! when a fixture needs to replay one back to the model.
//!
//! **Wire types.** Frames deserialize into [`super::wire`], generated from
//! Anthropic's OpenAPI spec (T30.12). Its block and delta enums are closed,
//! so `content_block_start` / `content_block_delta` peek at the `type` tag
//! first and skip kinds cox does not handle before parsing; unknown event
//! types never reach a parser, and unknown fields are ignored by serde.

use cox_protocol::errors::ProviderError;
use cox_protocol::ids::CallId;
use cox_protocol::types::{ModelId, ProviderEvent, StopReason, Usage};
use serde_json::Value;

use super::wire;

/// Which content block is currently open, so a `content_block_delta` knows
/// which `ProviderEvent` to become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
}

/// The state carried across one `POST /v1/messages` SSE body: which block is
/// open, and the usage counters accumulated from `message_start` and
/// `message_delta` (only the fields each carries are overwritten — see
/// [`AnthropicStream::apply_usage`]).
#[derive(Debug)]
pub struct AnthropicStream {
    current_block: Option<BlockKind>,
    usage: Usage,
    /// SSE frame ordinal, for `ProviderError::Parse { line }`. Not a byte
    /// line number — Anthropic frames are one JSON object each, so the
    /// frame count is the closest useful locator without re-parsing bytes.
    frame_no: u64,
}

impl Default for AnthropicStream {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicStream {
    /// Starts a fresh state machine for one streamed call. `cost_usd` stays
    /// `0.0` (pricing is T1.7's ledger, not this state machine) and
    /// `latency_ms` is filled in by the caller, the only one that knows
    /// wall-clock time.
    pub fn new() -> Self {
        Self {
            current_block: None,
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

    /// The usage accumulated so far (cost/latency are filled in by the
    /// caller, which is the only one that knows wall-clock time and prices).
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// Feeds one SSE frame (`event:`, `data:` pair) and returns the
    /// `ProviderEvent`s it produces — zero for framing/heartbeat frames
    /// (`ping`, `message_stop`, a text or thinking `content_block_stop`),
    /// one otherwise.
    pub fn feed(
        &mut self,
        event: Option<&str>,
        data: &str,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.frame_no += 1;
        let value: Value = serde_json::from_str(data).map_err(|_| ProviderError::Parse {
            line: self.frame_no,
        })?;
        // Anthropic always sends a matching `event:` name, but the JSON
        // body's own "type" field carries the same string — falling back to
        // it costs nothing and tolerates a frame missing the SSE field. This
        // dispatch stays on the raw `Value`: which generated type to parse
        // the body into is exactly what `kind` decides next.
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
            "message_start" => self.on_message_start(value),
            "content_block_start" => self.on_block_start(value),
            "content_block_delta" => self.on_block_delta(value),
            // `cox-core` commits a tool call only on `ToolUseEnd`; without
            // it the accumulated input is dropped with the turn.
            "content_block_stop" => match self.current_block.take() {
                Some(BlockKind::ToolUse) => Ok(vec![ProviderEvent::ToolUseEnd]),
                _ => Ok(vec![]),
            },
            "message_delta" => self.on_message_delta(value),
            "error" => self.on_error(value).map(|e| vec![e]),
            // "ping", "message_stop", and anything unrecognised (forward
            // compatibility): no event, no error — a stray frame must never
            // fail the whole call.
            _ => Ok(vec![]),
        }
    }

    fn on_message_start(&mut self, v: Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        let body: wire::MessageStartEvent = self.parse(v)?;
        let usage = &body.message.usage;
        self.apply_usage(
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        );
        Ok(vec![ProviderEvent::MessageStart {
            model: ModelId(body.message.model.0),
        }])
    }

    fn on_block_start(&mut self, v: Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        // The generated block enum is closed: a block kind added after the
        // spec snapshot would fail it. Peek at the tag first, so any kind
        // cox does not act on (redacted_thinking, server tools, or one it
        // has never heard of) is skipped before the typed parse. A frame
        // with no `content_block` at all still goes on to fail that parse.
        if let Some(kind) = v.pointer("/content_block/type").and_then(Value::as_str)
            && !matches!(kind, "text" | "thinking" | "tool_use")
        {
            self.current_block = None;
            return Ok(vec![]);
        }
        let body: wire::ContentBlockStartEvent = self.parse(v)?;
        match body.content_block {
            wire::ContentBlockStartEventContentBlock::Text { .. } => {
                self.current_block = Some(BlockKind::Text);
                Ok(vec![])
            }
            wire::ContentBlockStartEventContentBlock::Thinking { .. } => {
                self.current_block = Some(BlockKind::Thinking);
                Ok(vec![])
            }
            wire::ContentBlockStartEventContentBlock::ToolUse { name, .. } => {
                self.current_block = Some(BlockKind::ToolUse);
                Ok(vec![ProviderEvent::ToolUseStart {
                    id: CallId::new(),
                    name: name.unwrap_or_default(),
                }])
            }
            _ => {
                self.current_block = None;
                Ok(vec![])
            }
        }
    }

    fn on_block_delta(&mut self, v: Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        // Same closed-enum peek as `on_block_start`. signature_delta: see
        // the module header — no ProviderEvent carries it, so it is
        // consumed and dropped with the other kinds cox does not act on.
        if let Some(kind) = v.pointer("/delta/type").and_then(Value::as_str)
            && !matches!(kind, "text_delta" | "thinking_delta" | "input_json_delta")
        {
            return Ok(vec![]);
        }
        let body: wire::ContentBlockDeltaEvent = self.parse(v)?;
        let event = match body.delta {
            wire::ContentBlockDeltaEventDelta::TextDelta { text } => ProviderEvent::TextDelta {
                text: text.unwrap_or_default(),
            },
            wire::ContentBlockDeltaEventDelta::ThinkingDelta { thinking } => {
                ProviderEvent::ThinkingDelta {
                    text: thinking.unwrap_or_default(),
                }
            }
            wire::ContentBlockDeltaEventDelta::InputJsonDelta { partial_json } => {
                ProviderEvent::ToolUseInputDelta {
                    text: partial_json.unwrap_or_default(),
                }
            }
            _ => return Ok(vec![]),
        };
        Ok(vec![event])
    }

    fn on_message_delta(&mut self, v: Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        let body: wire::MessageDeltaEvent = self.parse(v)?;
        let usage = &body.usage;
        self.apply_usage(
            usage.input_tokens,
            // The spec gives this one no `minimum`, so it is generated
            // signed; a negative count is no count.
            usage.output_tokens.and_then(|n| u64::try_from(n).ok()),
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        );
        let Some(reason) = body.delta.stop_reason else {
            // A message_delta that only carries usage (no stop yet): not
            // part of the documented shape, but ignoring it is harmless.
            return Ok(vec![]);
        };
        let stop = if reason.0 == "refusal" {
            let detail = body
                .delta
                .stop_details
                .as_ref()
                .map(refusal_detail)
                .unwrap_or_default();
            StopReason::Refusal { detail }
        } else {
            // end_turn, tool_use, max_tokens, stop_sequence: see the module
            // header on why these all collapse to EndTurn here.
            StopReason::EndTurn
        };
        Ok(vec![
            ProviderEvent::Stop { stop },
            ProviderEvent::Usage { usage: self.usage },
        ])
    }

    fn on_error(&mut self, v: Value) -> Result<ProviderEvent, ProviderError> {
        // Unlike the other frames, a malformed or absent `error` object is
        // never a `ProviderError::Parse` here: an error frame of a type the
        // snapshot does not know, or of no recognisable shape, still ends
        // the call as a generic network error.
        let mapped = match serde_json::from_value::<wire::ErrorResponse>(v).map(|b| b.error) {
            Ok(wire::ErrorResponseError::OverloadedError { .. }) => ProviderError::Overloaded,
            Ok(wire::ErrorResponseError::RateLimitError { .. }) => {
                ProviderError::RateLimited { retry_after: None }
            }
            Ok(wire::ErrorResponseError::InvalidRequestError { message }) => {
                ProviderError::BadRequest { message }
            }
            Ok(wire::ErrorResponseError::AuthenticationError { .. }) => ProviderError::Auth,
            _ => ProviderError::Network,
        };
        Ok(ProviderEvent::Error { error: mapped })
    }

    /// Deserializes one known frame body into its generated type; failure
    /// is a `Parse` error at this frame.
    fn parse<T: serde::de::DeserializeOwned>(&self, v: Value) -> Result<T, ProviderError> {
        serde_json::from_value(v).map_err(|_| ProviderError::Parse {
            line: self.frame_no,
        })
    }

    /// Only overwrites the counters present — `message_start` carries the
    /// input/cache trio, `message_delta` typically carries only
    /// `output_tokens`, and neither should blank out what the other set.
    fn apply_usage(
        &mut self,
        input: Option<u64>,
        output: Option<u64>,
        cache_read: Option<u64>,
        cache_write: Option<u64>,
    ) {
        let slots = [
            (input, &mut self.usage.input_tokens),
            (output, &mut self.usage.output_tokens),
            (cache_read, &mut self.usage.cache_read_tokens),
            (cache_write, &mut self.usage.cache_write_tokens),
        ];
        for (value, slot) in slots {
            if let Some(n) = value {
                // The spec's counters are unbounded integers; the ledger's
                // are `u32`. A count past four billion tokens saturates
                // instead of wrapping to a small, cheap-looking number.
                *slot = u32::try_from(n).unwrap_or(u32::MAX);
            }
        }
    }
}

/// `category: explanation`, falling back to whichever half is present —
/// both are optional and `stop_details` itself can be `null` even on a
/// refusal (claude-api skill, `shared/model-migration.md`).
fn refusal_detail(d: &wire::RefusalStopDetails) -> String {
    let category = d.category.as_ref().map(|c| c.0.as_str());
    let explanation = d.explanation.as_deref();
    match (category, explanation) {
        (Some(c), Some(e)) => format!("{c}: {e}"),
        (Some(c), None) => c.to_string(),
        (None, Some(e)) => e.to_string(),
        (None, None) => String::new(),
    }
}

/// Test-only: `ToolUseStart.id` is a freshly minted `CallId::new()` (a
/// random ULID — see the module header on why the wire id is never reused),
/// so it differs on every run and every call to `feed`. Snapshot and
/// live-vs-golden comparisons need it deterministic instead; this replaces
/// each `ToolUseStart` id with a counter-derived one, in event order, so two
/// parallel tool calls still get visibly distinct (but stable) ids. Used by
/// both this module's fixture snapshots and `crate::tests` (the
/// `wiremock` contract test), which is why it is `pub(crate)` instead of
/// nested inside `mod tests`.
#[cfg(test)]
pub(crate) fn normalize_tool_ids(events: Vec<ProviderEvent>) -> Vec<ProviderEvent> {
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

    use cox_protocol::types::ProviderEvent;

    use super::*;
    use crate::sse::parse_sse_str;

    /// Feeds a fixture through the same frame-by-frame path the live
    /// network client uses, and returns every `ProviderEvent` produced,
    /// with tool-call ids normalized (see [`normalize_tool_ids`]).
    fn run_fixture(name: &str) -> Vec<ProviderEvent> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/anthropic")
            .join(format!("{name}.sse"));
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path:?}: {e}"));
        let mut stream = AnthropicStream::new();
        let mut events = Vec::new();
        for (event, data) in parse_sse_str(&body) {
            events.extend(
                stream
                    .feed(event.as_deref(), &data)
                    .expect("fixture is well-formed"),
            );
        }
        normalize_tool_ids(events)
    }

    #[test]
    fn anthropic_stream_text_only() {
        insta::assert_json_snapshot!("anthropic_stream_text_only", run_fixture("text_only"));
    }

    #[test]
    fn anthropic_stream_one_tool_call() {
        insta::assert_json_snapshot!(
            "anthropic_stream_one_tool_call",
            run_fixture("one_tool_call")
        );
    }

    #[test]
    fn anthropic_stream_parallel_tool_calls() {
        let events = run_fixture("parallel_tool_calls");
        let starts = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolUseStart { .. }))
            .count();
        assert_eq!(starts, 2, "expected two parallel tool_use blocks");
        insta::assert_json_snapshot!("anthropic_stream_parallel_tool_calls", events);
    }

    #[test]
    fn anthropic_stream_tool_block_stop_ends_the_call() {
        // A real `claude-sonnet-5` stream (2026-09-25) for "Create hello.txt
        // containing exactly: hi" with one `write` tool.
        let events = run_fixture("live_tool_use");
        let last_delta = events
            .iter()
            .rposition(|e| matches!(e, ProviderEvent::ToolUseInputDelta { .. }))
            .expect("tool input deltas");
        assert!(matches!(
            events.get(last_delta + 1),
            Some(ProviderEvent::ToolUseEnd)
        ));
        let input: String = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::ToolUseInputDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let input: serde_json::Value = serde_json::from_str(&input).expect("input is JSON");
        assert_eq!(
            input,
            serde_json::json!({"path": "hello.txt", "content": "hi"})
        );
    }

    #[test]
    fn anthropic_stream_refusal() {
        let events = run_fixture("refusal");
        assert!(matches!(events.last(), Some(ProviderEvent::Usage { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Stop {
                stop: StopReason::Refusal { .. }
            }
        )));
        insta::assert_json_snapshot!("anthropic_stream_refusal", events);
    }

    #[test]
    fn anthropic_stream_max_tokens() {
        // §1.2 StopReason: a provider only ever emits EndTurn/Refusal/Error;
        // cox-core decides continuation from usage/output shape, not from a
        // dedicated MaxTokens variant (this module's header explains why).
        let events = run_fixture("max_tokens");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Stop {
                stop: StopReason::EndTurn
            }
        )));
        insta::assert_json_snapshot!("anthropic_stream_max_tokens", events);
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_panic() {
        let mut stream = AnthropicStream::new();
        let err = stream.feed(Some("message_start"), "{not json").unwrap_err();
        assert!(matches!(err, ProviderError::Parse { line: 1 }));
    }

    #[test]
    fn unknown_fields_and_block_types_are_ignored() {
        // T30.10/T30.12: proves the generated `wire` types don't turn an additive
        // API change (an extra field the schema never declared) or a block
        // kind cox does not know about yet into a `ProviderError::Parse`.
        let mut stream = AnthropicStream::new();
        let start = stream
            .feed(
                Some("message_start"),
                r#"{"type":"message_start","message":{"model":"claude-sonnet-5",
                    "container":null,"usage":{"input_tokens":5}},"some_future_top_level_field":1}"#,
            )
            .expect("an unrecognised field must never fail a known event");
        assert_eq!(
            start,
            vec![ProviderEvent::MessageStart {
                model: ModelId("claude-sonnet-5".to_string())
            }]
        );

        let block = stream
            .feed(
                Some("content_block_start"),
                r#"{"type":"content_block_start","index":0,
                    "content_block":{"type":"some_future_block","extra":true}}"#,
            )
            .expect("an unrecognised content-block type is ignored, not fatal");
        assert!(block.is_empty());

        // A content_block_stop following an unrecognised block type must
        // not emit ToolUseEnd: on_block_start already cleared current_block
        // for it (the `_ =>` arm), same as redacted_thinking.
        let stop = stream
            .feed(
                Some("content_block_stop"),
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .expect("stop after an unrecognised block type is harmless");
        assert!(stop.is_empty());
    }

    #[test]
    fn unknown_event_is_ignored_not_fatal() {
        let mut stream = AnthropicStream::new();
        let events = stream
            .feed(Some("some_future_event"), "{}")
            .expect("ignored");
        assert!(events.is_empty());
    }
}
