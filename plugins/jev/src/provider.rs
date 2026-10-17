//! `cox_provider_stream` for the `typesafe` section (T33.40.3, PL§7a): turns
//! one `ProviderCall` into one `POST /v1/systemone` through `cox_http` and
//! the reply into `ProviderEvent`s. The host injects the key and allows only
//! this section's host, so neither appears here. Pure apart from the two
//! closures [`stream`] takes, so the whole path tests on the host target with
//! a fake transport; `lib.rs` passes the real `cox_http` and `cox_notify`.
//!
//! Two kinds of request reach this export:
//! - a decision call (job `plugin:jev`, T33.40.1's own-provider call-out),
//!   whose one user message already holds the System One body; it is sent
//!   byte for byte, because the advisor built and capped it;
//! - a request from a tier that names `typesafe`. Jev generates no text
//!   (J13), so it gets the old lossy `jev.rs` mapping and one warning.

use std::sync::atomic::{AtomicBool, Ordering};

use cox_plugin_sdk::{AbiError, HttpReq, HttpResp, ProviderCall, SdkError, Value};
use serde::Deserialize;
use serde_json::json;

use crate::wire::{Question, SystemOneRequest};

/// The manifest's `base_url` (`plugin.toml`, J1). `ProviderCall` carries no
/// base URL, so the guest names it; the host still refuses any other host.
pub const BASE_URL: &str = "https://api.typesafe.ai";

/// The one warning a tier request gets (J13).
pub const TIER_WARNING: &str = "typesafe is a decision model; no tier should route to it";

/// The job a decision call carries: `Job::Plugin("jev")` as the host
/// serializes it (`cox_protocol::types::Job`).
const DECISION_JOB: &str = "plugin:jev";

/// The fields of `cox_protocol::types::Request` this export reads. The ABI
/// hands the request over as JSON (`cox-plugin-api` cannot depend on
/// `cox-protocol`), and unknown fields are ignored so a newer host's
/// additions do not break this guest.
#[derive(Deserialize)]
struct Request {
    job: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    system: Vec<Block>,
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    #[serde(default)]
    content: Vec<Block>,
}

/// One `Content` (or `SystemBlock`) with every variant's fields optional,
/// so images, thinking and pointers parse and are then skipped.
#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

/// The export's error. Its text is the `AbiError` as JSON, because extism
/// passes an export failure to the host only as text; JSON keeps the kind
/// (`transient` for 429/529) readable by the host's retry.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamError(pub AbiError);

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(&self.0) {
            Ok(json) => f.write_str(&json),
            Err(_) => write!(f, "{}", self.0),
        }
    }
}

impl From<AbiError> for StreamError {
    fn from(e: AbiError) -> Self {
        Self(e)
    }
}

impl From<SdkError> for StreamError {
    fn from(e: SdkError) -> Self {
        match e {
            SdkError::Host(abi) => Self(abi),
            SdkError::Wire(message) => Self(AbiError::Failed { message }),
        }
    }
}

/// Runs one provider call: builds the System One request, sends it with
/// `send`, reports the tier warning through `notify` the first time
/// `warned` is unset, and maps the reply.
pub fn stream(
    call: &ProviderCall,
    warned: &AtomicBool,
    send: impl FnOnce(&HttpReq) -> Result<HttpResp, SdkError>,
    notify: impl FnOnce(&str),
) -> Result<Vec<Value>, StreamError> {
    let request: Request = serde_json::from_value(call.request.clone())
        .map_err(|e| failed(format!("the provider request did not parse: {e}")))?;
    let body = if request.job == DECISION_JOB {
        decision_body(&request)?
    } else {
        if !warned.swap(true, Ordering::Relaxed) {
            notify(TIER_WARNING);
        }
        lossy_body(&request, &call.model)?
    };
    let response = send(&HttpReq {
        method: "POST".into(),
        // The host's bound base URL (a user override) is the only one
        // `cox_http` allows; the constant serves an older host.
        url: format!(
            "{}/v1/systemone",
            call.base_url.as_deref().unwrap_or(BASE_URL)
        ),
        headers: [("content-type".to_string(), "application/json".to_string())].into(),
        body: Some(body),
    })?;
    if let Some(error) = status_error(response.status, &response.body) {
        return Err(error.into());
    }
    Ok(events(&response.body))
}

