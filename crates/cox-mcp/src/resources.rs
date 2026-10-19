// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP resources and prompts (T67.10). One deferred read-only tool, found
//! through `tool_search`, lists `resources/list` and `prompts/list` and
//! reads `resources/read`. Server text is untrusted: the caller passes
//! `cox_sanitize::sanitize` (this crate cannot depend on it) and anything
//! over the cap is archived before the model sees a prefix.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{ArchivePut, Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use rmcp::ServiceError;
use rmcp::model::{ReadResourceRequestParams, ResourceContents};
use serde_json::{Value, json};

use super::McpClient;

/// Sanitized bytes kept in the tool result. The rest is an archive row
/// the model can `expand`. Same size as the default visible tool output.
const CAP: usize = 8192;

pub(super) fn catalog(clients: Vec<McpClient>, sanitize: fn(&str) -> String) -> Arc<dyn Tool> {
    Arc::new(McpResources { clients, sanitize })
}

struct McpResources {
    clients: Vec<McpClient>,
    sanitize: fn(&str) -> String,
}

impl McpResources {
    fn selected(&self, server: Option<&str>) -> Result<Vec<&McpClient>, ToolError> {
        match server {
            Some(name) => self
                .clients
                .iter()
                .find(|client| client.name == name)
                .map(|client| vec![client])
                .ok_or_else(|| denied(format!("no mcp server `{name}`"))),
            None => Ok(self.clients.iter().collect()),
        }
    }
}

#[async_trait]
impl Tool for McpResources {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_resources".into(),
            description: "List and read MCP resources and list MCP prompts from connected \
                servers. Omit `uri` to list resources and prompts; pass `uri` to read one \
                resource. Pass `server` when more than one MCP server is connected."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "uri": {"type": "string"}
                }
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("uri")
            .and_then(Value::as_str)
            .filter(|uri| !uri.is_empty())
            .unwrap_or("list")
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let server = str_opt(&input, "server");
        let uri = str_opt(&input, "uri");
        let selected = self.selected(server)?;
        let raw = if let Some(uri) = uri {
            if selected.len() != 1 {
                return Err(denied("name the server to read a resource"));
            }
            read_one(selected[0], uri, cx).await?
        } else {
            let mut out = String::new();
            for client in selected {
                out.push_str(&list_one(client, cx).await?);
            }
            out
        };
        bounded(cx, (self.sanitize)(&raw)).await
    }
}

fn str_opt<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn denied(why: impl Into<String>) -> ToolError {
    ToolError::Denied { why: why.into() }
}

async fn wait<T>(
    timeout: Duration,
    name: &str,
    cx: &ToolCx,
    fut: impl Future<Output = Result<T, ServiceError>>,
) -> Result<T, ToolError> {
    tokio::select! {
        _ = cx.cancel.cancelled() => Err(ToolError::Cancelled),
        result = tokio::time::timeout(timeout, fut) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(denied(format!("mcp server `{name}`: {err}"))),
            Err(_) => Err(ToolError::Timeout),
        },
    }
}

async fn list_one(client: &McpClient, cx: &ToolCx) -> Result<String, ToolError> {
    let name = client.name.clone();
    let resources = wait(
        client.timeout,
        &name,
        cx,
        client.service.list_all_resources(),
    )
    .await?;
    let prompts = wait(client.timeout, &name, cx, client.service.list_all_prompts()).await?;
    let mut out = format!("server `{}`\nresources:\n", client.name);
    for resource in resources {
        let description = resource.description.unwrap_or_default();
        out.push_str(&format!(
            "- {} {} {description}\n",
            resource.name, resource.uri
        ));
    }
    out.push_str("prompts:\n");
    for prompt in prompts {
        let description = prompt.description.unwrap_or_default();
        out.push_str(&format!("- {} {description}\n", prompt.name));
    }
    Ok(out)
}

