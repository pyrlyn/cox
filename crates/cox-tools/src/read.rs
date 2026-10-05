// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `read`: whole, ranged and outline reads of one confined file (plan.md
//! T3.2, §1.11), and an image file returned as an image (T40.4). Every path argument goes through `cox_tools::path::confine`
//! first (AGENTS.md trust boundary) — no other constructor for a `Path`
//! from `input` exists in this file, so the `confine_is_the_only_path_
//! constructor` grep guard in `tests/confine.rs` stays green.

use async_trait::async_trait;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec, image};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::outline;
use crate::path::confine;

/// Visible-byte backstop when no caller-supplied cap is available.
/// `ToolCx` (`cox-protocol::traits`) carries no `tool_output_visible_bytes`
/// field — that value lives in `cox_protocol::config::ToolsConfig` and only
/// reaches a running tool once `cox-core` wires session config through
/// `ToolCx` construction, which is out of scope here (see `tool_cx` in
/// `lib.rs`). `ToolOutput.text` is documented as untruncated, with the core
/// archiving and re-truncating it losslessly (plan.md §1.2/D6a); this cap
/// only stops one huge file from ballooning a single call's output before
/// that safety net runs.
pub(crate) const VISIBLE_CAP_BYTES: usize = 64 * 1024;

/// How many leading bytes are checked for a NUL byte to decide "binary"
/// (plan.md T3.2 step 2).
pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// `read`: text, ranged, or outline reads of one file inside the workspace.
pub struct ReadTool;

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadInput {
    /// Path to read, relative to the session `cwd` or absolute within a workspace root.
    path: String,
    /// Optional 1-based inclusive line range, e.g. `"120-180"`. Omitted reads the whole file.
    #[serde(default)]
    lines: Option<String>,
    /// `"text"` (default) for line-numbered content, or `"outline"` for a short signature listing.
    #[serde(default)]
    mode: Option<String>,
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        let input_schema = serde_json::to_value(schema_for!(ReadInput)).unwrap_or(Value::Null);
        ToolSpec {
            name: "read".to_string(),
            description: "Reads a file as `line<TAB>content` text, or a structural outline. \
                Default (mode \"text\", no `lines`) returns the whole file with line-number \
                prefixes and always reports the file's total line count. Pass `lines: \"a-b\"` \
                (1-based, inclusive) to read only that range once you know what you need — from \
                a prior `grep` hit or an `outline` result — instead of paying for the whole \
                file. Pass `mode: \"outline\"` to get a compact `line: signature` listing of the \
                file's top-level functions/types/classes/impls (tree-sitter for \
                .rs/.ts/.tsx/.py/.go; markdown headings or definition-keyword lines for \
                everything else). Use outline first on any file you have not read yet, \
                especially a large one, then follow up with `lines=` on the range that actually \
                matters. A PNG, JPEG, GIF or WebP file (up to 3.75 MB) comes back as the \
                image itself, which you can see; `lines` and `mode` do not apply to it. Refuses \
                other binary files."
                .to_string(),
            input_schema,
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: ReadInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid input: {e}"),
        })?;
        let path = confine(&cx.roots, &cx.cwd, &input.path)?;

        // ponytail: the whole file is loaded before the binary sniff below;
        // fine for source-sized files (this tool's use case), a real
        // ceiling only for very large binaries — upgrade to a bounded
        // `File::open` + `take(BINARY_SNIFF_BYTES)` pre-read if that shows
        // up in practice.
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ToolError::NotFound
            } else {
                ToolError::Io
            }
        })?;

        // Before the NUL sniff: every accepted image format carries NUL bytes.
        if let Some(media_type) = image::sniff(&bytes) {
            return read_image(media_type, &bytes, &input);
        }

        let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
        if bytes[..sniff_len].contains(&0) {
            return Err(ToolError::Binary);
        }

        let content = String::from_utf8_lossy(&bytes);
        let total_lines = if content.is_empty() {
            0
        } else {
            content.lines().count()
        };

        let mode = input.mode.as_deref().unwrap_or("text");
        let text = if mode == "outline" {
            let body = outline::outline(&path, &content);
            cap(format!("{body}\n\n[outline of {total_lines} lines total]"))
        } else {
            render_text(&content, total_lines, input.lines.as_deref())
        };

        // The file's line count as data (DT G3): a summary says "120
        // lines" without parsing the footer.
        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: Some(serde_json::json!({ "lines": total_lines })),
        })
    }
}

