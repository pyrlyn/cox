// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The session owner end to end (T37.39): `App` opens a real session built
//! by cox-session in a scratch `COX_HOME` over the Scripted provider, and
//! `LiveSession` runs intents as the macOS app sends them through
//! `cox-ffi`. The host is in memory, never the real Keychain (A49); nextest
//! runs each test in its own process, so each sets its own environment.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cox_app::Activity;
use cox_app::TimelinePatch;
use cox_app::app::{App, AppError, Host};
use cox_app::diffmodel::DiffLineKind;
use cox_app::live::LiveSession;
use cox_app::server::{ServerHost, serve};
use cox_app::wire::{self, Call, Line, Outcome, Reply, Request, ServerEvent};
use cox_app::{
    BlockId, BlockKind, CheckId, CheckStatus, FileChange, InboxItem, Intent, Layer, Need,
};
use cox_app::{Browser, BrowserError, PageText};
use cox_app::{TaskKind, TaskTarget, tasks};
use cox_protocol::traits::{Archive as _, Store as _};
use cox_protocol::types::{Attachment, Decision, Event, StopReason};

/// Reads `notes.md`, then replies in markdown.
const READ_AND_REPLY: &str = r#"
[[turn]]
text = "Reading the notes."
tool_calls = [{ name = "read", input = { path = "notes.md" } }]

[[turn]]
text = "The file says **hello**."
"#;

/// A write the default policy asks about.
const WRITE: &str = r#"
[[turn]]
text = "Writing it."
tool_calls = [{ name = "write", input = { path = "out.txt", content = "approved\n" } }]

[[turn]]
text = "Done."
"#;

/// A write held for approval, then a reply for the turn queued behind it.
const WRITE_THEN_REPLY: &str = r#"
[[turn]]
text = "Writing it."
tool_calls = [{ name = "write", input = { path = "out.txt", content = "approved\n" } }]

[[turn]]
text = "Done."

[[turn]]
text = "Seen."
"#;

/// A write granted for the session, then the same write in a second turn.
const WRITE_TWICE: &str = r#"
[[turn]]
text = "Writing it."
tool_calls = [{ name = "write", input = { path = "out.txt", content = "one\n" } }]

[[turn]]
text = "Done."

[[turn]]
text = "Writing it again."
tool_calls = [{ name = "write", input = { path = "out.txt", content = "two\n" } }]

[[turn]]
text = "Done again."
"#;

/// Two plain replies: one per turn.
const TWO_REPLIES: &str = r#"
[[turn]]
text = "One."

[[turn]]
text = "Two."
"#;

/// Edits `notes.md` and creates `new.rs` in one turn: one checkpoint.
const TWO_EDITS: &str = r#"
[[turn]]
text = "Changing two files."
tool_calls = [
  { name = "edit", input = { path = "notes.md", old = "hello", new = "bye" } },
  { name = "write", input = { path = "new.rs", content = "fn main() {}\n" } },
]

[[turn]]
text = "Done."
"#;

/// A foreground subagent, then a background shell.
const AGENT_AND_SHELL: &str = r#"
[[turn]]
text = "Delegating."
tool_calls = [{ name = "agent", input = { task = "find x", preset = "explore" } }]

[[turn]]
text = "result: x"

[[turn]]
text = "Building."
tool_calls = [{ name = "bash", input = { command = "echo built", background = true } }]

[[turn]]
text = "Done."
"#;

/// A turn that delegates to a subagent, then a plain second turn.
const DELEGATE_THEN_REPLY: &str = r#"
[[turn]]
text = "Delegating."
tool_calls = [{ name = "agent", input = { task = "find x", preset = "explore" } }]

[[turn]]
text = "result: x"

[[turn]]
text = "Found it."

[[turn]]
text = "Two."
"#;

/// Reads the page open in the browser pane (T51.7).
const BROWSE: &str = r#"
[[turn]]
text = "Reading the page."
tool_calls = [{ name = "browser_read", input = {} }]

[[turn]]
text = "Read it."
"#;

/// The Keychain as a map; remembers what it was asked and told.
#[derive(Default)]
struct MemoryHost {
    secrets: HashMap<String, String>,
    asked: Mutex<Vec<String>>,
    notes: Mutex<Vec<(InboxItem, u32)>>,
    badges: Mutex<Vec<u32>>,
    browser: Option<Arc<dyn Browser>>,
}

/// A browser pane showing one page of text.
struct Page(String);

#[async_trait::async_trait]
impl Browser for Page {
    async fn load(&self, _: &str) -> Result<(), BrowserError> {
        Ok(())
    }
    async fn text(&self) -> Result<PageText, BrowserError> {
        Ok(PageText {
            title: "Long".into(),
            url: "http://localhost:3000/".into(),
            text: self.0.clone(),
        })
    }
    async fn snapshot(&self) -> Result<Vec<u8>, BrowserError> {
        Err(BrowserError::NoPage)
    }
}

impl Host for MemoryHost {
    fn notify(&self, item: InboxItem, badge: u32) {
        self.notes.lock().expect("notes").push((item, badge));
    }
    fn badge(&self, badge: u32) {
        self.badges.lock().expect("badges").push(badge);
    }
    fn open_url(&self, _: &str) {}
    fn secret(&self, section: &str) -> Option<String> {
        self.asked.lock().expect("asked").push(section.into());
        self.secrets.get(section).cloned()
    }
    fn browser(&self) -> Option<Arc<dyn Browser>> {
        self.browser.clone()
    }
}

