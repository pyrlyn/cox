// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T26.2: `/rewind` restores pre-images through the checkpointer, cuts the
//! in-memory history without editing the rollout, and resume honours the
//! `Rewound` marker. T51.19: a hunk revert is checkpointed, confined,
//! refused on a stale digest and undone by `/redo`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{drain, open, scenario, spawn_turn};
use cox_core::History;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Archive, Store};
use cox_protocol::types::{CheckpointKind, Event, Level, Role, Submission, content_digest};
use cox_protocol::{
    Before, Change, Checkpointer, HunkReverter, PreImage, SkipReason, SkippedFile, Snapshot,
};

/// `a.rs` held "old" when the turn touched it and "now" by the time the
/// rewind looks; every restore is recorded.
#[derive(Default)]
struct Fake {
    reads: AtomicU32,
    restores: Mutex<Vec<(PathBuf, Option<Vec<u8>>)>>,
}

#[async_trait]
impl Checkpointer for Fake {
    async fn preimages(&self, roots: &[PathBuf], _cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        let first = self.reads.fetch_add(1, Ordering::SeqCst) == 0;
        paths
            .iter()
            .map(|p| PreImage {
                path: roots[0].join(p),
                before: if p.ends_with("a.rs") {
                    Before::Bytes(if first {
                        b"old".to_vec()
                    } else {
                        b"now".to_vec()
                    })
                } else {
                    Before::Absent
                },
            })
            .collect()
    }
    async fn snapshot(&self, _roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        Ok(Snapshot::default())
    }
    async fn changes(&self, _b: &Snapshot, _a: &Snapshot) -> Result<Vec<Change>, ToolError> {
        Ok(vec![])
    }
    async fn restore(
        &self,
        _roots: &[PathBuf],
        _cwd: &Path,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        self.restores
            .lock()
            .expect("lock")
            .push((path.to_path_buf(), bytes.map(<[u8]>::to_vec)));
        Ok(())
    }
}

fn allow_all() -> cox_protocol::Config {
    let mut config = cox_protocol::Config::default();
    config.permissions.allow = vec!["touch".into()];
    config
}

async fn turn(
    session: &cox_core::Session,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    text: &str,
) -> Vec<Event> {
    let running = spawn_turn(session, text);
    let events = drain(rx).await;
    running.await.expect("join").expect("turn");
    events
}

/// Every event up to the next `Rewound`, which is included.
async fn until_rewound(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("stream closed");
        let done = matches!(ev, Event::Notice { .. });
        out.push(ev);
        if done {
            break;
        }
    }
    out
}

