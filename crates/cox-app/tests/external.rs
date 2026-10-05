// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A top-level session driven by an external ACP agent (T52.4-T52.6,
//! DT§3.3.1), end to end: `App::open_agent` starts `tests/fixtures/fake_acp.sh` from a
//! scratch user config's `[external_agents.fake]`, under the real sandbox
//! wrap, and `LiveSession` drives it as the macOS app does. The key comes
//! from the in-memory host, never the Keychain (A49); nextest runs each
//! test in its own process, so each sets its own environment.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_app::app::{App, AppError, Host};
use cox_app::live::LiveSession;
use cox_app::{BlockKind, InboxItem, Intent, Need, TimelinePatch};
use cox_protocol::types::{Decision, StopReason};
use cox_protocol::{CallId, Config};
use cox_session::acp_session::AcpOpenError;

const THEME: &str = "base16-ocean.dark";

/// Holds the agent's key under its name, as the app's Keychain would.
struct Keys;

impl Host for Keys {
    fn notify(&self, _: InboxItem, _: u32) {}
    fn badge(&self, _: u32) {}
    fn open_url(&self, _: &str) {}
    fn secret(&self, section: &str) -> Option<String> {
        (section == "fake").then(|| String::from("test-key"))
    }
}

fn fake_agent() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp.sh")
}

/// A scratch home whose user config names `command` as the agent `fake`,
/// and a project to run it in. `make deploy` always asks, so the fake's
/// permission request reaches the user whatever the sandbox would allow.
fn scratch(command: &str) -> tempfile::TempDir {
    scratch_with(command, "[]")
}

/// [`scratch`] with the agent started with `args` (a TOML array).
fn scratch_with(command: &str, args: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("user");
    std::fs::create_dir_all(home.join(".cox")).expect("home");
    std::fs::create_dir_all(dir.path().join("project")).expect("project");
    let config = format!(
        "[permissions]\nask = [\"Bash(make deploy:*)\"]\n\n\
         [external_agents.fake]\ncommand = {command:?}\nargs = {args}\n\
         key_env = \"COX_FAKE_ACP_KEY\"\n"
    );
    std::fs::write(home.join(".cox/config.toml"), config).expect("config");
    // SAFETY: first thing in this test's own process (nextest).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("COX_HOME", home.join(".cox"));
        std::env::remove_var("COX_FAKE_ACP_KEY");
    }
    dir
}

/// False where this host has no argv sandbox backend: the wrap refuses
/// every agent there, which T35.2's own test covers.
fn wraps(dir: &Path) -> bool {
    let roots = [dir.join("project")];
    cox_session::agent_argv(&fake_agent(), &[], &Config::default(), &roots).is_ok()
}

fn app(dir: &Path) -> Arc<App> {
    App::new(Some(dir.join("user/.cox")), Arc::new(Keys)).expect("app")
}

async fn open(dir: &Path) -> Result<Arc<LiveSession>, AppError> {
    open_in(&app(dir), dir).await
}

async fn open_in(app: &Arc<App>, dir: &Path) -> Result<Arc<LiveSession>, AppError> {
    app.open_agent(dir.join("project"), "fake", THEME.into())
        .await
}

fn send(text: &str) -> Intent {
    Intent::Send {
        text: text.into(),
        attachments: vec![],
        confirm_think: false,
    }
}

/// Pulls until the running turn ends; its model chip and stop reason.
async fn turn_end(live: &LiveSession) -> (String, StopReason) {
    while let Some(batch) = live.next_patches().await {
        for patch in batch {
            if let TimelinePatch::Upsert { block, .. } = patch
                && let BlockKind::TurnMeta {
                    model,
                    stop: Some(stop),
                    ..
                } = block.kind
            {
                return (model.to_string(), stop);
            }
        }
    }
    panic!("the stream closed before the turn ended");
}

/// Pulls until an approval block that is still open; its call.
async fn asked(live: &LiveSession) -> CallId {
    while let Some(batch) = live.next_patches().await {
        for patch in batch {
            if let TimelinePatch::Upsert { block, .. } = patch
                && let BlockKind::Approval {
                    call,
                    decision: None,
                    ..
                } = block.kind
            {
                return call;
            }
        }
    }
    panic!("the stream closed before the agent asked");
}

