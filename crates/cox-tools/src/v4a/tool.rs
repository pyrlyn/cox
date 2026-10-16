// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `apply_patch`'s `Tool` impl: resolves every patch path through
//! `crate::path::confine` before touching disk, archives what a hunk is
//! about to overwrite, then writes through `crate::write::atomic_write`.
//! Parsing, hunk matching and staging are `cox-patch` (T32.6) — this module
//! is the one place that talks to the filesystem and the trust guards it
//! needs stay in `cox-tools` (docs/design/crates.md, AGENTS.md trust
//! boundaries).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use cox_patch::{Op, Patch, parse, stage};
use cox_protocol::{
    ArchivePut, Concurrency, Diff, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec,
};
use serde_json::{Value, json};
use similar::TextDiff;

use crate::open::read_confined;
use crate::path::confine;
use crate::write::{atomic_write, str_field};

/// Past this many deletions one patch stops being an edit and starts being
/// a way to lose work, so it is escalated to `Risk::Destructive` and the
/// permission engine asks (plan.md §4 tool table, T3.5 step 4).
const DESTRUCTIVE_DELETES: usize = 5;

/// `apply_patch`: Codex's V4A grammar as a tool.
pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".to_string(),
            description: "Apply a V4A patch. `patch` is a `*** Begin Patch` … `*** End \
                 Patch` document containing `*** Add File: p` (with `+` lines), `*** \
                 Delete File: p`, and `*** Update File: p` (optionally followed by `*** \
                 Move to: q`) with `@@ context` hunks whose lines start with a space, \
                 `-` or `+`, and may end with `*** End of File`. Hunks are matched \
                 exactly first, then ignoring trailing whitespace, then ignoring all \
                 whitespace; a hunk that matches nowhere or in more than one place \
                 fails the whole patch and nothing is written. Errors: `invalid patch \
                 at line N: <why>`, `denied: hunk N <why>`, `not found` (an updated or \
                 deleted file does not exist), `io error`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "The V4A patch document."
                    }
                },
                "required": ["patch"]
            }),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }

    /// A patch that removes more than [`DESTRUCTIVE_DELETES`] files is
    /// escalated past the tool's default `Write`. An unparseable patch
    /// keeps the default: it will be rejected by `call` before it can do
    /// anything, and guessing a risk from text that has no grammar would
    /// be worse than declining to guess.
    fn risk(&self, input: &Value) -> Risk {
        match input.get("patch").and_then(Value::as_str).map(parse) {
            Some(Ok(p)) if p.deletes() > DESTRUCTIVE_DELETES => Risk::Destructive,
            _ => self.spec().risk,
        }
    }

    /// Every path the patch writes: each op's path plus a move's target.
    /// An unparseable patch touches nothing — `call` refuses it first.
    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        let patch = input
            .get("patch")
            .and_then(Value::as_str)
            .map(parse)?
            .ok()?;
        let mut paths = Vec::new();
        for op in &patch.ops {
            paths.push(op.path().to_string());
            if let Op::Update {
                move_to: Some(to), ..
            } = op
            {
                paths.push(to.clone());
            }
        }
        Some(paths)
    }

    fn subject(&self, input: &Value) -> String {
        let Some(Ok(patch)) = input.get("patch").and_then(Value::as_str).map(parse) else {
            return String::new();
        };
        patch
            .ops
            .iter()
            .map(|op| op.path().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let patch = parse(&str_field(&input, "patch")?)?;

        // Confine every path up front: a patch that escapes the workspace
        // is refused before a single hunk is resolved.
        let mut resolved = BTreeMap::new();
        for op in &patch.ops {
            let path = op.path().to_string();
            resolved.insert(path.clone(), confine(&cx.writable_roots, &cx.cwd, &path)?);
            if let Op::Update {
                move_to: Some(to), ..
            } = op
            {
                resolved.insert(to.clone(), confine(&cx.writable_roots, &cx.cwd, to)?);
            }
        }

        apply_at(&patch, &resolved, cx).await
    }
}

