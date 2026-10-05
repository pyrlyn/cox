// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The ACP server over cox sessions (T11.1): `initialize`, `authenticate`,
//! `session/new`, `session/load`, `session/prompt` and `session/cancel`
//! handlers over one `Event` stream per session. Request handlers never
//! block the dispatch loop: `session/prompt` spawns its turn driver and
//! answers late through the moved `Responder`, so `session/cancel` and
//! permission replies keep flowing mid-turn. `drive_prompt`'s loop also
//! gives a background task's lifecycle and any delivered follow-up
//! (`Event::TaskCreated`/`TaskCompleted`/`TaskMessage`, T34.8) their own
//! `session/update` notifications instead of dropping them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, ClientCapabilities, ContentBlock, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, SessionId, SessionNotification,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use cox_protocol::ids::TaskId;
use cox_protocol::image;
use cox_protocol::types::{Attachment, Decision, Event, Submission, ToolCall, Why};
use tokio::sync::{broadcast, mpsc};

use crate::client_tools::ClientLink;
use crate::map::{self, CallTable};

/// Builds live cox sessions for ACP sessions. Implemented in the binary
/// (which owns config, providers, tools and the store); the server only
/// drives the returned `Session` through `Submission`s and `Event`s.
/// Async because session assembly (`cox_session::open`, T37.2) starts MCP
/// servers and plugins on the server's own runtime.
#[async_trait::async_trait]
pub trait SessionFactory: Send + Sync + 'static {
    /// Creates the cox session for an ACP `session/new`.
    async fn create(&self, req: FactoryRequest) -> anyhow::Result<cox_core::Session>;
}

/// What the factory needs: where the session lives, which client tools to
/// use, and the link those tools call back through.
pub struct FactoryRequest {
    /// Session working directory from `session/new`.
    pub cwd: PathBuf,
    /// Additional workspace roots from the client.
    pub roots: Vec<PathBuf>,
    /// The client offers `fs/read_text_file` + `fs/write_text_file`.
    pub client_fs: bool,
    /// The client offers `terminal/*`.
    pub client_terminal: bool,
    /// Client link for this ACP session's proxy tools.
    pub link: ClientLink,
}

/// One live ACP session: the cox session, its event broadcast for prompt
/// drivers, and a mutex serialising prompts.
#[derive(Clone)]
struct LiveSession {
    cox: cox_core::Session,
    bcast: broadcast::Sender<Event>,
    prompt_lock: Arc<tokio::sync::Mutex<()>>,
    cwd: PathBuf,
}

/// Shared server state behind every handler.
#[derive(Clone)]
pub struct ServerState {
    factory: Arc<dyn SessionFactory>,
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
    client_caps: Arc<Mutex<ClientCapabilities>>,
}

impl ServerState {
    /// New server state over `factory`.
    pub fn new(factory: Arc<dyn SessionFactory>) -> Self {
        Self {
            factory,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            client_caps: Arc::new(Mutex::new(ClientCapabilities::default())),
        }
    }

    fn caps(&self) -> ClientCapabilities {
        self.client_caps
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }
}

/// Runs the ACP server on stdio until the client goes away.
pub async fn serve_stdio(factory: Arc<dyn SessionFactory>) -> anyhow::Result<()> {
    serve_transport(factory, agent_client_protocol::Stdio::new()).await
}

/// Runs the ACP server over an in-process channel pair (tests).
pub async fn serve_channel(
    factory: Arc<dyn SessionFactory>,
    channel: agent_client_protocol::Channel,
) -> anyhow::Result<()> {
    serve_transport(factory, channel).await
}

