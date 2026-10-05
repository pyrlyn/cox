// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T6.2: an rmcp client over an in-process duplex lists the served tools and
//! calls one; the gate turns a write into an error result, never a call.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cox_mcp::server::{CxTemplate, Gate, ToolServer};
use cox_protocol::errors::{StoreError, ToolError};
use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::traits::{Archive, ArchivePut, Tool, ToolCx};
use cox_protocol::types::{
    Concurrency, LinuxBackend, Risk, SandboxMode, SandboxPolicy, ToolCall, ToolOutput, ToolSpec,
};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};

struct Echo;
struct Touch;

fn spec(name: &str, risk: Risk) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
        deferred: false,
        risk,
        concurrency: Concurrency::Parallel,
    }
}

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        spec("echo", Risk::ReadOnly)
    }
    fn subject(&self, input: &Value) -> String {
        input["text"].as_str().unwrap_or("").into()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: format!("echo: {}", input["text"].as_str().unwrap_or("")),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[async_trait]
impl Tool for Touch {
    fn spec(&self) -> ToolSpec {
        spec("touch", Risk::Write)
    }
    fn subject(&self, _input: &Value) -> String {
        "touch".into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        panic!("the gate must stop a write before it runs")
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

struct ReadOnlyGate;

impl Gate for ReadOnlyGate {
    fn check(&self, call: &ToolCall) -> Result<(), String> {
        match call.risk {
            Risk::ReadOnly => Ok(()),
            other => Err(format!("{other:?} calls are not allowed here")),
        }
    }
}

fn server() -> ToolServer {
    ToolServer::new(
        vec![Arc::new(Echo), Arc::new(Touch)],
        Arc::new(ReadOnlyGate),
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
    )
}

fn text_of(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn params(name: &'static str, arguments: Option<Value>) -> CallToolRequestParams {
    let params = CallToolRequestParams::new(name);
    match arguments.and_then(|v| v.as_object().cloned()) {
        Some(args) => params.with_arguments(args),
        None => params,
    }
}

#[tokio::test]
async fn server_lists_tools_and_runs_a_gated_call_over_a_duplex() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_io);
    let server_task = tokio::spawn(async move {
        let running = server().serve((sr, sw)).await.expect("server handshake");
        running.waiting().await.expect("server task");
    });
    let (cr, cw) = tokio::io::split(client_io);
    let client = ().serve((cr, cw)).await.expect("client handshake");

    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .expect("list")
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["echo", "touch"]);

    let ok = client
        .call_tool(params("echo", Some(json!({ "text": "hi" }))))
        .await
        .expect("echo call");
    assert_eq!(ok.is_error, Some(false));
    assert_eq!(text_of(&ok), "echo: hi");

    let denied = client
        .call_tool(params("touch", None))
        .await
        .expect("touch call returns a result, not a protocol error");
    assert_eq!(denied.is_error, Some(true));
    assert!(text_of(&denied).starts_with("denied: Write"), "{denied:?}");

    assert!(client.call_tool(params("nope", None)).await.is_err());

    client.cancel().await.expect("client shutdown");
    server_task
        .await
        .expect("server exits when the client hangs up");
}
