// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Anthropic Messages API backend: the client handle, its headers and
//! its credential lookup. The body translation lives next door in
//! [`request`] so it can be snapshot-tested with no key and no socket; the
//! SSE → `ProviderEvent` state machine lives in [`stream`] for the same
//! reason (fixtures, no socket).
//!
//! Kept apart from the OpenAI backends because the fields that decide cost
//! here — `cache_control` placement, `output_config.effort`, adaptive
//! `thinking`, `fallbacks` — have no counterpart there (D3).
//!
//! Its own crate (T32.13, `docs/design/crates.md`) because it alone runs the
//! typify build step over the vendored spec in `schema/`, and for its size;
//! `cox-provider` re-exports it at the old `cox_provider::anthropic` path.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod request;
pub mod stream;
/// Wire types generated from the vendored OpenAPI spec (T30.12) — internal
/// to this module; `request` and `stream` are the only consumers.
mod wire;

use async_trait::async_trait;
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, Usage};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
// Imported at the crate root so the `crate::http`/`crate::retry`/`crate::sse`
// paths this wire used inside `cox-provider` resolve unchanged.
use cox_provider_http::{http, retry, sse};

/// The API version cox pins; Anthropic requires it on every request.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The beta that gates the scalar `fallbacks: "default"` form. The array
/// form uses `server-side-fallback-2026-06-01` instead and pairing either
/// header with the other form is a 400, so the two are never both sent.
pub const FALLBACKS_BETA: &str = "server-side-fallback-2026-07-01";

/// How long a cache entry written by a `cache_control` breakpoint lives
/// (`providers.anthropic.cache_ttl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheTtl {
    /// The API default.
    #[default]
    FiveMinutes,
    /// Longer TTL for bursty sessions; writes cost more.
    OneHour,
}

impl CacheTtl {
    /// The wire value for `cache_control.ttl`.
    pub fn as_str(self) -> &'static str {
        match self {
            CacheTtl::FiveMinutes => "5m",
            CacheTtl::OneHour => "1h",
        }
    }
}

/// A configured Anthropic Messages client.
///
/// Fields are public because `cox-core` builds one straight from
/// `[providers.anthropic]`; there is no behaviour worth hiding behind
/// accessors.
pub struct AnthropicProvider {
    /// `providers.anthropic.base_url`, without a trailing slash.
    pub base_url: String,
    /// The resolved credential (see [`crate::http::resolve_key`]), or
    /// `None` for a keyless client (T30.15: LM Studio's `/v1/messages`
    /// without "Require Authentication" needs no `x-api-key` at all — the
    /// real Anthropic API always fails `Auth` before a `None` reaches
    /// here, since [`Self::new`] requires a resolved key).
    pub api_key: Option<String>,
    /// Sent as `anthropic-workspace-id` (see [`resolve_workspace_id`]).
    pub workspace_id: Option<String>,
    /// TTL written into every `cache_control` block.
    pub ttl: CacheTtl,
    /// Whether to send `fallbacks: "default"` and its beta header.
    pub fallbacks: bool,
    /// The shared connection pool.
    pub http: reqwest::Client,
    /// Backoff for transient failures before the first byte (T1.6).
    pub retry: crate::retry::Policy,
    /// `Caps.max_context` (T30.25): resolved once by the caller from the
    /// model catalog (`cox_models::Catalog`, merged with `[providers.
    /// anthropic].models`) for `tiers.code.model`, not looked up here —
    /// `capabilities()` carries no per-request model, so this is the same
    /// "resolve at construction" shape `openai_shaped`'s `context_window`
    /// parameter already used before this card. `backend_for_with` in
    /// `crates/cox-session/src/provider.rs` does the lookup and falls back to
    /// 200 000 — today's literal — when the catalog has no row for the
    /// configured model.
    pub max_context: u32,
}

