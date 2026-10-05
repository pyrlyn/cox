// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Re-exports of the `async-openai` crate's Responses-API types
//! (`response-types` feature only — no HTTP client, no second
//! `reqwest`/`tokio`/`futures` in the dependency graph; verified with
//! `cargo tree -p cox-provider -e normal -i reqwest|hyper|tokio` before this
//! module existed, T30.11/A40 step 1). `responses.rs` builds request bodies
//! and parses known stream-event payloads through these types instead of
//! hand-written structs and `serde_json::Value` walks. Transport, retry, SSE
//! framing, the `ProviderEvent` mapping and the usage ledger all stay
//! `cox-provider`'s own — this module is data shapes only, nothing calls out.
//!
//! Not re-exported: `ResponseStreamEvent` (the crate's own externally-tagged
//! top-level event enum) and `Response`/`ResponseCompletedEvent` (the full
//! response object). `responses.rs` already knows a frame's `type` from the
//! SSE `event:` line (or the JSON body's own `"type"`, same fallback as
//! before this task), so it deserializes straight into the matching payload
//! struct below rather than through the tagged enum — the enum has no
//! catch-all variant, so an unrecognised `type` would turn into a hard
//! deserialize error instead of the silent ignore `feed` has always given
//! unknown frames. `Response` carries several non-`Option` fields (e.g.
//! `created_at`) that this crate's minimal SSE fixtures don't set and cox
//! never reads; only `ResponseUsage`, nested out of the `response.completed`
//! frame's `response.usage`, is typed.

pub use async_openai::types::responses::{
    CreateResponse, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, InputContent, InputImageContent,
    InputItem, InputParam, Item, OutputItem, Reasoning, ReasoningEffort,
    ResponseErrorEvent as WireErrorEvent, ResponseFunctionCallArgumentsDeltaEvent,
    ResponseFunctionCallArgumentsDoneEvent, ResponseTextDeltaEvent, ResponseUsage,
    Role as WireRole, Tool,
};

#[cfg(test)]
mod tests {
    //! Direct tests against the SDK types on real SSE frame bytes (fixture
    //! files under `fixtures/openai-responses/`), proving the shapes
    //! `responses.rs` deserializes into actually match what OpenAI sends —
    //! `responses.rs`'s own tests cover the `ProviderEvent` translation this
    //! module doesn't know about.

    use std::fs;
    use std::path::Path;

    use async_openai::types::responses::ResponseOutputItemAddedEvent;
    use serde_json::Value;

    use super::*;

    /// One `data:` line's JSON payload from a recorded fixture, by its 0-based
    /// position among the fixture's `data:` lines (matches the frames listed
    /// in `responses.rs`'s own fixture files).
    fn frame(fixture: &str, index: usize) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/openai-responses")
            .join(format!("{fixture}.sse"));
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path:?}: {e}"));
        text.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .nth(index)
            .map(|l| serde_json::from_str(l).expect("fixture line is valid JSON"))
            .unwrap_or_else(|| panic!("fixture {fixture} has no data line #{index}"))
    }

    #[test]
    fn output_item_added_function_call_deserializes_from_fixture() {
        // one_tool_call.sse frame #1: `response.output_item.added` opening a
        // `function_call` item.
        let event: ResponseOutputItemAddedEvent =
            serde_json::from_value(frame("one_tool_call", 1)).expect("known event shape");
        let OutputItem::FunctionCall(call) = event.item else {
            panic!("fixture item is a function_call");
        };
        assert_eq!(call.name, "read");
        assert_eq!(call.call_id, "call_ReadA1B2C3");
    }

    #[test]
    fn output_item_added_message_is_not_a_function_call() {
        // text_only.sse frame #2: `response.output_item.added` opening a
        // `message` item — the shape `on_output_item_added` treats as "no
        // event to emit here" rather than a tool-call start.
        let event: ResponseOutputItemAddedEvent =
            serde_json::from_value(frame("text_only", 2)).expect("known event shape");
        assert!(matches!(event.item, OutputItem::Message(_)));
    }

    #[test]
    fn function_call_arguments_delta_deserializes_from_fixture() {
        // one_tool_call.sse frame #2: first arguments delta.
        let event: ResponseFunctionCallArgumentsDeltaEvent =
            serde_json::from_value(frame("one_tool_call", 2)).expect("known event shape");
        assert_eq!(event.delta, "{\"path\":");
    }

    #[test]
    fn function_call_arguments_done_deserializes_from_fixture() {
        // one_tool_call.sse frame #4: `response.function_call_arguments.done`.
        let event: ResponseFunctionCallArgumentsDoneEvent =
            serde_json::from_value(frame("one_tool_call", 4)).expect("known event shape");
        assert_eq!(event.name.as_deref(), Some("read"));
        assert_eq!(event.arguments, "{\"path\":\"a.rs\"}");
    }

    #[test]
    fn output_text_delta_deserializes_from_fixture() {
        // text_only.sse frame #4: first text delta.
        let event: ResponseTextDeltaEvent =
            serde_json::from_value(frame("text_only", 4)).expect("known event shape");
        assert_eq!(event.delta, "cox-provider owns ");
    }

    #[test]
    fn usage_deserializes_from_completed_frame() {
        // one_tool_call.sse frame #6: `response.completed`; only the nested
        // `response.usage` object is typed (see module header on why the
        // full `Response` isn't).
        let completed = frame("one_tool_call", 6);
        let usage_value = completed
            .get("response")
            .and_then(|r| r.get("usage"))
            .expect("fixture completed frame carries usage");
        let usage: ResponseUsage =
            serde_json::from_value(usage_value.clone()).expect("known usage shape");
        assert_eq!(usage.input_tokens, 512);
        assert_eq!(usage.output_tokens, 24);
        assert_eq!(usage.input_tokens_details.cached_tokens, 50);
    }

    #[test]
    fn error_event_deserializes_rate_limit_code() {
        // No recorded fixture carries an `error` frame (OpenAI's error
        // payload is undocumented-by-example here); this literal mirrors the
        // documented shape (`sequence_number`, `code`, `message`, `param`).
        let value: Value = serde_json::from_str(
            r#"{"type":"error","sequence_number":0,"code":"rate_limit_exceeded","message":"too many requests","param":null}"#,
        )
        .expect("literal is valid JSON");
        let event: WireErrorEvent = serde_json::from_value(value).expect("known event shape");
        assert_eq!(event.code.as_deref(), Some("rate_limit_exceeded"));
        assert_eq!(event.message, "too many requests");
    }
}
