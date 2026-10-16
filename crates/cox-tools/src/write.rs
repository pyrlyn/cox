// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `write`: create a new file, or fully replace a small one (plan.md D8,
//! T3.6). `write` is deliberately not an edit tool — rewriting an existing
//! file over 200 lines is refused with a hint toward `edit`/`apply_patch`,
//! since a whole-file rewrite of anything larger throws away far more
//! diff-review value than it saves (D8's "5-20x fewer output tokens"
//! reasoning for edit-shaped tools applies in reverse here). Every path
//! goes through `cox_tools::path::confine` first (AGENTS.md trust
//! boundary); the pre-write content (empty, for a new file) is archived
//! before anything on disk changes, so `cox expand` can restore it.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use cox_protocol::{ArchivePut, Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::open::read_confined;
use crate::path::confine;

/// A file over this many lines is not a `write` target — plan.md §1.11:
/// "existing file > 200 lines -> error \"use edit\"".
const MAX_REWRITE_LINES: usize = 200;

/// Pulls a required string field out of a tool input `Value`. Shared by
/// `edit`/`write`/`todo` input parsing so a missing/mistyped field always
/// produces the same `ToolError::Denied` shape instead of three slightly
/// different ones.
pub(crate) fn str_field(input: &Value, field: &str) -> Result<String, ToolError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::Denied {
            why: format!("missing or non-string \"{field}\" field"),
        })
}

/// Writes `bytes` to `path` atomically: write a sibling temp file in the
/// same directory, then rename over the target. A rename within one
/// filesystem is atomic, so a crash mid-write never leaves a half-written
/// file at `path` — the temp file must be a sibling (not `/tmp`) because a
/// rename across filesystems is neither atomic nor guaranteed to succeed.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let tmp = stage(path, bytes)?;
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Io
    })
}

/// Writes `bytes` to a fresh sibling temp file of `path` and returns it,
/// the first half of [`atomic_write`].
fn stage(path: &Path, bytes: &[u8]) -> Result<PathBuf, ToolError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|_| ToolError::Io)?;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cox-write");
    let tmp = dir.join(format!(".{name}.cox-tmp-{pid}-{nanos}"));

    std::fs::write(&tmp, bytes).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Io
    })?;
    Ok(tmp)
}

/// One file a multi-file change rewrites (`rename_symbol`, T59.7).
pub(crate) struct Rewrite {
    /// The confined absolute path.
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

/// Writes every rewrite or none: each pre-image is archived first (lossless
/// by default), every new text is staged in a sibling temp file before any
/// target is replaced, so a full disk fails with nothing changed, and a
/// replace that fails puts back the files already replaced.
pub(crate) async fn rewrite_all(
    tool: &str,
    rewrites: &[Rewrite],
    cx: &ToolCx,
) -> Result<(), ToolError> {
    for r in rewrites {
        cx.archive
            .put(ArchivePut {
                session: cx.session,
                call: cx.call,
                tool: tool.to_string(),
                // The archive row `edit` writes for the same file.
                subject: Some(r.path.display().to_string()),
                bytes: r.before.clone().into_bytes(),
            })
            .await
            .map_err(|_| ToolError::Io)?;
    }
    let mut staged = Vec::with_capacity(rewrites.len());
    for r in rewrites {
        match stage(&r.path, r.after.as_bytes()) {
            Ok(tmp) => staged.push(tmp),
            Err(e) => {
                discard(&staged);
                return Err(e);
            }
        }
    }
    for (i, (r, tmp)) in rewrites.iter().zip(&staged).enumerate() {
        if std::fs::rename(tmp, &r.path).is_err() {
            discard(&staged[i..]);
            for done in &rewrites[..i] {
                // Best effort: the archive above still holds each pre-image.
                let _ = atomic_write(&done.path, done.before.as_bytes());
            }
            return Err(ToolError::Io);
        }
    }
    Ok(())
}

fn discard(temps: &[PathBuf]) {
    for t in temps {
        let _ = std::fs::remove_file(t);
    }
}

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_string(),
            description: format!(
                "Create a new file, or fully overwrite a small existing one, at `path` \
                 with `content`. Parent directories are created as needed and the write \
                 is atomic. Refuses to overwrite an existing file with more than \
                 {MAX_REWRITE_LINES} lines: returns `denied: file has <N> lines; use edit \
                 or apply_patch` — use `edit` for a targeted change to a large file. Other \
                 errors this tool may return: `path {{path}} escapes workspace root \
                 {{root}}` (path outside the workspace), `binary file` (the existing file \
                 at `path` is not valid UTF-8 text), `io error`."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File to write, relative to the workspace root."
                    },
                    "content": {
                        "type": "string",
                        "description": "The full file content to write."
                    }
                },
                "required": ["path", "content"]
            }),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Parallel,
        }
    }

    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        Some(vec![self.subject(input)])
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let path_arg = str_field(&input, "path")?;
        let content = str_field(&input, "content")?;
        let path = confine(&cx.writable_roots, &cx.cwd, &path_arg)?;

        write_at(path, &content, cx).await
    }
}