impl AnthropicProvider {
    /// Builds a provider from `&Transport` (`providers.anthropic`, T30.23)
    /// with an already-resolved credential, or `None` for a keyless client
    /// (T30.15: LM Studio's `/v1/messages` without "Require
    /// Authentication"). Prefer [`Self::new`] at session startup for the
    /// real Anthropic API, which always needs a key; this stays for tests
    /// and any caller that already resolved (or deliberately skipped) one
    /// (mirrors `JevProvider::with_key`) — `backend_for_with` in
    /// `crates/cox-session/src/provider.rs` builds through this so a test can inject
    /// the lookup instead of ever reaching the real keyring (A49, T30.28).
    pub fn with_key(
        transport: &cox_protocol::config::Transport,
        api_key: Option<String>,
        ttl: CacheTtl,
        fallbacks: bool,
        max_context: u32,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            base_url: transport.base_url.trim_end_matches('/').to_string(),
            api_key,
            workspace_id: resolve_workspace_id(),
            ttl,
            fallbacks,
            http: crate::http::client_with_timeout(transport.timeout_s)?,
            retry: crate::retry::Policy {
                max_retries: transport.max_retries,
                ..Default::default()
            },
            max_context,
        })
    }

    /// Builds a provider from the section's `&Transport` (`providers.
    /// anthropic`, T30.23) plus its own section-specific knobs (cache TTL,
    /// whether to send `fallbacks`, the catalog-resolved `max_context`,
    /// T30.25). Resolves the credential from `transport.api_key_env`
    /// (default `ANTHROPIC_API_KEY`) or the keyring entry `cox/anthropic`
    /// (T30.21). Fails with [`ProviderError::Auth`] rather than panicking
    /// when neither has one. `transport.timeout_s` is the idle-read timeout
    /// between stream chunks, not a whole-call cap: a long answer streams
    /// for minutes without ever being idle.
    pub fn new(
        transport: &cox_protocol::config::Transport,
        ttl: CacheTtl,
        fallbacks: bool,
        max_context: u32,
    ) -> Result<Self, ProviderError> {
        let api_key = crate::http::resolve_key(&transport.api_key_env, "anthropic")?;
        Self::with_key(transport, Some(api_key), ttl, fallbacks, max_context)
    }

    /// The headers every call carries. `anthropic-beta` is assembled from
    /// the features actually enabled, so a request never claims a beta it
    /// does not use (some betas 400 when paired with the wrong body shape).
    pub fn headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut h = HeaderMap::new();
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        // An api key with non-ASCII bytes is a misconfigured credential, not
        // a transport failure: report it as an auth problem. `None` (T30.15:
        // LM Studio without "Require Authentication") sends no header at
        // all, rather than an empty one.
        if let Some(key) = &self.api_key {
            h.insert("x-api-key", crate::http::api_key(key)?);
        }
        if let Some(id) = &self.workspace_id {
            // Same reasoning as the key: a bad id is a credential problem.
            let value = HeaderValue::from_str(id).map_err(|_| ProviderError::Auth)?;
            h.insert("anthropic-workspace-id", value);
        }

        let betas = self.betas();
        if !betas.is_empty() {
            let value = HeaderValue::from_str(&betas.join(",")).map_err(|_| {
                ProviderError::Unsupported {
                    feature: "anthropic-beta".into(),
                }
            })?;
            h.insert("anthropic-beta", value);
        }
        Ok(h)
    }

    /// The `anthropic-beta` values implied by the enabled features.
    pub fn betas(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.fallbacks {
            v.push(FALLBACKS_BETA);
        }
        v
    }

    /// The config [`request::build_body`] needs from this provider.
    pub fn build_cfg<'a>(
        &self,
        thinking_model: Option<&'a cox_protocol::types::ModelId>,
    ) -> request::BuildCfg<'a> {
        request::BuildCfg {
            ttl: self.ttl,
            fallbacks: self.fallbacks,
            thinking_model,
        }
    }
}