/// Points cox at a tempdir (and, with a script, at the Scripted provider)
/// and makes `project/notes.md`.
fn scratch(script: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("user");
    std::fs::create_dir_all(dir.path().join("project")).expect("project");
    std::fs::write(dir.path().join("project/notes.md"), "hello\n").expect("notes");
    // SAFETY: first thing in this test's own process (nextest).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("COX_HOME", home.join(".cox"));
        std::env::remove_var("ANTHROPIC_API_KEY");
        if let Some(script) = script {
            let path = dir.path().join("scenario.toml");
            std::fs::write(&path, script).expect("scenario");
            std::env::set_var("COX_PROVIDER", "scripted");
            std::env::set_var("COX_SCENARIO", path);
        }
    }
    dir
}

fn app(dir: &Path, host: Arc<MemoryHost>) -> Arc<App> {
    App::new(Some(dir.join("user/.cox")), host).expect("app")
}

async fn open(dir: &Path, host: Arc<MemoryHost>) -> Result<Arc<LiveSession>, AppError> {
    let theme = "base16-ocean.dark".to_string();
    app(dir, host).open(dir.join("project"), None, theme).await
}

fn send(text: &str) -> Intent {
    Intent::Send {
        text: text.into(),
        attachments: vec![],
        confirm_think: false,
    }
}

fn ends_turn(patch: &TimelinePatch) -> bool {
    matches!(patch, TimelinePatch::Upsert { block, .. }
        if matches!(block.kind, BlockKind::TurnMeta { stop: Some(_), .. }))
}

/// Pulls until the running turn ends.
async fn finish(session: &LiveSession) {
    while let Some(batch) = session.next_patches().await {
        if batch.iter().any(ends_turn) {
            return;
        }
    }
    panic!("the stream closed before the turn ended");
}

/// Pulls until an approval is pending; its call.
async fn pending(session: &LiveSession) -> cox_protocol::CallId {
    loop {
        let batch = session.next_patches().await.expect("open stream");
        let pending = batch.iter().find_map(|p| match p {
            TimelinePatch::Upsert { block, .. } => match &block.kind {
                BlockKind::Approval {
                    call,
                    decision: None,
                    ..
                } => Some(*call),
                _ => None,
            },
            _ => None,
        });
        if let Some(call) = pending {
            return call;
        }
    }
}