/// Everything after `confine`: its own function so a test can swap a path
/// for a symlink between the two, the window T64.15 closes.
async fn apply_at(
    patch: &Patch,
    resolved: &BTreeMap<String, PathBuf>,
    cx: &ToolCx,
) -> Result<ToolOutput, ToolError> {
    // `stage` sees a read as present or absent, so a refused open is
    // kept aside and wins over whatever `stage` makes of the miss. The
    // block ends the closure's borrow before the first await below.
    let changes = {
        let refused = RefCell::new(None);
        let read = |p: &str| {
            let full = resolved.get(p)?;
            match read_confined(&cx.writable_roots, full) {
                Ok(bytes) => String::from_utf8(bytes).ok(),
                Err(e @ ToolError::Confined { .. }) => {
                    refused.borrow_mut().get_or_insert(e);
                    None
                }
                Err(_) => None,
            }
        };
        let staged = stage(patch, &read);
        if let Some(e) = refused.into_inner() {
            return Err(e);
        }
        staged?
    };

    // Archive what is about to be lost before anything on disk moves,
    // so `cox expand` can restore it (AGENTS.md: lossless by default).
    for c in changes.iter().filter(|c| c.before.is_some()) {
        let Some(before) = &c.before else { continue };
        cx.archive
            .put(ArchivePut {
                session: cx.session,
                call: cx.call,
                tool: "apply_patch".to_string(),
                subject: Some(c.from.clone().unwrap_or_else(|| c.path.clone())),
                bytes: before.clone().into_bytes(),
            })
            .await
            .map_err(|_| ToolError::Io)?;
    }

    let mut summary = Vec::new();
    let mut unified = String::new();
    for c in &changes {
        let target = resolved.get(&c.path).ok_or(ToolError::Io)?;
        match &c.after {
            Some(after) => atomic_write(&cx.writable_roots, target, after.as_bytes())?,
            None => std::fs::remove_file(target).map_err(|_| ToolError::Io)?,
        }
        // A move is a write to the new path plus a removal of the old.
        if let Some(from) = &c.from {
            let old = resolved.get(from).ok_or(ToolError::Io)?;
            std::fs::remove_file(old).map_err(|_| ToolError::Io)?;
        }
        summary.push(match &c.from {
            Some(from) => format!("{} {from} -> {}", c.status(), c.path),
            None => format!("{} {}", c.status(), c.path),
        });
        unified.push_str(
            &TextDiff::from_lines(
                c.before.as_deref().unwrap_or_default(),
                c.after.as_deref().unwrap_or_default(),
            )
            .unified_diff()
            .context_radius(3)
            .header(&c.from.clone().unwrap_or_else(|| c.path.clone()), &c.path)
            .to_string(),
        );
    }

    Ok(ToolOutput {
        text: summary.join("\n"),
        is_error: false,
        diff: changes.first().map(|c| Diff {
            path: std::path::PathBuf::from(&c.path),
            unified,
        }),
        structured: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4a_risk_escalates_past_five_deletes() {
        let deletes = |n: usize| {
            let body: String = (0..n).map(|i| format!("*** Delete File: f{i}\n")).collect();
            json!({ "patch": format!("*** Begin Patch\n{body}*** End Patch") })
        };
        assert_eq!(ApplyPatchTool.risk(&deletes(5)), Risk::Write);
        assert_eq!(ApplyPatchTool.risk(&deletes(6)), Risk::Destructive);
        assert_eq!(
            ApplyPatchTool.risk(&json!({ "patch": "not a patch" })),
            Risk::Write,
            "an unparseable patch keeps the default risk"
        );
    }

    #[cfg(unix)]
    fn swap_cx(swap: &crate::swap_fixture::Swap) -> ToolCx {
        use std::sync::Arc;

        use cox_protocol::{ArchiveId, SandboxMode, SandboxPolicy, SessionId, StoreError};

        struct NoopArchive;

        #[async_trait]
        impl cox_protocol::Archive for NoopArchive {
            async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
                Ok(ArchiveId::new())
            }
            async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
                Ok(Vec::new())
            }
        }

        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        crate::tool_cx(
            vec![swap.root.clone()],
            swap.root.clone(),
            SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            Arc::new(NoopArchive),
            tokio_util::sync::CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_patch_refuses_a_path_swapped_to_a_symlink_after_confine() {
        let swap = crate::swap_fixture::Swap::new();
        swap.file("secret.txt", "mine\n");
        swap.file("sub/secret.txt", "mine\n");
        let cx = swap_cx(&swap);
        let mut resolved = BTreeMap::new();
        for rel in ["secret.txt", "sub/secret.txt"] {
            resolved.insert(
                rel.to_string(),
                confine(&cx.writable_roots, &cx.cwd, rel).expect("confine"),
            );
        }
        swap.swap_leaf("secret.txt");
        swap.swap_parent("sub");

        for rel in ["secret.txt", "sub/secret.txt"] {
            let patch = parse(&format!(
                "*** Begin Patch\n*** Update File: {rel}\n@@\n-secret\n+changed\n*** End Patch"
            ))
            .expect("parse");
            let err = apply_at(&patch, &resolved, &cx)
                .await
                .expect_err("must refuse");
            assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
        }
        assert_eq!(swap.outside_secret(), crate::swap_fixture::SECRET);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_patch_add_file_refuses_a_parent_swapped_to_a_symlink_after_confine() {
        let swap = crate::swap_fixture::Swap::new();
        swap.file("sub/keep.txt", "mine\n");
        let cx = swap_cx(&swap);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            "sub/new.txt".to_string(),
            confine(&cx.writable_roots, &cx.cwd, "sub/new.txt").expect("confine"),
        );
        swap.swap_parent("sub");

        let patch = parse("*** Begin Patch\n*** Add File: sub/new.txt\n+changed\n*** End Patch")
            .expect("parse");
        let err = apply_at(&patch, &resolved, &cx)
            .await
            .expect_err("must refuse");
        assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
        assert!(!swap.outside.join("new.txt").exists());
    }
}