/// An image comes back as one text line for the transcript plus the bytes
/// in `structured`, which the core forwards to the model (T40.5). The cap is
/// checked before encoding, so an oversized file costs no base64 copy.
fn read_image(media_type: &str, bytes: &[u8], input: &ReadInput) -> Result<ToolOutput, ToolError> {
    if bytes.len() > image::MAX_IMAGE_BYTES {
        return Err(ToolError::TooLarge {
            bytes: bytes.len() as u64,
            cap: image::MAX_IMAGE_BYTES as u64,
        });
    }
    let mut text = format!("{media_type}, {}", human_size(bytes.len()));
    if input.lines.is_some() || input.mode.is_some() {
        text.push_str(" (`lines` and `mode` do not apply to images)");
    }
    Ok(ToolOutput {
        text,
        is_error: false,
        diff: None,
        structured: Some(image::to_structured(media_type, bytes)),
    })
}

/// `16 B`, `48.2 KiB`, `3.5 MiB`: short enough for the one image line.
fn human_size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KIB {
        format!("{bytes} B")
    } else if b < KIB * KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{:.1} MiB", b / (KIB * KIB))
    }
}

/// Renders `content` as `n\tline` text for the requested range (or the
/// whole file), always reporting the total line count (plan.md T3.2 step
/// 1: "`read` reports total lines"), then applies the visible-byte cap.
fn render_text(content: &str, total_lines: usize, lines_spec: Option<&str>) -> String {
    if total_lines == 0 {
        return "(empty file, 0 lines)".to_string();
    }

    let (start, end) = lines_spec
        .and_then(|s| parse_range(s, total_lines))
        .unwrap_or((1, total_lines));

    let body: String = content
        .lines()
        .enumerate()
        .skip(start - 1)
        .take(end - start + 1)
        .map(|(idx, line)| format!("{}\t{line}", idx + 1))
        .collect::<Vec<_>>()
        .join("\n");

    let footer = if start == 1 && end == total_lines {
        format!("\n\n[{total_lines} lines total]")
    } else {
        format!("\n\n[showing lines {start}-{end} of {total_lines} total]")
    };

    cap(body + &footer)
}

/// Parses a `"a-b"` 1-based inclusive range, clamped into `[1, total_lines]`.
/// A string that does not parse as `usize-usize`, or an inverted/zero
/// range, is ignored (falls back to the whole file) rather than failing
/// the call — a malformed `lines` argument should not cost the model a
/// whole failed tool round trip.
fn parse_range(spec: &str, total_lines: usize) -> Option<(usize, usize)> {
    let (a, b) = spec.split_once('-')?;
    let start: usize = a.trim().parse().ok()?;
    let end: usize = b.trim().parse().ok()?;
    if start == 0 || end == 0 || start > end {
        return None;
    }
    Some((start.min(total_lines).max(1), end.min(total_lines).max(1)))
}

