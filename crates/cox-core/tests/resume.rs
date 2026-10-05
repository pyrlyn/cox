// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Resume rebuilds the same `Request` a live session would assemble (T2.4).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cox_core::router::strip_thinking;
use cox_core::{History, MemoryStore, Session, assemble};
use cox_protocol::errors::{ProviderError, ToolError};
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{Provider, Store, Tool, ToolCx};
use cox_protocol::types::{
    Attachment, Caps, Concurrency, Content, Event, Message, PermissionMode, ProviderEvent,
    ProviderId, Request, Risk, Submission, ToolOutput, ToolSpec, Usage,
};
use cox_provider::scripted::Scripted;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Echo;

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "echo input text".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: self.subject(&input),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

fn scenario() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios/one_tool.toml");
    std::fs::read_to_string(&path).expect("one_tool.toml")
}

/// A hand-built signed stream (T39.2): `Scripted`'s events with a
/// `ToolUseSignature` after every `ToolUseStart`, as the Chat wire emits one
/// for Gemini (T39.1). Kept here so the scenario format needs no new key.
struct Signed(Scripted);

#[async_trait]
impl Provider for Signed {
    fn id(&self) -> ProviderId {
        self.0.id()
    }
    fn capabilities(&self) -> Caps {
        self.0.capabilities()
    }
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        // One scripted call is a handful of events, so the buffer never fills.
        let (tx, mut rx) = mpsc::channel(64);
        let usage = self.0.stream(req, tx, cancel).await?;
        let mut n = 0;
        while let Some(ev) = rx.recv().await {
            let started = matches!(ev, ProviderEvent::ToolUseStart { .. });
            sink.send(ev).await.map_err(|_| ProviderError::Cancelled)?;
            if started {
                n += 1;
                let signature = format!("sig-{n}");
                let ev = ProviderEvent::ToolUseSignature { signature };
                sink.send(ev).await.map_err(|_| ProviderError::Cancelled)?;
            }
        }
        Ok(usage)
    }
    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        self.0.count_tokens(req).await
    }
}

/// `Scripted` on a wire that takes images, so a user attachment or a tool
/// image reaches history as `Content::Image` (T40.2, T40.5); keeps every
/// request it is sent.
struct Seeing(Scripted, std::sync::Mutex<Vec<Request>>);

fn seeing(toml: &str) -> Arc<Seeing> {
    let scripted = Scripted::from_toml(toml, "").expect("scenario");
    Arc::new(Seeing(scripted, std::sync::Mutex::default()))
}

#[async_trait]
impl Provider for Seeing {
    fn id(&self) -> ProviderId {
        self.0.id()
    }
    fn capabilities(&self) -> Caps {
        self.0.capabilities()
    }
    fn accepts_images(&self, _model: &str) -> bool {
        true
    }
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        self.1
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        self.0.stream(req, sink, cancel).await
    }
    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        self.0.count_tokens(req).await
    }
}

fn scripted() -> Scripted {
    Scripted::from_toml(&scenario(), "").expect("scenario")
}

/// T39.2: the live history keeps the signed block directly before its
/// `ToolUse`, and a model switch still strips it.
#[tokio::test]
async fn signed_tool_call_keeps_signature_before_its_tool_use() {
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Echo)];
    let cwd = PathBuf::from("/tmp/cox-turn");
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![cwd.clone()];
    let provider = Arc::new(Signed(scripted()));
    let session =
        Session::new(config, provider, tools, store.clone(), store, cwd).expect("session");
    let turn = Submission::UserTurn {
        text: "one_tool".into(),
        attachments: vec![],
        confirm_think: false,
    };
    session.submit(turn).await.expect("submit");
    let live = session.history().await;
    let asst = &live[1].content;
    assert!(matches!(&asst[0], Content::Text { .. }), "{asst:?}");
    assert_eq!(
        asst[1],
        Content::Thinking {
            text: String::new(),
            signature: Some("sig-1".into()),
        }
    );
    assert!(matches!(&asst[2], Content::ToolUse { .. }), "{asst:?}");
    let stripped = strip_thinking(&live);
    assert!(
        stripped
            .iter()
            .flat_map(|m| &m.content)
            .all(|c| !matches!(c, Content::Thinking { .. }))
    );
}

/// T40.2: a user image comes back from the rollout as the same block,
/// ahead of the text, and never inside the system blocks (§1.9).
#[tokio::test]
async fn resume_builds_identical_request() {
    let shot = Attachment {
        name: "shot.png".into(),
        media_type: "image/png".into(),
        data_b64: "iVBORw0KGgo=".into(),
    };
    let (live, req) = resume_matches_live(seeing(&scenario()), vec![shot.clone()]).await;
    assert_eq!(
        live[0].content[0],
        Content::Image {
            media_type: "image/png".into(),
            data_b64: shot.data_b64.clone(),
        }
    );
    assert!(matches!(&live[0].content[1], Content::Text { text } if text == "one_tool"));
    assert!(req.system.iter().all(|b| !b.text.contains(&shot.data_b64)));
}

