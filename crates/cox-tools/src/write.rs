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

use std::path::Path;

use async_trait::async_trait;
use cox_protocol::{ArchivePut, Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

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

    std::fs::write(&tmp, bytes).map_err(|_| ToolError::Io)?;
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Io
    })?;
    Ok(())
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

        let previous = match std::fs::read(&path) {
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
}
