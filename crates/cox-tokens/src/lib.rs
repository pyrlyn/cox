// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Token estimation and counting for a `Request` (plan.md T1.8), moved to
//! its own crate by T32.10 (dependency (a): `tiktoken-rs` and its BPE data
//! are the only reason `cox-provider` used to pull them in). Three ways to
//! size a request before or after it goes over the wire, in increasing
//! order of accuracy and decreasing order of availability:
//!
//! 1. [`estimate`] — a byte-counting heuristic, no I/O, always available;
//!    used to trigger compaction/budget checks before a provider call is
//!    even made (`Session::step`, plan.md §1.3 step 3.c).
//! 2. [`count_openai`] — exact for OpenAI-family models, via `tiktoken-rs`.
//! 3. [`count_anthropic`] — exact for Anthropic models, via the provider's
//!    own `count_tokens` endpoint. Not yet called from `Provider::count_tokens`
//!    — `anthropic/mod.rs` is mid-edit under T1.2 — wired in T1.6. Takes a
//!    plain `reqwest::Client`/`HeaderMap` rather than any `cox-provider`
//!    type, so this crate needs no dependency back on `cox-provider`.
//!
//! `cox-provider` re-exports this crate at the old `tokens` path
//! (`crates/cox-provider/src/lib.rs`), so `cox_provider::tokens::*` keeps
//! working for existing callers.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

use cox_protocol::errors::ProviderError;
use cox_protocol::image::IMAGE_TOKEN_ESTIMATE;
use cox_protocol::types::{Content, Request};
use serde_json::Value;

/// A heuristic size estimate for a `Request`, computed with no I/O and no
/// tokenizer. Always [`Estimate::estimated`] — a stand-in, never presented
/// as a real count (`Usage::estimated`, plan.md §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    /// The estimated token count.
    pub tokens: u32,
    /// Always `true`: `estimate` never reports a provider-confirmed count.
    pub estimated: bool,
}

/// UTF-8 bytes per token. Anthropic's tokenizer is not public, so this is a
/// byte-costed guess. Tuned (not the plan.md-suggested 3.5) against
/// `fixtures/count_tokens/*.json` — tiktoken `o200k_base` counts used as a
/// documented stand-in, see that directory's `_note` — to land within
/// +/-15% on prose, code, unicode and tool-heavy requests alike; the
/// fixtures show real bytes-per-token varies by content (~2.9 for dense
/// code, ~4.6 for English prose, higher still for accented/CJK text under
/// this proxy tokenizer specifically), so this is the least-bad single
/// constant across that spread, not a fit to any one of them.
const BYTES_PER_TOKEN: f64 = 3.8;

/// Tokens charged per JSON key found anywhere in a tool's `input_schema`
/// (nested keywords included) — roughly what each key costs once rendered
/// into the tool definition the provider actually sees. Tuned down from the
/// plan.md-suggested 6 to 5: `fixtures/count_tokens/03_tool_schemas.json`
/// and `04_tool_results.json` measure ~5.0-5.8 tiktoken tokens per schema
/// key in isolation (tool name + description + `input_schema` rendered as
/// text, independent of any message content).
const TOKENS_PER_SCHEMA_KEY: u32 = 5;

/// Fixed per-message overhead (role wrapper, content-block framing). Tuned
/// down from the plan.md-suggested 4 to 1: with 4, a one-message fixture's
/// overhead alone was 15-30% of its total token count, larger than the
/// variance the byte term is meant to absorb, so no single `BYTES_PER_TOKEN`
/// could satisfy both a short and a long fixture at once.
const TOKENS_PER_MESSAGE: u32 = 1;

/// Heuristic size of `req`: UTF-8 bytes of all text content divided by
/// [`BYTES_PER_TOKEN`], plus [`TOKENS_PER_SCHEMA_KEY`] per JSON key in every
/// tool's `input_schema`, plus [`TOKENS_PER_MESSAGE`] per message, plus
/// `IMAGE_TOKEN_ESTIMATE` per `Content::Image` (T40.3). The fallback when neither `count_openai` nor `count_anthropic` is available
/// (plan.md T1.8 step 3).
pub fn estimate(req: &Request) -> Estimate {
    let text_bytes = rendered_message_text(req).len() as f64;
    let schema_keys: u32 = req
        .tools
        .iter()
        .map(|t| count_json_keys(&t.input_schema))
        .sum();
    let tokens = (text_bytes / BYTES_PER_TOKEN).ceil() as u32
        + schema_keys * TOKENS_PER_SCHEMA_KEY
        + req.messages.len() as u32 * TOKENS_PER_MESSAGE
        + image_count(req).saturating_mul(IMAGE_TOKEN_ESTIMATE as u32);
    Estimate {
        tokens,
        estimated: true,
    }
}

