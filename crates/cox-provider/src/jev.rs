// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The TypeSafe Jev decision-model backend (type-1 native, T21.1): the System
//! One wire format (`POST /v1/systemone`) behind the same `Provider` trait
//! every other backend speaks. Same split as every other backend: a *pure*
//! request translator ([`build_body`]) plus pure response parsers, fixture-
//! tested with no key and no socket (D12); [`JevProvider`] is the thin
//! client that sends the body and drives the parse over one HTTP response.
//!
//! **Why native, not compatible.** Type-2 means "same wire client, different
//! base URL" — but Jev's wire is not an LLM chat shape at all: the request
//! is `{state, model, questions}` (typed Choice/Score/Noul), the response is
//! `{answers, usage}` in one non-streamed JSON body. No Chat/Responses
//! client parses it, so per the providers.md falsifier ("a wire format none
//! of the three clients parses graduates to Type 1") this is Type 1.
//!
//! **Decision answers ride the text channel.** The `Provider` contract only
//! carries `TextDelta`/`ThinkingDelta`/tool events back to the core — there
//! is no `DecisionDelta`. So a Jev answer is serialised to one compact JSON
//! `TextDelta` (plus terminal `Stop{EndTurn}` + `Usage`), and the single
//! `cheap`-job caller parses it back. The JSON shape is fixed here
//! (`answer_kind`/`choice`/`score`/`noul`/`probabilities`/`confidence`) so
//! exactly one parser exists.
//!
//! **Trust.** The caller — never this crate — enforces the gate-doc
//! boundary: Jev output is untrusted input, fail-open on missing key,
//! network failure, or low confidence (T21.0).

use async_trait::async_trait;
use cox_protocol::config::Transport;
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, StopReason, Usage};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Model id sent when the tier names no explicit Jev model.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// Builds the System One body for `POST /v1/systemone` from a `Request`.
///
/// The mapping is deliberately lossy and documented: the provider-neutral
/// `Request` (system + messages + tools) collapses into one `state` string,
/// and the single question is a Noul relevance probe. Real Choice/Score call
/// sites (router pick, risk score, skill rank) are composed by the
/// `cox-core` judge layer, which owns the questions and thresholds per
/// TypeSafe's review guidance; this translator only proves the wire shape is
/// well-formed without a key. `Request.effort` is not sent: the wire has no
/// such field, and `cox_models::effort_for(Api::Jev, ..)` is `None` (T30.26).
pub fn build_body(req: &Request) -> Value {
    let mut parts: Vec<&str> = Vec::new();
    for b in &req.system {
        if !b.text.is_empty() {
            parts.push(b.text.as_str());
        }
    }
    for m in &req.messages {
        for c in &m.content {
            match c {
                cox_protocol::types::Content::Text { text } => {
                    if !text.is_empty() {
                        parts.push(text.as_str());
                    }
                }
                // Tool history is state too: name + result, not the schema.
                cox_protocol::types::Content::ToolUse { name, .. } => {
                    parts.push(name.as_str());
                }
                cox_protocol::types::Content::ToolResult { content, .. } if !content.is_empty() => {
                    parts.push(content.as_str());
                }
                // Thinking blocks, images and archive pointers are not
                // decision state: dropped rather than mistranslated.
                _ => {}
            }
        }
    }
    json!({
        "state": parts.join("\n\n"),
        "model": if req.model.0.is_empty() { DEFAULT_MODEL } else { req.model.0.as_str() },
        "questions": {
            "relevant": {
                "type": "noul",
                "instructions": "Is this state worth a typed decision (route, rank, or gate)?",
            },
        },
    })
}

