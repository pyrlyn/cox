// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.6: the client namespaces and gates a server's tools over an
//! in-process duplex, survives a server that dies mid-session, and
//! discovers servers from config, `.mcp.json` and `~/.claude.json`.
//! T47.2: a server's `elicitation/create` reaches the person through the
//! asker, and is declined when nobody can answer.
//! T55.1 (A123, P55 option (a)): a tool's MCP App UI resource is ignored —
//! the handshake declares no `io.modelcontextprotocol/ui` extension, a
//! `ui://` resource is never fetched, and a tool with
//! `_meta.ui.resourceUri` yields the same `ToolOutput` as its UI-less twin.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cox_mcp::client::{McpClient, connect_all};
use cox_mcp::discovery::discover;
use cox_mcp::elicit::{Ask, Asker};
use cox_mcp::server::{CxTemplate, Gate, ToolServer};
use cox_protocol::config::McpServerConfig;
use cox_protocol::errors::{StoreError, ToolError};
use cox_protocol::ids::{ArchiveId, CallId, SessionId};
use cox_protocol::traits::{Archive, ArchivePut, Tool, ToolCx};
use cox_protocol::types::{
    Concurrency, LinuxBackend, Risk, SandboxMode, SandboxPolicy, ToolCall, ToolOutput, ToolSpec,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientResult,
    ContentBlock, ElicitRequest, ElicitRequestParams, ListToolsResult, MetaObject,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ResourceContents, ServerCapabilities, ServerConfig, ServerRequest,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Echo;

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: json!({"type":"object","properties":{"text":{"type":"string"}}}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input["text"].as_str().unwrap_or_default().to_string()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: format!("echo: {}", self.subject(&input)),
            is_error: false,
            diff: None,
            structured: None,
        })
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

struct Open;

impl Gate for Open {
    fn check(&self, _call: &ToolCall) -> Result<(), String> {
        Ok(())
    }
}

fn cx() -> ToolCx {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    ToolCx {
        roots: vec![PathBuf::from("/tmp")],
        writable_roots: vec![PathBuf::from("/tmp")],
        cwd: PathBuf::from("/tmp"),
        sandbox: SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            network: false,
            writable: Vec::new(),
            readonly_in_workspace: Vec::new(),
            linux_backend: LinuxBackend::Auto,
        },
        archive: Arc::new(NoArchive),
        cancel: CancellationToken::new(),
        output: tx,
        session: SessionId::new(),
        call: CallId::new(),
        agent: None,
        preset: None,
        relay: None,
    }
}

/// A `ToolServer` on one end of a duplex; returns the client end and the
/// server task so a test can kill the server.
async fn serve() -> (McpClient, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_io);
    let server = ToolServer::new(
        vec![Arc::new(Echo)],
        Arc::new(Open),
        CxTemplate {
            roots: vec![PathBuf::from("/tmp")],
            writable_roots: vec![PathBuf::from("/tmp")],
            cwd: PathBuf::from("/tmp"),
            sandbox: cx().sandbox,
            archive: Arc::new(NoArchive),
            session: SessionId::new(),
        },
    );
    let task = tokio::spawn(async move {
        use rmcp::ServiceExt;
        let running = server.serve((sr, sw)).await.expect("server handshake");
        let _ = running.waiting().await;
    });
    let (cr, cw) = tokio::io::split(client_io);
    let client = McpClient::from_transport("t", (cr, cw), Duration::from_secs(2), None)
        .await
        .expect("client handshake");
    (client, task)
}

#[tokio::test]
async fn client_round_trips_a_namespaced_call_over_a_duplex() {
    let (client, task) = serve().await;
    let tools = client.tools(true).await.expect("list");
    assert_eq!(tools.len(), 1);
    let spec = tools[0].spec();
    assert_eq!(spec.name, "mcp__t__echo");
    assert!(spec.deferred);
    // No annotations from the server: the default risk is `Write`.
    assert_eq!(spec.risk, Risk::Write);
    assert_eq!(spec.input_schema["type"], "object");
    assert_eq!(tools[0].subject(&json!({})), "mcp__t__echo");
    let out = tools[0]
        .call(json!({ "text": "hi" }), &cx())
        .await
        .expect("call");
    assert!(!out.is_error);
    assert_eq!(out.text, "echo: hi");
    client.close().await;
    task.abort();
}