/// `ANTHROPIC_WORKSPACE_ID`, blank meaning unset. A key that is not scoped
/// to a workspace gets a 400 on every call unless this header names one.
/// Not a secret, so there is no keyring fallback.
pub fn resolve_workspace_id() -> Option<String> {
    std::env::var("ANTHROPIC_WORKSPACE_ID")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: true,
            thinking: true,
            server_tools: true,
            count_tokens: true,
            // Resolved by the caller from the catalog (T30.25); see
            // `Self::max_context`'s doc.
            max_context: self.max_context,
        }
    }

    /// Every Messages model takes base64 image blocks.
    fn accepts_images(&self, _model: &str) -> bool {
        true
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
        // `POST {base_url}/v1/messages/count_tokens` with the same body;
        // out of scope for T1.2 (streaming only) — lands in T1.8.
        Err(ProviderError::Unsupported {
            feature: "count_tokens lands in T1.8".into(),
        })
    }
}

impl AnthropicProvider {
    /// One HTTP attempt; `stream` wraps it in the retry policy.
    async fn stream_once(
        &self,
        req: &Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let started = std::time::Instant::now();
        // No `ModelSwitched` signal reaches a `Provider`: `stream`'s
        // signature (cox-protocol T0.2) carries only the `Request`, so
        // thinking-block replay stays off here — see request.rs's
        // `BuildCfg::thinking_model` doc and this module's `stream` doc.
        let body = request::build_body(req, self.build_cfg(None))?;

        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
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
            return Err(http_error(status, &body_text, retry_after));
        }

        let mut frames = std::pin::pin!(crate::sse::sse_stream(response.bytes_stream()));
        let mut machine = stream::AnthropicStream::new();
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
                // The receiving end (`cox-core`) hung up: nothing left to
                // stream to, so unwind as a cancellation rather than
                // silently dropping the rest of the call.
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

/// Maps a non-2xx `/v1/messages` response to a `ProviderError` (plan.md
/// §1.14). `retry_after` comes from the `retry-after` header, read before
/// the body is consumed. One shared mapping lives in [`crate::http`]; this
/// stays as the module's named entry point for it.
fn http_error(status: reqwest::StatusCode, body: &str, retry_after: Option<u64>) -> ProviderError {
    crate::http::map_http_error(status, body, retry_after)
}

#[cfg(test)]
// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#[allow(unsafe_code)]
mod tests {
    use super::*;

    fn provider(fallbacks: bool) -> AnthropicProvider {
        AnthropicProvider {
            base_url: "https://api.anthropic.com".into(),
            api_key: Some("sk-test".into()),
            workspace_id: None,
            ttl: CacheTtl::FiveMinutes,
            fallbacks,
            http: reqwest::Client::new(),
            retry: crate::retry::Policy {
                max_retries: 4,
                base: std::time::Duration::from_millis(1),
            },
            max_context: 200_000,
        }
    }

    async fn drain_events(rx: &mut mpsc::Receiver<ProviderEvent>) -> Vec<ProviderEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    #[test]
    fn beta_header_only_lists_enabled_features() {
        assert!(provider(false).betas().is_empty());
        assert_eq!(provider(true).betas(), vec![FALLBACKS_BETA]);

        let h = provider(false).headers().expect("headers build");
        assert!(h.get("anthropic-beta").is_none());
        assert_eq!(
            h.get("anthropic-version").and_then(|v| v.to_str().ok()),
            Some(ANTHROPIC_VERSION)
        );

        let h = provider(true).headers().expect("headers build");
        assert_eq!(
            h.get("anthropic-beta").and_then(|v| v.to_str().ok()),
            Some(FALLBACKS_BETA)
        );
    }