/// Parses one `POST /v1/systemone` JSON response body into the events a
/// `Provider::stream` caller expects: one JSON `TextDelta` carrying the
/// normalised decision, then terminal `Stop{EndTurn}` and `Usage`.
///
/// The normalised shape is fixed: `{"answer_kind","choice"|"score"|"noul",
/// "probabilities","confidence"}` — one parser for the judge layer.
/// Anything the body does not carry (unknown question id, missing answer
/// object) is `Parse`, never a guess: a silent default would be a decision
/// credited to evidence that never arrived.
pub fn parse_response(body: &str) -> Result<(Vec<ProviderEvent>, Usage), ProviderError> {
    let value: Value = serde_json::from_str(body).map_err(|_| ProviderError::Parse { line: 1 })?;
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(ProviderError::Parse { line: 1 })?;
    let (_qid, answer) = answers
        .iter()
        .next()
        .ok_or(ProviderError::Parse { line: 1 })?;
    let answer = answer.as_object().ok_or(ProviderError::Parse { line: 1 })?;
    let kind = answer
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ProviderError::Parse { line: 1 })?;

    let normalised: Value = match kind {
        "choice" => {
            let choice = answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or(ProviderError::Parse { line: 1 })?;
            let probabilities = answer
                .get("probabilities")
                .cloned()
                .unwrap_or_else(|| json!({ choice: 1.0 }));
            let confidence = answer
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            json!({
                "answer_kind": "choice",
                "choice": choice,
                "probabilities": probabilities,
                "confidence": confidence,
            })
        }
        "score" => {
            let score = answer
                .get("score")
                .and_then(Value::as_f64)
                .ok_or(ProviderError::Parse { line: 1 })?;
            let probabilities = answer
                .get("probabilities")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let confidence = answer
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            json!({
                "answer_kind": "score",
                "score": score,
                "probabilities": probabilities,
                "confidence": confidence,
            })
        }
        "noul" => {
            let noul = answer
                .get("noul")
                .and_then(Value::as_f64)
                .ok_or(ProviderError::Parse { line: 1 })?;
            let confidence = answer
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(noul);
            json!({
                "answer_kind": "noul",
                "noul": noul,
                "probabilities": { "true": noul, "false": 1.0 - noul },
                "confidence": confidence,
            })
        }
        _ => return Err(ProviderError::Parse { line: 1 }),
    };

    let wire_usage = value.get("usage");
    let usage = Usage {
        input_tokens: wire_usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        // Jev output is unmetered (too cheap to bill): always 0, by
        // contract — not by accident of a missing field.
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        estimated: wire_usage.is_none(),
        cost_usd: 0.0,
        latency_ms: 0,
    };
    let events = vec![
        ProviderEvent::TextDelta {
            text: normalised.to_string(),
        },
        ProviderEvent::Stop {
            stop: StopReason::EndTurn,
        },
        ProviderEvent::Usage { usage },
    ];
    Ok((events, usage))
}

/// Maps a non-2xx Jev response to a [`ProviderError`], reusing the shared
/// [`crate::http`] taxonomy: 401 is `Auth`, 429 is `RateLimited`, 529/5xx is
/// `Overloaded` (retryable), anything else is `BadRequest` with the body's
/// message verbatim.
pub fn http_error(
    status: reqwest::StatusCode,
    body: &str,
    retry_after: Option<u64>,
) -> ProviderError {
    if status.as_u16() == 401 {
        return ProviderError::Auth;
    }
    if status.as_u16() == 429 {
        return ProviderError::RateLimited { retry_after };
    }
    if status.as_u16() == 529 || status.is_server_error() {
        return ProviderError::Overloaded;
    }
    ProviderError::BadRequest {
        message: crate::http::error_message(body),
    }
}

/// A configured System One client.
///
/// Fields are public for the same reason as `AnthropicProvider`'s: `cox`
/// builds one straight from `[providers.typesafe]`, there is no behaviour
/// worth hiding behind accessors.
pub struct JevProvider {
    /// `providers.typesafe.base_url`, without a trailing slash.
    pub base_url: String,
    /// The resolved credential (`TYPESAFE_API_KEY` env, else keyring
    /// `cox/typesafe`); sent as `Authorization: Bearer`.
    pub api_key: String,
    /// Model id sent as `model` (default `jev-latest`).
    pub model: String,
    /// The shared connection pool.
    pub http: reqwest::Client,
    /// Backoff for transient failures before the first byte (same policy as
    /// every other network backend).
    pub retry: crate::retry::Policy,
    /// `Caps.max_context` (T30.25): resolved once by the caller from the
    /// model catalog (`cox_models::Catalog`, merged with `[providers.
    /// typesafe].models`) for `model`, not looked up here — same
    /// "resolve at construction" shape as `AnthropicProvider::max_context`.
    /// `backend_for_with` in `crates/cox-session/src/provider.rs` does the lookup
    /// and falls back to 128 000 — today's literal — when the catalog has
    /// no row for the configured model (expected: Jev/TypeSafe models have
    /// no models.dev counterpart, so only an explicit `[providers.
    /// typesafe].models` entry ever overrides this).
    pub max_context: u32,
}