#[tokio::test]
async fn client_server_crash_does_not_end_session() {
    let (client, task) = serve().await;
    let tools = client.tools(true).await.expect("list");
    task.abort();
    let _ = task.await;
    // The dead server is an error the model reads, not a panic or a hang.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        tools[0].call(json!({ "text": "hi" }), &cx()),
    )
    .await
    .expect("no hang");
    match result {
        Ok(out) => assert!(out.is_error, "{}", out.text),
        Err(e) => assert!(matches!(e, ToolError::Timeout | ToolError::Io), "{e:?}"),
    }
    // A server that never starts is a notice, not an error.
    let mut servers = HashMap::new();
    servers.insert(
        "ghost".to_string(),
        McpServerConfig {
            command: Some("/definitely/not/a/server".into()),
            ..McpServerConfig::default()
        },
    );
    let (clients, tools, notices) = connect_all(
        &servers,
        Duration::from_secs(2),
        true,
        &cox_mcp::client::Auth::none(),
    )
    .await;
    assert!(clients.is_empty() && tools.is_empty());
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].starts_with("mcp server `ghost` skipped:"),
        "{notices:?}"
    );
}

#[test]
fn client_discovery_prefers_config_over_mcp_json_over_claude_json() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".claude.json"),
        json!({
            "mcpServers": { "a": { "command": "claude-a" }, "c": { "type": "http", "url": "https://c/${MCP_C_PATH:-mcp}" } },
            "projects": { project.path().display().to_string(): { "mcpServers": { "d": { "command": "claude-d" } } } }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        json!({ "mcpServers": { "a": { "command": "json-a", "args": ["--token", "${COX_TEST_TOKEN}"] }, "b": { "command": "json-b", "env": { "K": "v" } } } })
            .to_string(),
    )
    .unwrap();
    let mut config = HashMap::new();
    config.insert(
        "b".to_string(),
        McpServerConfig {
            command: Some("config-b".into()),
            ..McpServerConfig::default()
        },
    );
    // SAFETY: this test is the only one in the binary touching this variable.
    unsafe { std::env::set_var("COX_TEST_TOKEN", "sekrit") };
    let found = discover(&config, Some(project.path()), Some(home.path()));
    assert!(found.notices.is_empty(), "{:?}", found.notices);
    assert_eq!(found.servers["a"].command.as_deref(), Some("json-a"));
    assert_eq!(found.servers["a"].args, ["--token", "sekrit"]);
    assert_eq!(found.servers["b"].command.as_deref(), Some("config-b"));
    assert!(found.servers["b"].env.is_empty());
    assert_eq!(found.servers["c"].url.as_deref(), Some("https://c/mcp"));
    assert_eq!(found.servers["d"].command.as_deref(), Some("claude-d"));
    assert_eq!(found.sources["a"], ".mcp.json");
    assert_eq!(found.sources["b"], "config");
    assert_eq!(found.sources["c"], "~/.claude.json");

    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join(".mcp.json"), "{").unwrap();
    let found = discover(&HashMap::new(), Some(broken.path()), None);
    assert!(found.servers.is_empty());
    assert_eq!(found.notices.len(), 1);
}

/// An MCP server whose one tool, `ask`, sends `params` as an
/// `elicitation/create` and returns the client's answer as JSON text. It
/// records the capabilities the client declared at the handshake.
#[derive(Clone)]
struct Eliciting {
    params: ElicitRequestParams,
    caps: Arc<Mutex<Option<ClientCapabilities>>>,
}

impl ServerHandler for Eliciting {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![
            rmcp::model::Tool::new("ask", "asks the person", serde_json::Map::new()),
        ]))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Ok(mut caps) = self.caps.lock() {
            *caps = context.peer.peer_info().map(|i| i.capabilities.clone());
        }
        let sent = context
            .peer
            .send_request(ServerRequest::ElicitRequest(ElicitRequest::new(
                self.params.clone(),
            )))
            .await;
        let text = match sent {
            Ok(ClientResult::ElicitResult(result)) => {
                serde_json::to_string(&result).expect("elicit result json")
            }
            other => format!("unexpected: {other:?}"),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

/// A two-field form; `from_str` keeps `name` before `age`.
fn form() -> ElicitRequestParams {
    serde_json::from_str(
        r#"{"mode":"form","message":"Sign up","requestedSchema":{"type":"object",
            "properties":{"name":{"type":"string"},"age":{"type":"integer","minimum":0}},
            "required":["name"]}}"#,
    )
    .expect("form params")
}