/// The user and assistant texts, in order.
fn texts(session: &LiveSession) -> Vec<String> {
    let blocks = session.snapshot().into_iter().map(|b| b.kind);
    blocks
        .filter_map(|k| match k {
            BlockKind::User { text, .. } | BlockKind::Assistant { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_sent_turn_reaches_the_timeline_as_blocks() {
    let dir = scratch(Some(READ_AND_REPLY));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    let returned = session.send(send("read the notes")).await.expect("send");
    assert!(returned.is_none(), "a turn is spawned, not a child");
    finish(&session).await;
    let kinds: Vec<BlockKind> = session.snapshot().into_iter().map(|b| b.kind).collect();
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, BlockKind::Tool { tool, .. } if tool == "read"))
    );
    assert!(kinds.iter().any(|k| matches!(
        k,
        BlockKind::TurnMeta {
            stop: Some(StopReason::EndTurn),
            ..
        }
    )));
    assert_eq!(
        texts(&session),
        [
            "read the notes",
            "Reading the notes.",
            "The file says **hello**."
        ]
    );
}

#[tokio::test]
async fn an_approval_reaches_the_inbox_and_the_host_and_the_intent_answers_it() {
    let dir = scratch(Some(WRITE));
    let host = Arc::new(MemoryHost::default());
    let app = app(dir.path(), Arc::clone(&host));
    let theme = "base16-ocean.dark".to_string();
    let session = app.open(dir.path().join("project"), None, theme).await;
    let session = session.expect("open");
    session.send(send("write it")).await.expect("send");
    let call = pending(&session).await;
    let notes = host.notes.lock().expect("notes").clone();
    let asks =
        |item: &InboxItem| matches!(&item.need, Need::Approval { call: c, .. } if c.id == call);
    assert!(matches!(&notes[..], [(item, 1)] if asks(item)), "{notes:?}");
    assert_eq!(app.badge(), 1);
    let approve = Intent::Approve {
        call,
        decision: Decision::Allow,
    };
    session.send(approve).await.expect("approve");
    finish(&session).await;
    let written = std::fs::read_to_string(dir.path().join("project/out.txt"));
    assert_eq!(written.ok().as_deref(), Some("approved\n"));
    assert_eq!(app.badge(), 0);
    assert_eq!(
        *host.badges.lock().expect("badges"),
        [0],
        "the Dock badge falls"
    );
}

#[tokio::test]
async fn a_queued_turn_carries_its_attachments_and_counts_until_it_starts() {
    let dir = scratch(Some(WRITE_THEN_REPLY));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session.send(send("write it")).await.expect("send");
    let queue = Intent::Queue {
        text: "look".into(),
        attachments: vec![Attachment {
            name: "shot.png".into(),
            media_type: "image/png".into(),
            data_b64: "iVBORw0KGgo=".into(),
        }],
        confirm_think: false,
    };
    session.send(queue).await.expect("queue");
    // The write waits for approval, so the queued turn cannot start first.
    let (mut queued, mut ended) = (Vec::new(), 0);
    while ended < 2 {
        let batch = session.next_patches().await.expect("open stream");
        for patch in &batch {
            match patch {
                TimelinePatch::Status { status } => queued.push(status.queued),
                TimelinePatch::Upsert { block, .. } => {
                    if let BlockKind::Approval {
                        call,
                        decision: None,
                        ..
                    } = &block.kind
                    {
                        let decision = Decision::Allow;
                        let approve = Intent::Approve {
                            call: *call,
                            decision,
                        };
                        session.send(approve).await.expect("approve");
                    }
                }
                _ => {}
            }
        }
        ended += batch.iter().filter(|p| ends_turn(p)).count();
    }
    assert_eq!(queued, [1, 0]);
    let users: Vec<_> = session
        .snapshot()
        .into_iter()
        .filter_map(|b| match b.kind {
            BlockKind::User { text, attachments } => Some((text, attachments)),
            _ => None,
        })
        .collect();
    let shot = vec!["shot.png".to_string()];
    assert_eq!(users, [("write it".into(), vec![]), ("look".into(), shot)]);
}

#[tokio::test]
async fn fork_opens_the_child_with_the_parents_blocks() {
    let dir = scratch(Some(READ_AND_REPLY));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session.send(send("read the notes")).await.expect("send");
    finish(&session).await;
    let child = session
        .send(Intent::Fork { turn: None })
        .await
        .expect("fork");
    let child = child.expect("a fork returns its child");
    assert_ne!(child.id(), session.id());
    assert_eq!(texts(&child), texts(&session));
}

#[tokio::test]
async fn resume_reopens_the_session_with_its_blocks() {
    let dir = scratch(Some(READ_AND_REPLY));
    let app = app(dir.path(), Arc::default());
    let (cwd, theme) = (dir.path().join("project"), "base16-ocean.dark");
    let first = app.open(cwd.clone(), None, theme.into()).await;
    let first = first.expect("open");
    first.send(send("read the notes")).await.expect("send");
    finish(&first).await;
    let (id, before) = (first.id(), texts(&first));
    first.end();
    drop(first);
    let resumed = app.open(cwd, Some(id), theme.into()).await;
    let resumed = resumed.expect("resume");
    assert_eq!(resumed.id(), id);
    assert_eq!(texts(&resumed), before);
}

/// A113 (T37.22.9): the open session renames through its core; a closed
/// one through `App::rename`; both land in the list the sidebar reads.
#[tokio::test]
async fn a_rename_reaches_the_session_list_open_or_closed() {
    let dir = scratch(Some(READ_AND_REPLY));
    let app = app(dir.path(), Arc::default());
    let cwd = dir.path().join("project");
    let session = app
        .open(cwd.clone(), None, "base16-ocean.dark".into())
        .await;
    let session = session.expect("open");
    let title = |app: &App| {
        let rows = app.workspace().sessions(&cwd, 10).expect("sessions");
        rows.into_iter()
            .map(|row| row.info.title)
            .collect::<Vec<_>>()
    };
    let rename = Intent::Rename {
        title: "Fix the ledger".into(),
    };
    session.send(rename).await.expect("rename");
    assert_eq!(title(&app), [Some("Fix the ledger".to_string())]);
    let id = session.id();
    session.end();
    drop(session);
    assert!(app.rename(id, " Ledger fix \n").expect("rename closed"));
    assert!(!app.rename(id, "  ").expect("blank"));
    assert_eq!(title(&app), [Some("Ledger fix".to_string())]);
}

#[tokio::test]
async fn history_is_the_sessions_own_prompts_newest_first() {
    let dir = scratch(Some(TWO_REPLIES));
    let app = app(dir.path(), Arc::default());
    let (cwd, theme) = (dir.path().join("project"), "base16-ocean.dark");
    let other = app.open(cwd.clone(), None, theme.into()).await;
    let other = other.expect("open the other session");
    other.send(send("elsewhere")).await.expect("send");
    finish(&other).await;
    let session = app.open(cwd, None, theme.into()).await.expect("open");
    assert_eq!(session.history(10).expect("history"), Vec::<String>::new());
    for prompt in ["first prompt", "second prompt"] {
        session.send(send(prompt)).await.expect("send");
        finish(&session).await;
    }
    let history = session.history(10).expect("history");
    assert_eq!(history, ["second prompt", "first prompt"]);
    assert_eq!(session.history(1).expect("history"), ["second prompt"]);
}

#[tokio::test]
async fn an_empty_intent_fails_without_touching_the_session() {
    let dir = scratch(Some(READ_AND_REPLY));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    let err = session.send(send("  ")).await.err();
    assert!(matches!(err, Some(AppError::Intent(_))), "{err:?}");
    assert!(texts(&session).is_empty());
}

#[tokio::test]
async fn the_host_supplies_the_provider_key_and_its_absence_fails() {
    let dir = scratch(None);
    let empty = Arc::new(MemoryHost::default());
    let err = open(dir.path(), Arc::clone(&empty)).await.err();
    assert!(matches!(err, Some(AppError::Session(_))), "{err:?}");
    assert_eq!(*empty.asked.lock().expect("asked"), ["anthropic"]);

    let mut keyed = MemoryHost::default();
    keyed.secrets.insert("anthropic".into(), "sk-test".into());
    let session = open(dir.path(), Arc::new(keyed)).await;
    assert!(session.is_ok(), "{:?}", session.err());
}

#[test]
fn the_checklist_asks_the_host_for_the_provider_key() {
    let dir = scratch(None);
    let project = dir.path().join("project");
    let empty = Arc::new(MemoryHost::default());
    let rows = app(dir.path(), Arc::clone(&empty))
        .checklist(&project)
        .expect("checklist");
    assert_eq!(rows[0].id, CheckId::ProviderKey);
    assert_eq!(rows[0].status, CheckStatus::Fail, "{}", rows[0].detail);
    assert_eq!(*empty.asked.lock().expect("asked"), ["anthropic"]);
    // `load_login_env` never ran in this process.
    let shell = rows.last().expect("shell row");
    assert_eq!(
        (shell.id, shell.status),
        (CheckId::ShellEnv, CheckStatus::Warn)
    );

    let mut keyed = MemoryHost::default();
    keyed.secrets.insert("anthropic".into(), "sk-test".into());
    let rows = app(dir.path(), Arc::new(keyed))
        .checklist(&project)
        .expect("checklist");
    assert_eq!(rows[0].status, CheckStatus::Ok, "{}", rows[0].detail);
}

/// A session that ran `TWO_EDITS` with both tools allowed.
async fn edited(dir: &Path) -> Arc<LiveSession> {
    let config = "[permissions]\nallow = [\"edit\", \"write\"]\n";
    std::fs::create_dir_all(dir.join("user/.cox")).expect("home");
    std::fs::write(dir.join("user/.cox/config.toml"), config).expect("config");
    let session = open(dir, Arc::default()).await.expect("open");
    session.send(send("change them")).await.expect("send");
    finish(&session).await;
    session
}

#[tokio::test]
async fn changes_lists_the_edited_and_created_files_and_the_turn_to_rewind_to() {
    let dir = scratch(Some(TWO_EDITS));
    let session = edited(dir.path()).await;
    let changes = session.changes().await.expect("changes");
    let mut files = changes.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let got: Vec<_> = files
        .iter()
        .map(|f| (f.path.to_str(), f.change, f.added, f.removed, f.turn))
        .collect();
    assert_eq!(
        got,
        [
            (Some("new.rs"), FileChange::Created, 1, 0, 1),
            (Some("notes.md"), FileChange::Edited, 1, 1, 1),
        ]
    );
    let blocks = session.snapshot();
    for file in &files {
        let id = BlockId(format!("call:{}", file.call));
        let tool = blocks.iter().find(|b| b.id == id).map(|b| &b.kind);
        assert!(matches!(tool, Some(BlockKind::Tool { .. })), "{file:?}");
    }
    let [checkpoint] = changes.checkpoints.as_slice() else {
        panic!("one turn changed files: {:?}", changes.checkpoints);
    };
    assert_eq!(checkpoint.turn, 1);
    assert!(
        checkpoint.label.starts_with("Turn 1 · before "),
        "{checkpoint:?}"
    );
    assert!(checkpoint.label.ends_with(" and 1 more"), "{checkpoint:?}");
    assert!(checkpoint.time.starts_with("20"), "{checkpoint:?}");
    assert_eq!(changes.worktree, None, "a tempdir is no linked worktree");
}

/// Sends the rewind the timeline (T37.28.1) sends for the session's one
/// checkpoint, then pulls until its notice lands.
async fn rewind(session: &LiveSession, code: bool, conversation: bool) {
    let changes = session.changes().await.expect("changes");
    let [checkpoint] = changes.checkpoints.as_slice() else {
        panic!("one checkpoint: {:?}", changes.checkpoints);
    };
    let intent = Intent::Rewind {
        to_turn: checkpoint.turn,
        code,
        conversation,
    };
    session.send(intent).await.expect("rewind");
    let done = |s: &LiveSession| {
        s.snapshot().iter().any(|b| {
            matches!(&b.kind, BlockKind::Notice { text, .. } if text.starts_with("rewound to T"))
        })
    };
    while !done(session) {
        session.next_patches().await.expect("the stream stays open");
    }
}

#[tokio::test]
async fn rewinding_code_to_a_checkpoint_restores_its_files_and_keeps_the_conversation() {
    let dir = scratch(Some(TWO_EDITS));
    let session = edited(dir.path()).await;
    let notes = std::fs::read_to_string(dir.path().join("project/notes.md"));
    assert_eq!(notes.ok().as_deref(), Some("bye\n"));
    rewind(&session, true, false).await;
    let notes = std::fs::read_to_string(dir.path().join("project/notes.md"));
    assert_eq!(notes.ok().as_deref(), Some("hello\n"));
    assert!(
        !dir.path().join("project/new.rs").exists(),
        "a created file goes"
    );
    assert_eq!(
        texts(&session),
        ["change them", "Changing two files.", "Done."]
    );
}

/// Pulls until a notice starting with `prefix` lands.
async fn notice(session: &LiveSession, prefix: &str) {
    let done = |s: &LiveSession| {
        s.snapshot()
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Notice { text, .. } if text.starts_with(prefix)))
    };
    while !done(session) {
        session.next_patches().await.expect("the stream stays open");
    }
}

