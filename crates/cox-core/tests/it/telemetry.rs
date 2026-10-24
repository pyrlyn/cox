// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! OpenTelemetry contract tests for the session → turn → provider/tool trace.

use crate::common;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tracing_subscriber::layer::SubscriberExt as _;

async fn captured_spans() -> Vec<opentelemetry_sdk::trace::SpanData> {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("cox-core-test")));
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let (_, _, session) = common::run_with("one_tool", cox_protocol::Config::default()).await;
    drop(session);
    provider.force_flush().expect("flush telemetry");
    exporter.get_finished_spans().expect("finished spans")
}

fn attr<'a>(
    span: &'a opentelemetry_sdk::trace::SpanData,
    key: &str,
) -> Option<&'a opentelemetry::Value> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| &attribute.value)
}

/// Runs one `#[ignore]`d child test alone in a fresh copy of this test binary.
///
/// `tracing` caches per-callsite interest in process-global state. When other
/// tests in the same binary hit the same callsites concurrently (with no
/// subscriber, or with their own scoped one), that cache can be rebuilt while
/// this test's scoped subscriber is active, and spans such as
/// `invoke_agent cox.turn` are silently dropped. A child process that runs a
/// single test on a single thread has no such neighbours, so the capture is
/// deterministic.
fn run_isolated_child(name: &str, capture_content: bool) {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test binary"));
    command.args([
        "--exact",
        name,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    if capture_content {
        command.env("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true");
    } else {
        command.env_remove("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT");
    }
    let output = command.output().expect("run isolated child test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated child test `{name}` failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // `--exact` with a wrong path matches nothing and still exits 0.
    assert!(
        stdout.contains("1 passed"),
        "isolated child test `{name}` did not run:\n{stdout}"
    );
}

#[test]
fn telemetry_correlates_agent_provider_and_tool_without_content_by_default() {
    run_isolated_child("telemetry::telemetry_default_child", false);
    run_isolated_child("telemetry::telemetry_content_capture_child", true);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run in an isolated process by telemetry_correlates_agent_provider_and_tool_without_content_by_default"]
async fn telemetry_default_child() {
    let spans = captured_spans().await;

    let session = spans
        .iter()
        .find(|span| span.name == "invoke_agent cox")
        .expect("session span");
    let turn = spans
        .iter()
        .find(|span| span.name == "invoke_agent cox.turn")
        .expect("turn span");
    let provider = spans
        .iter()
        .find(|span| span.name == "chat")
        .expect("provider span");
    let tool = spans
        .iter()
        .find(|span| span.name == "execute_tool")
        .expect("tool span");

    assert_eq!(turn.parent_span_id, session.span_context.span_id());
    assert_eq!(provider.parent_span_id, turn.span_context.span_id());
    assert_eq!(tool.parent_span_id, turn.span_context.span_id());
    // A finish reason is a stable identifier, never Rust's `Debug` spelling
    // of the enum (which would export as `Some(EndTurn)`).
    assert_eq!(
        attr(provider, "gen_ai.response.finish_reasons").map(ToString::to_string),
        Some("end_turn".to_string())
    );
    assert_eq!(
        attr(turn, "cox.turn.stop_reason").map(ToString::to_string),
        Some("end_turn".to_string())
    );
    assert!(attr(provider, "gen_ai.usage.input_tokens").is_some());
    assert!(attr(provider, "cox.cost.usd").is_some());
    assert!(attr(tool, "cox.tool.output_bytes").is_some());
    assert!(attr(provider, "gen_ai.input.messages").is_none());
    assert!(attr(provider, "gen_ai.output.messages").is_none());
    assert!(attr(tool, "gen_ai.tool.call.arguments").is_none());
    assert!(attr(tool, "gen_ai.tool.call.result").is_none());
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "run in an isolated process with the opt-in variable by telemetry_correlates_agent_provider_and_tool_without_content_by_default"]
async fn telemetry_content_capture_child() {
    let spans = captured_spans().await;
    let provider = spans
        .iter()
        .find(|span| span.name == "chat")
        .expect("provider span");
    let tool = spans
        .iter()
        .find(|span| span.name == "execute_tool")
        .expect("tool span");
    assert!(attr(provider, "gen_ai.input.messages").is_some());
    assert!(attr(provider, "gen_ai.output.messages").is_some());
    assert!(attr(tool, "gen_ai.tool.call.arguments").is_some());
    assert!(attr(tool, "gen_ai.tool.call.result").is_some());
}