/// A decision call's body: the text of its one user message, unchanged.
/// Anything else is refused before a paid request is sent.
fn decision_body(request: &Request) -> Result<String, StreamError> {
    let shape = || failed("a decision call carries its System One body as one user text message");
    let [message] = request.messages.as_slice() else {
        return Err(shape());
    };
    let [block] = message.content.as_slice() else {
        return Err(shape());
    };
    let text = match (message.role.as_str(), block.kind.as_str(), &block.text) {
        ("user", "text", Some(text)) => text,
        _ => return Err(shape()),
    };
    serde_json::from_str::<Value>(text)
        .map_err(|_| failed("a decision call's body is not JSON"))?;
    Ok(text.clone())
}

/// The old `jev.rs` `build_body` mapping: system, text, tool names and tool
/// results joined into `state`, with one Noul. Kept only so a misrouted
/// tier gets an answer instead of an error; `SystemOneRequest::new` adds
/// the J6 cap the old code lacked.
fn lossy_body(request: &Request, model: &str) -> Result<String, StreamError> {
    let mut parts: Vec<&str> = request
        .system
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect();
    for block in request.messages.iter().flat_map(|m| &m.content) {
        let part = match block.kind.as_str() {
            "text" => block.text.as_deref(),
            "tool_use" => block.name.as_deref(),
            "tool_result" => block.content.as_deref(),
            // Thinking, images and archive pointers are not decision state.
            _ => None,
        };
        parts.extend(part);
    }
    parts.retain(|p| !p.is_empty());
    let model = if model.is_empty() {
        request.model.as_str()
    } else {
        model
    };
    let question = Question::Noul {
        instructions: json!("Is this state worth a typed decision (route, rank, or gate)?"),
        criteria: None,
    };
    let body = SystemOneRequest::new(
        json!(parts.join("\n\n")),
        model,
        [("relevant".to_string(), question)].into(),
    );
    serde_json::to_string(&body).map_err(|e| failed(e.to_string()))
}

/// J4's status codes as ABI error kinds. 429 (rate limit) and 529
/// (overload) are what the vendor says to back off on, so they and other
/// 5xx are `Transient`; 401 and 422 are the caller's fault and retrying
/// cannot fix them. `None` for a 2xx.
pub fn status_error(status: u16, body: &str) -> Option<AbiError> {
    let detail = |what: &str| format!("typesafe returned {status} ({what}): {}", snippet(body));
    match status {
        200..=299 => None,
        429 => Some(AbiError::Transient {
            message: detail("rate limited"),
        }),
        529 => Some(AbiError::Transient {
            message: detail("overloaded"),
        }),
        500..=599 => Some(AbiError::Transient {
            message: detail("server error"),
        }),
        401 => Some(AbiError::Failed {
            message: detail("invalid API key"),
        }),
        422 => Some(AbiError::Failed {
            message: detail("invalid request body"),
        }),
        _ => Some(AbiError::Failed {
            message: detail("unexpected status"),
        }),
    }
}

/// The reply as events: the body verbatim as text (the advisor parses it
/// with `wire::parse`), a stop, and the usage Jev reported. Usage passes
/// through unchanged; the host fills or floors it from its own estimate
/// (T33.18), so none is invented here.
fn events(body: &str) -> Vec<Value> {
    let reply = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut out = Vec::with_capacity(4);
    if let Some(model) = reply.get("model").and_then(Value::as_str) {
        out.push(json!({ "type": "message_start", "model": model }));
    }
    out.push(json!({ "type": "text_delta", "text": body }));
    out.push(json!({ "type": "stop", "stop": { "type": "end_turn" } }));
    if let Some(usage) = reply.get("usage") {
        let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
        out.push(json!({
            "type": "usage",
            "usage": {
                "input_tokens": count("input_tokens"),
                "output_tokens": count("output_tokens"),
                "cache_read_tokens": 0,
                "cache_write_tokens": 0,
                "estimated": false,
                "cost_usd": 0.0,
                "latency_ms": 0,
            },
        }));
    }
    out
}