/// T37.28.3: the Changes tab's Revert on one file puts that file back and
/// leaves the other; the revert is checkpointed, so `/redo` undoes it.
#[tokio::test]
async fn reverting_one_file_restores_it_and_leaves_the_other() {
    let dir = scratch(Some(TWO_EDITS));
    let session = edited(dir.path()).await;
    let changes = session.changes().await.expect("changes");
    let file = changes
        .files
        .iter()
        .find(|f| f.path.ends_with("notes.md"))
        .expect("notes.md changed");
    let intent = Intent::RevertFile {
        path: file.path.to_string_lossy().into_owned(),
        to_turn: 1,
    };
    session.send(intent).await.expect("revert");
    notice(&session, "reverted notes.md to before T1").await;
    let notes = std::fs::read_to_string(dir.path().join("project/notes.md"));
    assert_eq!(notes.ok().as_deref(), Some("hello\n"));
    assert!(
        dir.path().join("project/new.rs").exists(),
        "the other file stays"
    );

    session.send(Intent::Redo).await.expect("redo");
    notice(&session, "rewound to T2").await;
    let notes = std::fs::read_to_string(dir.path().join("project/notes.md"));
    assert_eq!(notes.ok().as_deref(), Some("bye\n"));
}

/// Each line of `path`'s Review diff as `+text`, `-text` or ` text`.
async fn reviewed(session: &LiveSession, path: &str) -> Vec<String> {
    let diff = session.review(path).await.expect("review");
    let diff = diff.unwrap_or_else(|| panic!("{path} has a diff"));
    assert_eq!(diff.path, Path::new(path));
    let lines = diff.hunks.iter().flat_map(|h| &h.lines);
    lines
        .map(|l| {
            let sign = match l.kind {
                DiffLineKind::Add => '+',
                DiffLineKind::Del => '-',
                DiffLineKind::Context => ' ',
            };
            let text: String = l.spans.iter().map(|s| s.text.as_str()).collect();
            format!("{sign}{text}")
        })
        .collect()
}