/// T39.2 (§1.15 invariant 6): a signed tool round rebuilds the same way.
#[tokio::test]
async fn resume_builds_identical_request_with_signature() {
    let (live, _) = resume_matches_live(Arc::new(Signed(scripted())), vec![]).await;
    let signed = live
        .iter()
        .flat_map(|m| &m.content)
        .any(|c| matches!(c, Content::Thinking { signature: Some(sig), .. } if sig == "sig-1"));
    assert!(signed, "no signed block to rebuild: {live:?}");
}

/// Returns a PNG under `structured["image"]`, as `read` does (T40.4).
struct Shot;

#[async_trait]
impl Tool for Shot {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shot".into(),
            description: "a png".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        String::new()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        Ok(ToolOutput {
            text: "image/png, 16 B".into(),
            is_error: false,
            diff: None,
            structured: Some(cox_protocol::image::to_structured("image/png", png)),
        })
    }
}

fn user_turn(text: &str) -> Submission {
    Submission::UserTurn {
        text: text.into(),
        attachments: vec![],
        confirm_think: false,
    }
}

fn has_image(messages: &[Message]) -> bool {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|c| matches!(c, Content::Image { .. }))
}

/// T40.6 (invariant 6): turn 1 reads an image, turn 2 is sent. The live
/// session drops the image from turn 2's request while its history keeps
/// it; the rollout never stored it, so a session resumed after turn 1
/// sends the very same turn-2 request.
#[tokio::test]
async fn resume_builds_identical_request_after_a_tool_image() {
    let cwd = PathBuf::from("/tmp/cox-turn");
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![cwd.clone()];
    // No title job: it would take one of the scripted replies.
    config.session.auto_title = false;
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Shot)];
    let store = Arc::new(MemoryStore::new());
    let live = seeing(
        "[[turn]]\ntext = \"looking\"\ntool_calls = [{ name = \"shot\", input = {} }]\n\n\
         [[turn]]\ntext = \"seen\"\n\n[[turn]]\ntext = \"again\"\n",
    );
    let session = Session::new(
        config.clone(),
        live.clone(),
        tools.clone(),
        store.clone(),
        store.clone(),
        cwd.clone(),
    )
    .expect("session");
    session.submit(user_turn("first")).await.expect("turn 1");
    let id = session.id();
    let history = History::from_events(&store.rollout_read(&id).expect("rollout"));
    assert!(
        !has_image(&history.messages),
        "the rollout stores no tool image"
    );
    session.submit(user_turn("second")).await.expect("turn 2");
    assert!(
        has_image(&session.history().await),
        "history is append-only"
    );

    let resumed_provider = seeing("[[turn]]\ntext = \"again\"\n");
    let other = Arc::new(MemoryStore::new());
    let resumed = Session::resume(
        config,
        resumed_provider.clone(),
        tools,
        other.clone(),
        other,
        cwd,
        id,
        history,
    )
    .expect("resume");
    resumed
        .submit(user_turn("second"))
        .await
        .expect("resumed turn 2");

    let live_seen = live.1.lock().unwrap_or_else(|e| e.into_inner());
    let resumed_seen = resumed_provider.1.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(live_seen.len(), 3);
    assert!(has_image(&live_seen[1].messages), "turn 1 sees its image");
    assert!(!has_image(&live_seen[2].messages), "turn 2 does not");
    assert_eq!(resumed_seen.last(), live_seen.last());
}

/// Runs `one_tool`, rebuilds history from the rollout, and asserts it and
/// the assembled request match the live ones; returns the live history and
/// request.
async fn resume_matches_live(
    provider: Arc<dyn Provider>,
    attachments: Vec<Attachment>,
) -> (Vec<Message>, Request) {
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Echo)];
    let cwd = PathBuf::from("/tmp/cox-turn");
    let session = Session::new(
        config.clone(),
        provider,
        tools.clone(),
        store.clone(),
        store.clone(),
        cwd.clone(),
    )
    .expect("session");
    session
        .submit(Submission::UserTurn {
            text: "one_tool".into(),
            attachments,
            confirm_think: false,
        })
        .await
        .expect("submit");
    let live = session.history().await;
    let events = store.rollout_read(&SessionId::new()).expect("rollout");
    assert!(
        events.len() >= 15,
        "expected a full tool turn, got {}",
        events.len()
    );
    assert!(matches!(events.last(), Some(Event::TurnDone { .. })));

    let rebuilt = History::from_events(&events);
    assert_eq!(rebuilt.messages, live);

    let live_req = assemble(&live, &config, &tools, &cwd, "");
    let resume_req = assemble(&rebuilt.messages, &config, &tools, &cwd, "");
    assert_eq!(live_req, resume_req);
    (live, live_req)
}