impl JevProvider {
    /// Builds a provider from `&Transport` (`providers.typesafe`, T30.23)
    /// with an already-resolved credential. Prefer [`Self::new`] at session
    /// startup, where the config names the env var; this stays for tests
    /// and any caller that already has a key.
    pub fn with_key(
        transport: &Transport,
        api_key: String,
        model: impl Into<String>,
        max_context: u32,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: transport.base_url.trim_end_matches('/').to_string(),
            api_key,
            model: {
                let m = model.into();
                if m.is_empty() {
                    DEFAULT_MODEL.to_string()
                } else {
                    m
                }
            },
            http: crate::http::client_with_timeout(transport.timeout_s)?,
            retry: crate::retry::Policy {
                max_retries: transport.max_retries,
                ..Default::default()
            },
            max_context,
        })
    }

    /// Builds a provider, resolving the credential from
    /// `transport.api_key_env` (`TYPESAFE_API_KEY` by default) or the
    /// keyring entry `cox/typesafe` (T30.21). Fails with
    /// [`ProviderError::Auth`] rather than panicking when neither has one —
    /// a missing key is the fail-open path, and it must read as auth, not
    /// as a transport failure.
    pub fn new(
        transport: &Transport,
        model: impl Into<String>,
        max_context: u32,
    ) -> Result<Self, ProviderError> {
        let api_key = crate::http::resolve_key(&transport.api_key_env, "typesafe")?;
        Self::with_key(transport, api_key, model, max_context)
    }

    /// One HTTP attempt; `stream` wraps it in the retry policy. Jev answers
    /// in a single JSON body (no SSE): the whole body parses at once, and
    /// cancellation only applies to the wait, not to a partial stream —
    /// there is no partial state to rewind.
    async fn stream_once(
        &self,
        req: &Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let started = std::time::Instant::now();
        let mut body = build_body(req);
        // The tier model wins over the section default when set.
        if !req.model.0.is_empty() {
            body["model"] = json!(req.model.0);
        }

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            res = self
                .http
                // CodeQL cleartext-transmission: the key travels only in the
                // Authorization header; base_url is user-configured (https by default).
                .post(format!("{}/v1/systemone", self.base_url))
                .header("content-type", "application/json")
                .header("authorization", crate::http::bearer(&self.api_key)?)
                .json(&body)
                .send() => res.map_err(|e| {
                    if e.is_timeout() {
                        ProviderError::Timeout
                    } else {
                        ProviderError::Network
                    }
                })?,
        };

        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body_text = response.text().await.unwrap_or_default();
            return Err(http_error(status, &body_text, retry_after));
        }

        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            t = response.text() => t.map_err(|_| ProviderError::Network)?,
        };
        let (mut events, mut usage) = parse_response(&text)?;
        usage.latency_ms = started.elapsed().as_millis() as u64;
        if let Some(ProviderEvent::Usage { usage: u }) = events.last_mut() {
            *u = usage;
        }
        for event in events {
            if sink.send(event).await.is_err() {
                return Err(ProviderError::Cancelled);
            }
        }
        Ok(usage)
    }
}

#[async_trait]
impl Provider for JevProvider {
    fn id(&self) -> ProviderId {
        // Type-1 native id: the wire is System One JSON, not an
        // OpenAI-compatible chat shape, so the Local-family bucket would
        // mislabel the ledger row (providers.md split is by wire protocol).
        ProviderId::Jev
    }

