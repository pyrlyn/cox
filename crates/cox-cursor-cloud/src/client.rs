//! The one place a socket to `api.cursor.com` opens (T56.2, A123): create,
//! follow up, read, stream, cancel and usage over `cox-provider-http`
//! (connection setup, Bearer auth, status mapping, SSE framing, backoff).
//!
//! It is its own module because the rules here are specific to a paid,
//! non-idempotent API: only GETs retry, since a repeated create could start
//! and bill a second run, and a dropped stream resumes with `Last-Event-ID`
//! instead of starting over. The key is held for redaction only; it reaches
//! the wire as one sensitive `Authorization` header and never an error text.

// why: each method is named for the endpoint it calls (see `wire` for the docs
// page); a sentence per method would only restate that.
#![allow(missing_docs)]

use std::{fmt, pin::Pin, time::Duration};

use cox_protocol::errors::ProviderError;
use cox_provider_http::http::{bearer, client_with_timeout, map_http_error, resolve_key};
use cox_provider_http::retry::{Policy, retryable};
use cox_provider_http::sse::{SseFrame, sse_stream_with_id};
use futures::{Stream, StreamExt};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue, RETRY_AFTER, USER_AGENT};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};

use crate::wire::*;

const API: &str = "https://api.cursor.com";
const KEY_ENV: &str = "CURSOR_API_KEY";
// why: no User-Agent detail beyond name and version, so a request says nothing
// about the machine or the user (A123 (5)).
const AGENT: &str = concat!("cox/", env!("CARGO_PKG_VERSION"));

/// Why a Cloud Agents call failed. No variant carries the key.
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("cursor cloud: {0}")]
    Http(#[from] ProviderError),
    #[error("cursor cloud sent an unreadable response: {0}")]
    Decode(String),
    #[error("cursor cloud: refusing the id {0:?}")]
    InvalidId(String),
    #[error("cursor cloud: refusing to send the key to {0}")]
    InsecureBase(String),
    /// Cursor no longer keeps this run's stream; `get_run` has the final state.
    #[error("cursor cloud: the run's stream expired")]
    StreamExpired,
}

type Frames = Pin<Box<dyn Stream<Item = Result<(Option<String>, SseFrame), ()>> + Send>>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    auth: HeaderValue,
    key: String,
    policy: Policy,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("base", &self.base).finish()
    }
}

fn transport(e: reqwest::Error) -> CloudError {
    CloudError::Http(if e.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Network
    })
}

// why: ids come from Cursor; one with `/` or `?` would redirect the request.
fn id(s: &str) -> Result<&str, CloudError> {
    let ok = !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    ok.then_some(s)
        .ok_or_else(|| CloudError::InvalidId(s.chars().take(40).collect()))
}

impl Client {
    pub fn new(key: impl Into<String>) -> Result<Self, CloudError> {
        let key = key.into();
        Ok(Self {
            http: client_with_timeout(120)?,
            base: API.to_owned(),
            auth: bearer(&key)?,
            key,
            policy: Policy::default(),
        })
    }

    /// `CURSOR_API_KEY`, then the keyring entry `cox/cursor`.
    pub fn from_env() -> Result<Self, CloudError> {
        Self::new(resolve_key(KEY_ENV, "cursor")?)
    }

