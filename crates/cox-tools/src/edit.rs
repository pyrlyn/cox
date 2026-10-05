// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `edit`: exact `str_replace` with a whitespace-insensitive fallback
//! (plan.md D8/T3.4, §1.11). Every path goes through
//! `cox_tools::path::confine` first (AGENTS.md trust boundary). Pre-edit
//! content is archived before the file is touched so `cox expand` can
//! restore it without git; the write itself goes through
//! `write::atomic_write` (temp file + rename in the same directory) so a
//! crash never leaves a half-written file.
//!
//! `ToolError` deviation (documented once here, reused by `write.rs` and
//! `todo.rs`): plan.md's algorithm asks for a `NotFound` error carrying
//! the three closest lines by `similar` ratio, and a ">200 lines"/
//! validation message with dynamic text. `cox-protocol::ToolError::NotFound`
//! is a unit variant (no room for a message) and this task's instructions
//! say not to edit `cox-protocol`. The closest existing variant that can
//! actually carry a message is `Denied { why: String }`, so every
//! message-carrying failure in these three tools (no-match-at-all,
//! oversized-file, bad todo input) is surfaced as `Denied` with `why` set
//! to the exact text the plan specifies. `Ambiguous { matches }` is used
//! only for its literal purpose — more than one match — with `matches`
//! holding the 1-based line numbers, matching plan.md §1.11's "ambiguity is
//! an error listing match lines".

use async_trait::async_trait;
use cox_protocol::{
    ArchivePut, Concurrency, Diff, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec,
};
use serde_json::{Value, json};
use similar::TextDiff;

use crate::path::confine;
use crate::write::{atomic_write, str_field};

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Replace text in a file: finds `old` and replaces it with `new`. \
                `old` must match exactly once (or, if no exact match exists, once after \
                collapsing runs of spaces/tabs and trimming line ends on both sides) unless \
                `replace_all` is set, in which case every match is replaced. Errors this tool \
                may return: `path {path} escapes workspace root {root}` (path outside the \
                workspace), `not found` (the file itself does not exist), `binary file` (the \
                file is not valid UTF-8 text), `ambiguous match: N candidates` (with the \
                matching line numbers — narrow `old` to make it unique), `denied: old_string \
                not found. Closest lines:\n<line>: <text>` (no match even with the whitespace \
                fallback — the closest lines are shown for reference), `io error`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File to edit, relative to the workspace root."
                    },
                    "old": {
                        "type": "string",
                        "description": "Exact text to find and replace."
                    },
                    "new": {
                        "type": "string",
                        "description": "Text to replace it with."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every match instead of requiring exactly one.",
                        "default": false
                    }
                },
                "required": ["path", "old", "new"]
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
        let old = str_field(&input, "old")?;
        let new = str_field(&input, "new")?;
        let replace_all = input
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let path = confine(&cx.writable_roots, &cx.cwd, &path_arg)?;

        let raw = std::fs::read(&path).map_err(|_| ToolError::NotFound)?;
        if raw.contains(&0) {
            return Err(ToolError::Binary);
        }
        let original = String::from_utf8(raw).map_err(|_| ToolError::Binary)?;

        // Normalise to `\n` for matching/splicing, then restore CRLF (if the
        // file had it) and let the untouched-region splice carry the
        // trailing-newline state through unchanged.
        let crlf = original.contains("\r\n");
        let work = if crlf {
            original.replace("\r\n", "\n")
        } else {
            original.clone()
        };

        let new_work = apply_replace(&work, &old, &new, replace_all)?;
        let result = if crlf {
            new_work.replace('\n', "\r\n")
        } else {
            new_work
        };

        cx.archive
            .put(ArchivePut {
                session: cx.session,
                call: cx.call,
                tool: "edit".to_string(),
                subject: Some(path.display().to_string()),
                bytes: original.clone().into_bytes(),
            })
            .await
            .map_err(|_| ToolError::Io)?;

        atomic_write(&path, result.as_bytes())?;

        let label = path.display().to_string();
        let text_diff = TextDiff::from_lines(&original, &result);
        let mut unified = text_diff.unified_diff();
        unified.context_radius(3).header(&label, &label);

        Ok(ToolOutput {
            text: format!("edited {label}"),
            is_error: false,
            diff: Some(Diff {
                path,
                unified: unified.to_string(),
            }),
            structured: None,
        })
    }
}