/// Backstop truncation at the last whole line inside `VISIBLE_CAP_BYTES`,
/// with a trailing note. See `VISIBLE_CAP_BYTES` doc comment: this is a
/// per-call safety net, not the lossless truncation path (that is the
/// core's job, downstream of this `ToolOutput`).
fn cap(text: String) -> String {
    if text.len() <= VISIBLE_CAP_BYTES {
        return text;
    }
    let mut boundary = VISIBLE_CAP_BYTES;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let cut = text[..boundary].rfind('\n').unwrap_or(boundary);
    format!(
        "{}\n\n[... truncated at {VISIBLE_CAP_BYTES} bytes; re-read with a narrower `lines=` range for the rest]",
        &text[..cut]
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use cox_protocol::{ArchiveId, ArchivePut, SandboxMode, SandboxPolicy, StoreError};
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

    fn test_cx(roots: Vec<PathBuf>, cwd: PathBuf) -> ToolCx {
        let (tx, _rx) = mpsc::channel(8);
        crate::tool_cx(
            roots,
            cwd,
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
            cox_protocol::SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    #[tokio::test]
    async fn read_ranged_read_returns_only_the_requested_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let content = (1..=10)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(root.join("f.txt"), &content).expect("write fixture");

        let cx = test_cx(vec![root.clone()], root.clone());
        let out = ReadTool
            .call(serde_json::json!({"path": "f.txt", "lines": "3-5"}), &cx)
            .await
            .expect("read");

        assert!(out.text.contains("3\tline3"), "{}", out.text);
        assert!(out.text.contains("4\tline4"), "{}", out.text);
        assert!(out.text.contains("5\tline5"), "{}", out.text);
        assert!(!out.text.contains("line1\n"), "{}", out.text);
        assert!(!out.text.contains("line6"), "{}", out.text);
        assert!(out.text.contains("lines 3-5 of 10 total"), "{}", out.text);
        assert_eq!(out.structured, Some(serde_json::json!({ "lines": 10 })));
    }

    #[tokio::test]
    async fn read_binary_file_is_rejected_with_binary_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        std::fs::write(root.join("f.bin"), [0u8, 1, 2, 3, 0, 5]).expect("write fixture");

        let cx = test_cx(vec![root.clone()], root.clone());
        let err = ReadTool
            .call(serde_json::json!({"path": "f.bin"}), &cx)
            .await
            .expect_err("must reject binary");

        assert!(matches!(err, ToolError::Binary), "{err:?}");
    }

    /// A PNG signature and a NUL-bearing tail: without the image path it
    /// would be refused as binary.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[tokio::test]
    async fn read_png_returns_structured_image() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        std::fs::write(root.join("shot.png"), PNG).expect("write fixture");

        let cx = test_cx(vec![root.clone()], root.clone());
        let mut out = ReadTool
            .call(serde_json::json!({"path": "shot.png", "lines": "1-2"}), &cx)
            .await
            .expect("read image");

        assert!(out.text.starts_with("image/png, 16 B"), "{}", out.text);
        assert!(out.text.contains("`lines`"), "{}", out.text);
        let (media_type, data_b64) =
            cox_protocol::image::take_structured(&mut out).expect("structured image");
        assert_eq!(media_type, "image/png");
        let expected = cox_protocol::image::to_structured("image/png", PNG);
        assert_eq!(
            Some(data_b64.as_str()),
            expected["image"]["data_b64"].as_str()
        );
    }

    #[tokio::test]
    async fn read_oversized_image_is_too_large() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let mut big = PNG.to_vec();
        big.resize(cox_protocol::image::MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(root.join("big.png"), &big).expect("write fixture");

        let cx = test_cx(vec![root.clone()], root.clone());
        let err = ReadTool
            .call(serde_json::json!({"path": "big.png"}), &cx)
            .await
            .expect_err("must refuse an oversized image");

        let cap = cox_protocol::image::MAX_IMAGE_BYTES as u64;
        assert_eq!(
            err,
            ToolError::TooLarge {
                bytes: cap + 1,
                cap
            }
        );
    }

    #[tokio::test]
    async fn read_confinement_refuses_a_path_outside_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path())
            .expect("canonicalize")
            .join("root");
        std::fs::create_dir(&root).expect("mkdir");

        let cx = test_cx(vec![root.clone()], root.clone());
        let err = ReadTool
            .call(serde_json::json!({"path": "../outside.txt"}), &cx)
            .await
            .expect_err("must confine");

        assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn read_outline_of_1000_line_rust_fixture_is_short_and_lists_every_pub_fn() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/outline/large.rs");
        let src = std::fs::read_to_string(&fixture).expect("read fixture");
        assert_eq!(
            src.lines().count(),
            1000,
            "fixture must be exactly 1000 lines"
        );

        let expected_pub_fns: Vec<&str> = src
            .lines()
            .filter_map(|l| {
                let t = l.trim_start();
                t.starts_with("pub fn ").then(|| {
                    let after = &t["pub fn ".len()..];
                    after.split(['(', '<']).next().unwrap_or("").trim()
                })
            })
            .collect();
        assert!(!expected_pub_fns.is_empty(), "fixture must declare pub fns");

        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        std::fs::copy(&fixture, root.join("large.rs")).expect("copy fixture");

        let cx = test_cx(vec![root.clone()], root.clone());
        let out = ReadTool
            .call(
                serde_json::json!({"path": "large.rs", "mode": "outline"}),
                &cx,
            )
            .await
            .expect("outline read");

        let line_count = out.text.lines().count();
        assert!(
            line_count < 120,
            "outline was {line_count} lines:\n{}",
            out.text
        );

        for name in &expected_pub_fns {
            assert!(
                out.text.contains(&format!("pub fn {name}")),
                "outline missing `pub fn {name}`:\n{}",
                out.text
            );
        }
    }
}