/// Exact count for OpenAI-family models: `tiktoken-rs`'s `o200k_base`
/// encoding over [`rendered_full_text`] (message text *and* tool
/// definitions — unlike [`estimate`], which prices schema keys separately,
/// a real tokenizer just sees the whole rendered request). No network call
/// — `tiktoken-rs` ships the vocabulary.
pub fn count_openai(req: &Request) -> Result<u32, ProviderError> {
    let bpe = tiktoken_rs::o200k_base().map_err(|e| ProviderError::BadRequest {
        message: format!("tiktoken o200k_base: {e}"),
    })?;
    let text = rendered_full_text(req);
    Ok(bpe.encode_ordinary(&text).len() as u32)
}

/// Exact count for Anthropic models via `POST {base_url}/v1/messages/count_tokens`
/// — the same messages-request body, minus `stream`/`max_tokens` (see
/// [`strip_for_count`]). The response is `{"input_tokens": N}`.
///
/// Not wired into `Provider::count_tokens` yet: `anthropic/mod.rs` is
/// mid-edit under T1.2. Wired in T1.6.
pub async fn count_anthropic(
    http: &reqwest::Client,
    base_url: &str,
    headers: reqwest::header::HeaderMap,
    mut body: Value,
) -> Result<u32, ProviderError> {
    strip_for_count(&mut body);
    let url = format!("{base_url}/v1/messages/count_tokens");
    let resp = http
        .post(url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|_| ProviderError::Network)?;
    if !resp.status().is_success() {
        return Err(ProviderError::BadRequest {
            message: format!("count_tokens: HTTP {}", resp.status()),
        });
    }
    let parsed: Value = resp.json().await.map_err(|_| ProviderError::Network)?;
    parsed
        .get("input_tokens")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .ok_or_else(|| ProviderError::BadRequest {
            message: "count_tokens: response missing input_tokens".into(),
        })
}

/// Removes the two fields a `count_tokens` body must not carry: `stream`
/// (the endpoint never streams) and `max_tokens` (irrelevant to an input
/// count, and rejected in some combinations).
pub fn strip_for_count(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("stream");
        obj.remove("max_tokens");
    }
}

/// A request's system + message text: each message's
/// text/thinking/tool-result/pointer-summary content and each tool-use's
/// JSON input, joined with newlines. This is the "text content" [`estimate`]
/// prices by the byte — tool schemas are deliberately excluded here because
/// `estimate` prices those separately, per JSON key ([`TOKENS_PER_SCHEMA_KEY`]).
/// Images carry no text and are not represented here; [`estimate`] prices
/// them flat through [`image_count`].
fn rendered_message_text(req: &Request) -> String {
    let mut s = String::new();
    for block in &req.system {
        s.push_str(&block.text);
        s.push('\n');
    }
    for message in &req.messages {
        for content in &message.content {
            match content {
                Content::Text { text } => {
                    s.push_str(text);
                    s.push('\n');
                }
                Content::Thinking { text, .. } => {
                    s.push_str(text);
                    s.push('\n');
                }
                Content::ToolUse { input, .. } => {
                    s.push_str(&input.to_string());
                    s.push('\n');
                }
                Content::ToolResult { content, .. } => {
                    s.push_str(content);
                    s.push('\n');
                }
                Content::Pointer { summary, .. } => {
                    s.push_str(summary);
                    s.push('\n');
                }
                Content::Image { .. } => {}
            }
        }
    }
    s
}

/// The `Content::Image` blocks in `req`'s messages. Why a flat cost per
/// image: Anthropic bills an image by pixel area, capped at 1568 visual
/// tokens on standard models (plan.md P40), and sizing one would need
/// decoding it; the provider's reported usage corrects the guess.
fn image_count(req: &Request) -> u32 {
    let images = req
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, Content::Image { .. }))
        .count();
    u32::try_from(images).unwrap_or(u32::MAX)
}

/// [`rendered_message_text`] plus each tool's name, description and
/// `input_schema` (serialized as JSON text) — everything a real tokenizer
/// would see if the whole request body were flattened to text. Used only by
/// [`count_openai`]; `estimate`'s heuristic prices schemas separately so it
/// never runs `serde_json::Value::to_string` on a large schema on every
/// budget check.
fn rendered_full_text(req: &Request) -> String {
    let mut s = rendered_message_text(req);
    for tool in &req.tools {
        s.push_str(&tool.name);
        s.push('\n');
        s.push_str(&tool.description);
        s.push('\n');
        s.push_str(&tool.input_schema.to_string());
        s.push('\n');
    }
    s
}