    /// Points the client elsewhere (a test server). Plain http is allowed to
    /// loopback only, so the key cannot go out unencrypted.
    pub fn with_base_url(mut self, base: &str) -> Result<Self, CloudError> {
        let url = reqwest::Url::parse(base)
            .map_err(|_| CloudError::InsecureBase("an unparsable url".into()))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !loopback {
            return Err(CloudError::InsecureBase(
                url.host_str().unwrap_or("").into(),
            ));
        }
        self.base = base.trim_end_matches('/').to_owned();
        Ok(self)
    }

    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header(AUTHORIZATION, self.auth.clone())
            .header(USER_AGENT, AGENT)
    }

    fn scrub(&self, text: &str) -> String {
        text.replace(&self.key, "[redacted]")
    }

    async fn send(&self, req: RequestBuilder) -> Result<Response, CloudError> {
        let resp = req.send().await.map_err(transport)?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if status == StatusCode::GONE {
            return Err(CloudError::StreamExpired);
        }
        let retry_after = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok()?.parse().ok());
        let body = resp.text().await.unwrap_or_default();
        // A server that echoes the request back must not echo the key into an error.
        Err(CloudError::Http(
            match map_http_error(status, &body, retry_after) {
                ProviderError::BadRequest { message } => ProviderError::BadRequest {
                    message: self.scrub(&message),
                },
                other => other,
            },
        ))
    }

    async fn json<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, CloudError> {
        let bytes = self.send(req).await?.bytes().await.map_err(transport)?;
        serde_json::from_slice(&bytes).map_err(|e| CloudError::Decode(self.scrub(&e.to_string())))
    }

    /// A create is sent exactly once: a retry could start a second billed run.
    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CloudError> {
        self.json(self.req(Method::POST, path).json(body)).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, CloudError> {
        let mut attempt = 0;
        loop {
            match self.json(self.req(Method::GET, path)).await {
                Err(CloudError::Http(e)) if attempt < self.policy.max_retries && retryable(&e) => {
                    let after = match e {
                        ProviderError::RateLimited { retry_after } => retry_after,
                        _ => None,
                    };
                    tokio::time::sleep(self.policy.delay(attempt, after)).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    pub async fn create_agent(
        &self,
        request: &CreateAgentRequest,
    ) -> Result<CreateAgentResponse, CloudError> {
        self.post("/v1/agents", request).await
    }

    pub async fn create_run(
        &self,
        agent: &str,
        request: &CreateRunRequest,
    ) -> Result<CreateRunResponse, CloudError> {
        self.post(&format!("/v1/agents/{}/runs", id(agent)?), request)
            .await
    }

    pub async fn get_run(&self, agent: &str, run: &str) -> Result<Run, CloudError> {
        self.get(&format!("/v1/agents/{}/runs/{}", id(agent)?, id(run)?))
            .await
    }

    pub async fn cancel_run(&self, agent: &str, run: &str) -> Result<CancelResponse, CloudError> {
        let path = format!("/v1/agents/{}/runs/{}/cancel", id(agent)?, id(run)?);
        self.json(self.req(Method::POST, &path)).await
    }

    pub async fn usage(&self, agent: &str) -> Result<UsageResponse, CloudError> {
        self.get(&format!("/v1/agents/{}/usage", id(agent)?)).await
    }

    /// The run's events, ending after `done`. A dropped connection re-reads
    /// the run and, unless it is terminal, resumes after the last event seen
    /// and skips the `status` frame Cursor re-sends on every resume, so no
    /// event arrives twice. A run that ended with its stream cut short ends
    /// the stream without its `result` event: read it with [`Client::get_run`].
    pub fn stream_run(
        &self,
        agent: &str,
        run: &str,
    ) -> Result<impl Stream<Item = Result<StreamEvent, CloudError>> + use<>, CloudError> {
        let live = Live {
            client: self.clone(),
            path: format!("/v1/agents/{}/runs/{}", id(agent)?, id(run)?),
            last_id: None,
            frames: None,
            failures: 0,
            resumed: false,
            ended: false,
        };
        Ok(futures::stream::unfold(live, |mut live| async move {
            let item = live.next().await?;
            Some((item, live))
        }))
    }
}

struct Live {
    client: Client,
    path: String,
    last_id: Option<String>,
    frames: Option<Frames>,
    /// Consecutive drops since an event last arrived; bounded by the policy.
    failures: u32,
    resumed: bool,
    ended: bool,
}

impl Live {
    async fn open(&self) -> Result<Frames, CloudError> {
        let mut req = self
            .client
            .req(Method::GET, &format!("{}/stream", self.path))
            .header(ACCEPT, "text/event-stream");
        if let Some(last) = &self.last_id {
            req = req.header("Last-Event-ID", last);
        }
        let resp = self.client.send(req).await?;
        Ok(Box::pin(
            sse_stream_with_id(resp.bytes_stream()).map(|frame| frame.map_err(|_| ())),
        ))
    }

    /// The run's state after the stream ended without `done`: `Ok(true)` when
    /// terminal.
    async fn terminal(&self) -> Result<bool, CloudError> {
        let run: Run = self.client.get(&self.path).await?;
        Ok(run.status.is_terminal())
    }

    async fn next(&mut self) -> Option<Result<StreamEvent, CloudError>> {
        loop {
            if self.ended {
                return None;
            }
            let Some(frames) = self.frames.as_mut() else {
                match self.open().await {
                    Ok(frames) => self.frames = Some(frames),
                    Err(CloudError::StreamExpired) => return self.finish().await,
                    Err(CloudError::Http(e)) if retryable(&e) => {
                        if let Err(e) = self.backoff(e).await {
                            return Some(Err(e));
                        }
                    }
                    Err(e) => return self.fail(e),
                }
                continue;
            };
            let Some(Ok((id, (event, data)))) = frames.next().await else {
                self.frames = None;
                match self.terminal().await {
                    Ok(true) => return self.finish().await,
                    Ok(false) => {}
                    Err(e) => return self.fail(e),
                }
                if let Err(e) = self.backoff(ProviderError::Network).await {
                    return Some(Err(e));
                }
                continue;
            };
            if id.is_some() {
                self.last_id = id;
            }
            let name = event.as_deref().unwrap_or("message");
            if std::mem::take(&mut self.resumed) && name == "status" {
                continue;
            }
            self.failures = 0;
            let parsed = StreamEvent::parse(name, &data)
                .map_err(|e| CloudError::Decode(self.client.scrub(&e.to_string())));
            self.ended = matches!(parsed, Ok(StreamEvent::Done) | Err(_));
            return Some(parsed);
        }
    }

    async fn finish(&mut self) -> Option<Result<StreamEvent, CloudError>> {
        self.ended = true;
        None
    }

    fn fail(&mut self, e: CloudError) -> Option<Result<StreamEvent, CloudError>> {
        self.ended = true;
        Some(Err(e))
    }

    /// Waits before the next reopen, or gives up after the policy's retries.
    async fn backoff(&mut self, error: ProviderError) -> Result<(), CloudError> {
        if self.failures >= self.client.policy.max_retries {
            self.ended = true;
            return Err(CloudError::Http(error));
        }
        let wait = self.client.policy.delay(self.failures, None);
        self.failures += 1;
        self.resumed = self.last_id.is_some();
        tokio::time::sleep(wait.min(Duration::from_secs(60))).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cox_provider_http::http::resolve_key_with;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const KEY: &str = "crsr_test_key_0123456789";

    fn client(server: &MockServer) -> Client {
        let key = resolve_key_with("COX_TEST_CURSOR_KEY_UNSET", "cursor", |_| Some(KEY.into()))
            .expect("injected key");
        Client::new(key)
            .and_then(|c| c.with_base_url(&server.uri()))
            .expect("client")
            .with_policy(Policy {
                max_retries: 2,
                base: Duration::from_millis(1),
            })
    }

    fn request() -> CreateAgentRequest {
        CreateAgentRequest::new(
            "fix it",
            Repo {
                url: "https://github.com/a/b".into(),
                starting_ref: None,
            },
        )
    }

    fn sse(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/event-stream")
    }

    const RUN: &str = r#"{"id":"run-1","status":"RUNNING"}"#;

    #[tokio::test]
    async fn client_sends_the_key_as_bearer_only() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(RUN, "application/json"))
            .mount(&server)
            .await;
        let run = client(&server).get_run("bc-1", "run-1").await.expect("run");
        assert_eq!(run.status, RunStatus::Running);
        let seen = &server.received_requests().await.expect("recorded")[0];
        assert_eq!(
            seen.headers.get("authorization").expect("auth"),
            &format!("Bearer {KEY}")
        );
        assert!(seen.headers.get("x-api-key").is_none());
        assert!(!seen.url.to_string().contains(KEY));
    }

    #[tokio::test]
    async fn client_user_agent_is_cox_version_only() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/usage"))
            .and(header("user-agent", AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_raw("{}", "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        client(&server).usage("bc-1").await.expect("usage");
        assert_eq!(AGENT, format!("cox/{}", env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn client_never_retries_create_but_retries_get() {
        let server = MockServer::start().await;
        for verb in ["POST", "GET"] {
            Mock::given(method(verb))
                .respond_with(ResponseTemplate::new(503))
                .mount(&server)
                .await;
        }
        let client = client(&server);
        assert!(client.create_agent(&request()).await.is_err());
        assert!(
            client
                .create_run("bc-1", &CreateRunRequest::new("more"))
                .await
                .is_err()
        );
        assert!(client.cancel_run("bc-1", "run-1").await.is_err());
        let posts = server.received_requests().await.expect("recorded").len();
        assert_eq!(posts, 3, "one attempt per POST");
        assert!(client.get_run("bc-1", "run-1").await.is_err());
        let all = server.received_requests().await.expect("recorded").len();
        assert_eq!(all - posts, 3, "a GET gets the first try plus max_retries");
    }

    #[tokio::test]
    async fn client_error_text_never_contains_the_key() {
        let server = MockServer::start().await;
        let echo = format!(r#"{{"error":{{"message":"bad header Bearer {KEY}"}}}}"#);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_raw(echo, "application/json"))
            .mount(&server)
            .await;
        let err = client(&server)
            .create_agent(&request())
            .await
            .expect_err("400");
        assert!(!format!("{err} {err:?}").contains(KEY));
        assert!(err.to_string().contains("[redacted]"));
        assert!(!format!("{:?}", client(&server)).contains(KEY));
    }

    #[tokio::test]
    async fn client_stream_reconnects_without_duplicate_events() {
        let server = MockServer::start().await;
        let status = "event: status\ndata: {\"status\":\"RUNNING\"}\n\n";
        let rest = "id: 2-0\nevent: assistant\ndata: {\"text\":\"two\"}\n\nid: 3-0\nevent: result\n\
                    data: {\"runId\":\"run-1\",\"status\":\"FINISHED\"}\n\nid: 4-0\nevent: done\ndata: {}\n\n";
        let first = format!("{status}id: 1-0\nevent: assistant\ndata: {{\"text\":\"one\"}}\n\n");
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .and(header("last-event-id", "1-0"))
            .respond_with(sse(&format!("{status}{rest}")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .respond_with(sse(&first))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(RUN, "application/json"))
            .mount(&server)
            .await;
        let events: Vec<_> = client(&server)
            .stream_run("bc-1", "run-1")
            .expect("ids")
            .collect()
            .await;
        let events: Vec<_> = events.into_iter().map(|e| e.expect("event")).collect();
        let text = |t: &str| StreamEvent::Assistant { text: t.into() };
        assert_eq!(events.len(), 5, "{events:?}");
        assert_eq!(events[0], StreamEvent::Status(RunStatus::Running));
        assert_eq!([&events[1], &events[2]], [&text("one"), &text("two")]);
        assert!(matches!(events[3], StreamEvent::Result(_)));
        assert_eq!(events[4], StreamEvent::Done);
    }

    #[tokio::test]
    async fn client_stream_ends_when_the_dropped_run_is_terminal() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1/stream"))
            .respond_with(sse("event: status\ndata: {\"status\":\"RUNNING\"}\n\n"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/agents/bc-1/runs/run-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"id":"run-1","status":"FINISHED"}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let events: Vec<_> = client(&server)
            .stream_run("bc-1", "run-1")
            .expect("ids")
            .collect()
            .await;
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn client_refuses_ids_that_change_the_path() {
        for bad in ["", "..", "a/b", "a?x=1", "a#b", "a b"] {
            assert!(matches!(id(bad), Err(CloudError::InvalidId(_))), "{bad}");
        }
        assert_eq!(id("bc-1.x_2").expect("fine"), "bc-1.x_2");
    }

    #[test]
    fn client_refuses_plain_http_to_a_remote_host() {
        let client = Client::new(KEY).expect("client");
        assert!(client.clone().with_base_url("http://example.com").is_err());
        assert!(
            client
                .clone()
                .with_base_url("http://localhost.evil.com")
                .is_err()
        );
        assert!(client.with_base_url("http://127.0.0.1:9").is_ok());
    }
}
