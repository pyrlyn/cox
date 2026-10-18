// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T3.4 step 6: the two properties `edit`'s match ladder has to hold, driven
//! through the real `Tool` surface (confine, archive, atomic write) rather
//! than the private `apply_replace`, so a regression in the write path fails
//! here too.

use std::path::PathBuf;
use std::sync::Arc;

use cox_protocol::{
    Archive, ArchiveId, ArchivePut, CallId, SandboxMode, SandboxPolicy, SessionId, StoreError,
    Tool, ToolCx, ToolError,
};
use cox_tools::edit::EditTool;
use proptest::prelude::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

fn cx(root: PathBuf) -> ToolCx {
    let (tx, _rx) = mpsc::channel(16);
    ToolCx {
        roots: vec![root.clone()],
        writable_roots: vec![root.clone()],
        cwd: root,
        sandbox: SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        },
        archive: Arc::new(NoopArchive),
        cancel: CancellationToken::new(),
        output: tx,
        session: SessionId::new(),
        call: CallId::new(),
        agent: None,
        preset: None,
        relay: None,
    }
}

/// Runs one `edit` call against `file` in a fresh runtime. Proptest bodies are
/// synchronous, so the runtime is built per call rather than via
/// `#[tokio::test]`.
fn edit(dir: &tempfile::TempDir, old: &str, new: &str) -> Result<(), ToolError> {
    edit_input(
        dir,
        serde_json::json!({ "path": "f.txt", "old": old, "new": new }),
    )
}

/// Same as [`edit`] with `replace_all: true`.
fn edit_replace_all(dir: &tempfile::TempDir, old: &str, new: &str) -> Result<(), ToolError> {
    edit_input(
        dir,
        serde_json::json!({ "path": "f.txt", "old": old, "new": new, "replace_all": true }),
    )
}

fn edit_input(dir: &tempfile::TempDir, input: serde_json::Value) -> Result<(), ToolError> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(EditTool.call(input, &cx(dir.path().to_path_buf())))
        .map(|_| ())
}

fn write_fixture(content: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("f.txt"), content).expect("write fixture");
    dir
}

#[tokio::test]
async fn edit_cannot_mutate_a_read_only_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let main = dir.path().join("main");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&main).expect("main");
    std::fs::create_dir_all(&worktree).expect("worktree");
    let file = main.join("f.txt");
    std::fs::write(&file, "old").expect("fixture");
    let mut cx = cx(worktree.clone());
    cx.roots.push(main.clone());

    let result = EditTool
        .call(
            serde_json::json!({"path": file, "old": "old", "new": "new"}),
            &cx,
        )
        .await;

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(file).expect("unchanged"), "old");
}

proptest! {
    /// Replacing a unique marker and then replacing it back is a no-op on the
    /// file's bytes — which is what makes an `edit` undoable, and what would
    /// break first if the splice mishandled a trailing newline or a line end.
    #[test]
    fn edit_then_reverse_edit_is_identity(
        // A deliberately narrow alphabet: it cannot spell either marker, so
        // both `old` and `new` are guaranteed to occur exactly once.
        lines in prop::collection::vec("[a-c ]{0,6}", 0..8),
        at in 0usize..8,
        trailing_newline in any::<bool>(),
    ) {
        let mut lines = lines;
        let at = at.min(lines.len());
        lines.insert(at, "MARKER_ONE".to_string());
        let mut content = lines.join("\n");
        if trailing_newline {
            content.push('\n');
        }

        let dir = write_fixture(&content);
        let path = dir.path().join("f.txt");

        edit(&dir, "MARKER_ONE", "MARKER_TWO").expect("forward edit");
        let after = std::fs::read(&path).expect("read after forward edit");
        prop_assert_ne!(after, content.as_bytes().to_vec(), "the edit did nothing");

        edit(&dir, "MARKER_TWO", "MARKER_ONE").expect("reverse edit");
        let restored = std::fs::read(&path).expect("read after reverse edit");
        prop_assert_eq!(restored, content.as_bytes().to_vec());
    }
}