/// Counts every JSON object key, recursively (arrays are walked but do not
/// themselves count).
fn count_json_keys(v: &Value) -> u32 {
    match v {
        Value::Object(map) => map.len() as u32 + map.values().map(count_json_keys).sum::<u32>(),
        Value::Array(items) => items.iter().map(count_json_keys).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use reqwest::header::HeaderMap;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/count_tokens")
    }

    /// Every `fixtures/count_tokens/*.json` file as `(name, request, input_tokens)`.
    fn fixtures() -> Vec<(String, Request, u32)> {
        let dir = fixtures_dir();
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        entries.sort();
        entries
            .into_iter()
            .map(|path| {
                let raw = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                let value: Value = serde_json::from_str(&raw)
                    .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
                let req: Request = serde_json::from_value(value["request"].clone())
                    .unwrap_or_else(|e| panic!("deserialize request in {}: {e}", path.display()));
                let input_tokens = value["input_tokens"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("{}: input_tokens missing", path.display()))
                    as u32;
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("<unknown>")
                    .to_string();
                (name, req, input_tokens)
            })
            .collect()
    }

    #[test]
    fn tokens_estimate_within_15_percent_of_fixtures() {
        let cases = fixtures();
        assert!(cases.len() >= 5, "expected at least 5 fixtures");
        for (name, req, fixture_tokens) in cases {
            let got = estimate(&req).tokens;
            let low = (fixture_tokens as f64 * 0.85).floor() as u32;
            let high = (fixture_tokens as f64 * 1.15).ceil() as u32;
            assert!(
                (low..=high).contains(&got),
                "{name}: estimate {got} not within +/-15% of fixture {fixture_tokens} ({low}..={high})"
            );
        }
    }

    #[test]
    fn tokens_estimate_is_always_flagged_estimated() {
        let (_, req, _) = &fixtures()[0];
        assert!(estimate(req).estimated);
    }

    /// T40.3: each image adds `IMAGE_TOKEN_ESTIMATE`, whatever its size.
    #[test]
    fn estimate_counts_each_image_flat() {
        use cox_protocol::types::{Message, Role};
        let (_, mut req, _) = fixtures().swap_remove(0);
        let before = estimate(&req).tokens;
        let image = |data_b64: String| Content::Image {
            media_type: "image/png".into(),
            data_b64,
        };
        req.messages.push(Message {
            role: Role::User,
            content: vec![image("iVBORw==".into()), image("A".repeat(4000))],
        });
        let expected = before + TOKENS_PER_MESSAGE + 2 * IMAGE_TOKEN_ESTIMATE as u32;
        assert_eq!(estimate(&req).tokens, expected);
    }

    #[test]
    fn tokens_count_json_keys_walks_nested_schemas() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "opts": {"type": "object", "properties": {"limit": {"type": "integer"}}}
            }
        });
        // top-level: type, properties (2); properties' value: path, opts (2);
        // path's value: type (1); opts's value: type, properties (2); that
        // properties' value: limit (1); limit's value: type (1). 2+2+1+2+1+1=9.
        assert_eq!(count_json_keys(&schema), 9);
    }

    #[test]
    fn tokens_strip_for_count_removes_stream_and_max_tokens() {
        let mut body =
            json!({"model": "claude-sonnet-5", "messages": [], "stream": true, "max_tokens": 1024});
        strip_for_count(&mut body);
        assert!(body.get("stream").is_none());
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["model"], "claude-sonnet-5");
    }

    #[tokio::test]
    async fn tokens_count_anthropic_parses_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages/count_tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 1234})))
            .expect(1)
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let body =
            json!({"model": "claude-sonnet-5", "messages": [], "stream": true, "max_tokens": 1024});
        let got = count_anthropic(&http, &server.uri(), HeaderMap::new(), body)
            .await
            .expect("count_tokens succeeds");
        assert_eq!(got, 1234);
    }

    #[tokio::test]
    async fn tokens_count_anthropic_strips_stream_and_max_tokens_before_sending() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages/count_tokens"))
            .and(wiremock::matchers::body_json(
                json!({"model": "claude-sonnet-5", "messages": []}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 7})))
            .expect(1)
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let body =
            json!({"model": "claude-sonnet-5", "messages": [], "stream": true, "max_tokens": 1024});
        let got = count_anthropic(&http, &server.uri(), HeaderMap::new(), body)
            .await
            .expect("count_tokens succeeds");
        assert_eq!(got, 7);
    }

    #[tokio::test]
    async fn tokens_count_anthropic_reports_bad_request_on_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages/count_tokens"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "bad"})))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let body = json!({"model": "claude-sonnet-5", "messages": []});
        let err = count_anthropic(&http, &server.uri(), HeaderMap::new(), body)
            .await
            .expect_err("HTTP 400 must not parse as a count");
        assert!(matches!(err, ProviderError::BadRequest { .. }));
    }
}
