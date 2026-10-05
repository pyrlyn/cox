// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Built-in tools: read, edit, write, bash, grep, glob, outline, web, todo,
//! ask_user, agent, send_message. Separate from `cox-core` because every tool touches the
//! filesystem or a process and must go through a trait, never called
//! directly by the loop. The same rule puts the `/rewind` pre-image reader
//! (`checkpoint`) here. The sandbox (Seatbelt, Landlock/bwrap) and
//! `path::confine` live in `cox-sandbox` (T32.3), re-exported here at their
//! old paths. Tree-sitter and its grammars live in `cox-syntax` (T32.4);
//! `outline` is re-exported the same way, and `bash/classify.rs` calls into
//! it for parsing while keeping its own risk walk here. The pure `grep`/
//! `glob` walk and match engine lives in `cox-search` (T32.5); the `Tool`
//! impls (`path::confine`, archiving) stay here. `web_fetch`'s HTTP
//! GET and HTML→text engine live in `cox-web` (T32.7); `web_fetch.rs` keeps
//! only the `Tool` glue (`ToolCx`, input parsing, output framing).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod ask_user;
pub mod bash;
pub mod checkpoint;
pub mod edit;
pub mod expand;
pub mod git;
pub mod glob;
pub mod grep;
pub mod lsp;
pub mod memory;
pub mod read;
pub mod repomap;
pub mod send_message;
pub mod todo;
pub mod tool_search;
pub mod v4a;
pub mod web_fetch;
pub mod write;

/// T32.3: `path::confine` and `sandbox` moved to their own crate (dependency
/// (a): `landlock`/`seccompiler` are Linux-only, and guard (b) —
/// `docs/design/crates.md`); re-exported here at the old paths so
/// `cox_tools::path::confine` and `cox_tools::sandbox::Policy` keep working
/// for existing callers.
pub use cox_sandbox::path;
pub use cox_sandbox::sandbox;

/// T32.4: `outline` (AST signature extraction) moved to `cox-syntax`
/// (dependency (a): tree-sitter and its five grammars are each a C build);
/// re-exported here at the old path so `cox_tools::outline::outline` keeps
/// working for existing callers (`read.rs`).
pub use cox_syntax::outline;

use std::path::PathBuf;
use std::sync::Arc;

use cox_protocol::{Archive, CallId, SandboxPolicy, SessionId, ToolCx};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Assembles a `ToolCx` from its parts. Every field on `ToolCx`
/// (`cox-protocol::traits`) is already `pub`, so this is a thin, named
/// constructor rather than a real builder — it exists so callers (session
/// setup in `cox-core`, tool tests here) have one place to look instead of
/// repeating the struct literal. Wiring this up from live session config
/// (`T2.2`/`T0.3`) is a later, separate task: every argument here is a
/// plain value the caller must already have in hand.
#[allow(clippy::too_many_arguments)]
pub fn tool_cx(
    roots: Vec<PathBuf>,
    cwd: PathBuf,
    sandbox: SandboxPolicy,
    archive: Arc<dyn Archive>,
    cancel: CancellationToken,
    output: mpsc::Sender<String>,
    session: SessionId,
    call: CallId,
) -> ToolCx {
    let writable_roots = roots.clone();
    ToolCx {
        roots,
        writable_roots,
        cwd,
        sandbox,
        archive,
        cancel,
        output,
        session,
        call,
        // T34.3: callers that need a labelled child `ToolCx` build it with
        // `..tool_cx(...)` struct-update syntax rather than a new
        // constructor arg here — every existing caller stays unchanged.
        agent: None,
        preset: None,
        // T34.6: no session builds this bare helper's caller, so no relay;
        // real per-call binding happens in `cox-core/src/turn.rs`.
        relay: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cox_protocol::{ArchiveId, ArchivePut, SandboxMode, StoreError};

    use super::*;

    struct NoopArchive;

    #[async_trait::async_trait]
    impl Archive for NoopArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn tool_cx_wires_every_field_through() {
        let (tx, _rx) = mpsc::channel(1);
        let roots = vec![PathBuf::from("/tmp/root")];
        let cwd = PathBuf::from("/tmp/root");
        let sandbox = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        };
        let session = SessionId::new();
        let call = CallId::new();

        let cx = tool_cx(
            roots.clone(),
            cwd.clone(),
            sandbox,
            Arc::new(NoopArchive),
            CancellationToken::new(),
            tx,
            session,
            call,
        );

        assert_eq!(cx.roots, roots);
        assert_eq!(cx.cwd, Path::new("/tmp/root"));
        assert_eq!(cx.session, session);
        assert_eq!(cx.call, call);
    }
}