/// Everything after `confine`: its own function so a test can swap the path
/// for a symlink between the two, the window T64.15 closes.
async fn write_at(path: PathBuf, content: &str, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
    let previous = match read_confined(&cx.writable_roots, &path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| ToolError::Binary)?;
            let lines = text.lines().count();
            if lines > MAX_REWRITE_LINES {
                return Err(ToolError::Denied {
                    why: format!("file has {lines} lines; use edit or apply_patch"),
                });
            }
            text.into_bytes()
        }
        // A new file has no prior content; a swapped path is not new.
        Err(e @ ToolError::Confined { .. }) => return Err(e),
        Err(_) => Vec::new(),
    };

    cx.archive
        .put(ArchivePut {
            session: cx.session,
            call: cx.call,
            tool: "write".to_string(),
            subject: Some(path.display().to_string()),
            bytes: previous,
        })
        .await
        .map_err(|_| ToolError::Io)?;

    atomic_write(&path, content.as_bytes())?;

    Ok(ToolOutput {
        text: format!("wrote {} ({} bytes)", path.display(), content.len()),
        is_error: false,
        diff: None,
        structured: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cox_protocol::{ArchiveId, SandboxMode, SandboxPolicy, SessionId, StoreError};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;

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

    fn cx(dir: &Path) -> ToolCx {
        let (tx, _rx) = mpsc::channel(8);
        crate::tool_cx(
            vec![dir.to_path_buf()],
            dir.to_path_buf(),
            SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            Arc::new(NoopArchive),
            CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    #[tokio::test]
    async fn write_creates_parent_dirs_and_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cx = cx(dir.path());

        let out = WriteTool
            .call(json!({"path": "a/b/c.txt", "content": "hi\n"}), &cx)
            .await
            .expect("write");

        assert!(!out.is_error);
        let got = std::fs::read_to_string(dir.path().join("a/b/c.txt")).expect("read back");
        assert_eq!(got, "hi\n");
    }

    #[tokio::test]
    async fn write_cannot_mutate_a_read_only_workspace_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("main");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&main).expect("main");
        std::fs::create_dir_all(&worktree).expect("worktree");
        let mut cx = cx(&worktree);
        cx.roots.push(main.clone());

        let result = WriteTool
            .call(json!({"path": main.join("new.txt"), "content": "no"}), &cx)
            .await;

        assert!(result.is_err());
        assert!(!main.join("new.txt").exists());
    }

    #[tokio::test]
    async fn write_refuses_big_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.txt");
        let big = "line\n".repeat(201);
        std::fs::write(&path, &big).expect("seed file");
        let cx = cx(dir.path());

        let err = WriteTool
            .call(json!({"path": "big.txt", "content": "new"}), &cx)
            .await
            .expect_err("must refuse");

        match err {
            ToolError::Denied { why } => {
                assert!(why.contains("201 lines"), "why was: {why}");
                assert!(why.contains("use edit or apply_patch"), "why was: {why}");
            }
            other => panic!("expected Denied, got {other:?}"),
        }
        // File on disk is untouched.
        assert_eq!(std::fs::read_to_string(&path).expect("read"), big);
    }

    #[tokio::test]
    async fn write_archives_previous_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "old\n").expect("seed file");

        struct RecordingArchive(std::sync::Mutex<Vec<ArchivePut>>);
        #[async_trait]
        impl cox_protocol::Archive for RecordingArchive {
            async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
                self.0.lock().expect("lock").push(put);
                Ok(ArchiveId::new())
            }
            async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
                Ok(Vec::new())
            }
        }
        let archive = Arc::new(RecordingArchive(std::sync::Mutex::new(Vec::new())));
        let (tx, _rx) = mpsc::channel(8);
        let cx = crate::tool_cx(
            vec![dir.path().to_path_buf()],
            dir.path().to_path_buf(),
            SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            archive.clone(),
            CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        );

        WriteTool
            .call(json!({"path": "f.txt", "content": "new\n"}), &cx)
            .await
            .expect("write");

        let puts = archive.0.lock().expect("lock");
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].bytes, b"old\n");
    }

    #[tokio::test]
    async fn rewrite_all_puts_back_what_it_replaced_when_a_later_file_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        std::fs::write(&a, "old a\n").expect("seed");
        // A non-empty directory cannot be replaced by a file, so the second
        // rename fails after the first has landed.
        let b = dir.path().join("b");
        std::fs::create_dir_all(b.join("inside")).expect("seed dir");
        let rewrite = |path: &Path, before: &str| Rewrite {
            path: path.to_path_buf(),
            before: before.to_string(),
            after: "new\n".to_string(),
        };

        let out = rewrite_all(
            "t",
            &[rewrite(&a, "old a\n"), rewrite(&b, "")],
            &cx(dir.path()),
        )
        .await;
        assert!(matches!(out, Err(ToolError::Io)), "{out:?}");
        assert_eq!(std::fs::read_to_string(&a).expect("a"), "old a\n");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("cox-tmp"))
            .collect();
        assert!(left.is_empty(), "temp files left: {left:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_refuses_a_path_swapped_to_a_symlink_after_confine() {
        let swap = crate::swap_fixture::Swap::new();
        swap.file("secret.txt", "mine");
        swap.file("sub/secret.txt", "mine");
        let cx = cx(&swap.root);
        let leaf = confine(&cx.writable_roots, &swap.root, "secret.txt").expect("confine");
        let nested = confine(&cx.writable_roots, &swap.root, "sub/secret.txt").expect("confine");
        swap.swap_leaf("secret.txt");
        swap.swap_parent("sub");

        for path in [leaf, nested] {
            let err = write_at(path, "changed", &cx)
                .await
                .expect_err("must refuse");
            assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
        }
        assert_eq!(swap.outside_secret(), crate::swap_fixture::SECRET);
    }
}