#[tokio::test]
async fn review_diffs_each_file_against_its_checkpoint_and_after_a_code_rewind_nets_to_nothing() {
    let dir = scratch(Some(TWO_EDITS));
    let session = edited(dir.path()).await;
    assert_eq!(reviewed(&session, "notes.md").await, ["-hello", "+bye"]);
    assert_eq!(reviewed(&session, "new.rs").await, ["+fn main() {}"]);
    let outside = session.review("../scenario.toml").await.expect("review");
    assert_eq!(outside, None, "a path outside the roots is not read");
    assert_eq!(session.review("never.md").await.expect("review"), None);

    rewind(&session, true, false).await;
    assert!(reviewed(&session, "notes.md").await.is_empty());
    assert!(reviewed(&session, "new.rs").await.is_empty());
}

#[tokio::test]
async fn rewinding_code_and_conversation_leaves_nothing_to_review() {
    let dir = scratch(Some(TWO_EDITS));
    let session = edited(dir.path()).await;
    rewind(&session, true, true).await;
    let notes = std::fs::read_to_string(dir.path().join("project/notes.md"));
    assert_eq!(notes.ok().as_deref(), Some("hello\n"));
    assert!(
        !dir.path().join("project/new.rs").exists(),
        "a created file goes"
    );
    assert!(texts(&session).is_empty(), "{:?}", texts(&session));
    let changes = session.changes().await.expect("changes");
    assert_eq!((changes.files, changes.checkpoints), (vec![], vec![]));
}

#[tokio::test]
async fn open_task_finds_the_subagents_session_and_the_shells_output() {
    let dir = scratch(Some(AGENT_AND_SHELL));
    std::fs::create_dir_all(dir.path().join("user/.cox")).expect("home");
    let config = "[permissions]\nallow = [\"bash\"]\n";
    std::fs::write(dir.path().join("user/.cox/config.toml"), config).expect("config");
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session
        .send(send("delegate, then build"))
        .await
        .expect("send");
    let tasks = loop {
        let tasks: Vec<_> = session
            .snapshot()
            .into_iter()
            .filter_map(|b| match b.kind {
                BlockKind::Task {
                    task, done, kind, ..
                } => Some((task, done, kind)),
                _ => None,
            })
            .collect();
        if tasks.len() == 2 && tasks.iter().all(|t| t.1) {
            break tasks;
        }
        session.next_patches().await.expect("the stream stays open");
    };
    let [(agent, _, TaskKind::Agent), (shell, _, TaskKind::Shell)] = tasks[..] else {
        panic!("a subagent, then a shell: {tasks:?}");
    };

    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let Some(TaskTarget::Transcript { session: child }) = session.open_task(agent).expect("agent")
    else {
        panic!("a subagent opens its session");
    };
    assert_ne!(child, session.id());
    let rollout = store.rollout_read(&child).expect("child rollout");
    assert_eq!(tasks::first_prompt(&rollout).as_deref(), Some("find x"));

    let Some(TaskTarget::Output { archive }) = session.open_task(shell).expect("shell") else {
        panic!("a finished shell opens its output");
    };
    let output = store.get(&archive).await.expect("archived output");
    assert!(String::from_utf8_lossy(&output).contains("built"));
    let shown = session.output(&archive).expect("the viewer's text");
    assert!(shown.contains("built"), "{shown:?}");
}