/// Two identical candidates must be refused, not silently resolved to the
/// first — and the error has to name the lines so the model can narrow `old`.
#[test]
fn edit_ambiguous_match_is_rejected() {
    let dir = write_fixture("alpha\ntarget\nbeta\ntarget\ngamma\n");
    let before = std::fs::read(dir.path().join("f.txt")).expect("read before");

    let err = edit(&dir, "target", "replaced").expect_err("two matches must be refused");
    match err {
        ToolError::Ambiguous { matches } => {
            assert_eq!(matches, vec!["2".to_string(), "4".to_string()]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }

    let after = std::fs::read(dir.path().join("f.txt")).expect("read after");
    assert_eq!(before, after, "a refused edit must not touch the file");
}

/// Pins what the fallback does and does not forgive. plan.md T3.4 says
/// "collapse runs of spaces/tabs, trim line ends" — so interior runs and
/// trailing space are free, but leading indentation collapses to a single
/// space rather than disappearing: `old` still has to be indented, just not
/// by the right amount. `new` is spliced in verbatim, so it carries whatever
/// indentation the caller wrote.
#[test]
fn edit_falls_back_to_whitespace_insensitive_match() {
    let dir = write_fixture("fn main() {\n    let  x\t= 1;  \n}\n");
    edit(&dir, " let x = 1;", "    let x = 2;").expect("whitespace-insensitive match");
    let after = std::fs::read_to_string(dir.path().join("f.txt")).expect("read");
    assert_eq!(after, "fn main() {\n    let x = 2;\n}\n");
}

/// The edge of that contract. Dropping the indentation alone is fine — step
/// 1 is a plain substring search, so an unindented `old` still matches inside
/// an indented line. But once interior whitespace also differs, only the
/// fallback can match, and there leading space *is* significant: the file's
/// indent collapses to one space, which an unindented `old` cannot equal.
/// If this ever starts passing, `normalize_line` began trimming both ends and
/// plan.md T3.4's "trim line ends" no longer describes it.
#[test]
fn edit_fallback_does_not_forgive_missing_indentation() {
    let dir = write_fixture("fn main() {\n    let  x = 1;\n}\n");
    let err = edit(&dir, "let x = 1;", "let x = 2;").expect_err("leading space is significant");
    assert!(
        matches!(&err, ToolError::Denied { why } if why.starts_with("old_string not found")),
        "got {err:?}"
    );
}

/// T62.2. `replace_all` on overlapping fallback windows used to panic: every
/// window was spliced against the original byte offsets, so after the first
/// (rightmost, shorter) replacement shrank the string, the next window's
/// `end_byte` ran past it — `range end index out of range`. The fix keeps
/// only non-overlapping windows; a dropped window's lines are inside the one
/// already replaced. Every line here carries a trailing space so the exact
/// substring path never fires and the fallback is what matches.
#[test]
fn edit_replace_all_overlapping_fallback_windows_do_not_panic() {
    let dir = write_fixture("AAA \nAAA  \nAAA \n");
    edit_replace_all(&dir, "AAA\nAAA", "B").expect("overlapping windows replaced");

    let after = std::fs::read_to_string(dir.path().join("f.txt")).expect("read");
    // Window at line 0 (lines 0–1) is kept; the window at line 1 overlaps it
    // and is dropped, leaving line 2 untouched.
    assert_eq!(after, "B\nAAA \n");
}

/// T69.7: content-hash anchors, driven through `read` (which renders them)
/// and `edit` (which consumes them), so the two halves cannot drift apart.
mod anchors {
    use super::*;
    use cox_tools::read::ReadTool;

    fn run<T: Tool>(
        tool: T,
        dir: &tempfile::TempDir,
        input: serde_json::Value,
    ) -> Result<String, ToolError> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(tool.call(input, &cx(dir.path().to_path_buf())))
            .map(|out| out.text)
    }

    /// The `N:abc` anchor `read` prints for line `n`.
    fn anchor_of(dir: &tempfile::TempDir, n: usize) -> String {
        let text = run(
            ReadTool,
            dir,
            serde_json::json!({ "path": "f.txt", "anchors": true }),
        )
        .expect("read");
        let prefix = format!("{n}:");
        text.lines()
            .find(|l| l.starts_with(&prefix))
            .and_then(|l| l.split('→').next())
            .expect("anchor line")
            .to_string()
    }

    fn contents(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(dir.path().join("f.txt")).expect("read back")
    }

    #[test]
    fn edit_read_without_anchors_is_unchanged() {
        let dir = write_fixture("a\nb\n");
        let text = run(ReadTool, &dir, serde_json::json!({ "path": "f.txt" })).expect("read");
        assert!(text.starts_with("1\ta\n2\tb"), "got {text:?}");
    }

    #[test]
    fn edit_anchor_survives_an_indent_only_change() {
        let dir = write_fixture("fn f() {\n    let x = 1;\n}\n");
        let anchor = anchor_of(&dir, 2);
        std::fs::write(
            dir.path().join("f.txt"),
            "fn f() {\n        let  x = 1;\n}\n",
        )
        .expect("reindent");
        run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "f.txt",
                "ops": [{ "op": "replace", "anchor": anchor, "content": "    let x = 2;" }]
            }),
        )
        .expect("indent-only change still matches");
        assert_eq!(contents(&dir), "fn f() {\n    let x = 2;\n}\n");
    }

    #[test]
    fn edit_stale_anchor_names_the_fresh_one_and_changes_nothing() {
        let dir = write_fixture("one\ntwo\nthree\nfour\n");
        let stale = anchor_of(&dir, 2);
        std::fs::write(dir.path().join("f.txt"), "one\nTWO changed\nthree\nfour\n").expect("drift");
        let fresh = anchor_of(&dir, 2);
        assert_ne!(stale, fresh);

        let err = run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "f.txt",
                "ops": [
                    { "op": "replace", "anchor": anchor_of(&dir, 1), "content": "uno" },
                    { "op": "replace", "anchor": stale, "content": "dos" }
                ]
            }),
        )
        .expect_err("stale anchor rejects the batch");
        match err {
            ToolError::Denied { why } => {
                assert!(why.contains("stale"), "got {why}");
                assert!(why.contains(&format!("{fresh}→TWO changed")), "got {why}");
                assert!(why.contains("three"), "neighbours shown: {why}");
            }
            other => panic!("expected Denied, got {other:?}"),
        }
        assert_eq!(contents(&dir), "one\nTWO changed\nthree\nfour\n");
    }

    #[test]
    fn edit_two_anchored_ops_apply_bottom_up() {
        let dir = write_fixture("a\nb\nc\nd\n");
        let (a1, a3) = (anchor_of(&dir, 1), anchor_of(&dir, 3));
        run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "f.txt",
                "ops": [
                    { "op": "insert_after", "anchor": a1, "content": "a2\na3" },
                    { "op": "replace", "anchor": a3, "content": "C" }
                ]
            }),
        )
        .expect("both ops apply against the original line numbers");
        assert_eq!(contents(&dir), "a\na2\na3\nb\nC\nd\n");
    }

    #[test]
    fn edit_anchored_ops_keep_crlf_and_missing_final_newline() {
        let dir = write_fixture("a\r\nb");
        let anchor = anchor_of(&dir, 2);
        run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "f.txt",
                "ops": [{ "op": "insert_after", "anchor": anchor, "content": "c" }]
            }),
        )
        .expect("edit");
        assert_eq!(contents(&dir), "a\r\nb\r\nc");
    }

    #[test]
    fn edit_anchored_path_outside_the_workspace_is_confined() {
        let dir = write_fixture("a\n");
        let err = run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "../escape.txt",
                "ops": [{ "op": "replace", "anchor": "1:aaa", "content": "x" }]
            }),
        )
        .expect_err("outside the workspace");
        assert!(matches!(err, ToolError::Confined { .. }), "got {err:?}");
    }

    #[test]
    fn edit_rejects_ops_mixed_with_old_string() {
        let dir = write_fixture("a\n");
        let err = run(
            EditTool,
            &dir,
            serde_json::json!({
                "path": "f.txt", "old": "a", "new": "b",
                "ops": [{ "op": "replace", "anchor": anchor_of(&dir, 1), "content": "x" }]
            }),
        )
        .expect_err("one mode per call");
        assert!(matches!(err, ToolError::Denied { .. }), "got {err:?}");
        assert_eq!(contents(&dir), "a\n");
    }
}