#[tokio::test]
async fn resume_session_reuses_id_and_history() {
    let toml = scenario();
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider = Arc::new(Scripted::from_toml(&toml, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Echo)];
    let cwd = PathBuf::from("/tmp/cox-turn");
    let session = Session::new(
        config.clone(),
        provider.clone(),
        tools.clone(),
        store.clone(),
        store.clone(),
        cwd.clone(),
    )
    .expect("session");
    session
        .submit(Submission::UserTurn {
            text: "one_tool".into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("submit");
    let id = session.id();
    let live = session.history().await;
    let events = store.rollout_read(&id).expect("rollout");
    let history = History::from_events(&events);
    let expected_grants = history.grants.clone();

    let resumed = Session::resume(
        config.clone(),
        provider,
        tools.clone(),
        store.clone(),
        store.clone(),
        cwd.clone(),
        id,
        history,
    )
    .expect("resume");

    assert_eq!(resumed.id(), id);
    assert_eq!(resumed.history().await, live);

    let roundtrip = History::from_events(&store.rollout_read(&id).expect("rollout"));
    assert_eq!(roundtrip.grants, expected_grants);

    let resumed_history = resumed.history().await;
    let live_req = assemble(&live, &config, &tools, &cwd, "");
    let resume_req = assemble(&resumed_history, &config, &tools, &cwd, "");
    assert_eq!(live_req, resume_req);
}

/// T50.2: a mode change is written to the rollout, so resume rebuilds the
/// last recorded mode instead of always coming back in `Default`.
#[tokio::test]
async fn resume_restores_last_recorded_permission_mode() {
    let provider = Arc::new(Scripted::from_toml(&scenario(), "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Echo)];
    let cwd = PathBuf::from("/tmp/cox-turn");
    let config = cox_protocol::Config::default();
    let session =
        Session::new(config, provider, tools, store.clone(), store.clone(), cwd).expect("session");
    for mode in [PermissionMode::Plan, PermissionMode::Auto] {
        let sub = Submission::SetPermissionMode { mode };
        session.submit(sub).await.expect("set mode");
    }
    let events = store.rollout_read(&session.id()).expect("rollout");
    let history = History::from_events(&events);
    assert_eq!(history.permission_mode, Some(PermissionMode::Auto));
}

/// T50.4: runs one turn of `turn` in a session configured `configured`, then
/// resumes it under `resumed_config` (after `edit` rewrites its rollout) and
/// runs `ask_then_deny`, whose `touch` is a write. Returns the resumed
/// turn's events, stopping at the first `ApprovalRequired` so a session
/// that asks does not hang waiting for an answer.
async fn resume_then_write(
    configured: PermissionMode,
    resumed_config: cox_protocol::Config,
    edit: impl Fn(Vec<Event>) -> Vec<Event>,
) -> Vec<Event> {
    let mut config = cox_protocol::Config::default();
    config.permissions.mode = configured;
    let (session, store, mut rx) = common::open(&common::scenario("text_only"), config);
    common::spawn_turn(&session, "hi")
        .await
        .expect("join")
        .expect("turn");
    common::drain(&mut rx).await;
    let events = edit(store.rollout_read(&session.id()).expect("rollout"));
    let mut resumed_config = resumed_config;
    resumed_config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider =
        Arc::new(Scripted::from_toml(&common::scenario("ask_then_deny"), "").expect("scenario"));
    let resumed = Session::resume(
        resumed_config,
        provider,
        common::tools(),
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-turn"),
        session.id(),
        History::from_events(&events),
    )
    .expect("resume");
    let mut rx = resumed.events().expect("events");
    let _running = common::spawn_turn(&resumed, "write");
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("stream closed");
        let stop = matches!(ev, Event::TurnDone { .. } | Event::ApprovalRequired { .. });
        out.push(ev);
        if stop {
            return out;
        }
    }
}

fn denied_as_plan(events: &[Event]) -> bool {
    let asked = events
        .iter()
        .any(|e| matches!(e, Event::ApprovalRequired { .. }));
    let results = common::tool_results(events);
    !asked && results.len() == 1 && !results[0].0 && results[0].1.contains("plan mode")
}

/// T50.4: a session configured `plan` that never switched records its
/// starting mode, so it resumes in Plan even under a `Default` config.
#[tokio::test]
async fn resumed_plan_session_denies_a_write_as_plan_does() {
    let events = resume_then_write(
        PermissionMode::Plan,
        cox_protocol::Config::default(),
        |events| events,
    )
    .await;
    assert!(
        denied_as_plan(&events),
        "resumed wider than Plan: {events:?}"
    );
}

/// T50.4: a rollout written before any mode record still loads, and
/// resumes in the configured mode rather than `Default`.
#[tokio::test]
async fn resume_without_a_mode_record_uses_the_configured_mode() {
    let mut plan = cox_protocol::Config::default();
    plan.permissions.mode = PermissionMode::Plan;
    let events = resume_then_write(PermissionMode::Default, plan, |events| {
        events
            .into_iter()
            .filter(|e| !matches!(e, Event::StateChanged { .. }))
            .collect()
    })
    .await;
    assert!(
        denied_as_plan(&events),
        "old rollout ignored the configured mode: {events:?}"
    );
}
