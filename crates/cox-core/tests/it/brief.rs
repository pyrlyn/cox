// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Session brief (docs/design/history-summarization.md): a compaction keeps
//! the facts the summariser wrote, evicts the old turns, saves the summary
//! for the project, and only a new session in that same project opens with
//! it in `system[3]`, after the cached prefix.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{drain, spawn_turn};
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::{Provider, Store as _};
use cox_protocol::types::{
    Caps, Content, Event, ProviderEvent, ProviderId, Request, Submission, Usage,
};
use cox_provider::scripted::Scripted;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Facts a correct summary must carry into the next request and session.
const FACTS: [&str; 3] = [
    "FACT-db-is-postgres-17",
    "DECISION-keep-diesel",
    "TODO-port-auth-tests",
];

fn summary() -> String {
    let pad = "Keep the parser change small and covered by a test. ".repeat(12);
    format!(
        "## Goal\\nship\\n## Facts\\n- {}\\n## Decisions\\n- {}\\n{pad}\\n## Open todo\\n- {}\\n## Next step\\ngo",
        FACTS[0], FACTS[1], FACTS[2]
    )
}

/// Three turns, then the compact job's summary, then one more turn.
fn script() -> String {
    format!(
        concat!(
            "[[turn]]\ntext = \"OLD-TURN-ZERO reply\"\n",
            "[[turn]]\ntext = \"second\"\n",
            "[[turn]]\ntext = \"third\"\n",
            "[[turn]]\ntext = \"{}\"\n",
            "[[turn]]\ntext = \"fourth\"\n",
        ),
        summary()
    )
}

/// Keeps every request it is sent.
struct Seeing(Scripted, Mutex<Vec<Request>>);

#[async_trait]
impl Provider for Seeing {
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

fn open(
    toml: &str,
    store: &Arc<MemoryStore>,
    cwd: &Path,
    brief: bool,
) -> (Session, Arc<Seeing>, mpsc::Receiver<Event>) {
    let mut config = cox_protocol::Config::default();
    config.session.auto_title = false;
    config.context.session_brief = brief;
    let provider = Arc::new(Seeing(
        Scripted::from_toml(toml, "").expect("scenario"),
        Mutex::default(),
    ));
    let session = Session::new(
        config,
        provider.clone(),
        common::tools(),
        store.clone(),
        store.clone(),
        cwd.to_path_buf(),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, provider, rx)
}

async fn turn(session: &Session, rx: &mut mpsc::Receiver<Event>, text: &str) {
    let running = spawn_turn(session, text);
    drain(rx).await;
    running.await.expect("join").expect("turn");
}

fn texts(messages: &[cox_protocol::types::Message]) -> String {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A scratch dir removed on drop; cox-core has no `tempfile` dev-dependency.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("cox-brief-{}", cox_protocol::ids::SessionId::new()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A project dir that is its own git root, so two of them never share a key.
fn project(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name).join("app");
    std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
    dir
}

async fn compacted_in(cwd: &Path, store: &Arc<MemoryStore>, brief: bool) -> Arc<Seeing> {
    let (session, provider, mut rx) = open(&script(), store, cwd, brief);
    for t in ["OLD-USER-ZERO", "t1", "t2"] {
        turn(&session, &mut rx, t).await;
    }
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    turn(&session, &mut rx, "t3").await;
    provider
}

#[tokio::test]
async fn brief_summary_keeps_facts_and_evicts_old_turns() {
    let tmp = Scratch::new();
    let store = Arc::new(MemoryStore::new());
    let provider = compacted_in(&project(tmp.0.as_path(), "a"), &store, true).await;
    let sent = provider.1.lock().expect("lock").clone();
    let last = sent.last().expect("a request after compaction");
    let history = texts(&last.messages);
    for fact in FACTS {
        assert!(history.contains(fact), "summary lost {fact}");
    }
    // The dropped user message survives only as the summary's
    // `## Last request` line (by design), never as a message of its own.
    let own = last
        .messages
        .iter()
        .any(|m| texts(std::slice::from_ref(m)).trim() == "OLD-USER-ZERO");
    assert!(!own, "old user turn not evicted:\n{history}");
    assert!(!history.contains("OLD-TURN-ZERO"), "old reply not evicted");
}

#[tokio::test]
async fn brief_reaches_a_new_session_in_the_same_project_after_the_prefix() {
    let tmp = Scratch::new();
    let store = Arc::new(MemoryStore::new());
    let cwd = project(tmp.0.as_path(), "a");
    compacted_in(&cwd, &store, true).await;

    let (fresh, seen, mut rx) = open("[[turn]]\ntext = \"hi\"\n", &store, &cwd, true);
    turn(&fresh, &mut rx, "new").await;
    let req = seen.1.lock().expect("lock")[0].clone();
    for fact in FACTS {
        assert!(
            req.system[3].text.contains(fact),
            "new session misses {fact}"
        );
    }
    for slot in 0..=2 {
        assert!(
            !req.system[slot].text.contains(FACTS[0]),
            "brief in cached system[{slot}]"
        );
    }
    assert!(!req.system[3].cache);
}

#[tokio::test]
async fn brief_never_leaks_into_another_project_with_the_same_folder_name() {
    let tmp = Scratch::new();
    let store = Arc::new(MemoryStore::new());
    compacted_in(&project(tmp.0.as_path(), "a"), &store, true).await;

    let other = project(tmp.0.as_path(), "b");
    let (fresh, seen, mut rx) = open("[[turn]]\ntext = \"hi\"\n", &store, &other, true);
    turn(&fresh, &mut rx, "new").await;
    let req = seen.1.lock().expect("lock")[0].clone();
    let all: String = req.system.iter().map(|b| b.text.as_str()).collect();
    for fact in FACTS {
        assert!(!all.contains(fact), "foreign brief leaked: {fact}");
    }
    assert!(
        store
            .brief_get(&other.to_string_lossy())
            .expect("get")
            .is_none()
    );
}

#[tokio::test]
async fn brief_off_neither_saves_nor_reads() {
    let tmp = Scratch::new();
    let store = Arc::new(MemoryStore::new());
    let cwd = project(tmp.0.as_path(), "a");
    compacted_in(&cwd, &store, false).await;
    let key = cwd.canonicalize().expect("canon");
    assert!(
        store
            .brief_get(&key.to_string_lossy())
            .expect("get")
            .is_none()
    );
}