#[tokio::test]
async fn rewind_code_restores_bytes() {
    let (session, store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let fake = Arc::new(Fake::default());
    session.set_checkpointer(fake.clone());
    turn(&session, &mut rx, "edit a.rs and new.rs").await;
    // The old pre-image, not today's bytes, is what a rewind writes back.
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    let pre = rows
        .iter()
        .find(|r| r.kind == CheckpointKind::Pre)
        .expect("pre row");
    assert_eq!(
        store
            .get(&pre.archive.expect("archived"))
            .await
            .expect("bytes"),
        b"old"
    );

    session
        .submit(Submission::Rewind {
            to_turn: 1,
            code: true,
            conversation: false,
        })
        .await
        .expect("rewind");
    let events = until_rewound(&mut rx).await;
    let Some(Event::Rewound {
        to_turn,
        restored,
        skipped,
        ..
    }) = events.iter().find(|e| matches!(e, Event::Rewound { .. }))
    else {
        panic!("no Rewound: {events:?}");
    };
    assert_eq!(*to_turn, 1);
    assert_eq!(
        restored,
        &vec![
            PathBuf::from("/tmp/cox-turn/a.rs"),
            PathBuf::from("/tmp/cox-turn/new.rs")
        ]
    );
    assert!(skipped.is_empty());
    assert_eq!(
        *fake.restores.lock().expect("lock"),
        vec![
            (PathBuf::from("/tmp/cox-turn/a.rs"), Some(b"old".to_vec())),
            (PathBuf::from("/tmp/cox-turn/new.rs"), None),
        ]
    );
    // The rewind's own pre-images landed under a new turn so it is undoable.
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    let rewind_rows: Vec<_> = rows.iter().filter(|r| r.turn == 2).collect();
    assert_eq!(rewind_rows[0].kind, CheckpointKind::Turn);
    assert_eq!(rewind_rows[1].kind, CheckpointKind::Pre);
    assert_eq!(rewind_rows[1].path, PathBuf::from("/tmp/cox-turn/a.rs"));
    assert!(rewind_rows[1].call.is_none());
}

#[tokio::test]
async fn rewind_conversation_is_append_only() {
    let (session, store, mut rx) = open(&scenario("rewind_two"), cox_protocol::Config::default());
    turn(&session, &mut rx, "one").await;
    turn(&session, &mut rx, "two").await;
    let before = store.rollout_read(&session.id()).expect("rollout").len();

    session
        .submit(Submission::Rewind {
            to_turn: 2,
            code: false,
            conversation: true,
        })
        .await
        .expect("rewind");
    let events = until_rewound(&mut rx).await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Rewound {
                to_turn: 2,
                conversation: true,
                ..
            }
        )),
        "{events:?}"
    );

    // Nothing before the marker changed; the marker is appended.
    let rollout = store.rollout_read(&session.id()).expect("rollout");
    assert!(rollout.len() > before);
    let users: Vec<_> = rollout
        .iter()
        .filter_map(|e| match e {
            Event::ItemStarted {
                kind: cox_protocol::types::ItemKind::UserMessage { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["one", "two"]);
    let last_user = rollout
        .iter()
        .rposition(|e| {
            matches!(
                e,
                Event::ItemStarted {
                    kind: cox_protocol::types::ItemKind::UserMessage { .. },
                    ..
                }
            )
        })
        .expect("user item");
    let rewound = rollout
        .iter()
        .position(|e| matches!(e, Event::Rewound { .. }))
        .expect("marker");
    assert!(rewound > last_user);

    // The next turn is T3 and the model sees only turn one before it.
    let events = turn(&session, &mut rx, "three").await;
    assert!(
        matches!(events.first(), Some(Event::TurnStarted { seq: 3, .. })),
        "{:?}",
        events.first()
    );
}

#[tokio::test]
async fn resume_after_rewind_stops_at_marker() {
    let (session, store, mut rx) = open(&scenario("rewind_two"), cox_protocol::Config::default());
    turn(&session, &mut rx, "one").await;
    turn(&session, &mut rx, "two").await;
    session
        .submit(Submission::Rewind {
            to_turn: 2,
            code: false,
            conversation: true,
        })
        .await
        .expect("rewind");
    until_rewound(&mut rx).await;
    turn(&session, &mut rx, "three").await;

    let history = History::from_events(&store.rollout_read(&session.id()).expect("rollout"));
    let users: Vec<String> = history
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .map(|m| format!("{:?}", m.content))
        .collect();
    assert_eq!(users.len(), 2, "{users:?}");
    assert!(
        users[0].contains("one") && users[1].contains("three"),
        "{users:?}"
    );
    assert_eq!(history.turns, 3);
    assert_eq!(
        history
            .turn_marks
            .iter()
            .map(|mark| (mark.seq, mark.message_index))
            .collect::<Vec<_>>(),
        vec![(1, 0), (3, 2)]
    );
}

#[tokio::test]
async fn rewind_refuses_unknown_turns_with_a_notice() {
    let (session, _store, mut rx) = open(&scenario("rewind_two"), cox_protocol::Config::default());
    turn(&session, &mut rx, "one").await;
    session
        .submit(Submission::Rewind {
            to_turn: 5,
            code: true,
            conversation: true,
        })
        .await
        .expect("submit");
    let events = until_rewound(&mut rx).await;
    let Some(Event::Notice {
        level: Level::Warn,
        text,
    }) = events.last()
    else {
        panic!("no warning: {events:?}");
    };
    assert_eq!(text, "rewind: no turn T5; this session has T1..T1");
}

/// A workspace that lives in a map: pre-images read it, restores write it,
/// so what a rewind and a redo leave behind can be compared byte for byte.
#[derive(Default)]
struct Disk {
    files: Mutex<std::collections::BTreeMap<PathBuf, Vec<u8>>>,
}

impl Disk {
    fn set(&self, files: &[(&str, &str)]) {
        *self.files.lock().expect("lock") = files
            .iter()
            .map(|(p, b)| {
                (
                    PathBuf::from("/tmp/cox-turn").join(p),
                    b.as_bytes().to_vec(),
                )
            })
            .collect();
    }

    fn snapshot(&self) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        self.files.lock().expect("lock").clone()
    }
}

#[async_trait]
impl Checkpointer for Disk {
    async fn preimages(&self, roots: &[PathBuf], _cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        let files = self.files.lock().expect("lock");
        paths
            .iter()
            // An escape is dropped, as `confine` drops it; an absolute path
            // inside the root (what `/redo` passes back) is kept.
            .filter(|p| {
                let path = Path::new(p);
                !p.starts_with("..") && (!path.is_absolute() || path.starts_with(&roots[0]))
            })
            .map(|p| {
                let path = roots[0].join(p);
                let before = match files.get(&path) {
                    Some(bytes) => Before::Bytes(bytes.clone()),
                    None => Before::Absent,
                };
                PreImage { path, before }
            })
            .collect()
    }
    async fn snapshot(&self, _roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        Ok(Snapshot::default())
    }
    async fn changes(&self, _b: &Snapshot, _a: &Snapshot) -> Result<Vec<Change>, ToolError> {
        Ok(vec![])
    }
    async fn restore(
        &self,
        _roots: &[PathBuf],
        _cwd: &Path,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        let mut files = self.files.lock().expect("lock");
        match bytes {
            Some(b) => files.insert(path.to_path_buf(), b.to_vec()),
            None => files.remove(path),
        };
        Ok(())
    }
}

/// Every event up to the next `Notice`, returned as that notice's text.
async fn notice(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> String {
    match until_rewound(rx).await.pop() {
        Some(Event::Notice { text, .. }) => text,
        other => panic!("no notice: {other:?}"),
    }
}

/// T26.4: `/undo` is a code-only rewind of the last turn; `/redo` right
/// after it puts back exactly what was there, and only once.
#[tokio::test]
async fn undo_then_redo_is_identity() {
    let (session, _store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = Arc::new(Disk::default());
    disk.set(&[("a.rs", "old")]);
    session.set_checkpointer(disk.clone());
    turn(&session, &mut rx, "edit a.rs and new.rs").await;
    // What the turn's tools wrote; `touch` itself writes nothing here.
    disk.set(&[("a.rs", "now"), ("new.rs", "fresh")]);
    let edited = disk.snapshot();

    session.submit(Submission::Redo).await.expect("redo");
    assert!(notice(&mut rx).await.starts_with("redo: nothing to redo"));
    assert_eq!(disk.snapshot(), edited, "a refused redo writes nothing");

    session
        .submit(Submission::Rewind {
            to_turn: 1,
            code: true,
            conversation: false,
        })
        .await
        .expect("undo");
    notice(&mut rx).await;
    assert_eq!(
        disk.snapshot(),
        [(PathBuf::from("/tmp/cox-turn/a.rs"), b"old".to_vec())].into()
    );

    session.submit(Submission::Redo).await.expect("redo");
    assert!(notice(&mut rx).await.starts_with("rewound"));
    assert_eq!(disk.snapshot(), edited);

    session.submit(Submission::Redo).await.expect("redo twice");
    assert!(notice(&mut rx).await.starts_with("redo: nothing to redo"));
    assert_eq!(
        disk.snapshot(),
        edited,
        "a second redo does not undo the first"
    );
}

/// `a.rs` is over the size cap and `new.rs` cannot be written back.
struct Stuck;

#[async_trait]
impl Checkpointer for Stuck {
    async fn preimages(&self, roots: &[PathBuf], _cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        paths
            .iter()
            .map(|p| PreImage {
                path: roots[0].join(p),
                before: if p.ends_with("a.rs") {
                    Before::TooLarge
                } else {
                    Before::Bytes(b"x".to_vec())
                },
            })
            .collect()
    }
    async fn snapshot(&self, _roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        Ok(Snapshot::default())
    }
    async fn changes(&self, _b: &Snapshot, _a: &Snapshot) -> Result<Vec<Change>, ToolError> {
        Ok(vec![])
    }
    async fn restore(
        &self,
        _roots: &[PathBuf],
        _cwd: &Path,
        _path: &Path,
        _bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        Err(ToolError::Io)
    }
}

/// A101: each file a rewind could not restore says why, and the notice
/// counts them by reason instead of calling every failure too large.
#[tokio::test]
async fn a_skipped_restore_carries_its_reason() {
    let (session, _store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    session.set_checkpointer(Arc::new(Stuck));
    turn(&session, &mut rx, "edit a.rs and new.rs").await;
    session
        .submit(Submission::Rewind {
            to_turn: 1,
            code: true,
            conversation: false,
        })
        .await
        .expect("rewind");
    let events = until_rewound(&mut rx).await;
    let Some(Event::Rewound { skipped, .. }) =
        events.iter().find(|e| matches!(e, Event::Rewound { .. }))
    else {
        panic!("no Rewound: {events:?}");
    };
    assert_eq!(
        skipped,
        &vec![
            SkippedFile {
                path: PathBuf::from("/tmp/cox-turn/a.rs"),
                reason: SkipReason::TooLarge,
            },
            SkippedFile {
                path: PathBuf::from("/tmp/cox-turn/new.rs"),
                reason: SkipReason::Failed {
                    error: "io error".into()
                },
            },
        ]
    );
    let Some(Event::Notice { text, .. }) = events.last() else {
        panic!("no notice: {events:?}");
    };
    assert_eq!(
        text,
        "rewound to T1: 0 files restored, 1 too large to restore, 1 failed: io error"
    );
}

/// T37.28.3: a revert of a file no turn from `to_turn` on touched writes
/// nothing and says so; one it did touch goes back alone.
#[tokio::test]
async fn revert_file_restores_only_that_file() {
    let (session, _store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = Arc::new(Disk::default());
    disk.set(&[("a.rs", "old")]);
    session.set_checkpointer(disk.clone());
    turn(&session, &mut rx, "edit a.rs and new.rs").await;
    disk.set(&[("a.rs", "now"), ("new.rs", "fresh")]);

    let revert = |path: &str| Submission::RevertFile {
        path: path.into(),
        to_turn: 1,
    };
    session.submit(revert("other.rs")).await.expect("revert");
    assert_eq!(
        notice(&mut rx).await,
        "revert: other.rs has no checkpoint from T1 on; left as it is"
    );
    session.submit(revert("a.rs")).await.expect("revert");
    assert_eq!(notice(&mut rx).await, "reverted a.rs to before T1");
    assert_eq!(
        disk.snapshot(),
        [
            (PathBuf::from("/tmp/cox-turn/a.rs"), b"old".to_vec()),
            (PathBuf::from("/tmp/cox-turn/new.rs"), b"fresh".to_vec()),
        ]
        .into()
    );
}

/// Every change is one hunk, index 0: enough to drive the core, whose job
/// is the checks and the checkpoint, not the diff (cox-render's).
struct OneHunk;

impl HunkReverter for OneHunk {
    fn revert(&self, before: &str, now: &str, index: usize) -> Option<String> {
        (index == 0 && before != now).then(|| before.to_owned())
    }
}

/// A session whose turn changed `a.rs` from "old" to "now" and created
/// `new.rs`, with the disk it wrote.
async fn edited(
    session: &cox_core::Session,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
) -> Arc<Disk> {
    let disk = Arc::new(Disk::default());
    disk.set(&[("a.rs", "old")]);
    session.set_checkpointer(disk.clone());
    session.set_hunk_reverter(Arc::new(OneHunk));
    turn(session, rx, "edit a.rs and new.rs").await;
    disk.set(&[("a.rs", "now"), ("new.rs", "fresh")]);
    disk
}

/// Hunk 0 of `path` as Review showed it over `shown`.
fn revert_hunk(path: &str, shown: &str) -> Submission {
    Submission::RevertHunk {
        path: path.into(),
        to_turn: 1,
        hunk: 0,
        now_digest: content_digest(shown.as_bytes()),
    }
}

fn a_rs() -> PathBuf {
    PathBuf::from("/tmp/cox-turn/a.rs")
}

#[tokio::test]
async fn revert_hunk_checkpoints_before_writing() {
    let (session, store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = edited(&session, &mut rx).await;
    session
        .submit(revert_hunk("a.rs", "now"))
        .await
        .expect("revert");
    assert_eq!(notice(&mut rx).await, "reverted hunk 0 of a.rs");
    assert_eq!(disk.snapshot()[&a_rs()], b"old");
    // The bytes it overwrote are kept under a turn of the revert's own.
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    let own: Vec<_> = rows.iter().filter(|r| r.turn == 2).collect();
    assert_eq!(own[0].kind, CheckpointKind::Turn);
    assert_eq!(own[1].path, a_rs());
    assert!(own[1].call.is_none());
    let kept = store
        .get(&own[1].archive.expect("archived"))
        .await
        .expect("bytes");
    assert_eq!(kept, b"now");
}

#[tokio::test]
async fn revert_hunk_outside_the_workspace_is_refused() {
    let (session, _store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = edited(&session, &mut rx).await;
    let before = disk.snapshot();
    session
        .submit(revert_hunk("../etc/passwd", ""))
        .await
        .expect("revert");
    assert_eq!(
        notice(&mut rx).await,
        "revert hunk: ../etc/passwd is outside the workspace roots or cannot be read"
    );
    assert_eq!(disk.snapshot(), before);
}

#[tokio::test]
async fn revert_hunk_with_a_stale_digest_is_refused() {
    let (session, store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = edited(&session, &mut rx).await;
    let before = disk.snapshot();
    // Review diffed bytes the file no longer holds.
    session
        .submit(revert_hunk("a.rs", "what Review saw"))
        .await
        .expect("revert");
    assert_eq!(
        notice(&mut rx).await,
        "revert hunk: a.rs changed since Review showed it; review it again"
    );
    assert_eq!(disk.snapshot(), before);
    let rows = store.checkpoint_list(&session.id()).expect("rows");
    assert!(
        rows.iter().all(|r| r.turn < 2),
        "a refusal checkpoints nothing"
    );
}

#[tokio::test]
async fn revert_hunk_is_undone_by_redo() {
    let (session, _store, mut rx) = open(&scenario("checkpoint_edit"), allow_all());
    let disk = edited(&session, &mut rx).await;
    let before = disk.snapshot();
    session
        .submit(revert_hunk("a.rs", "now"))
        .await
        .expect("revert");
    notice(&mut rx).await;
    assert_eq!(disk.snapshot()[&a_rs()], b"old");
    session.submit(Submission::Redo).await.expect("redo");
    assert!(notice(&mut rx).await.starts_with("rewound"));
    assert_eq!(disk.snapshot(), before);
}