/// The person at the surface: answers each question in turn after
/// `delay`; `None` dismisses it (Esc). Returns the questions it saw.
fn person(
    answers: Vec<Option<&'static str>>,
    delay: Duration,
) -> (Asker, tokio::task::JoinHandle<Vec<String>>) {
    let (asker, mut asks) = mpsc::channel::<Ask>(4);
    let task = tokio::spawn(async move {
        let mut seen = Vec::new();
        for answer in answers {
            let Some(ask) = asks.recv().await else { break };
            assert_eq!(ask.server, "e");
            seen.push(ask.question);
            tokio::time::sleep(delay).await;
            if let Some(text) = answer {
                let _ = ask.reply.send(text.to_string());
            }
        }
        seen
    });
    (asker, task)
}

/// Calls `ask` on an [`Eliciting`] server; returns the answer the server
/// got and the capabilities it saw.
async fn elicit(
    params: ElicitRequestParams,
    ask: Option<Asker>,
    timeout: Duration,
) -> (Result<Value, ToolError>, Option<ClientCapabilities>) {
    let caps = Arc::new(Mutex::new(None));
    let server = Eliciting {
        params,
        caps: caps.clone(),
    };
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        use rmcp::ServiceExt;
        let running = server
            .serve(tokio::io::split(server_io))
            .await
            .expect("server handshake");
        let _ = running.waiting().await;
    });
    let client = McpClient::from_transport("e", tokio::io::split(client_io), timeout, ask)
        .await
        .expect("client handshake");
    let tools = client.tools(false).await.expect("list");
    let out = tools[0].call(json!({}), &cx()).await.map(|out| {
        assert!(!out.is_error, "{}", out.text);
        serde_json::from_str(&out.text).expect("answer json")
    });
    client.close().await;
    task.abort();
    let caps = caps.lock().expect("caps").clone();
    (out, caps)
}

#[tokio::test]
async fn form_elicitation_round_trips_through_the_asker() {
    let (asker, seen) = person(vec![Some("Ada"), Some("36"), Some("send")], Duration::ZERO);
    let (answer, caps) = elicit(form(), Some(asker), Duration::from_secs(5)).await;
    assert_eq!(
        answer.expect("call"),
        json!({ "action": "accept", "content": { "name": "Ada", "age": 36 } })
    );
    let caps = caps.expect("client capabilities");
    let elicitation = caps.elicitation.expect("elicitation");
    assert!(elicitation.form.is_some());
    assert!(elicitation.url.is_some());
    let seen = seen.await.expect("person");
    assert_eq!(seen.len(), 3);
    assert!(seen[0].starts_with("Sign up — name"), "{seen:?}");
    // The review names the fields, never the answers (A78).
    assert_eq!(seen[2], "Sign up — send name, age?");
}

#[tokio::test]
async fn review_decline_answers_decline() {
    let (asker, _seen) = person(vec![Some("Ada"), Some(""), Some("decline")], Duration::ZERO);
    let (answer, _) = elicit(form(), Some(asker), Duration::from_secs(5)).await;
    assert_eq!(answer.expect("call"), json!({ "action": "decline" }));
}

#[tokio::test]
async fn dismissed_question_answers_cancel() {
    let (asker, _seen) = person(vec![None], Duration::ZERO);
    let (answer, _) = elicit(form(), Some(asker), Duration::from_secs(5)).await;
    assert_eq!(answer.expect("call"), json!({ "action": "cancel" }));
}

#[tokio::test]
async fn no_asker_declares_no_elicitation_capability() {
    let (answer, caps) = elicit(form(), None, Duration::from_secs(5)).await;
    assert!(caps.expect("client capabilities").elicitation.is_none());
    assert_eq!(answer.expect("call"), json!({ "action": "decline" }));
}

/// A78: the call's 200 ms deadline stops while the person takes 500 ms
/// per question.
#[tokio::test]
async fn waiting_for_a_person_does_not_time_out_the_call() {
    let (asker, _seen) = person(
        vec![Some("Ada"), Some("7"), Some("send")],
        Duration::from_millis(500),
    );
    let (answer, _) = elicit(form(), Some(asker), Duration::from_millis(200)).await;
    assert_eq!(
        answer.expect("no timeout while asking"),
        json!({ "action": "accept", "content": { "name": "Ada", "age": 7 } })
    );
}

/// A URL elicitation as a server sends it.
fn url(target: &str) -> ElicitRequestParams {
    serde_json::from_value(json!({
        "mode": "url",
        "message": "Link your account",
        "url": target,
        "elicitationId": "e1",
    }))
    .expect("url params")
}