    #[test]
    fn workspace_header_is_sent_only_when_configured() {
        let h = provider(false).headers().expect("headers build");
        assert!(h.get("anthropic-workspace-id").is_none());

        let mut p = provider(false);
        p.workspace_id = Some("wrkspc_01abc".into());
        let h = p.headers().expect("headers build");
        assert_eq!(
            h.get("anthropic-workspace-id")
                .and_then(|v| v.to_str().ok()),
            Some("wrkspc_01abc")
        );

        p.workspace_id = Some("bad\nid".into());
        assert!(matches!(p.headers(), Err(ProviderError::Auth)));
    }

    #[test]
    fn blank_workspace_env_means_unset() {
        // Safety: see `missing_credential_is_auth_error_not_a_panic`.
        unsafe { std::env::set_var("ANTHROPIC_WORKSPACE_ID", "  ") };
        assert_eq!(resolve_workspace_id(), None);
        unsafe { std::env::set_var("ANTHROPIC_WORKSPACE_ID", " wrkspc_01abc ") };
        assert_eq!(resolve_workspace_id().as_deref(), Some("wrkspc_01abc"));
        unsafe { std::env::remove_var("ANTHROPIC_WORKSPACE_ID") };
    }

    #[test]
    fn missing_credential_is_auth_error_not_a_panic() {
        // A49 (T30.28): inject the lookup rather than calling `resolve_key`,
        // whose fallback is the real platform keyring.
        // Safety: single-threaded test process section; no other thread reads
        // the environment while this runs.
        unsafe { std::env::remove_var("COX_TEST_ANTHROPIC_MISSING") };
        assert!(matches!(
            crate::http::resolve_key_with("COX_TEST_ANTHROPIC_MISSING", "anthropic", |_| None),
            Err(ProviderError::Auth)
        ));

        unsafe { std::env::set_var("COX_TEST_ANTHROPIC_MISSING", "sk-from-env") };
        assert_eq!(
            crate::http::resolve_key_with("COX_TEST_ANTHROPIC_MISSING", "anthropic", |_| {
                panic!("the keyring must not be consulted when the env var is set")
            })
            .ok()
            .as_deref(),
            Some("sk-from-env")
        );
        unsafe { std::env::remove_var("COX_TEST_ANTHROPIC_MISSING") };
    }

    /// T30.21: `AnthropicProvider::new` reads whatever env var
    /// `providers.anthropic.api_key_env` names, not a hardcoded
    /// `ANTHROPIC_API_KEY` — a renamed section still resolves its key.
    #[test]
    fn provider_new_honours_a_renamed_api_key_env() {
        // Safety: see `missing_credential_is_auth_error_not_a_panic`.
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        unsafe { std::env::set_var("MY_RENAMED_ANTHROPIC_KEY", "sk-renamed") };
        let transport = cox_protocol::config::Transport {
            base_url: "https://api.anthropic.com".to_string(),
            api_key_env: "MY_RENAMED_ANTHROPIC_KEY".to_string(),
            timeout_s: 30,
            max_retries: 1,
        };
        let p = AnthropicProvider::new(&transport, CacheTtl::FiveMinutes, true, 200_000)
            .expect("builds with the renamed env var");
        assert_eq!(p.api_key.as_deref(), Some("sk-renamed"));
        unsafe { std::env::remove_var("MY_RENAMED_ANTHROPIC_KEY") };
    }

    /// T30.25: `capabilities().max_context` is whatever the caller resolved
    /// at construction, not a fixed literal — this is the provider-level
    /// half of the "1M-context model reports 1M" claim; the catalog lookup
    /// itself is proved in `crates/cox-session/src/provider.rs`'s
    /// `anthropic_capabilities_report_the_configured_models_context_window`.
    #[test]
    fn capabilities_max_context_is_whatever_was_resolved_at_construction() {
        let mut p = provider(false);
        p.max_context = 1_000_000;
        assert_eq!(p.capabilities().max_context, 1_000_000);
    }