async fn serve_transport(
    factory: Arc<dyn SessionFactory>,
    transport: impl agent_client_protocol::ConnectTo<Agent>,
) -> anyhow::Result<()> {
    let state = ServerState::new(factory);
    let s = state.clone();
    Agent
        .builder()
        .name("cox")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _conn| {
                handle_initialize(&s, req, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let s = state.clone();
                async move |req: AuthenticateRequest, responder, _conn| {
                    handle_authenticate(&s, req, responder)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let s = state.clone();
                async move |req: NewSessionRequest, responder, conn| {
                    handle_new_session(&s, req, responder, conn).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let s = state.clone();
                async move |req: LoadSessionRequest, responder, _conn| {
                    handle_load_session(&s, req, responder)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let s = state.clone();
                async move |req: PromptRequest, responder, conn| {
                    handle_prompt(&s, req, responder, conn).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let s = state.clone();
                async move |notif: agent_client_protocol::schema::v1::CancelNotification, _conn| {
                    handle_cancel(&s, notif)
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await
        .map_err(|e| anyhow::anyhow!("acp transport closed: {e}"))
}

fn handle_initialize(
    state: &ServerState,
    req: InitializeRequest,
    responder: agent_client_protocol::Responder<InitializeResponse>,
) -> Result<(), agent_client_protocol::Error> {
    if let Ok(mut caps) = state.client_caps.lock() {
        *caps = req.client_capabilities.clone();
    }
    // v1-only server: this build has no V2 surface, so answer V1.
    let version = ProtocolVersion::V1;
    let mut caps = agent_client_protocol::schema::v1::AgentCapabilities::new();
    caps.load_session = true;
    // T40.8: an image block in a prompt becomes an attachment.
    caps.prompt_capabilities.image = true;
    responder.respond(InitializeResponse::new(version).agent_capabilities(caps))
}

fn handle_authenticate(
    _state: &ServerState,
    _req: AuthenticateRequest,
    responder: agent_client_protocol::Responder<AuthenticateResponse>,
) -> Result<(), agent_client_protocol::Error> {
    // No authentication: cox trusts the local client (same policy as the
    // TUI and headless surfaces on this machine).
    responder.respond(AuthenticateResponse::new())
}

async fn handle_new_session(
    state: &ServerState,
    req: NewSessionRequest,
    responder: agent_client_protocol::Responder<NewSessionResponse>,
    conn: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let acp_id = SessionId::new(cox_protocol::SessionId::new().to_string());
    let caps = state.caps();
    let link = ClientLink {
        conn: conn.clone(),
        session: acp_id.clone(),
        cwd: req.cwd.clone(),
    };
    let factory_req = FactoryRequest {
        cwd: req.cwd.clone(),
        roots: req.additional_directories.clone(),
        client_fs: caps.fs.read_text_file && caps.fs.write_text_file,
        client_terminal: caps.terminal,
        link: link.clone(),
    };
    let cox = match state.factory.create(factory_req).await {
        Ok(session) => session,
        Err(_) => {
            return responder.respond_with_error(agent_client_protocol::Error::internal_error());
        }
    };
    let rx = cox
        .events()
        .ok_or_else(agent_client_protocol::Error::internal_error)?;
    let (bcast, _) = broadcast::channel(256);
    let live = LiveSession {
        cox: cox.clone(),
        bcast: bcast.clone(),
        prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
        cwd: req.cwd.clone(),
    };
    state
        .sessions
        .lock()
        .map_err(|_| agent_client_protocol::Error::internal_error())?
        .insert(acp_id.to_string(), live);
    tokio::spawn(forward_events(rx, bcast, link, req.cwd));
    responder.respond(NewSessionResponse::new(acp_id))
}

fn handle_load_session(
    state: &ServerState,
    req: LoadSessionRequest,
    responder: agent_client_protocol::Responder<LoadSessionResponse>,
) -> Result<(), agent_client_protocol::Error> {
    // Resume within the server's lifetime: the live session keeps its full
    // history. A restart drops sessions (core has no rehydration API), which
    // is an explicit error rather than an empty session.
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| agent_client_protocol::Error::internal_error())?;
    match sessions.get(&req.session_id.to_string()) {
        Some(live) if live.cwd == req.cwd => responder.respond(LoadSessionResponse::new()),
        Some(_) => responder.respond_with_error(agent_client_protocol::Error::internal_error()),
        None => responder.respond_with_error(agent_client_protocol::Error::internal_error()),
    }
}

/// Forwards one session's events as `session/update` notifications and onto
/// the prompt drivers' broadcast.
async fn forward_events(
    mut rx: mpsc::Receiver<Event>,
    bcast: broadcast::Sender<Event>,
    link: ClientLink,
    cwd: PathBuf,
) {
    let mut calls = CallTable::new();
    while let Some(ev) = rx.recv().await {
        let _ = bcast.send(ev.clone());
        for update in map::updates_for(&mut calls, &ev, &cwd) {
            let notif = SessionNotification::new(link.session.clone(), update);
            if link.conn.send_notification(notif).is_err() {
                return;
            }
        }
    }
}

async fn handle_prompt(
    state: &ServerState,
    req: PromptRequest,
    responder: agent_client_protocol::Responder<PromptResponse>,
    conn: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let live = {
        let sessions = state
            .sessions
            .lock()
            .map_err(|_| agent_client_protocol::Error::internal_error())?;
        let Some(live) = sessions.get(&req.session_id.to_string()).cloned() else {
            return responder.respond_with_error(agent_client_protocol::Error::internal_error());
        };
        live
    };
    // The turn runs outside the dispatch loop so `session/cancel` and
    // permission replies keep flowing; the moved responder answers late.
    let link_session = req.session_id.clone();
    let s = state.clone();
    let conn2 = conn.clone();
    conn.spawn(async move {
        let response = drive_prompt(&s, &live, &link_session, conn2, req).await;
        responder.respond_with_result(response)
    })
    .map_err(|_| agent_client_protocol::Error::internal_error())?;
    Ok(())
}

/// Prompt text and image attachments out of ACP content blocks (T40.8).
/// An image that fails `image::validate` becomes a text note saying why,
/// not a failed prompt; one given only by `uri` would need fetching and
/// stays unsupported, as does every other non-text block.
fn prompt_parts(prompt: &[ContentBlock]) -> (String, Vec<Attachment>) {
    let mut lines = Vec::new();
    let mut attachments = Vec::new();
    for block in prompt {
        match block {
            ContentBlock::Text(text) => lines.push(text.text.clone()),
            ContentBlock::Image(img) if img.data.is_empty() => {
                lines.push("[unsupported content block: image by uri]".to_string());
            }
            ContentBlock::Image(img) => {
                let name = img
                    .uri
                    .clone()
                    .unwrap_or_else(|| format!("image {}", attachments.len() + 1));
                let attachment = Attachment {
                    name,
                    media_type: img.mime_type.clone(),
                    data_b64: img.data.clone(),
                };
                match image::validate(&attachment) {
                    Ok(()) => attachments.push(attachment),
                    Err(e) => lines.push(format!("[image {:?} dropped: {e}]", attachment.name)),
                }
            }
            _ => lines.push("[unsupported content block]".to_string()),
        }
    }
    (lines.join("\n"), attachments)
}

/// Submits the turn and drives it to `TurnDone`, answering approvals.
async fn drive_prompt(
    _state: &ServerState,
    live: &LiveSession,
    acp_id: &SessionId,
    conn: ConnectionTo<Client>,
    req: PromptRequest,
) -> Result<PromptResponse, agent_client_protocol::Error> {
    let _guard = live.prompt_lock.lock().await;
    let mut bcast = live.bcast.subscribe();
    let (text, attachments) = prompt_parts(&req.prompt);
    // Labels of tasks created during this prompt, so a `TaskMessage` from a
    // sibling can be shown by name instead of its bare `TaskId` (SM§6).
    let mut task_labels: HashMap<TaskId, String> = HashMap::new();
    let turn = tokio::spawn({
        let live = live.clone();
        async move {
            live.cox
                .submit(Submission::UserTurn {
                    text,
                    attachments,
                    confirm_think: false,
                })
                .await
        }
    });
    let stop = loop {
        match bcast.recv().await {
            Ok(Event::TurnDone { stop, .. }) => break stop,
            Ok(Event::ApprovalRequired { call, why, source }) => {
                let agent = source.and_then(|s| s.agent);
                let decision = ask_permission(&conn, acp_id, &call, &why, agent).await;
                let _ = live
                    .cox
                    .submit(Submission::Approve {
                        call_id: call.id,
                        decision,
                    })
                    .await;
            }
            // A background task (subagent or detached `bash`) started: shown
            // as a tool call so the client's task list picks it up, labelled
            // like `ask_permission` labels a relayed approval (T34.8).
            Ok(Event::TaskCreated { task, label, .. }) => {
                let update = task_created_update(&mut task_labels, task, &label);
                let _ = conn.send_notification(SessionNotification::new(acp_id.clone(), update));
            }
            // The matching completion: same `ToolCallId`, moved to `Completed`.
            Ok(Event::TaskCompleted {
                task,
                cost_usd,
                exit_code,
                ..
            }) => {
                let update = task_completed_update(task, cost_usd, exit_code);
                let _ = conn.send_notification(SessionNotification::new(acp_id.clone(), update));
            }
            // A delivered/relayed follow-up (SM§3): rendered as an agent
            // message chunk, prefixed with the sender's label the same way
            // `ask_permission` prefixes a relayed approval's title; `from:
            // None` (the parent/user) gets no prefix, matching its fallback.
            Ok(Event::TaskMessage { from, text, .. }) => {
                let update = task_message_update(&task_labels, from, &text);
                let _ = conn.send_notification(SessionNotification::new(acp_id.clone(), update));
            }
            Ok(_) => {}
            Err(_) => {
                let _ = turn.await;
                return Err(agent_client_protocol::Error::internal_error());
            }
        }
    };
    let _ = turn.await;
    if let Some(detail) = map::stop_detail(&stop) {
        let _ = conn.send_notification(SessionNotification::new(
            acp_id.clone(),
            agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(
                agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                    agent_client_protocol::schema::v1::TextContent::new(detail),
                )),
            ),
        ));
    }
    Ok(PromptResponse::new(map::map_stop(&stop)))
}

/// `Event::TaskCreated` → a `ToolCall` update (T34.8): the client's task/tool
/// list picks up the background task the same way it does a normal tool
/// call. Remembers the sanitized label so a later `TaskMessage` from this
/// task can be shown by name.
fn task_created_update(
    task_labels: &mut HashMap<TaskId, String>,
    task: TaskId,
    label: &str,
) -> agent_client_protocol::schema::v1::SessionUpdate {
    let title = cox_sanitize::sanitize(label);
    task_labels.insert(task, title.clone());
    let call = agent_client_protocol::schema::v1::ToolCall::new(
        agent_client_protocol::schema::v1::ToolCallId::new(task.to_string()),
        format!("task: {title}"),
    )
    .kind(agent_client_protocol::schema::v1::ToolKind::Other)
    .status(agent_client_protocol::schema::v1::ToolCallStatus::InProgress);
    agent_client_protocol::schema::v1::SessionUpdate::ToolCall(call)
}

/// `Event::TaskCompleted` → the matching `ToolCallUpdate`, same `ToolCallId`
/// as `task_created_update` built, moved to `Completed` (T34.8).
fn task_completed_update(
    task: TaskId,
    cost_usd: f64,
    exit_code: Option<i32>,
) -> agent_client_protocol::schema::v1::SessionUpdate {
    let summary = match exit_code {
        Some(code) => format!("finished (${cost_usd:.4}), exit {code}"),
        None => format!("finished (${cost_usd:.4})"),
    };
    let fields = agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
        .status(Some(
            agent_client_protocol::schema::v1::ToolCallStatus::Completed,
        ))
        .content(Some(vec![
            agent_client_protocol::schema::v1::ToolCallContent::from(ContentBlock::Text(
                agent_client_protocol::schema::v1::TextContent::new(summary),
            )),
        ]));
    let update = agent_client_protocol::schema::v1::ToolCallUpdate::new(
        agent_client_protocol::schema::v1::ToolCallId::new(task.to_string()),
        fields,
    );
    agent_client_protocol::schema::v1::SessionUpdate::ToolCallUpdate(update)
}

/// `Event::TaskMessage` → an agent-message chunk (T34.8), prefixed with the
/// sender's label the same way `ask_permission` prefixes a relayed
/// approval's title; `from: None` (the parent/user) gets no prefix, matching
/// its fallback. An unknown sender (a task this connection never saw
/// `TaskCreated` for) falls back to its bare `TaskId`.
fn task_message_update(
    task_labels: &HashMap<TaskId, String>,
    from: Option<TaskId>,
    text: &str,
) -> agent_client_protocol::schema::v1::SessionUpdate {
    let text = cox_sanitize::sanitize(text);
    let rendered = match from {
        None => text,
        Some(f) => {
            let label = task_labels
                .get(&f)
                .cloned()
                .unwrap_or_else(|| f.to_string());
            format!("{label}: {text}")
        }
    };
    let chunk = agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
        agent_client_protocol::schema::v1::TextContent::new(rendered),
    ));
    agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk)
}

/// `ApprovalRequired` → `session/request_permission` with allow,
/// allow-always and reject options; a subagent's prompt (T27.2) names it
/// in the title, the one field every client shows.
async fn ask_permission(
    conn: &ConnectionTo<Client>,
    acp_id: &SessionId,
    call: &ToolCall,
    why: &Why,
    agent: Option<String>,
) -> Decision {
    let _ = why;
    let title = match agent {
        Some(agent) => format!("{agent} asks: {} {}", call.name, call.subject),
        None => format!("{} {}", call.name, call.subject),
    };
    let fields = agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
        .title(Some(title))
        .status(Some(
            agent_client_protocol::schema::v1::ToolCallStatus::Pending,
        ));
    let update = agent_client_protocol::schema::v1::ToolCallUpdate::new(
        agent_client_protocol::schema::v1::ToolCallId::new(call.id.to_string()),
        fields,
    );
    let options = vec![
        PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            "allow-always",
            "Allow always",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ];
    let req = RequestPermissionRequest::new(acp_id.clone(), update, options);
    let outcome = conn.send_request(req).block_task().await.map(|r| r.outcome);
    match outcome {
        Ok(RequestPermissionOutcome::Selected(selected)) => match selected.option_id.0.as_ref() {
            "allow" => Decision::Allow,
            "allow-always" => Decision::AllowForSession,
            _ => Decision::Deny {
                reason: "rejected by client".to_string(),
            },
        },
        _ => Decision::Deny {
            reason: "permission request failed".to_string(),
        },
    }
}

fn handle_cancel(
    state: &ServerState,
    notif: agent_client_protocol::schema::v1::CancelNotification,
) -> Result<(), agent_client_protocol::Error> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| agent_client_protocol::Error::internal_error())?;
    if let Some(live) = sessions.get(&notif.session_id.to_string()) {
        live.cox.interrupt();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T34.8 Check: `TaskCreated` becomes a `ToolCall` labelled with the
    /// task's own sanitized label, and the matching `TaskCompleted` moves
    /// the same `ToolCallId` to `Completed`. Drives `drive_prompt`'s two
    /// arms directly through the pure functions they delegate to (sending
    /// the built `SessionUpdate` over a live `ConnectionTo<Client>` is
    /// already exercised by `tests/conformance.rs`).
    #[test]
    fn acp_reports_task_created_and_completed() {
        let mut labels = HashMap::new();
        let task = TaskId::new();
        let created = task_created_update(&mut labels, task, "explore\u{1b}[31m: find x");
        let json = serde_json::to_value(&created).expect("serialize");
        assert_eq!(json["sessionUpdate"], "tool_call");
        assert_eq!(json["toolCallId"], task.to_string());
        assert_eq!(json["status"], "in_progress");
        let title = json["title"].as_str().expect("title");
        assert!(title.starts_with("task: explore"), "{title}");
        assert!(
            !title.contains('\u{1b}'),
            "escape sequence not stripped: {title}"
        );
        assert_eq!(
            labels.get(&task).map(String::as_str),
            Some("explore: find x")
        );

        let completed = task_completed_update(task, 0.0042, Some(1));
        let json = serde_json::to_value(&completed).expect("serialize");
        assert_eq!(json["sessionUpdate"], "tool_call_update");
        assert_eq!(json["toolCallId"], task.to_string());
        assert_eq!(json["status"], "completed");
        let content = json["content"][0]["content"]["text"]
            .as_str()
            .expect("content text");
        assert!(content.contains("0.0042"), "{content}");
        assert!(content.contains("exit 1"), "{content}");
    }

    /// T40.8: a valid image block becomes an attachment, an invalid one a
    /// note naming why, and a `uri`-only image or other block the old
    /// placeholder; the text blocks stay in order.
    #[test]
    fn prompt_parts_turns_image_blocks_into_attachments() {
        use agent_client_protocol::schema::v1::ImageContent;
        // `\x89PNG\r\n\x1a\n` in base64.
        let png = "iVBORw0KGgo=";
        let prompt = vec![
            ContentBlock::from("look"),
            ContentBlock::Image(ImageContent::new(png, "image/png")),
            ContentBlock::Image(ImageContent::new("aGVsbG8=", "image/png")),
            ContentBlock::Image(ImageContent::new("", "image/png").uri("https://x/y.png")),
        ];
        let (text, attachments) = prompt_parts(&prompt);
        assert_eq!(
            attachments,
            vec![Attachment {
                name: "image 1".into(),
                media_type: "image/png".into(),
                data_b64: png.into(),
            }]
        );
        assert_eq!(
            text,
            "look\n[image \"image 2\" dropped: not a PNG, JPEG, GIF or WebP image]\n\
             [unsupported content block: image by uri]"
        );
    }

    /// T34.8 Check: a delivered `TaskMessage` renders as an agent-message
    /// chunk prefixed with the sender's label, the same way `ask_permission`
    /// prefixes a relayed approval's title; the parent/user (`from: None`)
    /// gets no prefix, and a sender this connection never saw `TaskCreated`
    /// for falls back to its bare `TaskId`.
    #[test]
    fn acp_reports_a_delivered_task_message() {
        let mut labels = HashMap::new();
        let sibling = TaskId::new();
        labels.insert(sibling, "explore-2".to_string());

        let update = task_message_update(&labels, Some(sibling), "ping\u{202e}evil");
        let json = serde_json::to_value(&update).expect("serialize");
        assert_eq!(json["sessionUpdate"], "agent_message_chunk");
        let text = json["content"]["text"].as_str().expect("text");
        assert!(text.starts_with("explore-2: ping"), "{text}");
        assert!(
            !text.contains('\u{202e}'),
            "bidi override not stripped: {text}"
        );

        let update = task_message_update(&labels, None, "follow up");
        let json = serde_json::to_value(&update).expect("serialize");
        assert_eq!(json["content"]["text"], "follow up");

        let unknown = TaskId::new();
        let update = task_message_update(&labels, Some(unknown), "hi");
        let json = serde_json::to_value(&update).expect("serialize");
        assert_eq!(json["content"]["text"], format!("{unknown}: hi"));
    }
}
