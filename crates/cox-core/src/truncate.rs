// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Lossless, line-safe tool-result truncation. Kept in core because tools
//! return complete output and only the loop decides what a model may see.
//! The archive row already exists when this runs (D6a), so the trailer is
//! always a valid handle to the rest.

use std::borrow::Cow;

use cox_protocol::ArchiveId;

/// Shortest run worth folding: two lines cost about as much as the marker.
const MIN_RUN: usize = 3;

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
    cut(
        &lines,
        text.len(),
        id,
        max_bytes,
        head_lines,
        tail_lines,
        true,
    )
}

/// [`visible`] after folding repeated lines. The archive keeps the raw
/// bytes, so a folded result always names its row, even when nothing was
/// cut: the model must be able to tell that lines were merged.
pub(crate) fn visible_folding(
    text: &str,
    id: ArchiveId,
    max_bytes: usize,
    head_lines: usize,
    tail_lines: usize,
) -> String {
    let Cow::Owned(folded) = fold_repeats(text) else {
        return visible(text, id, max_bytes, head_lines, tail_lines);
    };
    let lines: Vec<&str> = folded.lines().collect();
    let whole = compose(&lines, lines.len(), 0, text.len(), id, false);
    if whole.len() <= max_bytes {
        return whole;
    }
    cut(
        &lines,
        text.len(),
        id,
        max_bytes,
        head_lines,
        tail_lines,
        false,
    )
}

/// `ranged` is false for folded lines: their numbers are not the archive's,
/// so a range in the trailer would point `expand` at the wrong lines.
fn cut(
    lines: &[&str],
    total: usize,
    id: ArchiveId,
    max_bytes: usize,
    head_lines: usize,
    tail_lines: usize,
    ranged: bool,
) -> String {
    let mut head = head_lines.min(lines.len());
    let mut tail = tail_lines.min(lines.len() - head);
    loop {
        let out = compose(lines, head, tail, total, id, ranged);
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

fn compose(
    lines: &[&str],
    head: usize,
    tail: usize,
    total: usize,
    id: ArchiveId,
    ranged: bool,
) -> String {
    let tail_start = lines.len() - tail;
    let kib = total.div_ceil(1024);
    let trailer = if ranged {
        format!(
            "[… {kib} KiB archived; expand #{id} lines {}–{}]",
            head + 1,
            tail_start
        )
    } else {
        format!("[… {kib} KiB archived; repeated lines folded; expand #{id}]")
    };
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

/// Folds each run of at least [`MIN_RUN`] lines that match after digit runs
/// are masked into its first line plus ` (×N)`: progress counters and
/// `Compiling …` chatter. A diagnostic line (`error`, `panicked`, `failed`,
/// `warning:`) matches only its exact repeats, because its digits are what
/// tells two diagnostics apart; the first of any run stays verbatim, so no
/// distinct diagnostic is lost.
pub(crate) fn fold_repeats(text: &str) -> Cow<'_, str> {
    let lines: Vec<&str> = text.lines().collect();
    let keys: Vec<Cow<str>> = lines.iter().map(|l| fold_key(l)).collect();
    let mut out = String::with_capacity(text.len());
    let mut folded = false;
    let mut i = 0;
    while i < lines.len() {
        let run = keys[i..].iter().take_while(|k| **k == keys[i]).count();
        if i > 0 {
            out.push('\n');
        }
        out.push_str(lines[i]);
        if run >= MIN_RUN {
            folded = true;
            out.push_str(&format!(" (×{run})"));
            i += run;
        } else {
            i += 1;
        }
    }
    if !folded {
        return Cow::Borrowed(text);
    }
    if text.ends_with('\n') {
        out.push('\n');
    }
    Cow::Owned(out)
}

fn fold_key(line: &str) -> Cow<'_, str> {
    if is_diagnostic(line) {
        return Cow::Borrowed(line);
    }
    let mut key = String::with_capacity(line.len());
    let mut in_digits = false;
    for c in line.chars() {
        if !c.is_ascii_digit() {
            key.push(c);
        } else if !in_digits {
            key.push('#');
        }
        in_digits = c.is_ascii_digit();
    }
    Cow::Owned(key)
}

fn is_diagnostic(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ["error", "panicked", "failed", "warning:"]
        .iter()
        .any(|word| lower.contains(word))
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

    #[test]
    fn fold_collapses_runs_of_three_or_more_identical_lines() {
        let folded = fold_repeats("start\nstep\nstep\nstep\nstep\nend\n");
        assert_eq!(folded, "start\nstep (×4)\nend\n");
    }

    #[test]
    fn fold_leaves_runs_shorter_than_three_and_borrows_the_text() {
        let text = "a\na\nb\nb\nc";
        assert!(matches!(fold_repeats(text), Cow::Borrowed(t) if t == text));
    }

    #[test]
    fn fold_masks_digit_runs_and_keeps_the_first_lines_digits() {
        let folded = fold_repeats("Downloading 1/10\nDownloading 2/10\nDownloading 10/10\nok");
        assert_eq!(folded, "Downloading 1/10 (×3)\nok");
    }

    #[test]
    fn fold_keeps_each_distinct_diagnostic_and_folds_only_exact_repeats() {
        let distinct = "error[E0308]: a\nerror[E0425]: a\nerror[E0599]: a";
        assert!(matches!(fold_repeats(distinct), Cow::Borrowed(_)));
        let repeated =
            "warning: unused x\nwarning: unused x\nwarning: unused x\nFAILED 1\nFAILED 2";
        assert_eq!(
            fold_repeats(repeated),
            "warning: unused x (×3)\nFAILED 1\nFAILED 2"
        );
    }

    #[test]
    fn fold_does_not_join_runs_separated_by_another_line() {
        let text = "x\nx\ny\nx\nx";
        assert!(matches!(fold_repeats(text), Cow::Borrowed(_)));
    }

    #[test]
    fn folded_output_names_the_archive_row_without_a_line_range() {
        let text = format!("head\n{}tail\n", "Compiling 1\n".repeat(50));
        let id = ArchiveId::new();
        let shown = visible_folding(&text, id, 1024, 2, 2);
        assert_eq!(
            shown,
            format!(
                "head\nCompiling 1 (×50)\ntail\n[… 1 KiB archived; repeated lines folded; \
                 expand #{id}]"
            )
        );
    }

    #[test]
    fn folded_output_still_cut_to_the_cap_keeps_the_handle() {
        let mut text = "Compiling 1\n".repeat(20);
        for n in 1..=40 {
            text.push_str(&format!("{}\n", "z".repeat(n)));
        }
        let id = ArchiveId::new();
        let shown = visible_folding(&text, id, 160, 1, 1);
        assert!(shown.len() <= 160, "{shown}");
        assert!(shown.starts_with("Compiling 1 (×20)\n[… "));
        assert!(shown.contains(&format!("repeated lines folded; expand #{id}]")));
        assert!(!shown.contains('–'), "no folded line range: {shown}");
    }

    #[test]
    fn unfoldable_output_takes_the_plain_cut() {
        let text = (1..=100)
            .map(|n| "y".repeat(n))
            .collect::<Vec<_>>()
            .join("\n");
        let id = ArchiveId::new();
        assert_eq!(
            visible_folding(&text, id, 200, 2, 2),
            visible(&text, id, 200, 2, 2)
        );
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
