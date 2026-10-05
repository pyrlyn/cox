// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Generated Rust types for the Anthropic Messages API: the request body
//! (`CreateMessageParams`) and the stream frame bodies (T30.12). `build.rs`
//! runs typify over the schemas of the vendored OpenAPI spec
//! (`schema/anthropic-openapi.json`, source and re-vendoring in
//! `schema/README.md`) that those two reach, and this module includes the
//! result. Nothing here is hand-written but this header and the tests: the
//! request translation lives in [`super::request`], the SSE ->
//! `ProviderEvent` state machine in [`super::stream`].

// Generated code: lints that judge style are the generator's business, and
// most of the ~280 types are there because the spec reaches them, not
// because cox uses them.
#[allow(clippy::all, clippy::pedantic, dead_code, missing_docs, unused)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/anthropic_wire.rs"));
}
pub use generated::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// The stream-side types were made tolerant in `build.rs`: extra fields
    /// never break parsing; the corresponding assertion is
    /// `unknown_fields_on_event_block_and_usage_are_ignored` below.

    #[test]
    fn message_start_reads_model_and_usage() {
        let raw = r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-5",
            "usage":{"input_tokens":25,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        let event: MessageStartEvent =
            serde_json::from_str(raw).expect("well-formed message_start");
        assert_eq!(event.message.model.0, "claude-sonnet-5");
        let usage = event.message.usage;
        assert_eq!(usage.input_tokens, Some(25));
        assert_eq!(usage.output_tokens, Some(1));
        assert_eq!(usage.cache_creation_input_tokens, Some(0));
        assert_eq!(usage.cache_read_input_tokens, Some(0));
    }

    #[test]
    fn content_block_start_reads_text_block_type() {
        let raw =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let event: ContentBlockStartEvent =
            serde_json::from_str(raw).expect("well-formed text block");
        assert!(matches!(
            event.content_block,
            ContentBlockStartEventContentBlock::Text { .. }
        ));
    }

    #[test]
    fn content_block_start_reads_thinking_block_type() {
        let raw = r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#;
        let event: ContentBlockStartEvent =
            serde_json::from_str(raw).expect("well-formed thinking block");
        assert!(matches!(
            event.content_block,
            ContentBlockStartEventContentBlock::Thinking { .. }
        ));
    }

    #[test]
    fn content_block_start_reads_tool_use_name() {
        let raw = r#"{"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"toolu_1","name":"write","input":{}}}"#;
        let event: ContentBlockStartEvent =
            serde_json::from_str(raw).expect("well-formed tool_use block");
        let ContentBlockStartEventContentBlock::ToolUse { name, .. } = event.content_block else {
            panic!("expected a tool_use block");
        };
        assert_eq!(name.as_deref(), Some("write"));
    }

    #[test]
    fn content_block_delta_reads_text_delta() {
        let raw =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let event: ContentBlockDeltaEvent =
            serde_json::from_str(raw).expect("well-formed text_delta");
        let ContentBlockDeltaEventDelta::TextDelta { text } = event.delta else {
            panic!("expected a text_delta");
        };
        assert_eq!(text.as_deref(), Some("hi"));
    }

    #[test]
    fn content_block_delta_reads_input_json_delta() {
        let raw = r#"{"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#;
        let event: ContentBlockDeltaEvent =
            serde_json::from_str(raw).expect("well-formed input_json_delta");
        let ContentBlockDeltaEventDelta::InputJsonDelta { partial_json } = event.delta else {
            panic!("expected an input_json_delta");
        };
        assert_eq!(partial_json.as_deref(), Some(r#"{"a":1}"#));
    }

    #[test]
    fn content_block_delta_reads_thinking_delta() {
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#;
        let event: ContentBlockDeltaEvent =
            serde_json::from_str(raw).expect("well-formed thinking_delta");
        let ContentBlockDeltaEventDelta::ThinkingDelta { thinking } = event.delta else {
            panic!("expected a thinking_delta");
        };
        assert_eq!(thinking.as_deref(), Some("hm"));
    }

    #[test]
    fn content_block_delta_reads_signature_delta_type() {
        // stream.rs consumes and drops the signature itself (module header:
        // no ProviderEvent carries it yet); the wire type still has to
        // accept the frame, so a replay that does need it can parse it.
        let raw = r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#;
        let event: ContentBlockDeltaEvent =
            serde_json::from_str(raw).expect("well-formed signature_delta");
        assert!(matches!(
            event.delta,
            ContentBlockDeltaEventDelta::SignatureDelta { .. }
        ));
    }

    #[test]
    fn message_delta_reads_stop_reason_and_usage() {
        let raw = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"output_tokens":8}}"#;
        let event: MessageDeltaEvent =
            serde_json::from_str(raw).expect("well-formed message_delta");
        let reason = event.delta.stop_reason.expect("stop_reason present");
        assert_eq!(reason.0, "end_turn");
        assert_eq!(event.usage.output_tokens, Some(8));
    }

    #[test]
    fn message_delta_reads_refusal_stop_details() {
        let raw = r#"{"type":"message_delta","delta":{"stop_reason":"refusal",
            "stop_details":{"category":"cyber","explanation":"matched a policy classifier"}},"usage":{}}"#;
        let event: MessageDeltaEvent =
            serde_json::from_str(raw).expect("well-formed refusal message_delta");
        let details = event.delta.stop_details.expect("stop_details present");
        assert_eq!(details.category.map(|c| c.0).as_deref(), Some("cyber"));
        assert_eq!(
            details.explanation.as_deref(),
            Some("matched a policy classifier")
        );
    }

    #[test]
    fn error_event_reads_type_and_message() {
        let raw = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let event: ErrorResponse = serde_json::from_str(raw).expect("well-formed error frame");
        let ErrorResponseError::OverloadedError { message } = event.error else {
            panic!("expected an overloaded_error");
        };
        assert_eq!(message, "Overloaded");
    }

    #[test]
    fn unknown_fields_on_event_block_and_usage_are_ignored() {
        // Every field here that is not `model` or `usage.*` is a real field
        // from fixtures/anthropic/live_tool_use.sse (container, diagnostics,
        // service_tier, the nested cache_creation object) or one the spec
        // snapshot does not list (`diagnostics`, the top-level extra). The
        // spec never sets `additionalProperties: false` on a response
        // schema, so none may cause a deserialize failure here.
        let raw = r#"{
            "type":"message_start",
            "message":{
                "id":"msg_1",
                "model":"claude-sonnet-5",
                "container": null,
                "diagnostics": null,
                "usage":{
                    "input_tokens":10,
                    "output_tokens":2,
                    "service_tier":"standard",
                    "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}
                }
            },
            "some_future_top_level_field": 42
        }"#;
        let event: MessageStartEvent =
            serde_json::from_str(raw).expect("unrecognised fields must never break parsing");
        assert_eq!(event.message.model.0, "claude-sonnet-5");
        assert_eq!(event.message.usage.input_tokens, Some(10));
    }

    #[test]
    fn usage_missing_fields_deserialize_to_none() {
        let raw = r#"{"output_tokens":8}"#;
        let usage: Usage = serde_json::from_str(raw).expect("partial usage object");
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, None);
    }

    #[test]
    fn usage_null_fields_deserialize_to_none() {
        // The spec types these `integer | null`; either way an explicit
        // `null` and an absent key both land as `None`.
        let raw = r#"{"output_tokens":8,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}"#;
        let usage: Usage = serde_json::from_str(raw).expect("explicit nulls parse");
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, None);
    }

    #[test]
    fn message_start_without_message_fails_to_deserialize() {
        // `message` is the one field `AnthropicStream::on_message_start`
        // (stream.rs) treats as fatal-if-missing (`ProviderError::Parse`);
        // build.rs keeps an object payload with no `default` required, and
        // this is the wire-level half of that contract.
        let raw = r#"{"type":"message_start"}"#;
        let result: Result<MessageStartEvent, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "message is required");
    }

    #[test]
    fn content_block_start_without_content_block_fails_to_deserialize() {
        // `content_block` is required, so the field is not an `Option` and
        // `stream.rs::on_block_start` matches on it directly; dropping the
        // requirement would stop the crate compiling, not just this test.
        let raw = r#"{"type":"content_block_start","index":0}"#;
        let result: Result<ContentBlockStartEvent, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "content_block is required");
    }

    #[test]
    fn message_delta_without_delta_fails_to_deserialize() {
        let raw = r#"{"type":"message_delta","usage":{"output_tokens":1}}"#;
        let result: Result<MessageDeltaEvent, _> = serde_json::from_str(raw);
        assert!(result.is_err(), "delta is required");
    }

    #[test]
    fn unknown_content_block_and_delta_type_are_rejected_at_the_wire_layer() {
        // The spec's unions are closed: generated from `oneOf` + a
        // discriminator, the block and delta enums reject a `type` they do
        // not list. Tolerating one is `AnthropicStream::on_block_start` /
        // `on_block_delta`'s job (they peek at the tag before parsing), and
        // `stream::tests::unknown_fields_and_block_types_are_ignored` proves
        // it; this test pins that the wire layer alone would not.
        let block = r#"{"type":"content_block_start","index":0,
            "content_block":{"type":"some_future_block"}}"#;
        assert!(serde_json::from_str::<ContentBlockStartEvent>(block).is_err());

        let delta =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"some_future_delta"}}"#;
        assert!(serde_json::from_str::<ContentBlockDeltaEvent>(delta).is_err());
    }

    #[test]
    fn request_blocks_serialize_with_their_type_tag() {
        // The request unions are internally tagged (build.rs inlines the
        // `$ref` members so typify can see each tag), so a block carries
        // its `type` without cox writing the string by hand.
        let block = InputContentBlock::ToolResult {
            cache_control: None,
            content: Some(RequestToolResultBlockContent::String("ok".into())),
            is_error: Some(false),
            tool_use_id: "toolu_1".into(),
            toolset_name: None,
        };
        assert_eq!(
            serde_json::to_value(&block).expect("serializes"),
            serde_json::json!({
                "type": "tool_result",
                "content": "ok",
                "is_error": false,
                "tool_use_id": "toolu_1"
            })
        );
    }
}