fn texts(live: &LiveSession) -> Vec<String> {
    live.snapshot()
        .into_iter()
        .filter_map(|b| match b.kind {
            BlockKind::User { text, .. } | BlockKind::Assistant { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// T52.4 Check: a prompt reaches the agent over `session/prompt`, with the
/// key from the host, and its message streams into the same timeline a cox
/// session uses, under the "<agent> · ACP" chip.
#[tokio::test]
async fn external_session_streams_into_the_timeline() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let live = open(dir.path()).await.expect("the agent starts");
    assert_eq!(live.agent(), Some("fake"));
    live.send(send("hi")).await.expect("sent");
    let (model, stop) = turn_end(&live).await;
    assert_eq!(model, "fake · ACP");
    assert_eq!(stop, StopReason::EndTurn);
    assert_eq!(texts(&live), ["hi", "hello from the fake agent with a key"]);
    live.end();
}

/// T52.4 Check: `Interrupt` sends `session/cancel`; the fake holds the
/// prompt until it arrives, so the turn ends `Interrupted` only if it did.
#[tokio::test]
async fn external_session_interrupt_sends_cancel() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let live = open(dir.path()).await.expect("the agent starts");
    live.send(send("wait")).await.expect("sent");
    live.send(Intent::Interrupt).await.expect("cancelled");
    let (_, stop) = turn_end(&live).await;
    assert_eq!(stop, StopReason::Interrupted);
    live.end();
}

/// T52.4 Check: an intent that needs cox's own history is refused, naming
/// the agent, and the session goes on.
#[tokio::test]
async fn external_session_refuses_rewind() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let live = open(dir.path()).await.expect("the agent starts");
    let rewind = Intent::Rewind {
        to_turn: 1,
        code: true,
        conversation: true,
    };
    let refused = live.send(rewind).await.err();
    assert!(
        matches!(&refused, Some(AppError::Unsupported { agent, intent: "Rewind" }) if agent == "fake"),
        "{refused:?}"
    );
    live.send(send("hi")).await.expect("still open");
    assert_eq!(turn_end(&live).await.1, StopReason::EndTurn);
    live.end();
}

/// T52.4 Check (EA§7): a program on no `PATH` directory is one warning,
/// naming it, and no session opens.
#[tokio::test]
async fn missing_agent_program_is_one_warning() {
    let dir = scratch("cox-no-such-acp-agent");
    if !wraps(dir.path()) {
        return;
    }
    let err = open(dir.path()).await.err();
    let Some(AppError::Agent(AcpOpenError::Unavailable(warning))) = &err else {
        panic!("{err:?}");
    };
    assert!(
        warning.contains("cox-no-such-acp-agent") && warning.contains("not on PATH"),
        "{warning}"
    );
}

/// T52.5 Check: the agent's `session/request_permission`, which the engine
/// escalates, is an inbox item for this session with the agent as its
/// source, and an approval block in its timeline.
#[tokio::test]
async fn external_ask_becomes_an_inbox_item() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let app = app(dir.path());
    let live = open_in(&app, dir.path()).await.expect("the agent starts");
    live.send(send("ask")).await.expect("sent");
    let call = asked(&live).await;
    let inbox = app.inbox();
    let item = inbox
        .iter()
        .find(|i| matches!(&i.need, Need::Approval { call: c, .. } if c.id == call))
        .expect("the ask is in the inbox");
    assert_eq!(item.session, live.id());
    assert!(!item.expired);
    let source = item.source.as_ref().expect("a source");
    assert_eq!(source.agent.as_deref(), Some("fake"));
    assert_eq!(source.session, live.id());
    live.end();
}

/// T52.5 Check: the user's answer goes back to the agent as the option it
/// stands for (allow once → `once`), the agent's turn goes on, and the
/// inbox item is gone.
#[tokio::test]
async fn external_answer_reaches_the_agent() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let app = app(dir.path());
    let live = open_in(&app, dir.path()).await.expect("the agent starts");
    live.send(send("ask")).await.expect("sent");
    let call = asked(&live).await;
    let approve = Intent::Approve {
        call,
        decision: Decision::Allow,
    };
    live.send(approve).await.expect("answered");
    assert_eq!(turn_end(&live).await.1, StopReason::EndTurn);
    assert!(
        texts(&live).iter().any(|t| t == "answered once"),
        "{:?}",
        texts(&live)
    );
    let open = app
        .inbox()
        .into_iter()
        .any(|i| matches!(&i.need, Need::Approval { call: c, .. } if c.id == call));
    assert!(!open, "a decided ask leaves the inbox");
    live.end();
}