async fn read_one(client: &McpClient, uri: &str, cx: &ToolCx) -> Result<String, ToolError> {
    let name = client.name.clone();
    let result = wait(
        client.timeout,
        &name,
        cx,
        client
            .service
            .read_resource(ReadResourceRequestParams::new(uri)),
    )
    .await?;
    let mut out = String::new();
    for contents in result.contents {
        match contents {
            ResourceContents::TextResourceContents { text, .. } => {
                out.push_str(&text);
                out.push('\n');
            }
            ResourceContents::BlobResourceContents { blob, .. } => {
                out.push_str(&blob);
                out.push('\n');
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Archive the full sanitized text first, then return a prefix that names
/// the row. Under the cap, nothing is archived.
async fn bounded(cx: &ToolCx, text: String) -> Result<ToolOutput, ToolError> {
    if text.len() <= CAP {
        return Ok(output(text));
    }
    let id = cx
        .archive
        .put(ArchivePut {
            session: cx.session,
            call: cx.call,
            tool: "mcp_resources".into(),
            subject: None,
            bytes: text.as_bytes().to_vec(),
        })
        .await
        .map_err(|_| ToolError::Io)?;
    let trailer = format!("\n[… {} bytes archived as {id}]", text.len());
    let keep = text.floor_char_boundary(CAP.saturating_sub(trailer.len()));
    Ok(output(format!("{}{trailer}", &text[..keep])))
}

fn output(text: String) -> ToolOutput {
    ToolOutput {
        text,
        is_error: false,
        diff: None,
        structured: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use cox_protocol::errors::StoreError;
    use cox_protocol::ids::{ArchiveId, CallId, SessionId};
    use cox_protocol::traits::{Archive, ArchivePut, ToolCx};
    use cox_protocol::types::{LinuxBackend, SandboxMode, SandboxPolicy};
    use rmcp::ServiceExt;
    use rmcp::model::{
        ListPromptsResult, ListResourcesResult, Prompt, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
    };
    use rmcp::service::RequestContext;
    use rmcp::{ErrorData, RoleServer, ServerHandler};
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::super::McpClient;
    use super::{CAP, catalog};

    struct Note {
        body: String,
    }

    impl ServerHandler for Note {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_resources()
                    .enable_prompts()
                    .build(),
            )
        }

        async fn list_resources(
            &self,
            _: Option<rmcp::model::PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new("mem://note", "note").with_description("one note"),
            ]))
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
            Ok(
                ReadResourceResult::new(vec![ResourceContents::text(&self.body, request.uri)])
                    .into(),
            )
        }

        async fn list_prompts(
            &self,
            _: Option<rmcp::model::PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
                "greet",
                Some("say hi"),
                None,
            )]))
        }
    }

    struct Rec {
        id: ArchiveId,
        bytes: Mutex<Option<Vec<u8>>>,
    }

    #[async_trait]
    impl Archive for Rec {
        async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
            *self.bytes.lock().expect("lock") = Some(put.bytes);
            Ok(self.id)
        }
        async fn get(&self, _: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(self.bytes.lock().expect("lock").clone().unwrap_or_default())
        }
    }

    fn cx(archive: Arc<Rec>) -> ToolCx {
        let (output, _rx) = mpsc::channel(1);
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
            archive,
            cancel: CancellationToken::new(),
            output,
            session: SessionId::new(),
            call: CallId::new(),
            agent: None,
            preset: None,
            relay: None,
        }
    }

    fn strip(text: &str) -> String {
        text.replace('\u{1b}', "")
    }

    async fn pair(body: &str) -> (McpClient, tokio::task::JoinHandle<()>) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (sr, sw) = tokio::io::split(server_io);
        let server = Note {
            body: body.to_string(),
        };
        let task = tokio::spawn(async move {
            let running = server.serve((sr, sw)).await.expect("server handshake");
            let _ = running.waiting().await;
        });
        let (cr, cw) = tokio::io::split(client_io);
        let client = McpClient::from_transport("notes", (cr, cw), Duration::from_secs(2), None)
            .await
            .expect("client handshake");
        (client, task)
    }

    #[tokio::test]
    async fn mcp_resources_lists_and_reads_one_resource_and_one_prompt() {
        let (client, task) = pair("hello\u{1b}[31m").await;
        let rec = Arc::new(Rec {
            id: ArchiveId::new(),
            bytes: Mutex::new(None),
        });
        let tool = catalog(vec![client], strip);
        let context = cx(rec);
        let listed = tool.call(json!({}), &context).await.expect("list");
        assert!(listed.text.contains("mem://note"), "{}", listed.text);
        assert!(listed.text.contains("one note"), "{}", listed.text);
        assert!(listed.text.contains("greet"), "{}", listed.text);
        assert!(listed.text.contains("say hi"), "{}", listed.text);
        assert!(tool.spec().deferred);
        assert_eq!(tool.spec().risk, cox_protocol::Risk::ReadOnly);
        let read = tool
            .call(json!({"uri": "mem://note"}), &context)
            .await
            .expect("read");
        assert_eq!(read.text, "hello[31m\n");
        assert!(!read.text.contains('\u{1b}'));
        tool_shutdown(tool, task).await;
    }

    #[tokio::test]
    async fn over_cap_resource_is_archived_before_the_model_sees_it() {
        let marker = "TAILMARKER";
        let body = format!("{}{marker}", "A".repeat(CAP));
        let (client, task) = pair(&body).await;
        let id = ArchiveId::new();
        let rec = Arc::new(Rec {
            id,
            bytes: Mutex::new(None),
        });
        let tool = catalog(vec![client], |text| text.to_string());
        let read = tool
            .call(json!({"uri": "mem://note"}), &cx(Arc::clone(&rec)))
            .await
            .expect("read");
        let archived = rec
            .bytes
            .lock()
            .expect("lock")
            .clone()
            .expect("archive row");
        let full = String::from_utf8(archived).expect("utf8");
        assert!(full.contains(marker), "full text archived first");
        assert!(
            read.text.contains(&id.to_string()),
            "prefix names the row put returned: {}",
            read.text
        );
        assert!(!read.text.contains(marker), "{}", read.text);
        tool_shutdown(tool, task).await;
    }

    async fn tool_shutdown(tool: Arc<dyn cox_protocol::Tool>, task: tokio::task::JoinHandle<()>) {
        // The catalog holds the only client clone; drop it so the server ends.
        drop(tool);
        task.abort();
    }
}