/// T37.22.6: the session list is read again when it may read differently,
/// not on a timer — quiet while nothing commits, woken by another
/// process's session.
#[tokio::test]
async fn workspace_changed_waits_for_another_connections_commit() {
    let dir = scratch(Some(READ_AND_REPLY));
    let sidebar = app(dir.path(), Arc::default());
    let quiet = tokio::time::timeout(
        std::time::Duration::from_millis(800),
        sidebar.workspace_changed(),
    );
    assert!(quiet.await.is_err(), "nothing committed yet");

    let watching = tokio::spawn({
        let sidebar = Arc::clone(&sidebar);
        async move { sidebar.workspace_changed().await }
    });
    // Let it take its position in the feed before anything commits.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Another `App` stands in for a TUI: its store is another connection.
    let other = app(dir.path(), Arc::default());
    let theme = "base16-ocean.dark".to_string();
    let session = other.open(dir.path().join("project"), None, theme).await;
    let session = session.expect("open");
    session.send(send("read the notes")).await.expect("send");
    let woke = tokio::time::timeout(std::time::Duration::from_secs(20), watching).await;
    assert!(matches!(woke, Ok(Ok(Ok(())))), "{woke:?}");
}

#[test]
fn the_model_popover_lists_the_code_tiers_model_first() {
    let dir = scratch(None);
    let models = app(dir.path(), Arc::default())
        .models(&dir.path().join("project"))
        .expect("models");
    let first = models.first().expect("a model");
    assert_eq!(first.tier, cox_protocol::types::Tier::Code);
    assert_eq!(first.id, "claude-sonnet-5");
}

#[tokio::test]
async fn usable_providers_count_a_stored_key_and_not_a_missing_one() {
    let dir = scratch(None);
    let mut keyed = MemoryHost::default();
    keyed.secrets.insert("anthropic".into(), "sk-test".into());
    // SAFETY: this test's own process (nextest).
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
    let usable = app(dir.path(), Arc::new(keyed))
        .usable_providers(&dir.path().join("project"))
        .await
        .expect("providers");
    assert!(usable.contains(&"anthropic".to_string()), "{usable:?}");
    assert!(!usable.contains(&"openai".to_string()), "{usable:?}");
}

#[tokio::test]
async fn info_names_the_session_its_cwd_rollout_and_the_user_config_it_read() {
    let dir = scratch(Some(READ_AND_REPLY));
    let user = dir.path().join("user/.cox/config.toml");
    std::fs::create_dir_all(dir.path().join("user/.cox")).expect("home");
    std::fs::write(&user, "[tui]\nvim = true\n").expect("config");
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session.send(send("read it")).await.expect("send");
    finish(&session).await;

    let info = session.info().await.expect("info");
    assert_eq!(info.session, session.id());
    assert_eq!(info.cwd, dir.path().join("project"));
    assert_eq!(info.worktree, None, "a tempdir is no linked worktree");
    let layers: Vec<_> = info.config.iter().map(|c| c.layer).collect();
    assert_eq!(layers.first(), Some(&Layer::Default), "{layers:?}");
    let from_user = info.config.iter().find(|c| c.layer == Layer::User);
    let from_user = from_user.map(|c| (c.file.clone(), c.keys));
    assert_eq!(from_user, Some((Some(user), 1)));
    let rollout = dir
        .path()
        .join(format!("user/.cox/sessions/{}.jsonl", session.id()));
    assert_eq!(info.rollout, rollout);
    assert!(info.rollout.is_file(), "the turn was appended to it");
}

#[tokio::test]
async fn turn_costs_group_the_ledger_by_turn_with_the_subagent_under_its_turn() {
    let dir = scratch(Some(DELEGATE_THEN_REPLY));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session.send(send("delegate")).await.expect("send");
    finish(&session).await;
    session.send(send("again")).await.expect("send");
    finish(&session).await;

    let costs = session.turn_costs().expect("costs");
    let rows: Vec<_> = costs
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.detail))
        .collect();
    assert_eq!(
        rows,
        [("1 · code", false), ("explore", true), ("2 · code", false)]
    );
    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let own = store.usage_ledger(&session.id()).expect("ledger");
    assert_eq!(own.len(), 3, "two calls in turn 1, one in turn 2");
    assert_eq!(costs.total.label, "Session");
    assert_ne!(costs.total.values[0], "0", "{:?}", costs.total);
    assert!(
        costs.project.starts_with(&format!(
            "Project project today: ${}",
            costs.total.values[3]
        )),
        "{}",
        costs.project
    );
    let caps: Vec<_> = costs.budget.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(caps, ["Session", "This month"]);
    assert!(
        costs.budget[0].text.ends_with(" of $5.00"),
        "{:?}",
        costs.budget
    );
}

/// A104: the meter formats the cache hit both for the last turn and for the
/// session, each from the ledger rows it covers.
#[tokio::test]
async fn the_cache_hit_is_formatted_for_the_last_turn_and_for_the_session() {
    let dir = scratch(Some(TWO_REPLIES));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    let mut text = None;
    for prompt in ["one", "two"] {
        session.send(send(prompt)).await.expect("send");
        loop {
            let batch = session.next_patches().await.expect("open stream");
            for patch in &batch {
                if let TimelinePatch::Usage { usage } = patch {
                    text = Some(usage.text.clone());
                }
            }
            if batch.iter().any(ends_turn) {
                break;
            }
        }
    }
    let text = text.expect("a usage patch");

    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let ledger = store.usage_ledger(&session.id()).expect("ledger");
    let last = ledger
        .iter()
        .rposition(|r| r.usage.turn == 1)
        .expect("turn 2");
    assert!(last > 0, "two turns: {ledger:?}");
    let hit = |rows: &[cox_store::queries::LedgerRow], span: &str| {
        let sum = |f: fn(&cox_protocol::types::Usage) -> u32| {
            rows.iter().map(|r| f(&r.usage.usage)).sum::<u32>()
        };
        let sent = sum(cox_protocol::types::Usage::context_tokens);
        let read = sum(|u| u.cache_read_tokens);
        format!("{:.0}% {span}", f64::from(read) / f64::from(sent) * 100.0)
    };
    assert_eq!(text.cache_hit, hit(&ledger[last..], "this turn"));
    assert_eq!(text.cache_hit_session, hit(&ledger, "this session"));
}