#[tokio::test]
async fn url_elicitation_shows_the_url_and_declines_on_decline() {
    // Only `decline` is answered here: `open` would launch a real browser.
    let (asker, seen) = person(vec![Some("decline")], Duration::ZERO);
    let (answer, _) = elicit(
        url("https://example.com/cb"),
        Some(asker),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(answer.expect("call"), json!({ "action": "decline" }));
    let seen = seen.await.expect("person");
    assert_eq!(
        seen,
        ["Link your account · open https://example.com/cb · host: example.com"]
    );
}

#[tokio::test]
async fn file_scheme_elicitation_is_declined_unasked() {
    let (asker, seen) = person(vec![], Duration::ZERO);
    let (answer, _) = elicit(
        url("file:///etc/passwd"),
        Some(asker),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(answer.expect("call"), json!({ "action": "decline" }));
    assert!(seen.await.expect("person").is_empty());
}

/// T55.1: a server with two tools, `app` (carrying `_meta.ui.resourceUri`,
/// per the MCP Apps extension) and `plain` (the same tool without it), both
/// returning identical text and structured content. Advertises the
/// `resources` capability so a client that wanted the UI resource could
/// fetch it; records whether it ever did, and the capabilities the client
/// declared at the handshake.
#[derive(Clone, Default)]
struct UiServer {
    caps: Arc<Mutex<Option<ClientCapabilities>>>,
    resource_reads: Arc<AtomicUsize>,
}

impl ServerHandler for UiServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if let Ok(mut caps) = self.caps.lock() {
            *caps = context.peer.peer_info().map(|i| i.capabilities.clone());
        }
        let ui_meta = MetaObject(
            json!({ "ui": { "resourceUri": "ui://widget" } })
                .as_object()
                .expect("object")
                .clone(),
        );
        Ok(ListToolsResult::with_all_items(vec![
            rmcp::model::Tool::new("app", "renders a widget", serde_json::Map::new())
                .with_meta(ui_meta),
            rmcp::model::Tool::new("plain", "renders a widget", serde_json::Map::new()),
        ]))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let mut result = CallToolResult::success(vec![ContentBlock::text("42")]);
        result.structured_content = Some(json!({ "value": 42 }));
        Ok(result.into())
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.resource_reads.fetch_add(1, Ordering::SeqCst);
        Ok(
            ReadResourceResult::new(vec![ResourceContents::text("<html></html>", "ui://widget")])
                .into(),
        )
    }
}

/// A [`UiServer`] on one end of a duplex; returns the client, the server
/// task, the resource-read counter and the capabilities the handshake
/// recorded.
async fn ui_server() -> (
    McpClient,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
    Arc<Mutex<Option<ClientCapabilities>>>,
) {
    let server = UiServer::default();
    let resource_reads = server.resource_reads.clone();
    let caps = server.caps.clone();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        use rmcp::ServiceExt;
        let running = server
            .serve(tokio::io::split(server_io))
            .await
            .expect("server handshake");
        let _ = running.waiting().await;
    });
    let client = McpClient::from_transport(
        "ui",
        tokio::io::split(client_io),
        Duration::from_secs(2),
        None,
    )
    .await
    .expect("client handshake");
    (client, task, resource_reads, caps)
}

#[tokio::test]
async fn mcp_app_tool_output_matches_the_same_tool_without_ui() {
    let (client, task, _reads, _caps) = ui_server().await;
    let tools = client.tools(false).await.expect("list");
    let app = tools
        .iter()
        .find(|t| t.spec().name == "mcp__ui__app")
        .expect("app tool");
    let plain = tools
        .iter()
        .find(|t| t.spec().name == "mcp__ui__plain")
        .expect("plain tool");
    let app_out = app.call(json!({}), &cx()).await.expect("app call");
    let plain_out = plain.call(json!({}), &cx()).await.expect("plain call");
    assert!(!app_out.is_error, "{}", app_out.text);
    assert_eq!(app_out, plain_out);
    assert_eq!(app_out.structured, Some(json!({ "value": 42 })));
    client.close().await;
    task.abort();
}

#[tokio::test]
async fn mcp_client_declares_no_ui_extension() {
    let (client, task, _reads, caps) = ui_server().await;
    let _ = client.tools(false).await.expect("list");
    let caps = caps
        .lock()
        .expect("caps")
        .clone()
        .expect("client capabilities");
    assert!(caps.extensions.is_none(), "{:?}", caps.extensions);
    client.close().await;
    task.abort();
}

#[tokio::test]
async fn mcp_client_never_reads_a_ui_resource() {
    let (client, task, reads, _caps) = ui_server().await;
    let tools = client.tools(false).await.expect("list");
    for tool in &tools {
        let out = tool.call(json!({}), &cx()).await.expect("call");
        assert!(!out.is_error, "{}", out.text);
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    client.close().await;
    task.abort();
}
