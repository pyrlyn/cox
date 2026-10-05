// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Shared fixtures for the cox-tools integration tests: an archive that
//! keeps nothing and a `ToolCx` over one root, so each test file states
//! only the policy it is about.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use cox_protocol::{
    Archive, ArchiveId, ArchivePut, CallId, SandboxMode, SandboxPolicy, SessionId, StoreError,
    ToolCx,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct NoopArchive;

#[async_trait::async_trait]
impl Archive for NoopArchive {
    async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
        Ok(ArchiveId::new())
    }
    async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        Ok(Vec::new())
    }
}

/// The default `[sandbox]` shape for `mode`: no network, `.git`/`.cox`
/// read-only inside the workspace.
pub fn policy(mode: SandboxMode) -> SandboxPolicy {
    SandboxPolicy {
        mode,
        network: false,
        writable: vec![],
        readonly_in_workspace: vec![PathBuf::from(".git"), PathBuf::from(".cox")],
        linux_backend: Default::default(),
    }
}

/// A `ToolCx` whose only root is also its cwd, plus the receiver for what
/// the tool streams.
pub fn cx(
    root: PathBuf,
    sandbox: SandboxPolicy,
    cancel: CancellationToken,
) -> (ToolCx, mpsc::Receiver<String>) {
    let (tx, rx) = mpsc::channel(256);
    let cx = cox_tools::tool_cx(
        vec![root.clone()],
        root,
        sandbox,
        Arc::new(NoopArchive),
        cancel,
        tx,
        SessionId::new(),
        CallId::new(),
    );
    (cx, rx)
}