/// At most 200 chars of an error body, cut at a char boundary.
fn snippet(body: &str) -> &str {
    match body.char_indices().nth(200) {
        Some((end, _)) => &body[..end],
        None => body,
    }
}

fn failed(message: impl Into<String>) -> StreamError {
    StreamError(AbiError::Failed {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const DECISION: &str = r#"{"state":{"cmd":"rm -rf build"},"model":"jev-1.13.0","questions":{"risky":{"type":"noul","instructions":"Is this irreversible?"}}}"#;

    const REPLY: &str = r#"{"model":"jev-1.13.0","answers":{"risky":{"type":"noul","noul":0.9}},"usage":{"input_tokens":41,"output_tokens":3}}"#;

    fn call(job: &str, messages: Value) -> ProviderCall {
        ProviderCall {
            request: json!({
                "tier": "cheap", "job": job, "model": "jev-1.13.0",
                "system": [{ "text": "you are cox", "cache": true }],
                "tools": [], "messages": messages, "effort": "low", "max_tokens": 16,
                "thinking": { "type": "off" }, "cache_breakpoints": [], "stop_sequences": [],
            }),
            model: "jev-1.13.0".into(),
            base_url: None,
        }
    }

    fn user(text: &str) -> Value {
        json!([{ "role": "user", "content": [{ "type": "text", "text": text }] }])
    }

    /// Runs [`stream`] against a fake transport answering `status`/`body`,
    /// returning the result, the request sent and the notices raised.
    fn run(
        call: &ProviderCall,
        warned: &AtomicBool,
        status: u16,
        body: &str,
    ) -> (
        Result<Vec<Value>, StreamError>,
        Option<HttpReq>,
        Vec<String>,
    ) {
        let sent = RefCell::new(None);
        let notices = RefCell::new(Vec::new());
        let result = stream(
            call,
            warned,
            |req| {
                *sent.borrow_mut() = Some(req.clone());
                Ok(HttpResp {
                    status,
                    headers: Default::default(),
                    body: body.into(),
                })
            },
            |text| notices.borrow_mut().push(text.to_string()),
        );
        (result, sent.into_inner(), notices.into_inner())
    }

    #[test]
    fn decision_call_body_is_sent_verbatim() {
        let warned = AtomicBool::new(false);
        let (result, sent, notices) = run(&call("plugin:jev", user(DECISION)), &warned, 200, REPLY);
        let sent = sent.expect("one request");
        assert_eq!(sent.method, "POST");
        assert_eq!(sent.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(sent.body.as_deref(), Some(DECISION), "byte for byte");
        assert!(
            !sent
                .headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("authorization")),
            "the host injects the key"
        );
        assert!(notices.is_empty(), "a decision call is not a misroute");
        let events = result.expect("events");
        assert_eq!(
            events[0],
            json!({ "type": "message_start", "model": "jev-1.13.0" })
        );
        assert_eq!(events[1], json!({ "type": "text_delta", "text": REPLY }));
        assert_eq!(events[2]["type"], "stop");
        assert_eq!(
            events[3]["usage"]["input_tokens"], 41,
            "usage passes through"
        );
        assert_eq!(events[3]["usage"]["output_tokens"], 3);
        assert_eq!(events[3]["usage"]["estimated"], false);
    }

    #[test]
    fn host_bound_base_url_overrides_the_manifest_one() {
        let warned = AtomicBool::new(false);
        let mut overridden = call("plugin:jev", user(DECISION));
        overridden.base_url = Some("http://127.0.0.1:9".into());
        let (_, sent, _) = run(&overridden, &warned, 200, REPLY);
        assert_eq!(
            sent.expect("one request").url,
            "http://127.0.0.1:9/v1/systemone"
        );
    }

    #[test]
    fn decision_call_with_another_shape_is_refused_unsent() {
        let warned = AtomicBool::new(false);
        let two = json!([
            { "role": "user", "content": [{ "type": "text", "text": DECISION }] },
            { "role": "user", "content": [{ "type": "text", "text": DECISION }] },
        ]);
        for messages in [two, user("not json")] {
            let (result, sent, _) = run(&call("plugin:jev", messages), &warned, 200, REPLY);
            assert!(matches!(result, Err(StreamError(AbiError::Failed { .. }))));
            assert!(sent.is_none(), "nothing is sent for a malformed call");
        }
    }

    #[test]
    fn tier_request_uses_lossy_mapping_and_warns() {
        let warned = AtomicBool::new(false);
        let messages = json!([
            { "role": "user", "content": [{ "type": "text", "text": "fix the test" }] },
            { "role": "assistant", "content": [
                { "type": "thinking", "text": "hidden", "signature": null },
                { "type": "tool_use", "id": "c1", "name": "bash", "input": {} },
            ] },
            { "role": "user", "content": [
                { "type": "tool_result", "call_id": "c1", "content": "1 failed", "is_error": false },
            ] },
        ]);
        let tier = call("title", messages);
        let (result, sent, notices) = run(&tier, &warned, 200, REPLY);
        assert_eq!(notices, [TIER_WARNING]);
        result.expect("a misrouted tier still gets an answer");
        let body: Value =
            serde_json::from_str(&sent.expect("sent").body.expect("body")).expect("json body");
        assert_eq!(
            body["state"],
            "you are cox\n\nfix the test\n\nbash\n\n1 failed"
        );
        assert_eq!(body["model"], "jev-1.13.0");
        assert_eq!(body["questions"]["relevant"]["type"], "noul");

        let (_, _, again) = run(&tier, &warned, 200, REPLY);
        assert!(again.is_empty(), "the warning is raised once");
    }

    #[test]
    fn status_529_maps_to_transient() {
        let warned = AtomicBool::new(false);
        let (result, _, _) = run(&call("plugin:jev", user(DECISION)), &warned, 529, "busy");
        let error = result.expect_err("529 fails the call");
        assert!(matches!(error.0, AbiError::Transient { .. }), "{error}");
        let text: Value = serde_json::from_str(&error.to_string()).expect("error text is JSON");
        assert_eq!(text["kind"], "transient", "the host reads the kind");

        assert!(matches!(
            status_error(429, ""),
            Some(AbiError::Transient { .. })
        ));
        assert!(matches!(
            status_error(503, ""),
            Some(AbiError::Transient { .. })
        ));
        for status in [401, 422, 404] {
            assert!(
                matches!(status_error(status, ""), Some(AbiError::Failed { .. })),
                "{status} is not worth a retry"
            );
        }
        assert_eq!(status_error(200, ""), None);
    }

    #[test]
    fn missing_usage_is_left_to_the_host() {
        let warned = AtomicBool::new(false);
        let reply = r#"{"answers":{"risky":{"type":"noul","noul":0.1}}}"#;
        let (result, _, _) = run(&call("plugin:jev", user(DECISION)), &warned, 200, reply);
        let events = result.expect("events");
        assert!(events.iter().all(|e| e["type"] != "usage"), "{events:?}");
    }

    #[test]
    fn transport_refusal_keeps_its_abi_kind() {
        let warned = AtomicBool::new(false);
        let result = stream(
            &call("plugin:jev", user(DECISION)),
            &warned,
            |_| Err(SdkError::Host(AbiError::Timeout)),
            |_| {},
        );
        assert_eq!(result, Err(StreamError(AbiError::Timeout)));
    }

    #[test]
    fn base_url_matches_the_manifest() {
        let manifest = include_str!("../plugin.toml");
        assert!(manifest.contains(&format!("base_url = \"{BASE_URL}\"")));
    }
}
