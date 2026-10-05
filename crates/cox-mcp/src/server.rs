// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox mcp`: built-in tools served over MCP stdio (T6.2). This module owns
//! the wire side only. Which tools are offered, how a call is gated and
//! where its output is archived come from the binary through [`Gate`] and
//! [`CxTemplate`], so the crate stays a leaf below `cox-core` (the gate the
//! binary plugs in is `cox_core::permission::Engine` with `policy = never`).

use std::sync::Arc;

use cox_protocol::ids::{CallId, SessionId};
use cox_protocol::traits::{Archive, Tool, ToolCx};
use cox_protocol::types::{SandboxPolicy, ToolCall};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool as McpTool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, ServiceExt};
use serde_json::Value;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("mcp server handshake: {0}")]
    Init(Box<rmcp::service::ServerInitializeError>),
    #[error("mcp server task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Allows or denies a call before it runs. Returns the denial reason.
pub trait Gate: Send + Sync {
    fn check(&self, call: &ToolCall) -> Result<(), String>;
}

/// Everything a [`ToolCx`] needs besides the per-call id and output channel.
pub struct CxTemplate {
    pub roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub sandbox: SandboxPolicy,
    pub archive: Arc<dyn Archive>,
    pub session: SessionId,
}

pub struct ToolServer {
    tools: Vec<Arc<dyn Tool>>,
    gate: Arc<dyn Gate>,
    cx: CxTemplate,
}

impl ToolServer {
    pub fn new(tools: Vec<Arc<dyn Tool>>, gate: Arc<dyn Gate>, cx: CxTemplate) -> Self {
        Self { tools, gate, cx }
    }

    /// Serves on the process's stdin/stdout until the client disconnects.
    pub async fn serve_stdio(self) -> Result<(), ServerError> {
        let running = self
            .serve(rmcp::transport::io::stdio())
            .await
            .map_err(|e| ServerError::Init(Box::new(e)))?;
        running.waiting().await?;
        Ok(())
    }

    async fn run_call(&self, tool: &dyn Tool, call: ToolCall) -> CallToolResult {
        if let Err(reason) = self.gate.check(&call) {
            return CallToolResult::error(vec![ContentBlock::text(format!("denied: {reason}"))]);
        }
        // Streamed output has no MCP channel; the final text carries it all.
        let (output, mut drain) = mpsc::channel::<String>(32);
        tokio::spawn(async move { while drain.recv().await.is_some() {} });
        let cx = ToolCx {
            roots: self.cx.roots.clone(),
            writable_roots: self.cx.writable_roots.clone(),
            cwd: self.cx.cwd.clone(),
            sandbox: self.cx.sandbox.clone(),
            archive: self.cx.archive.clone(),
            cancel: CancellationToken::new(),
            output,
            session: self.cx.session,
            call: call.id,
            // The MCP surface has no subagent notion (T34.3): every call
            // here is the top-level session, so also no `send_message`
            // (T34.6) — `relay: None` denies it rather than misrouting.
            agent: None,
            preset: None,
            relay: None,
        };
        match tool.call(call.input, &cx).await {
            Ok(out) if out.is_error => CallToolResult::error(vec![ContentBlock::text(out.text)]),
            Ok(out) => CallToolResult::success(vec![ContentBlock::text(out.text)]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        }
    }
}

impl ServerHandler for ToolServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("cox", env!("CARGO_PKG_VERSION")))
            .with_instructions("cox's built-in coding tools, confined to the workspace.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self
            .tools
            .iter()
            .map(|t| {
                let spec = t.spec();
                let schema = match spec.input_schema {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                };
                McpTool::new(spec.name, spec.description, schema)
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        let Some(tool) = self.tools.iter().find(|t| t.spec().name == name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool `{name}`"),
                None,
            ));
        };
        let input = Value::Object(request.arguments.unwrap_or_default());
        let call = ToolCall {
            id: CallId::new(),
            name,
            risk: tool.risk(&input),
            subject: tool.subject(&input),
            segments: tool.segments(&input),
            input,
        };
        Ok(self.run_call(tool.as_ref(), call).await.into())
    }
}