/// T51.3: the terminal pane is the user's own terminal — what its shell
/// prints reaches the pane and nothing else: not the timeline, not the
/// rollout, not the ledger. macOS only: the pane's shell runs under
/// Seatbelt, and Landlock cannot wrap a PTY's argv.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn terminal_output_is_not_in_the_rollout() {
    let dir = scratch(Some(TWO_REPLIES));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    session.send(send("one")).await.expect("send");
    finish(&session).await;
    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let rows = store.usage_ledger(&session.id()).expect("ledger").len();

    let term = session.open_terminal(80, 24).expect("terminal");
    // The shell computes the marker, so the echoed input never contains it.
    term.write(b"echo TERM-MARK-$((6*7))\n").expect("write");
    let seen = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut seen = Vec::new();
        while let Some(bytes) = term.next_output().await {
            seen.extend(bytes);
            if String::from_utf8_lossy(&seen).contains("TERM-MARK-42") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(seen, "the pane got the shell's output");
    term.close();
    let after = store.usage_ledger(&session.id()).expect("ledger").len();
    assert_eq!(after, rows, "the pane adds no usage row");

    // The session goes on writing its rollout after the pane closed.
    session.send(send("two")).await.expect("send");
    finish(&session).await;
    let rollout = store.rollout_read(&session.id()).expect("rollout");
    let json = serde_json::to_string(&rollout).expect("json");
    assert!(!json.contains("TERM-MARK"), "{json}");
    let blocks = format!("{:?}", session.snapshot());
    assert!(!blocks.contains("TERM-MARK"), "{blocks}");
}

/// T51.7: a page over the tool cap reaches the model shortened, and the
/// archive holds every byte of it before that (the lossless rule).
#[tokio::test]
async fn browser_read_over_cap_is_archived_first() {
    let dir = scratch(Some(BROWSE));
    let page: String = (0..2000).map(|i| format!("line {i}\n")).collect();
    let host = Arc::new(MemoryHost {
        browser: Some(Arc::new(Page(page.clone()))),
        ..MemoryHost::default()
    });
    let session = open(dir.path(), host).await.expect("open");
    session.send(send("read the page")).await.expect("send");
    finish(&session).await;

    let store = cox_store::Store::open(&dir.path().join("user/.cox")).expect("store");
    let rollout = store.rollout_read(&session.id()).expect("rollout");
    let result = rollout.iter().find_map(|e| match e {
        Event::ToolCallDone { result, .. } => Some(result.clone()),
        _ => None,
    });
    let result = result.expect("browser_read finished");
    assert!(result.ok, "{}", result.visible);
    let archive = result.archive.expect("the full text is archived");
    let full = store.get(&archive.id).await.expect("archived bytes");
    let full = String::from_utf8(full).expect("text");
    assert!(full.starts_with("Title: Long\nURL: http://localhost:3000/\n"));
    assert!(full.ends_with(&page), "every line of the page is archived");
    assert!(result.visible.len() < full.len(), "the model sees less");
    assert!(!result.visible.contains("line 1000\n"));
}

#[tokio::test]
async fn a_grant_revoked_from_settings_makes_the_next_write_ask_again() {
    let dir = scratch(Some(WRITE_TWICE));
    let app = app(dir.path(), Arc::default());
    let cwd = dir.path().join("project");
    let theme = "base16-ocean.dark".to_string();
    let session = app.open(cwd.clone(), None, theme).await.expect("open");
    session.send(send("write it")).await.expect("send");
    let call = pending(&session).await;
    let grant = Intent::Approve {
        call,
        decision: Decision::AllowForSession,
    };
    session.send(grant).await.expect("allow for session");
    finish(&session).await;

    let view = app.settings(&cwd).await.expect("settings");
    let [granted] = &view.grants[..] else {
        panic!("one grant: {:?}", view.grants);
    };
    assert_eq!(
        (granted.session, granted.tool.as_str()),
        (session.id(), "write")
    );
    let view = app.revoke_grant(&cwd, granted).await.expect("revoke");
    assert!(view.grants.is_empty(), "{:?}", view.grants);

    session.send(send("write it again")).await.expect("send");
    let asked = pending(&session).await;
    assert_ne!(asked, call, "the second write asks on its own");
    let elsewhere = cox_app::SessionGrant {
        session: cox_protocol::ids::SessionId::new(),
        ..granted.clone()
    };
    let refused = app.revoke_grant(&cwd, &elsewhere).await;
    assert!(matches!(refused, Err(AppError::NotOpen(_))));
}