/// Applies `old` -> `new` to `content` (already CRLF-normalised to `\n`),
/// following D8's match order:
///
/// 1. Exact substring match. Count 1 replaces; >1 is `Ambiguous` (matching
///    line numbers) unless `replace_all`, in which case every exact match
///    is replaced.
/// 2. If there is no exact match at all, fall back to whitespace-insensitive
///    line-window matching (`normalize_line`): `old` is split into lines,
///    each line collapsed/trimmed, and matched against every same-length
///    contiguous window of the file's lines, normalised the same way. Count
///    1 replaces; >1 is `Ambiguous`; `replace_all` replaces every window.
/// 3. Still zero matches: `NotFound`-shaped `Denied` naming the three lines
///    closest to `old` by `similar`'s character ratio.
fn apply_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, ToolError> {
    let exact: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();

    if !exact.is_empty() {
        return if replace_all {
            Ok(content.replace(old, new))
        } else if exact.len() == 1 {
            Ok(content.replacen(old, new, 1))
        } else {
            Err(ToolError::Ambiguous {
                matches: exact
                    .iter()
                    .map(|&i| line_number(content, i).to_string())
                    .collect(),
            })
        };
    }

    let starts = whitespace_fallback_matches(content, old);
    if starts.is_empty() {
        return Err(not_found(content, old));
    }
    if !replace_all && starts.len() > 1 {
        return Err(ToolError::Ambiguous {
            matches: starts.iter().map(|&i| (i + 1).to_string()).collect(),
        });
    }

    let old_line_count = old.split('\n').count();
    let content_lines: Vec<&str> = content.split('\n').collect();
    let targets = if replace_all { starts } else { vec![starts[0]] };

    // ponytail: fallback windows aren't de-overlapped — replacing from the
    // rightmost start backward keeps byte offsets valid for non-overlapping
    // windows (the normal case); a pathological `replace_all` where two
    // fallback windows overlap can still panic on the byte splice. Add
    // overlap filtering if repetitive-whitespace-variant content makes this
    // a real problem.
    let mut result = content.to_string();
    for &start_line in targets.iter().rev() {
        let (start_byte, end_byte) = line_span_bytes(&content_lines, start_line, old_line_count);
        result.replace_range(start_byte..end_byte, new);
    }
    Ok(result)
}

fn line_number(content: &str, byte_idx: usize) -> usize {
    content[..byte_idx].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Returns the 0-based starting line indices of every contiguous window in
/// `content` whose lines equal `old`'s lines once each side is run through
/// `normalize_line`.
fn whitespace_fallback_matches(content: &str, old: &str) -> Vec<usize> {
    let content_lines: Vec<String> = content.split('\n').map(normalize_line).collect();
    let old_lines: Vec<String> = old.split('\n').map(normalize_line).collect();
    if old_lines.is_empty() || old_lines.len() > content_lines.len() {
        return Vec::new();
    }
    let window = old_lines.len();
    (0..=content_lines.len() - window)
        .filter(|&start| content_lines[start..start + window] == old_lines[..])
        .collect()
}

/// Collapses runs of spaces/tabs to one space and trims the line end, so
/// `"  foo\t bar  "` and `"\tfoo bar"` normalise identically.
fn normalize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last_ws = false;
    for ch in line.chars() {
        if ch == ' ' || ch == '\t' {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    out.trim_end().to_string()
}

/// The original (`content_lines`, split by `\n`) byte range spanning lines
/// `[start_line, start_line + count)`. Since `split('\n')` + `join("\n")`
/// round-trips exactly for `\n`-only content, summing line lengths plus one
/// separator byte per line (except the last, hence `count - 1`) gives the
/// exact span `line_span_bytes` claims to cover.
fn line_span_bytes(lines: &[&str], start_line: usize, count: usize) -> (usize, usize) {
    let start_byte: usize = lines[..start_line].iter().map(|l| l.len() + 1).sum();
    let span_len: usize = lines[start_line..start_line + count]
        .iter()
        .map(|l| l.len())
        .sum::<usize>()
        + (count - 1);
    (start_byte, start_byte + span_len)
}

/// No match, even with the whitespace fallback: names the three lines in
/// `content` closest to `old` by `similar`'s character-level ratio, so the
/// model has something concrete to correct `old` against.
fn not_found(content: &str, old: &str) -> ToolError {
    let mut scored: Vec<(f32, usize, &str)> = content
        .split('\n')
        .enumerate()
        .map(|(i, line)| (TextDiff::from_chars(old, line).ratio(), i + 1, line))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let closest: Vec<String> = scored
        .into_iter()
        .take(3)
        .map(|(_, n, line)| format!("{n}: {line}"))
        .collect();

    ToolError::Denied {
        why: format!(
            "old_string not found. Closest lines:\n{}",
            closest.join("\n")
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
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
    async fn edit_replaces_unique_exact_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello world\n").expect("seed");
        let cx = cx(dir.path());

        EditTool
            .call(
                json!({"path": "f.txt", "old": "world", "new": "there"}),
                &cx,
            )
            .await
            .expect("edit");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "hello there\n"
        );
    }

    #[tokio::test]
    async fn edit_not_found_reports_closest_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("seed");
        let cx = cx(dir.path());

        let err = EditTool
            .call(json!({"path": "f.txt", "old": "betaa", "new": "x"}), &cx)
            .await
            .expect_err("must not match");

        match err {
            ToolError::Denied { why } => {
                assert!(why.contains("Closest lines"));
                assert!(why.contains("beta"));
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }
}