/// Runs one turn, "hi", in a new session and ends it; the session's id.
async fn one_turn(app: &Arc<App>, dir: &Path) -> cox_protocol::SessionId {
    let live = open_in(app, dir).await.expect("the agent starts");
    live.send(send("hi")).await.expect("sent");
    assert_eq!(turn_end(&live).await.1, StopReason::EndTurn);
    let id = live.id();
    live.end();
    id
}

/// T52.6 Check: an agent without `loadSession` reopens its stored session
/// read-only: the rollout fills the timeline, and a prompt is refused with
/// the way on named.
#[tokio::test]
async fn external_session_reopens_read_only_without_load_session() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let app = app(dir.path());
    let id = one_turn(&app, dir.path()).await;
    let reopened = app
        .open(dir.path().join("project"), Some(id), THEME.into())
        .await
        .expect("reopens");
    assert_eq!(reopened.id(), id);
    assert_eq!(reopened.agent(), Some("fake"));
    let why = reopened.read_only().expect("read-only");
    assert!(why.contains("session/load"), "{why}");
    assert_eq!(
        texts(&reopened),
        ["hi", "hello from the fake agent with a key"]
    );
    let refused = reopened.send(send("again")).await.err();
    assert!(
        matches!(&refused, Some(AppError::ReadOnly { agent, .. }) if agent == "fake"),
        "{refused:?}"
    );
}

/// T52.6 Check: an agent with `loadSession` gets its own session id back
/// (the fake fails any other), what it replays is not shown twice, and the
/// session goes on where it stopped.
#[tokio::test]
async fn external_session_resumes_with_load_session() {
    let dir = scratch_with(&fake_agent().display().to_string(), "[\"load\"]");
    if !wraps(dir.path()) {
        return;
    }
    let app = app(dir.path());
    let id = one_turn(&app, dir.path()).await;
    let reopened = app
        .open(dir.path().join("project"), Some(id), THEME.into())
        .await
        .expect("reopens");
    assert_eq!(reopened.id(), id);
    assert_eq!(reopened.read_only(), None);
    reopened.send(send("hi")).await.expect("sent");
    assert_eq!(turn_end(&reopened).await.1, StopReason::EndTurn);
    let greeting = "hello from the fake agent with a key";
    assert_eq!(texts(&reopened), ["hi", greeting, "hi", greeting]);
    reopened.end();
}

/// T52.7 Check: the picker lists cox first, then each configured agent
/// with why it cannot start: a program on no `PATH` directory is named.
#[tokio::test]
async fn agents_list_cox_first_and_why_an_agent_cannot_start() {
    let dir = scratch("cox-no-such-acp-agent");
    if !wraps(dir.path()) {
        return;
    }
    let agents = app(dir.path())
        .agents(&dir.path().join("project"))
        .expect("agents");
    let names: Vec<Option<&str>> = agents.iter().map(|a| a.name.as_deref()).collect();
    assert_eq!(names, [None, Some("fake")]);
    assert_eq!(agents[0].unavailable, None);
    let why = agents[1].unavailable.as_deref().unwrap_or_default();
    assert!(why.contains("not on PATH"), "{why}");
    assert!(
        agents[1].launch.contains("COX_FAKE_ACP_KEY"),
        "{}",
        agents[1].launch
    );
}

/// T52.7 Check: an agent whose program runs and whose key the host holds
/// can start, and a session opened for it lists with its name.
#[tokio::test]
async fn an_available_agent_session_lists_with_its_agent() {
    let dir = scratch(&fake_agent().display().to_string());
    if !wraps(dir.path()) {
        return;
    }
    let app = app(dir.path());
    let project = dir.path().join("project");
    let agents = app.agents(&project).expect("agents");
    assert_eq!(agents[1].unavailable, None, "{:?}", agents[1]);
    let live = app
        .open_as(project.clone(), None, Some("fake".into()), THEME.into())
        .await
        .expect("opens");
    // The project as the list groups it, so the test does not guess how.
    let projects = app.workspace().projects(10).expect("projects");
    let root = &projects.first().expect("one project").root;
    let listed = app.workspace().sessions(root, 10).expect("list");
    let row = listed
        .iter()
        .find(|e| e.info.id == live.id().to_string())
        .expect("listed");
    assert_eq!(row.agent.as_deref(), Some("fake"));
    live.end();
}
