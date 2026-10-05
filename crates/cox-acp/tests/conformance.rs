// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! ACP conformance (T11.1 step 5): the reference SDK client talks to this
//! server over an in-process channel pair - a scripted prompt completes, and
//! a permission round-trip allows the turn. In-process `Channel` transport
//! instead of the example-client subprocess (same SDK client code paths,
//! deterministic, no processes).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    InitializeRequest, PermissionOptionId, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, StopReason,
};
use agent_client_protocol::{Channel, Client, SessionMessage};
use async_trait::async_trait;
use cox_protocol::Config;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};

/// Read-only stub: returns `input.text`.
struct Echo;

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &serde_json::Value) -> String {
        input
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .into()
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _cx: &ToolCx,
    ) -> Result<ToolOutput, cox_protocol::ToolError> {
        Ok(ToolOutput {
            text: self.subject(&input),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// Builds scripted cox sessions for the server side.
struct TestFactory {
    toml: String,
    ask: Vec<String>,
}

#[async_trait]
impl cox_acp::SessionFactory for TestFactory {
    async fn create(&self, req: cox_acp::FactoryRequest) -> anyhow::Result<cox_core::Session> {
        let mut config = Config::default();
        config.core.workspace_roots = vec![req.cwd.clone()];
        config.permissions.ask = self.ask.clone();
        let provider = Arc::new(cox_provider::scripted::Scripted::from_toml(&self.toml, "")?);
        let store = Arc::new(cox_core::MemoryStore::new());
        Ok(cox_core::Session::new(
            config,
            provider,
            vec![Arc::new(Echo)],
            store.clone(),
            store,
            req.cwd,
        )?)
    }
}

fn serve(toml: &str, ask: Vec<String>, channel: Channel) -> tokio::task::JoinHandle<()> {
    let factory = Arc::new(TestFactory {
        toml: toml.into(),
        ask,
    });
    tokio::spawn(async move {
        let _ = cox_acp::serve_channel(factory, channel).await;
    })
}

/// Drives one prompt through the reference SDK client, collecting agent
/// text, the `sessionUpdate` kinds seen on the wire, and the stop reason.
async fn run_prompt(
    channel: Channel,
    cwd: PathBuf,
    prompt: &str,
    allow: Arc<AtomicBool>,
) -> Result<(String, Vec<String>, StopReason), agent_client_protocol::Error> {
    let kinds = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let kinds_handler = kinds.clone();
    Client
        .builder()
        .name("conformance")
        .on_receive_request(
            async move |_req: RequestPermissionRequest, responder, _conn| {
                allow.store(true, Ordering::SeqCst);
                kinds_handler
                    .lock()
                    .unwrap()
                    .push("request_permission".to_string());
                let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    PermissionOptionId::new("allow"),
                ));
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(channel, async |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let mut session = cx.build_session(&cwd).block_task().start_session().await?;
            session.send_prompt(prompt)?;
            let mut text = String::new();
            let stop = loop {
                match session.read_update().await? {
                    SessionMessage::SessionMessage(dispatch) => {
                        if let agent_client_protocol::Dispatch::Notification(notif) = dispatch
                            && notif.method() == "session/update"
                        {
                            let update = &notif.params()["update"];
                            if let Some(kind) = update.get("sessionUpdate").and_then(|k| k.as_str())
                            {
                                kinds.lock().unwrap().push(kind.to_string());
                            }
                            if let Some(chunk) = update
                                .get("content")
                                .and_then(|c| c.get("text"))
                                .and_then(|t| t.as_str())
                            {
                                text.push_str(chunk);
                            }
                        }
                    }
                    SessionMessage::StopReason(reason) => break reason,
                    _ => {}
                }
            };
            let kinds = kinds.lock().unwrap().clone();
            Ok::<_, agent_client_protocol::Error>((text, kinds, stop))
        })
        .await
}

#[tokio::test]
async fn acp_scripted_prompt_completes() {
    let work = tempfile::tempdir().unwrap();
    let (server_end, client_end) = Channel::duplex();
    let server = serve("[[turn]]\ntext = \"hello from acp\"\n", vec![], server_end);
    let (text, kinds, stop) = tokio::time::timeout(
        Duration::from_secs(30),
        run_prompt(
            client_end,
            work.path().to_path_buf(),
            "hi",
            Arc::new(AtomicBool::new(false)),
        ),
    )
    .await
    .expect("timeout")
    .expect("client run");
    server.abort();
    assert!(text.contains("hello from acp"), "{text}");
    assert!(
        kinds.iter().any(|k| k == "agent_message_chunk"),
        "{kinds:?}"
    );
    assert!(matches!(stop, StopReason::EndTurn), "{stop:?}");
}

#[tokio::test]
async fn acp_permission_round_trip_allows_the_turn() {
    let work = tempfile::tempdir().unwrap();
    let toml = concat!(
        "[[turn]]\ntext = \"calling\"\n",
        "tool_calls = [{ name = \"echo\", input = { text = \"hi\" } }]\n",
        "[[turn]]\ntext = \"done\"\n",
    );
    let (server_end, client_end) = Channel::duplex();
    let server = serve(toml, vec!["echo(hi)".into()], server_end);
    let allowed = Arc::new(AtomicBool::new(false));
    let (text, kinds, stop) = tokio::time::timeout(
        Duration::from_secs(30),
        run_prompt(client_end, work.path().to_path_buf(), "go", allowed.clone()),
    )
    .await
    .expect("timeout")
    .expect("client run");
    server.abort();
    assert!(
        allowed.load(Ordering::SeqCst),
        "client saw request_permission"
    );
    assert!(kinds.iter().any(|k| k == "request_permission"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "tool_call"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "tool_call_update"), "{kinds:?}");
    assert!(text.contains("done"), "{text}");
    assert!(matches!(stop, StopReason::EndTurn), "{stop:?}");
}