    #[test]
    fn http_error_maps_known_statuses() {
        assert!(matches!(
            http_error(reqwest::StatusCode::UNAUTHORIZED, "{}", None),
            ProviderError::Auth
        ));
        assert!(matches!(
            http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "{}", Some(30)),
            ProviderError::RateLimited {
                retry_after: Some(30)
            }
        ));
        assert!(matches!(
            http_error(reqwest::StatusCode::SERVICE_UNAVAILABLE, "{}", None),
            ProviderError::Overloaded
        ));
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 250000 tokens > 200000 maximum"}}"#;
        assert_eq!(
            http_error(reqwest::StatusCode::BAD_REQUEST, body, None),
            ProviderError::ContextTooLong {
                max: 200_000,
                got: 250_000,
            }
        );
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: roles must alternate"}}"#;
        assert_eq!(
            http_error(reqwest::StatusCode::BAD_REQUEST, body, None),
            ProviderError::BadRequest {
                message: "messages: roles must alternate".into(),
            }
        );
    }

    fn minimal_request() -> Request {
        use cox_protocol::types::{Content, Effort, Job, Message, Role, Thinking, Tier};

        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: cox_protocol::types::ModelId("claude-sonnet-5".into()),
            system: vec![],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "read a.rs".into(),
                }],
            }],
            effort: Effort::High,
            max_tokens: 1024,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    /// Contract test (plan.md §1.5): the wired-up `Provider::stream` client,
    /// driven against a `wiremock` server serving a real fixture byte-for-
    /// byte, must produce exactly what the pure state machine produces from
    /// the same fixture — and a `Usage` that carries the cache fields.
    #[tokio::test]
    async fn anthropic_stream_over_http() {
        let fixture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/anthropic/one_tool_call.sse"),
        )
        .expect("fixture reads");

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(fixture.clone(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let mut client = provider(false);
        client.base_url = server.uri();

        let (tx, mut rx) = mpsc::channel(64);
        let got_usage = client
            .stream(minimal_request(), tx, CancellationToken::new())
            .await
            .expect("stream succeeds");

        let got_events = drain_events(&mut rx).await;

        // Golden: the same fixture replayed straight through the pure state
        // machine (no network) must match exactly.
        let mut machine = stream::AnthropicStream::new();
        let mut want_events = Vec::new();
        for (event, data) in crate::sse::parse_sse_str(&fixture) {
            want_events.extend(
                machine
                    .feed(event.as_deref(), &data)
                    .expect("fixture parses"),
            );
        }

        // `ToolUseStart.id` is a freshly minted random `CallId` on every
        // `feed` call (see `stream.rs`'s module header), so the live and
        // golden runs mint different ones even for the same fixture;
        // normalize both before comparing structure.
        assert_eq!(
            stream::normalize_tool_ids(got_events),
            stream::normalize_tool_ids(want_events)
        );
        assert_eq!(got_usage.cache_read_tokens, 50);
        assert_eq!(got_usage.cache_write_tokens, 100);
        assert_eq!(got_usage.output_tokens, 24);
    }

    /// T30.15: `with_key(None, …)` — the LM Studio path when "Require
    /// Authentication" is off — sends no `x-api-key` header at all. The
    /// later-mounted mock only matches when `x-api-key` *is* sent and
    /// answers 401, so the client wrongly sending it fails the test (same
    /// shape as `openai::chat`'s `chat_over_http_ollama_shaped`). The
    /// stream still parses the tool-call fixture end to end, proving the
    /// wire and parsing are unchanged (no new wire types, plan.md T30.15).
    #[tokio::test]
    async fn stream_sends_no_x_api_key_header_without_a_key() {
        let fixture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/anthropic/one_tool_call.sse"),
        )
        .expect("fixture reads");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(fixture.clone(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::header_exists("x-api-key"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_string("no auth wanted"))
            .mount(&server)
            .await;

        let transport = cox_protocol::config::Transport {
            base_url: server.uri(),
            api_key_env: String::new(),
            timeout_s: 30,
            max_retries: 0,
        };
        let client =
            AnthropicProvider::with_key(&transport, None, CacheTtl::FiveMinutes, false, 32_768)
                .expect("builds keyless");

        let (tx, mut rx) = mpsc::channel(64);
        client
            .stream(minimal_request(), tx, CancellationToken::new())
            .await
            .expect("keyless stream succeeds");
        let events = drain_events(&mut rx).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::ToolUseStart { .. }))
        );
    }

    /// T30.15: a resolved key (LM Studio's `LM_API_TOKEN`, or Anthropic's
    /// own) is sent as `x-api-key` — the same header LM Studio's
    /// Anthropic-compatible path accepts (R§4.3.2).
    #[tokio::test]
    async fn stream_sends_x_api_key_header_with_a_key() {
        let fixture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/anthropic/one_tool_call.sse"),
        )
        .expect("fixture reads");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::header("x-api-key", "lm-test-token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(fixture, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let transport = cox_protocol::config::Transport {
            base_url: server.uri(),
            api_key_env: String::new(),
            timeout_s: 30,
            max_retries: 0,
        };
        let client = AnthropicProvider::with_key(
            &transport,
            Some("lm-test-token".to_string()),
            CacheTtl::FiveMinutes,
            false,
            32_768,
        )
        .expect("builds with a key");

        let (tx, mut rx) = mpsc::channel(64);
        client
            .stream(minimal_request(), tx, CancellationToken::new())
            .await
            .expect("keyed stream succeeds");
        let events = drain_events(&mut rx).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::ToolUseStart { .. }))
        );
    }

    /// T1.6: two 429s before any byte, then a 200 — the caller sees two
    /// `Retrying` events and one clean stream.
    #[tokio::test]
    async fn retry_retries_then_succeeds() {
        use wiremock::matchers::{method, path};
        let fixture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/anthropic/one_tool_call.sse"),
        )
        .expect("fixture reads");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(fixture, "text/event-stream"),
            )
            .mount(&server)
            .await;
        let mut client = provider(false);
        client.base_url = server.uri();

        let (tx, mut rx) = mpsc::channel(64);
        let usage = client
            .stream(minimal_request(), tx, CancellationToken::new())
            .await
            .expect("third attempt succeeds");
        assert_eq!(usage.output_tokens, 24);
        let events = drain_events(&mut rx).await;
        let attempts: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::Retrying { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(attempts, [1, 2]);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::MessageStart { .. }))
        );
        assert_eq!(server.received_requests().await.map(|r| r.len()), Some(3));
    }

    /// T1.6: cancelling mid-stream drops the response body, which closes the
    /// socket — observed by a raw server as EOF within 200 ms.
    #[tokio::test]
    async fn retry_cancel_mid_stream_drops_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
            let frame = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-5\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";
            let chunk = format!("{head}{:x}\r\n{frame}\r\n", frame.len());
            sock.write_all(chunk.as_bytes()).await.expect("write");
            // Keep the stream open; the client's close is the only way out.
            loop {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return std::time::Instant::now(),
                    Ok(_) => {}
                }
            }
        });
        let mut client = provider(false);
        client.base_url = format!("http://{addr}");
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(64);
        let streaming = {
            let cancel = cancel.clone();
            tokio::spawn(async move { client.stream(minimal_request(), tx, cancel).await })
        };
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("first event in time");
        assert!(matches!(first, Some(ProviderEvent::MessageStart { .. })));
        let cancelled_at = std::time::Instant::now();
        cancel.cancel();
        assert!(matches!(
            streaming.await.expect("join"),
            Err(ProviderError::Cancelled)
        ));
        let closed_at = tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("server saw the close")
            .expect("server task");
        assert!(closed_at.duration_since(cancelled_at) < std::time::Duration::from_millis(200));
    }
}
