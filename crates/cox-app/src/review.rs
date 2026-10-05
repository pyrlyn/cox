// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Review's diff of one changed file (DT§5.4, T37.28.2, A101): the net
//! difference between the copy the session's first checkpoint of it holds
//! and the file on disk now, not the model's calls one by one, so after a
//! code-only rewind it shows what is left rather than what was undone.
//! Separate from `changes.rs`, which lists the files and their turns from
//! what the session recorded; this reads one file's bytes on request. Also
//! the words of Review's line comments (T37.28.4), so every surface sends
//! the agent the same prompt for the same draft, and the core's hunk
//! reverter over cox-render's `revert_hunk` (T51.20), since the core may
//! not depend on cox-render.

use std::sync::Arc;

use std::fmt::Write as _;
use std::path::Path;

use cox_protocol::types::{CheckpointKind, content_digest};
use cox_protocol::{ArchiveId, Before, CheckpointRow, HunkReverter};
use cox_render::diffmodel::{self, DiffModel};

/// The first row that recorded `path` (confined, so canonical): the file as
/// it was before this session changed it. Rows are in insertion order.
pub fn base<'a>(rows: &'a [(CheckpointRow, String)], path: &Path) -> Option<&'a CheckpointRow> {
    rows.iter()
        .map(|(row, _)| row)
        .find(|row| row.kind != CheckpointKind::Turn && row.path == path)
}

/// What `row` kept of the file: nothing for one the session created, the
/// archived bytes, or [`Before::TooLarge`] for a copy over the size cap.
pub fn kept<E>(
    row: &CheckpointRow,
    archive: impl FnOnce(&ArchiveId) -> Result<Vec<u8>, E>,
) -> Result<Before, E> {
    Ok(match (row.kind, &row.archive) {
        (CheckpointKind::Created, _) => Before::Absent,
        (_, Some(id)) => Before::Bytes(archive(id)?),
        (_, None) => Before::TooLarge,
    })
}

/// `shown`'s change from `before` to `now`: no hunks when they are equal,
/// `None` when a side is over the size cap. Each hunk carries its index and
/// the model the digest of `now`, which a hunk revert sends back (T51.20);
/// a missing file digests as empty, as the core reads it.
pub fn diff(shown: &Path, before: &Before, now: &Before, theme: &str) -> Option<DiffModel> {
    let text = |side: &Before| match side {
        Before::Absent => Some(String::new()),
        Before::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Before::TooLarge => None,
    };
    let digest = match now {
        Before::Bytes(bytes) => content_digest(bytes),
        _ => content_digest(&[]),
    };
    let mut model = diffmodel::between(shown, &text(before)?, &text(now)?, theme);
    model.digest = Some(digest);
    Some(model)
}

/// The core's `HunkReverter`: cox-render's `revert_hunk`, the same diff
/// Review's hunks come from.
pub struct RenderHunks;

impl RenderHunks {
    pub fn shared() -> Arc<dyn HunkReverter> {
        Arc::new(Self)
    }
}

impl HunkReverter for RenderHunks {
    fn revert(&self, before: &str, now: &str, index: usize) -> Option<String> {
        diffmodel::revert_hunk(before, now, index).ok()
    }
}

/// One of Review's line comments: what the person wrote about a line of
/// `path` (T37.28.4). `removed` marks a line the diff shows as deleted, so
/// `line` numbers the file before the session changed it, not the file on
/// disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineComment {
    pub path: String,
    pub line: u32,
    pub removed: bool,
    pub text: String,
}

/// The draft as the one prompt "Send to agent" posts (DT§5.4): each
/// comment under its `path:line` anchor, in the order it was written, a
/// multi-line comment indented under its bullet. `None` when no comment
/// has text, so a surface has nothing to send.
pub fn message(comments: &[LineComment]) -> Option<String> {
    let mut out = String::from("Review comments on your changes:\n");
    let mut any = false;
    for comment in comments {
        let text = comment.text.trim();
        if text.is_empty() {
            continue;
        }
        any = true;
        let side = if comment.removed {
            " (removed line)"
        } else {
            ""
        };
        let body = text.lines().collect::<Vec<_>>().join("\n  ");
        let _ = write!(out, "\n- `{}:{}`{side}: {body}", comment.path, comment.line);
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(path: &str, line: u32, removed: bool, text: &str) -> LineComment {
        LineComment {
            path: path.into(),
            line,
            removed,
            text: text.into(),
        }
    }

    #[test]
    fn message_anchors_each_comment_at_its_file_and_line_in_draft_order() {
        let draft = [
            comment("src/retry.rs", 42, false, "  Use saturating_mul here. "),
            comment("notes.md", 3, false, "   "),
            comment(
                "src/retry.rs",
                17,
                true,
                "Why drop the jitter?\nIt kept retries apart.",
            ),
            comment("notes.md", 1, false, "Typo."),
        ];
        assert_eq!(
            message(&draft).as_deref(),
            Some(
                "Review comments on your changes:\n\
                 \n- `src/retry.rs:42`: Use saturating_mul here.\
                 \n- `src/retry.rs:17` (removed line): Why drop the jitter?\n  It kept retries apart.\
                 \n- `notes.md:1`: Typo."
            )
        );
    }

    #[test]
    fn review_diff_carries_hunk_indices() {
        let before: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let now = before
            .replace("line 2\n", "line two\n")
            .replace("line 18\n", "line eighteen\n");
        let old = Before::Bytes(before.clone().into_bytes());
        let new = Before::Bytes(now.clone().into_bytes());
        let model = diff(Path::new("notes.txt"), &old, &new, "base16-ocean.dark");
        let model = model.expect("both sides are text");
        let indices: Vec<u32> = model.hunks.iter().map(|h| h.index).collect();
        assert_eq!(indices, [0, 1]);
        assert_eq!(model.digest, Some(content_digest(now.as_bytes())));
        // The index is the one the core's reverter puts back.
        let reverted = RenderHunks.revert(&before, &now, 1).expect("hunk 1");
        assert_eq!(reverted, before.replace("line 2\n", "line two\n"));
    }

    #[test]
    fn message_is_none_when_no_comment_has_text() {
        assert_eq!(message(&[]), None);
        assert_eq!(message(&[comment("a.rs", 1, false, " \n ")]), None);
    }
}
