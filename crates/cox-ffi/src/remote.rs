// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A remote host's workspace behind the FFI (T52.20): forwards to
//! `cox_app::remote`, which owns the ssh connection and the wire; this side
//! only moves the calls onto the runtime. Separate from `session.rs` because
//! a remote session is a different owner with the same calls, so the app's
//! `SessionClient` adapter stays one per kind.

use std::path::PathBuf;
use std::sync::Arc;

use cox_app::TimelinePatch;
use cox_app::remote::{RemoteSession, RemoteWorkspace};
use cox_app::{Block, Changes, Completion, Intent, Project, SearchHit, SessionEntry};
use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::types::TodoItem;

use crate::types::OpenRequest;
use crate::{AppError, on_runtime};

#[derive(uniffi::Object)]
pub struct RemoteHandle {
    remote: Arc<RemoteWorkspace>,
}

impl RemoteHandle {
    pub(crate) fn new(remote: Arc<RemoteWorkspace>) -> Arc<Self> {
        Arc::new(Self { remote })
    }
}

#[uniffi::export]
impl RemoteHandle {
    pub fn host(&self) -> String {
        self.remote.host().to_owned()
    }

    /// False once the ssh connection dropped; `reconnect` is the action.
    pub fn is_connected(&self) -> bool {
        self.remote.is_connected()
    }

    pub async fn reconnect(self: Arc<Self>) -> Result<(), AppError> {
        Ok(on_runtime(async move { self.remote.reconnect().await }).await??)
    }

    pub async fn projects(self: Arc<Self>, limit: u32) -> Result<Vec<Project>, AppError> {
        Ok(on_runtime(async move { self.remote.projects(i64::from(limit)).await }).await??)
    }

    pub async fn sessions(
        self: Arc<Self>,
        project: String,
        limit: u32,
    ) -> Result<Vec<SessionEntry>, AppError> {
        Ok(on_runtime(async move {
            (self.remote)
                .sessions(PathBuf::from(project), i64::from(limit))
                .await
        })
        .await??)
    }

    pub async fn search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<SearchHit>, AppError> {
        Ok(on_runtime(async move { self.remote.search(query, i64::from(limit)).await }).await??)
    }

    pub async fn open(
        self: Arc<Self>,
        request: OpenRequest,
    ) -> Result<Arc<RemoteSessionHandle>, AppError> {
        Ok(RemoteSessionHandle::new(
            on_runtime(async move {
                self.remote
                    .open(request.cwd.into(), request.resume, request.theme)
                    .await
            })
            .await??,
        ))
    }
}

#[derive(uniffi::Object)]
pub struct RemoteSessionHandle {
    remote: Arc<RemoteSession>,
}

impl RemoteSessionHandle {
    pub(crate) fn new(remote: Arc<RemoteSession>) -> Arc<Self> {
        Arc::new(Self { remote })
    }
}

#[uniffi::export]
impl RemoteSessionHandle {
    pub fn id(&self) -> SessionId {
        self.remote.id()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.remote.warnings().to_vec()
    }

    /// The blocks it opened with, for the first frame without a round trip.
    pub fn opened(&self) -> Vec<Block> {
        self.remote.opened().to_vec()
    }

    /// False once the connection dropped: the session shows disconnected.
    pub fn is_connected(&self) -> bool {
        self.remote.is_connected()
    }

    /// The next batch; `None` once closed or disconnected.
    pub async fn next_patches(self: Arc<Self>) -> Option<Vec<TimelinePatch>> {
        on_runtime(async move { self.remote.next_patches().await })
            .await
            .ok()
            .flatten()
    }

    pub async fn snapshot(self: Arc<Self>) -> Result<Vec<Block>, AppError> {
        Ok(on_runtime(async move { self.remote.snapshot().await }).await??)
    }

    /// Returns at once for a turn; a fork or handoff returns its child.
    pub async fn send(
        self: Arc<Self>,
        intent: Intent,
    ) -> Result<Option<Arc<RemoteSessionHandle>>, AppError> {
        Ok(on_runtime(async move { self.remote.send(intent).await })
            .await??
            .map(Self::new))
    }

    pub async fn complete(
        self: Arc<Self>,
        token: String,
        limit: u32,
    ) -> Result<Vec<Completion>, AppError> {
        Ok(on_runtime(async move { self.remote.complete(token, limit).await }).await??)
    }

    pub async fn changes(self: Arc<Self>) -> Result<Changes, AppError> {
        Ok(on_runtime(async move { self.remote.changes().await }).await??)
    }

    pub async fn plan(self: Arc<Self>) -> Result<Vec<TodoItem>, AppError> {
        Ok(on_runtime(async move { self.remote.plan().await }).await??)
    }

    /// A truncated output in full, as `cox expand` prints it.
    pub async fn output(self: Arc<Self>, archive: ArchiveId) -> Result<String, AppError> {
        Ok(on_runtime(async move { self.remote.output(archive).await }).await??)
    }

    /// Stops the stream; the remote session keeps running.
    pub async fn close(self: Arc<Self>) -> Result<(), AppError> {
        Ok(on_runtime(async move { self.remote.close().await }).await??)
    }
}
