// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox mcp` tool-serving contract: `tools/list` carries each tool's schema,
//! `tools/call` maps success, tool-reported errors, `ToolError`s, invalid
//! arguments and unknown tools to the right MCP shape, and concurrent calls
//! on one connection each get their own answer.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cox_mcp::server::{CxTemplate, Gate, ToolServer};
use cox_protocol::errors::{StoreError, ToolError};
use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::traits::{Archive, ArchivePut, Tool, ToolCx};
use cox_protocol::types::{
    Concurrency, LinuxBackend, Risk, SandboxMode, SandboxPolicy, ToolCall, ToolOutput, ToolSpec,
};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": { "text": { "type": "string" } },
        "required": ["text"]
    })
}

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: schema(),
        deferred: false,
        risk: Risk::ReadOnly,
        concurrency: Concurrency::Parallel,
    }
}

fn output(text: String, is_error: bool) -> ToolOutput {
    ToolOutput {
        text,
        is_error,
        diff: None,
        structured: None,
    }
}

/// Echoes `text`; a missing or non-string `text` is an invalid-args result.
struct Echo;

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        spec("echo")
    }
    fn subject(&self, _input: &Value) -> String {
        "echo".into()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        match input["text"].as_str() {
            Some(text) => Ok(output(format!("echo: {text}"), false)),
            None => Ok(output("invalid arguments: `text` is required".into(), true)),
        }
    }
}

/// Fails with a typed `ToolError`.
struct Missing;

#[async_trait]
impl Tool for Missing {
    fn spec(&self) -> ToolSpec {
        spec("missing")
    }
    fn subject(&self, _input: &Value) -> String {
        "missing".into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Err(ToolError::NotFound)
    }
}

/// Sleeps, then echoes; counts how many calls overlap.
struct Slow {
    running: AtomicUsize,
    peak: AtomicUsize,
}

#[async_trait]
impl Tool for Slow {
    fn spec(&self) -> ToolSpec {
        spec("slow")
    }
    fn subject(&self, _input: &Value) -> String {
        "slow".into()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(output(
            format!("slow: {}", input["text"].as_str().unwrap_or("")),
            false,
        ))
    }
}

struct NoArchive;

#[async_trait]
impl Archive for NoArchive {
    async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
        Ok(ArchiveId::new())
    }
    async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        Ok(Vec::new())
    }
}

struct AllowAll;

impl Gate for AllowAll {
    fn check(&self, _call: &ToolCall) -> Result<(), String> {
        Ok(())
    }
}

struct Harness {
    client: RunningService<RoleClient, ()>,
    server: tokio::task::JoinHandle<()>,
    slow: Arc<Slow>,
}

impl Harness {
    async fn start() -> Self {
        let slow = Arc::new(Slow {
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Echo), Arc::new(Missing), slow.clone()];
        let server = ToolServer::new(
            tools,
            Arc::new(AllowAll),
            CxTemplate {
                roots: vec![PathBuf::from(".")],
                writable_roots: vec![PathBuf::from(".")],
                cwd: PathBuf::from("."),
                sandbox: SandboxPolicy {
                    mode: SandboxMode::ReadOnly,
                    network: false,
                    writable: Vec::new(),
                    readonly_in_workspace: Vec::new(),
                    linux_backend: LinuxBackend::Auto,
                },
                archive: Arc::new(NoArchive),
                session: SessionId::new(),
            },
        );
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (sr, sw) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            let running = server.serve((sr, sw)).await.expect("server handshake");
            running.waiting().await.expect("server task");
        });
        let (cr, cw) = tokio::io::split(client_io);
        let client = ().serve((cr, cw)).await.expect("client handshake");
        Self {
            client,
            server,
            slow,
        }
    }

    async fn stop(self) {
        self.client.cancel().await.expect("client shutdown");
        self.server.await.expect("server exits on hang-up");
    }
}

fn params(name: &'static str, arguments: Option<Value>) -> CallToolRequestParams {
    let params = CallToolRequestParams::new(name);
    match arguments.and_then(|v| v.as_object().cloned()) {
        Some(args) => params.with_arguments(args),
        None => params,
    }
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

#[tokio::test]
async fn tools_list_carries_names_descriptions_and_schemas() {
    let h = Harness::start().await;
    let mut tools = h.client.list_all_tools().await.expect("list");
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, ["echo", "missing", "slow"]);
    for tool in &tools {
        assert_eq!(
            tool.description.as_deref(),
            Some(format!("{} tool", tool.name).as_str())
        );
        assert_eq!(Value::Object((*tool.input_schema).clone()), schema());
    }
    h.stop().await;
}

#[tokio::test]
async fn tools_call_success_has_one_text_block_and_no_error_flag() {
    let h = Harness::start().await;
    let ok = h
        .client
        .call_tool(params("echo", Some(json!({ "text": "hi" }))))
        .await
        .expect("call");
    assert_eq!(ok.is_error, Some(false));
    assert_eq!(ok.content.len(), 1);
    assert_eq!(text_of(&ok), "echo: hi");
    h.stop().await;
}

#[tokio::test]
async fn unknown_tool_is_a_protocol_error_naming_the_tool() {
    let h = Harness::start().await;
    let err = h
        .client
        .call_tool(params("nope", None))
        .await
        .expect_err("unknown tool must not produce a result");
    assert!(err.to_string().contains("unknown tool `nope`"), "{err}");
    // The connection survives the error.
    let ok = h
        .client
        .call_tool(params("echo", Some(json!({ "text": "still up" }))))
        .await
        .expect("call after error");
    assert_eq!(text_of(&ok), "echo: still up");
    h.stop().await;
}

#[tokio::test]
async fn invalid_arguments_become_an_error_result() {
    let h = Harness::start().await;
    for args in [
        None,
        Some(json!({ "text": 42 })),
        Some(json!({ "other": "x" })),
    ] {
        let res = h
            .client
            .call_tool(params("echo", args.clone()))
            .await
            .expect("bad args are a tool result, not a protocol error");
        assert_eq!(res.is_error, Some(true), "{args:?}");
        assert!(text_of(&res).starts_with("invalid arguments"), "{res:?}");
    }
    h.stop().await;
}

#[tokio::test]
async fn tool_error_maps_to_error_result_with_its_message() {
    let h = Harness::start().await;
    let res = h
        .client
        .call_tool(params("missing", Some(json!({ "text": "x" }))))
        .await
        .expect("a ToolError is a result, not a protocol error");
    assert_eq!(res.is_error, Some(true));
    assert_eq!(text_of(&res), ToolError::NotFound.to_string());
    h.stop().await;
}

#[tokio::test]
async fn concurrent_calls_each_get_their_own_answer() {
    let h = Harness::start().await;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let peer = h.client.peer().clone();
        set.spawn(async move {
            let mut p = CallToolRequestParams::new("slow");
            p = p.with_arguments(
                json!({ "text": format!("n{i}") })
                    .as_object()
                    .cloned()
                    .expect("object"),
            );
            (i, peer.call_tool(p).await.expect("call"))
        });
    }
    let mut seen = Vec::new();
    while let Some(joined) = set.join_next().await {
        let (i, res) = joined.expect("task");
        assert_eq!(res.is_error, Some(false));
        assert_eq!(text_of(&res), format!("slow: n{i}"));
        seen.push(i);
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..8).collect::<Vec<_>>());
    assert!(
        h.slow.peak.load(Ordering::SeqCst) > 1,
        "calls on one connection should overlap"
    );
    h.stop().await;
}
