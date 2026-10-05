// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T26.1: the loop archives a pre-image before a write and snapshots
//! around a shell call, with a fake `Checkpointer` so no git runs here.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use common::{drain, open, scenario, spawn_turn, tool_results};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Archive, Store, Tool, ToolCx};
use cox_protocol::types::{CheckpointKind, Concurrency, Event, Level, Risk, ToolOutput, ToolSpec};
use cox_protocol::{Before, Change, Checkpointer, PreImage, Snapshot};
use serde_json::Value;

/// `a.rs` holds "old", nothing else exists; every snapshot is a new tree
/// and the first diff reports `gone.txt` deleted.
struct Fake {
    snapshots: AtomicU32,
    git_missing: bool,
}

#[async_trait]
impl Checkpointer for Fake {
    async fn preimages(&self, roots: &[PathBuf], _cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        paths
            .iter()
            .map(|p| PreImage {
                path: roots[0].join(p),
                before: if p == "a.rs" {
                    Before::Bytes(b"old".to_vec())
                } else {
                    Before::Absent
                },
            })
            .collect()
    }
    async fn snapshot(&self, roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        if self.git_missing {
            return Err(ToolError::Io);
        }
        let n = self.snapshots.fetch_add(1, Ordering::SeqCst);
        Ok(Snapshot {
            trees: vec![(roots[0].clone(), format!("tree{n}"))],
        })
    }
    async fn restore(
        &self,
        _roots: &[PathBuf],
        _cwd: &Path,
        _path: &Path,
        _bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        Ok(())
    }
    async fn changes(
        &self,
        before: &Snapshot,
        _after: &Snapshot,
    ) -> Result<Vec<Change>, ToolError> {
        if before.trees[0].1 != "tree0" {
            return Ok(vec![]);
        }
        Ok(vec![Change {
            path: before.trees[0].0.join("gone.txt"),
            deleted: true,
            before: Before::Bytes(b"bye".to_vec()),
        }])
    }
}

/// Writes and shell calls run without asking; the gate is not under test.
fn allow_all() -> cox_protocol::Config {
    let mut config = cox_protocol::Config::default();
    config.permissions.allow = vec!["touch".into(), "shell".into()];
    config
}

async fn run(
    name: &str,
    fake: Fake,
) -> (Vec<Event>, Arc<cox_core::MemoryStore>, cox_core::Session) {
    let (session, store, mut rx) = open(&scenario(name), allow_all());
    session.set_checkpointer(Arc::new(fake));
    let running = spawn_turn(&session, name);
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    (events, store, session)
}

fn checkpoint_files(events: &[Event]) -> Vec<Vec<(String, CheckpointKind)>> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Checkpoint { files, .. } => Some(
                files
                    .iter()
                    .map(|f| (f.path.display().to_string(), f.kind))
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn edit_has_preimage() {
    let fake = Fake {
        snapshots: AtomicU32::new(0),
        git_missing: false,
    };
    let (events, store, session) = run("checkpoint_edit", fake).await;
    assert_eq!(
        checkpoint_files(&events),
        vec![
            vec![("/tmp/cox-turn/a.rs".to_string(), CheckpointKind::Pre)],
            vec![("/tmp/cox-turn/new.rs".to_string(), CheckpointKind::Created)],
        ]
    );
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![
            CheckpointKind::Turn,
            CheckpointKind::Pre,
            CheckpointKind::Created
        ]
    );
    assert!(rows.iter().all(|r| r.turn == 1));
    let archived = rows[1].archive.expect("pre-image archived");
    assert_eq!(store.get(&archived).await.expect("bytes"), b"old");
    assert!(rows[2].archive.is_none());
    // A snapshot is never taken when the tool names its paths.
    assert!(events.iter().all(|e| !matches!(e, Event::Notice { .. })));
}