/// T52.19: a request line to `cox app-server`.
async fn request(to: &mut tokio::io::DuplexStream, id: u64, call: Call) {
    use tokio::io::AsyncWriteExt as _;
    let line = wire::encode(&Line::Request(Request {
        v: wire::VERSION,
        id,
        call,
    }))
    .expect("encode");
    to.write_all(line.as_bytes())
        .await
        .expect("write a request");
}

type ServerLines = tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>;

async fn next_line(lines: &mut ServerLines) -> Line {
    let text = lines.next_line().await.expect("read").expect("a line");
    wire::decode(&text).expect("a protocol line")
}

/// Reads past notifications to the response to `id`.
async fn response(lines: &mut ServerLines, id: u64) -> Outcome {
    loop {
        if let Line::Response(r) = next_line(lines).await
            && r.id == id
        {
            return r.outcome;
        }
    }
}

/// The client's ends and the server's, as two one-way pipes of `size` bytes.
fn pipes(
    size: usize,
) -> (
    tokio::io::DuplexStream,
    ServerLines,
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
) {
    use tokio::io::AsyncBufReadExt as _;
    let (to_server, server_in) = tokio::io::duplex(size);
    let (server_out, from_server) = tokio::io::duplex(size);
    let lines = tokio::io::BufReader::new(from_server).lines();
    (to_server, lines, server_in, server_out)
}

fn open_call(dir: &Path) -> Call {
    Call::Open {
        cwd: dir.join("project"),
        resume: None,
        theme: "base16-ocean.dark".into(),
    }
}

fn server_app(dir: &Path) -> (Arc<App>, tokio::sync::mpsc::UnboundedReceiver<ServerEvent>) {
    let (host, events) = ServerHost::channel();
    let app = App::new(Some(dir.join("user/.cox")), host).expect("app");
    (app, events)
}

#[tokio::test]
async fn app_server_serves_a_scripted_turn() {
    let dir = scratch(Some(TWO_REPLIES));
    let (app, events) = server_app(dir.path());
    let (mut to_server, mut lines, server_in, server_out) = pipes(64 * 1024);
    let client = async {
        request(&mut to_server, 1, open_call(dir.path())).await;
        let Outcome::Ok(Reply::Opened { session, .. }) = response(&mut lines, 1).await else {
            panic!("open failed");
        };
        request(
            &mut to_server,
            2,
            Call::Send {
                session,
                intent: send("hi"),
            },
        )
        .await;
        let mut sent = false;
        let mut ended = false;
        while !(sent && ended) {
            match next_line(&mut lines).await {
                Line::Response(r) if r.id == 2 => {
                    assert_eq!(r.outcome, Outcome::Ok(Reply::Sent { child: None }));
                    sent = true;
                }
                Line::Notification(n) => {
                    if let ServerEvent::Patches {
                        session: s,
                        patches,
                    } = n.event
                    {
                        assert_eq!(s, session);
                        ended |= patches.iter().any(ends_turn);
                    }
                }
                _ => {}
            }
        }
        request(&mut to_server, 3, Call::Projects { limit: 10 }).await;
        let projects = response(&mut lines, 3).await;
        assert!(
            matches!(projects, Outcome::Ok(Reply::Projects(_))),
            "{projects:?}"
        );
        // Closing stdin ends the server; its stream then ends too.
        drop(to_server);
        while lines.next_line().await.ok().flatten().is_some() {}
    };
    let ((), served) = tokio::join!(client, serve(app, events, server_in, server_out));
    served.expect("served");
}

#[tokio::test]
async fn app_server_never_answers_a_secret() {
    // No scripted provider and no key in the env: the default provider
    // needs a key, and the server's host has none to give.
    let dir = scratch(None);
    let (host, _events) = ServerHost::channel();
    assert_eq!(host.secret("anthropic"), None);

    let (app, events) = server_app(dir.path());
    let (mut to_server, mut lines, server_in, server_out) = pipes(64 * 1024);
    let client = async {
        request(&mut to_server, 1, open_call(dir.path())).await;
        let outcome = response(&mut lines, 1).await;
        assert!(matches!(outcome, Outcome::Err(_)), "{outcome:?}");
        drop(to_server);
        while lines.next_line().await.ok().flatten().is_some() {}
    };
    let ((), served) = tokio::join!(client, serve(app, events, server_in, server_out));
    served.expect("served");
}

#[tokio::test]
async fn app_server_stalled_client_does_not_delay_the_turn() {
    let dir = scratch(Some(WRITE));
    let (app, events) = server_app(dir.path());
    // One byte of pipe: once the client stops reading, every write waits.
    let (mut to_server, mut lines, server_in, server_out) = pipes(1);
    let client = async {
        request(&mut to_server, 1, open_call(dir.path())).await;
        let Outcome::Ok(Reply::Opened { session, .. }) = response(&mut lines, 1).await else {
            panic!("open failed");
        };
        request(
            &mut to_server,
            2,
            Call::Send {
                session,
                intent: send("write it"),
            },
        )
        .await;
        // The client never reads again; the turn still reaches its approval.
        let waiting = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while app.activity(session) != Activity::WaitingOnYou {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        // The client goes away: the blocked write fails and the server ends.
        drop(lines);
        drop(to_server);
        waiting
    };
    let (waiting, _served) = tokio::join!(
        client,
        serve(Arc::clone(&app), events, server_in, server_out)
    );
    assert!(
        waiting.is_ok(),
        "the turn waited on a client that stopped reading"
    );
}
