// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Lossless, line-safe tool-result truncation. Kept in core because tools
//! return complete output and only the loop decides what a model may see.
//! The archive row already exists when this runs (D6a), so the trailer is
//! always a valid handle to the rest.

use cox_protocol::ArchiveId;

/// Shortens `text` at line boundaries, retaining leading and trailing lines.
/// When even the requested head/tail lines exceed `max_bytes`, lines are
/// dropped from the tail first, then the head, so the trailer never falls
/// off: a pointer to the archive always survives.
pub(crate) fn visible(
    text: &str,
    id: ArchiveId,
    max_bytes: usize,
    head_lines: usize,
    tail_lines: usize,
) -> String {
    if text.len() <= max_bytes {
        return text.into();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut head = head_lines.min(lines.len());
    let mut tail = tail_lines.min(lines.len() - head);
    loop {
        let out = compose(&lines, head, tail, text.len(), id);
        if out.len() <= max_bytes || (head == 0 && tail == 0) {
            return out;
        }
        if tail > 0 {
            tail -= 1;
        } else {
            head -= 1;
        }
    }
}

fn compose(lines: &[&str], head: usize, tail: usize, total: usize, id: ArchiveId) -> String {
    let tail_start = lines.len() - tail;
    let trailer = format!(
        "[… {} KiB archived; expand #{id} lines {}–{}]",
        total.div_ceil(1024),
        head + 1,
        tail_start
    );
    let mut out = lines[..head].join("\n");
    if head > 0 {
        out.push('\n');
    }
    out.push_str(&trailer);
    if tail > 0 {
        out.push('\n');
        out.push_str(&lines[tail_start..].join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use cox_protocol::traits::{ArchivePut, Store as _};
    use cox_protocol::{CallId, SessionId};
    use proptest::prelude::*;

    use super::*;
    use crate::MemoryStore;

    #[test]
    fn truncate_keeps_head_tail_and_archive_handle() {
        let text = (1..=100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let id = ArchiveId::new();
        let result = visible(&text, id, 200, 2, 2);
        assert!(result.starts_with("line 1\nline 2\n[… 1 KiB archived; expand #"));
        assert!(result.ends_with("lines 3–98]\nline 99\nline 100"));
        assert!(result.contains(&id.to_string()));
    }

    #[test]
    fn truncate_keeps_trailer_when_one_line_exceeds_cap() {
        let text = "x".repeat(500) + "\nshort";
        let id = ArchiveId::new();
        let result = visible(&text, id, 100, 1, 1);
        assert!(result.contains(&id.to_string()));
        assert!(!result.contains("xxx"));
    }

    proptest! {
        #[test]
        fn truncate_is_lossless_via_archive(
            text in "[a-zé\n]{0,400}",
            max_bytes in 8usize..200,
            head in 0usize..6,
            tail in 0usize..6,
        ) {
            let store = MemoryStore::new();
            let id = store.archive_put(&ArchivePut {
                session: SessionId::new(),
                call: CallId::new(),
                tool: "t".into(),
                subject: None,
                bytes: text.as_bytes().to_vec(),
            }).unwrap();
            let shown = visible(&text, id, max_bytes, head, tail);
            prop_assert_eq!(store.archive_get(&id).unwrap(), text.as_bytes());
            if shown != text {
                prop_assert!(shown.contains(&id.to_string()));
                // `split`, not `lines`: a retained trailing empty line must count.
                let shown_lines: Vec<&str> = shown.split('\n').collect();
                let orig: Vec<&str> = text.lines().collect();
                let cut = shown_lines.iter().position(|l| l.starts_with("[…")).unwrap();
                prop_assert_eq!(&shown_lines[..cut], &orig[..cut]);
                let after = shown_lines.len() - cut - 1;
                prop_assert_eq!(&shown_lines[cut + 1..], &orig[orig.len() - after..]);
            }
        }
    }
}
