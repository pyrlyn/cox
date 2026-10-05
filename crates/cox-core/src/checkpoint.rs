// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! When the loop takes a checkpoint (T26.1): before a `Write`/`Destructive`
//! call whose tool names its paths (`Tool::touches`), and around every other
//! call that can change the workspace (a shell, an MCP tool) by comparing
//! two workspace snapshots. Separate from `turn.rs` because it owns one
//! ordering rule — the archive row and the `checkpoints` row exist before
//! the call runs or the event is emitted (lossless) — and one failure rule:
//! no checkpointer, no git or a store error means no checkpoint, warned
//! once per session, never a failed turn. Reading files and running git is
//! the `Checkpointer`'s job (`cox-tools`); this module only decides when.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use cox_protocol::ids::{CallId, TurnId};
use cox_protocol::traits::Tool;
use cox_protocol::types::{CheckpointFile, CheckpointKind, Event, Level, Risk};
use cox_protocol::{ArchivePut, Before, CheckpointRow, Snapshot};
use serde_json::Value;

use crate::session::Session;

/// What `before` leaves for `after` to finish.
pub(crate) enum Pending {
    /// Nothing more to do: no checkpointer, a read-only call, or the
    /// pre-images were already taken from the named paths.
    Done,
    /// The workspace before the call; `after` diffs a fresh snapshot
    /// against it.
    Snapshot(Snapshot),
}

/// Runs right before a tool call. Named paths are read now so the
/// pre-image is archived before the write happens.
pub(crate) async fn before(
    session: &Session,
    turn: TurnId,
    call: CallId,
    tool: &dyn Tool,
    input: &Value,
) -> Pending {
    let Some(cp) = session.checkpointer() else {
        return Pending::Done;
    };
    if tool.risk(input) == Risk::ReadOnly {
        return Pending::Done;
    }
    let roots = session.writable_roots();
    if let Some(paths) = tool.touches(input) {
        let files = cp
            .preimages(roots, &session.cwd, &paths)
            .await
            .into_iter()
            .map(|p| (p.path, false, p.before))
            .collect();
        record(session, turn, call, files).await;
        return Pending::Done;
    }
    match cp.snapshot(roots).await {
        Ok(snapshot) => Pending::Snapshot(snapshot),
        Err(e) => {
            warn_once(session, &e.to_string()).await;
            Pending::Done
        }
    }
}

/// Runs after the call returned: anything the call changed that no path
/// named (a shell's `rm`, an MCP tool's write) gets its pre-image from the
/// earlier snapshot.
pub(crate) async fn after(session: &Session, turn: TurnId, call: CallId, pending: Pending) {
    let Pending::Snapshot(earlier) = pending else {
        return;
    };
    let Some(cp) = session.checkpointer() else {
        return;
    };
    let roots = session.writable_roots();
    let changes = async {
        let later = cp.snapshot(roots).await?;
        cp.changes(&earlier, &later).await
    };
    match changes.await {
        Ok(changes) => {
            let files = changes
                .into_iter()
                .map(|c| (c.path, c.deleted, c.before))
                .collect();
            record(session, turn, call, files).await;
        }
        Err(e) => warn_once(session, &e.to_string()).await,
    }
}

/// One marker row per user turn so `/rewind` has a timeline even for turns
/// that changed nothing. Best-effort, like every index write.
pub(crate) fn mark_turn(session: &Session, seq: u32) {
    let _ = session.store.checkpoint_insert(&CheckpointRow {
        session: session.id,
        turn: seq,
        call: None,
        path: PathBuf::new(),
        kind: CheckpointKind::Turn,
        archive: None,
    });
}

/// Archives each pre-image, inserts its row, and only then emits the event
/// — a file whose row failed is left out of the event rather than promised.
async fn record(
    session: &Session,
    turn: TurnId,
    call: CallId,
    files: Vec<(PathBuf, bool, Before)>,
) {
    if files.is_empty() {
        return;
    }
    let seq = session.inner.lock().await.turn_seq;
    let emitted = store_rows(session, seq, Some(call), files).await;
    if !emitted.is_empty() {
        let _ = session
            .emit(Event::Checkpoint {
                turn,
                call: Some(call),
                files: emitted,
            })
            .await;
    }
}

/// The rows and archive entries for `files` under turn `seq`; what came
/// back is what exists. A rewind's own writes go through here too, with no
/// call and no event.
pub(crate) async fn store_rows(
    session: &Session,
    seq: u32,
    call: Option<CallId>,
    files: Vec<(PathBuf, bool, Before)>,
) -> Vec<CheckpointFile> {
    let mut emitted = Vec::new();
    for (path, deleted, before) in files {
        let kind = match (&before, deleted) {
            (_, true) => CheckpointKind::Deleted,
            (Before::Absent, false) => CheckpointKind::Created,
            _ => CheckpointKind::Pre,
        };
        let archive = match before {
            Before::Bytes(bytes) => {
                let put = ArchivePut {
                    session: session.id,
                    // A rewind has no call; its rows are found by turn.
                    call: call.unwrap_or_default(),
                    tool: "checkpoint".into(),
                    subject: Some(path.display().to_string()),
                    bytes,
                };
                match session.archive.put(put).await {
                    Ok(id) => Some(id),
                    Err(e) => {
                        warn_once(session, &e.to_string()).await;
                        continue;
                    }
                }
            }
            Before::Absent | Before::TooLarge => None,
        };
        let row = CheckpointRow {
            session: session.id,
            turn: seq,
            call,
            path: path.clone(),
            kind,
            archive,
        };
        if let Err(e) = session.store.checkpoint_insert(&row) {
            warn_once(session, &e.to_string()).await;
            continue;
        }
        emitted.push(CheckpointFile { path, kind });
    }
    emitted
}

/// The first failure is loud, the rest are silent: a missing `git` would
/// otherwise repeat on every call.
async fn warn_once(session: &Session, detail: &str) {
    if session.checkpoint_warned.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = session
        .emit(Event::Notice {
            level: Level::Warn,
            text: format!("checkpoints off for this session ({detail}); /rewind cannot restore files — is git installed?"),
        })
        .await;
}
