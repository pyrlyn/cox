// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T39.6: `cox run -p` through the built-in `[providers.gemini]` preset
//! against a mock server that speaks Gemini's OpenAI-compatible stream: one
//! tool round and a final answer, with no network. The round-1 fixture
//! carries the tool call's thought signature at
//! `extra_content.google.thought_signature`, a field path that is
//! UNVERIFIED until T39.7 records a real response (plan.md P39).

use std::path::Path;

use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const ROUND1: &str = include_str!("fixtures/gemini/round1.sse");
const ROUND2: &str = include_str!("fixtures/gemini/round2.sse");

/// Tells the two rounds apart by what the request carries rather than by
/// arrival order: only the second one holds a `role: "tool"` result.
struct CarriesToolResult(bool);

impl Match for CarriesToolResult {
    fn matches(&self, request: &Request) -> bool {
        tool_result_in(request) == self.0
    }
}

fn tool_result_in(request: &Request) -> bool {
    serde_json::from_slice::<Value>(&request.body)
        .ok()
        .and_then(|body| body["messages"].as_array().cloned())
        .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"))
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/event-stream")
}

/// The user config: every tier a headless turn can reach goes to the
/// preset, whose `base_url` points at the mock. The model row is restated
/// so the test does not depend on the vendored context window; no title
/// job, so the mock sees exactly the two rounds.
fn write_user_config(home: &Path, base_url: &str) {
    let config = format!(
        r#"[tiers.cheap]
provider = "gemini"
model = "gemini-3.8-flash"

[tiers.code]
provider = "gemini"
model = "gemini-3.8-flash"

[providers.gemini]
base_url = "{base_url}"
models = [{{id="gemini-3.8-flash", context_window=1000000, reasoning_effort=true}}]

[session]
auto_title = false
"#
    );
    std::fs::write(home.join("config.toml"), config).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_two_round_tool_loop_replays_the_signature() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(CarriesToolResult(false))
        .respond_with(sse(ROUND1))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(CarriesToolResult(true))
        .respond_with(sse(ROUND2))
        .expect(1)
        .mount(&server)
        .await;

    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir(work.path().join("src")).unwrap();
    std::fs::write(
        work.path().join("src/main.rs"),
        "fn main() { println!(\"hello\"); }\n",
    )
    .unwrap();
    write_user_config(home.path(), &server.uri());

    let (work_dir, home_dir) = (work.path().to_path_buf(), home.path().to_path_buf());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_cox"))
            .current_dir(&work_dir)
            .env("COX_HOME", &home_dir)
            .env("HOME", &home_dir)
            .env("COX_KEYRING", "off")
            .env("GEMINI_API_KEY", "test-key")
            .env_remove("COX_PROVIDER")
            .env_remove("COX_SCENARIO")
            .args(["--cwd", work_dir.to_str().unwrap()])
            .args(["run", "-p", "what does main.rs print?"])
            .args(["--output-format", "json"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["result"], "main.rs prints hello.", "{summary}");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "one request per round");
    for request in &requests {
        let auth = request.headers.get("authorization").unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer test-key");
    }
    let second = requests.iter().find(|r| tool_result_in(r)).unwrap();
    let body: Value = serde_json::from_slice(&second.body).unwrap();
    let assistant = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    let call = &assistant["tool_calls"][0];
    assert_eq!(call["function"]["name"], "read");
    assert_eq!(
        call["extra_content"]["google"]["thought_signature"], "sig-fixture",
        "{assistant}"
    );

    let session: SessionId = summary["session"].as_str().unwrap().parse().unwrap();
    let store = cox_store::Store::open(home.path()).unwrap();
    let rows = store.usage_for_session(&session).unwrap();
    let cached: Vec<u32> = rows.iter().map(|r| r.usage.cache_read_tokens).collect();
    assert_eq!(cached, [8, 32], "{rows:?}");
}