    fn capabilities(&self) -> Caps {
        Caps {
            // No prompt caching, no thinking replay, no server tools, no
            // count endpoint: Jev answers one JSON body per call.
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: false,
            // Resolved by the caller from the catalog (T30.25); see
            // `Self::max_context`'s doc. Decisions carry state, not
            // history, so the 128 000 fallback is roomy enough for any
            // single-turn state the judge composes.
            max_context: self.max_context,
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
        // No dedicated endpoint; T1.8's estimate covers it.
        Err(ProviderError::Unsupported {
            feature: "count_tokens".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::types::{Effort, Job, Message, ModelId, Role, Thinking, Tier};

    use super::*;

    fn req() -> Request {
        Request {
            tier: Tier::Cheap,
            job: Job::Hook,
            model: ModelId(String::new()),
            system: vec![cox_protocol::types::SystemBlock {
                text: "route this".into(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![cox_protocol::types::Content::Text {
                    text: "rm -rf /tmp/x".into(),
                }],
            }],
            effort: Effort::Low,
            max_tokens: 4096,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    #[test]
    fn jev_request_body_is_state_model_questions() {
        let body = build_body(&req());
        assert_eq!(body["model"], json!("jev-latest"));
        assert_eq!(body["questions"]["relevant"]["type"], json!("noul"));
        let state = body["state"].as_str().expect("state is a string");
        assert!(
            state.contains("route this"),
            "system text is state: {state}"
        );
        assert!(
            state.contains("rm -rf /tmp/x"),
            "message text is state: {state}"
        );
    }

    #[test]
    fn jev_choice_answer_becomes_one_json_text_delta() {
        let (events, usage) = parse_response(
            r#"{"model":"jev-1.13.0","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":0.88,"technical":0.12},"confidence":0.81}},"usage":{"input_tokens":318,"output_tokens":34}}"#,
        )
        .expect("well-formed choice parses");
        assert_eq!(events.len(), 3, "TextDelta + Stop + Usage: {events:?}");
        let text = match &events[0] {
            ProviderEvent::TextDelta { text } => text.clone(),
            other => panic!("first event is TextDelta, got {other:?}"),
        };
        let decision: Value = serde_json::from_str(&text).expect("decision is JSON");
        assert_eq!(decision["answer_kind"], json!("choice"));
        assert_eq!(decision["choice"], json!("billing"));
        assert_eq!(decision["confidence"], json!(0.81));
        assert_eq!(usage.input_tokens, 318);
        // Output is unmetered by contract, even when the wire says otherwise.
        assert_eq!(usage.output_tokens, 0);
        assert!(matches!(
            events[1],
            ProviderEvent::Stop {
                stop: StopReason::EndTurn
            }
        ));
    }

    #[test]
    fn jev_score_answer_carries_score_and_levels() {
        let (events, _) = parse_response(
            r#"{"model":"jev-1.13.0","answers":{"frustration":{"type":"score","score":1.05,"legend":{"0":"Calm","1":"Frustrated"},"probabilities":{"0":0.0,"1":0.95,"2":0.05},"confidence":0.92}},"usage":{"input_tokens":304,"output_tokens":18}}"#,
        )
        .expect("well-formed score parses");
        let text = match &events[0] {
            ProviderEvent::TextDelta { text } => text.clone(),
            other => panic!("first event is TextDelta, got {other:?}"),
        };
        let decision: Value = serde_json::from_str(&text).expect("decision is JSON");
        assert_eq!(decision["answer_kind"], json!("score"));
        assert_eq!(decision["score"], json!(1.05));
    }

    #[test]
    fn jev_noul_answer_carries_probability() {
        let (events, _) = parse_response(
            r#"{"model":"jev-1.13.0","answers":{"is_urgent":{"type":"noul","noul":0.95,"confidence":0.9}},"usage":{"input_tokens":307,"output_tokens":20}}"#,
        )
        .expect("well-formed noul parses");
        let text = match &events[0] {
            ProviderEvent::TextDelta { text } => text.clone(),
            other => panic!("first event is TextDelta, got {other:?}"),
        };
        let decision: Value = serde_json::from_str(&text).expect("decision is JSON");
        assert_eq!(decision["answer_kind"], json!("noul"));
        assert_eq!(decision["noul"], json!(0.95));
    }

    #[test]
    fn jev_missing_answers_is_parse_not_a_guess() {
        let err = parse_response(r#"{"model":"jev-1.13.0","answers":{}}"#)
            .expect_err("empty answers must fail");
        assert_eq!(err, ProviderError::Parse { line: 1 });
    }

    #[test]
    fn jev_unknown_answer_kind_is_parse() {
        let err = parse_response(
            r#"{"model":"jev-1.13.0","answers":{"q":{"type":"oracle","choice":"x"}}}"#,
        )
        .expect_err("unknown kind must fail");
        assert_eq!(err, ProviderError::Parse { line: 1 });
    }

    #[test]
    fn jev_http_errors_map_to_the_shared_taxonomy() {
        assert_eq!(
            http_error(reqwest::StatusCode::UNAUTHORIZED, "bad key", None),
            ProviderError::Auth
        );
        assert_eq!(
            http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "slow", Some(7)),
            ProviderError::RateLimited {
                retry_after: Some(7)
            }
        );
        assert_eq!(
            http_error(
                reqwest::StatusCode::from_u16(529).expect("529 exists"),
                "busy",
                None
            ),
            ProviderError::Overloaded
        );
        assert_eq!(
            http_error(
                reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                r#"{"error":{"message":"bad question"}}"#,
                None
            ),
            ProviderError::BadRequest {
                message: "bad question".into()
            }
        );
    }
}