#[tokio::test]
async fn bash_rm_has_preimage() {
    let fake = Fake {
        snapshots: AtomicU32::new(0),
        git_missing: false,
    };
    let (events, store, session) = run("checkpoint_shell", fake).await;
    assert_eq!(
        checkpoint_files(&events),
        vec![vec![(
            "/tmp/cox-turn/gone.txt".to_string(),
            CheckpointKind::Deleted
        )]]
    );
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    let deleted = rows
        .iter()
        .find(|r| r.kind == CheckpointKind::Deleted)
        .expect("deleted row");
    let archived = deleted.archive.expect("pre-image archived");
    assert_eq!(store.get(&archived).await.expect("bytes"), b"bye");
    // The second shell call changed nothing: no event, no row.
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn missing_git_warns_once_and_never_fails_the_turn() {
    let fake = Fake {
        snapshots: AtomicU32::new(0),
        git_missing: true,
    };
    let (events, store, session) = run("checkpoint_shell", fake).await;
    let warnings: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::Notice { level: Level::Warn, text } if text.contains("checkpoints off")))
        .collect();
    assert_eq!(warnings.len(), 1, "one warning for two shell calls");
    assert_eq!(
        tool_results(&events),
        vec![(true, "ran".to_string()), (true, "ran".to_string())]
    );
    assert!(checkpoint_files(&events).is_empty());
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    assert!(rows.iter().all(|r| r.kind == CheckpointKind::Turn));
}

/// A write tool that reports how many checkpoint rows existed when it ran.
struct Probe(Arc<cox_core::MemoryStore>);

#[async_trait]
impl Tool for Probe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "touch".into(),
            description: "counts rows at call time".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }
    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        Some(vec![self.subject(input)])
    }
    async fn call(&self, _input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let rows = self
            .0
            .checkpoint_list(&cx.session)
            .map_err(|_| ToolError::Io)?;
        let pre = rows
            .iter()
            .filter(|r| r.kind != CheckpointKind::Turn)
            .count();
        Ok(ToolOutput {
            text: format!("rows={pre}"),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[tokio::test]
async fn checkpoint_row_exists_before_write() {
    let mut config = allow_all();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider = Arc::new(
        cox_provider::scripted::Scripted::from_toml(&scenario("checkpoint_edit"), "")
            .expect("scenario"),
    );
    let store = Arc::new(cox_core::MemoryStore::new());
    let session = cox_core::Session::new(
        config,
        provider,
        vec![Arc::new(Probe(store.clone()))],
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    session.set_checkpointer(Arc::new(Fake {
        snapshots: AtomicU32::new(0),
        git_missing: false,
    }));
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "checkpoint_edit");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    // The first call saw its own row; the second saw both.
    assert_eq!(
        tool_results(&events),
        vec![(true, "rows=1".to_string()), (true, "rows=2".to_string())]
    );
}

/// A mutating tool with unknown paths must not overlap another snapshot
/// window even when its advertised concurrency is parallel.
struct ParallelUnknown {
    active: Arc<AtomicU32>,
    max_active: Arc<AtomicU32>,
}

#[async_trait]
impl Tool for ParallelUnknown {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "unknown".into(),
            description: "parallel tool with unknown writes".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, _input: &Value) -> String {
        "unknown".into()
    }

    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolOutput {
            text: "done".into(),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[tokio::test]
async fn unknown_mutations_are_serialized_before_snapshotting() {
    let mut config = allow_all();
    config.permissions.allow.push("unknown".into());
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider = Arc::new(
        cox_provider::scripted::Scripted::from_toml(&scenario("checkpoint_parallel_unknown"), "")
            .expect("scenario"),
    );
    let store = Arc::new(cox_core::MemoryStore::new());
    let active = Arc::new(AtomicU32::new(0));
    let max_active = Arc::new(AtomicU32::new(0));
    let session = cox_core::Session::new(
        config,
        provider,
        vec![Arc::new(ParallelUnknown {
            active,
            max_active: max_active.clone(),
        })],
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "serialize");
    let _events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");

    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}
