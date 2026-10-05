// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `DiffModel` (T37.23.5, DT§4.3): a `ToolResult.diff` as hunks of lines,
//! each with its kind, its old and new line numbers and its highlighted
//! runs, so the desktop app draws an edit without parsing unified text.
//! Also the unified-text parse the TUI's `diff` draws from, so both surfaces
//! read a hunk the same way, and the one word diff of a replaced line pair
//! both surfaces mark (T37.23.11), and `revert_hunk`, which puts one of
//! Review's hunks back (T51.18) from the same line diff `between` draws.
//! Outside `diff` because that module draws with ratatui, and `cox-app`
//! builds without the `ratatui` feature.

use std::ops::Range;
use std::path::{Path, PathBuf};

use cox_protocol::types::Diff;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, DiffTag, TextDiff};

use crate::doc::{StyledLine, StyledSpan};
use crate::markdown::{highlight_runs, theme_variants};

/// One file's change, hunk by hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffModel {
    pub path: PathBuf,
    pub hunks: Vec<DiffHunk>,
    /// `content_digest` of the new side's bytes when Review built the
    /// model from the file on disk (T51.20), so a hunk revert names the
    /// bytes it was shown; `None` for a tool's diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// An `@@` header and the lines under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// `@@ -41,12 +41,26 @@ impl Backoff`; empty for lines before any header.
    pub header: String,
    pub lines: Vec<DiffLine>,
    /// The hunk's place in its model, from 0: for Review's model, the
    /// index [`revert_hunk`] puts back (T51.20).
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// The line's number before the edit; `None` for an added line.
    pub old: Option<u32>,
    /// The line's number after the edit; `None` for a removed line.
    pub new: Option<u32>,
    /// The body without its marker, highlighted by the file's extension.
    /// Cut at every `words` edge, so a span is wholly inside or outside one.
    pub spans: StyledLine,
    /// The words that changed, when this line is one side of a replaced
    /// pair; empty otherwise, and for a pair past `WORD_DIFF_CAP`.
    #[serde(default)]
    pub words: Vec<WordRange>,
}

/// A changed stretch of a line's body, as UTF-8 byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

/// A hunk line that carries file content: its marker and the source under it.
/// `+++`/`---` are file markers, not additions, so they are not content.
fn content(l: &str) -> Option<(&str, &str)> {
    if l.starts_with("+++") || l.starts_with("---") {
        return None;
    }
    match l.chars().next() {
        Some('+') | Some('-') | Some(' ') => Some(l.split_at(1)),
        _ => None,
    }
}

/// One line of the unified text: a content line knows its body's index and
/// its line numbers; anything else (`@@`, file markers, `index`, `\ No
/// newline`) is printed whole.
pub(crate) enum Row<'a> {
    Meta(&'a str),
    Body {
        /// The whole line, which only the TUI's `diff` prints.
        #[cfg_attr(not(feature = "ratatui"), allow(dead_code))]
        raw: &'a str,
        marker: &'a str,
        body: usize,
        old: Option<u32>,
        new: Option<u32>,
    },
}

/// `@@ -a,b +c,d @@` → `(a, c)`, the first old and new line numbers.
fn hunk_start(l: &str) -> Option<(u32, u32)> {
    let mut parts = l.split_whitespace().skip(1);
    let num = |p: Option<&str>, sign: char| -> Option<u32> {
        p?.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    Some((num(parts.next(), '-')?, num(parts.next(), '+')?))
}

/// The rows of `unified` and the bodies of its content lines, in order.
pub(crate) fn parse(unified: &str) -> (Vec<Row<'_>>, Vec<&str>) {
    let (mut rows, mut bodies) = (Vec::new(), Vec::new());
    let (mut old, mut new) = (1, 1);
    for l in unified.lines() {
        if let Some((o, n)) = l.starts_with("@@").then(|| hunk_start(l)).flatten() {
            (old, new) = (o, n);
        }
        let Some((marker, text)) = content(l) else {
            rows.push(Row::Meta(l));
            continue;
        };
        let (o, n) = match marker {
            "-" => (Some(old), None),
            "+" => (None, Some(new)),
            _ => (Some(old), Some(new)),
        };
        old += u32::from(o.is_some());
        new += u32::from(n.is_some());
        rows.push(Row::Body {
            raw: l,
            marker,
            body: bodies.len(),
            old: o,
            new: n,
        });
        bodies.push(text);
    }
    (rows, bodies)
}

/// A pair with a longer line gets no word diff: the diff's cost grows with
/// the product of the two lengths, and a minified line would be noise word
/// by word anyway.
pub(crate) const WORD_DIFF_CAP: usize = 400;

/// A side-by-side row: a whole-width meta line, or the rows (indices into
/// the `Row` list) shown in the old and the new pane.
pub(crate) enum Aligned {
    /// Only the TUI's side-by-side layout prints a meta row by its index.
    Meta(#[cfg_attr(not(feature = "ratatui"), allow(dead_code))] usize),
    Pair(Option<usize>, Option<usize>),
}

/// Rows in pane order: a run of `-` lines and the `+` run right after it
/// are zipped line by line, so the n-th removed line faces the n-th added
/// one — that pair is also what the word diff compares.
pub(crate) fn align(rows: &[Row<'_>]) -> Vec<Aligned> {
    let marker = |i: usize| match rows.get(i) {
        Some(Row::Body { marker, .. }) => Some(*marker),
        _ => None,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        match marker(i) {
            None => {
                out.push(Aligned::Meta(i));
                i += 1;
            }
            Some("-") | Some("+") => {
                let dels = (i..).take_while(|&j| marker(j) == Some("-")).count();
                let adds = (i + dels..).take_while(|&j| marker(j) == Some("+")).count();
                for k in 0..dels.max(adds) {
                    out.push(Aligned::Pair(
                        (k < dels).then_some(i + k),
                        (k < adds).then_some(i + dels + k),
                    ));
                }
                i += dels + adds;
            }
            Some(_) => {
                out.push(Aligned::Pair(Some(i), Some(i)));
                i += 1;
            }
        }
    }
    out
}

/// Byte ranges of each side's changed words, adjacent ones merged.
pub(crate) type Words = (Vec<Range<usize>>, Vec<Range<usize>>);

/// A removed line facing an added one: their body indices and the words
/// that changed, `None` when either is past `WORD_DIFF_CAP`.
pub(crate) struct Replaced {
    pub old: usize,
    pub new: usize,
    pub words: Option<Words>,
}

/// Every replaced pair `align` zipped, with its word diff.
pub(crate) fn replaced(rows: &[Row<'_>], aligned: &[Aligned], texts: &[&str]) -> Vec<Replaced> {
    let body_of = |i: usize| match rows.get(i) {
        Some(Row::Body { body, .. }) => Some(*body),
        _ => None,
    };
    let text = |i: usize| texts.get(i).copied().unwrap_or_default();
    aligned
        .iter()
        .filter_map(|a| match *a {
            Aligned::Pair(Some(l), Some(r)) if l != r => Some((body_of(l)?, body_of(r)?)),
            _ => None,
        })
        .map(|(old, new)| {
            let (o, n) = (text(old), text(new));
            let within = o.len() <= WORD_DIFF_CAP && n.len() <= WORD_DIFF_CAP;
            Replaced {
                old,
                new,
                words: within.then(|| words(o, n)),
            }
        })
        .collect()
}

/// The word diff itself, the one both surfaces draw from.
fn words(old: &str, new: &str) -> Words {
    let extend = |ranges: &mut Vec<Range<usize>>, at: &mut usize, len: usize| {
        match ranges.last_mut() {
            Some(last) if last.end == *at => last.end += len,
            _ => ranges.push(*at..*at + len),
        }
        *at += len;
    };
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let (mut at_old, mut at_new) = (0, 0);
    for c in TextDiff::from_words(old, new).iter_all_changes() {
        let len = c.value().len();
        match c.tag() {
            _ if len == 0 => {}
            ChangeTag::Equal => (at_old, at_new) = (at_old + len, at_new + len),
            ChangeTag::Delete => extend(&mut left, &mut at_old, len),
            ChangeTag::Insert => extend(&mut right, &mut at_new, len),
        }
    }
    (left, right)
}

/// `spans` cut at every edge of `words`, so no span straddles one; an edge
/// off a char boundary (never one `words` makes) is left uncut.
fn cut(spans: StyledLine, words: &[Range<usize>]) -> StyledLine {
    let mut edges = words.iter().flat_map(|w| [w.start, w.end]).peekable();
    let (mut out, mut at) = (Vec::with_capacity(spans.len()), 0);
    for mut span in spans {
        let end = at + span.text.len();
        while let Some(&edge) = edges.peek() {
            if edge >= end {
                break;
            }
            edges.next();
            let Some((head, tail)) = span.text.split_at_checked(edge.saturating_sub(at)) else {
                continue;
            };
            if head.is_empty() {
                continue;
            }
            let (head, tail) = (head.to_owned(), tail.to_owned());
            out.push(StyledSpan {
                text: head,
                ..span.clone()
            });
            (span.text, at) = (tail, edge);
        }
        at = end;
        out.push(span);
    }
    out
}

/// Lines of context around a hunk, as the edit tools write.
const CONTEXT: usize = 3;

/// The change from `old` to `new` of the file at `path`, as [`model`] reads
/// it: for two texts no tool diffed, such as Review's checkpoint copy and the
/// file on disk (T37.28.2). Hunk `i` of the model is the one
/// [`revert_hunk`]`(old, new, i)` puts back.
pub fn between(path: &Path, old: &str, new: &str, theme: &str) -> DiffModel {
    let unified = TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(CONTEXT)
        .to_string();
    let diff = Diff {
        path: path.to_path_buf(),
        unified,
    };
    model(&diff, theme)
}

/// Why a hunk could not be put back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HunkError {
    /// The diff of these texts has no hunk at that index: the file changed
    /// since Review drew it, so the hunk shown is not the one there now.
    #[error("that hunk is no longer in the diff; review the file again")]
    Stale,
}

/// `now` with hunk `index` of [`between`]`(before, now)` put back to
/// `before`, every other line kept as it is, line endings included. The
/// hunks are the same line diff's groups `between`'s unified text is cut
/// from, so an index means the hunk Review shows. Whether `now` is still the
/// text Review diffed is the caller's check (the core compares digests);
/// an index past the last hunk is [`HunkError::Stale`].
pub fn revert_hunk(before: &str, now: &str, index: usize) -> Result<String, HunkError> {
    let diff = TextDiff::from_lines(before, now);
    let group = diff
        .grouped_ops(CONTEXT)
        .into_iter()
        // As the unified text skips them, so the indices line up.
        .filter(|ops| !ops.is_empty())
        .nth(index)
        .ok_or(HunkError::Stale)?;
    let mut changed = group.iter().filter(|op| op.tag() != DiffTag::Equal);
    let (Some(first), last) = (changed.next(), changed.next_back()) else {
        return Err(HunkError::Stale);
    };
    let last = last.unwrap_or(first);
    // Equal runs between a hunk's changes read alike on both sides, so the
    // hunk's whole span on `now` is swapped for its span on `before`.
    let old = first.old_range().start..last.old_range().end;
    let new = first.new_range().start..last.new_range().end;
    let before: Vec<&str> = diff.iter_old_slices().collect();
    let now: Vec<&str> = diff.iter_new_slices().collect();
    let (Some(head), Some(put_back), Some(tail)) =
        (now.get(..new.start), before.get(old), now.get(new.end..))
    else {
        return Err(HunkError::Stale);
    };
    Ok([head, put_back, tail].concat().concat())
}

/// `diff` as hunks. Bodies go through the one syntect pass the TUI's diff
/// and fenced blocks use, highlighted by the file's extension with both of
/// `theme`'s variants (`rgb` dark, `light` light; A95), so the app draws the
/// one its appearance asks for; a file without one stays plain, as it does
/// in the TUI. File markers, `index` and `\ No newline` lines are dropped:
/// the UI draws the path.
pub fn model(diff: &Diff, theme: &str) -> DiffModel {
    let (rows, texts) = parse(&diff.unified);
    let [dark, light] = theme_variants(theme);
    let mut spans = match diff.path.extension().and_then(|e| e.to_str()) {
        Some(token) => {
            let mut spans = highlight_runs(token, &texts, &dark);
            // Runs split where the syntax's scopes change, whatever the
            // theme, so both passes cut a line alike.
            let lit = highlight_runs(token, &texts, &light);
            for (line, lit) in spans.iter_mut().zip(lit) {
                for (span, lit) in line.iter_mut().zip(lit) {
                    if span.text == lit.text {
                        span.light = lit.rgb;
                    }
                }
            }
            spans
        }
        None => Vec::new(),
    };
    spans.resize_with(texts.len(), Vec::new);
    for (line, text) in spans.iter_mut().zip(&texts) {
        if line.is_empty() && !text.is_empty() {
            *line = vec![StyledSpan::plain(*text)];
        }
    }
    let mut words = vec![Vec::new(); texts.len()];
    for pair in replaced(&rows, &align(&rows), &texts) {
        if let (Some((o, n)), true) = (pair.words, pair.new < words.len()) {
            (words[pair.old], words[pair.new]) = (o, n);
        }
    }
    let mut hunks: Vec<DiffHunk> = Vec::new();
    for row in rows {
        match row {
            Row::Meta(l) if l.starts_with("@@") => hunks.push(DiffHunk {
                header: l.to_owned(),
                lines: Vec::new(),
                index: next_index(&hunks),
            }),
            Row::Meta(_) => {}
            Row::Body {
                marker,
                body,
                old,
                new,
                ..
            } => {
                if hunks.is_empty() {
                    hunks.push(DiffHunk {
                        header: String::new(),
                        lines: Vec::new(),
                        index: 0,
                    });
                }
                let kind = match marker {
                    "+" => DiffLineKind::Add,
                    "-" => DiffLineKind::Del,
                    _ => DiffLineKind::Context,
                };
                let changed = words.get_mut(body).map(std::mem::take).unwrap_or_default();
                let offset = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
                let line = DiffLine {
                    kind,
                    old,
                    new,
                    spans: cut(
                        spans.get_mut(body).map(std::mem::take).unwrap_or_default(),
                        &changed,
                    ),
                    words: changed
                        .iter()
                        .map(|w| WordRange {
                            start: offset(w.start),
                            end: offset(w.end),
                        })
                        .collect(),
                };
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.push(line);
                }
            }
        }
    }
    DiffModel {
        path: diff.path.clone(),
        hunks,
        digest: None,
    }
}

/// The index the next hunk pushed onto `hunks` takes.
fn next_index(hunks: &[DiffHunk]) -> u32 {
    u32::try_from(hunks.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::theme_name;

    fn diff(path: &str, unified: &str) -> Diff {
        Diff {
            path: path.into(),
            unified: unified.to_owned(),
        }
    }

    type Numbered = (DiffLineKind, Option<u32>, Option<u32>);

    fn numbers(m: &DiffModel) -> Vec<Vec<Numbered>> {
        let line = |l: &DiffLine| (l.kind, l.old, l.new);
        m.hunks
            .iter()
            .map(|h| h.lines.iter().map(line).collect())
            .collect()
    }

    #[test]
    fn each_hunk_numbers_its_lines_from_its_own_header() {
        let unified = "--- a/x.rs\n+++ b/x.rs\n@@ -3,2 +3,2 @@ fn a\n ctx\n-old\n+new\n\
            @@ -40 +40,2 @@\n keep\n+more\n\\ No newline at end of file\n";
        let m = model(&diff("x.rs", unified), theme_name(true, ""));
        use DiffLineKind::{Add, Context, Del};
        assert_eq!(
            m.hunks
                .iter()
                .map(|h| h.header.as_str())
                .collect::<Vec<_>>(),
            ["@@ -3,2 +3,2 @@ fn a", "@@ -40 +40,2 @@"]
        );
        assert_eq!(
            numbers(&m),
            [
                vec![
                    (Context, Some(3), Some(3)),
                    (Del, Some(4), None),
                    (Add, None, Some(4))
                ],
                vec![(Context, Some(40), Some(40)), (Add, None, Some(41))],
            ]
        );
    }

    #[test]
    fn a_known_extension_highlights_the_body_without_its_marker() {
        let m = model(
            &diff("x.rs", "@@ -1 +1 @@\n+fn main() {}\n"),
            "base16-ocean.dark",
        );
        let spans = &m.hunks[0].lines[0].spans;
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "fn main() {}");
        assert!(spans.len() > 1 && spans.iter().all(|s| s.rgb.is_some()));
    }

    #[test]
    fn a_one_word_change_marks_that_word_on_both_lines() {
        let m = model(
            &diff(
                "x.rs",
                "@@ -1 +1 @@\n-let a = old + 1;\n+let a = new + 1;\n",
            ),
            "base16-ocean.dark",
        );
        let lines = &m.hunks[0].lines;
        let marked = |l: &DiffLine| -> Vec<String> {
            let text: String = l.spans.iter().map(|s| s.text.as_str()).collect();
            l.words
                .iter()
                .map(|w| text[w.start as usize..w.end as usize].to_owned())
                .collect()
        };
        assert_eq!(marked(&lines[0]), ["old"]);
        assert_eq!(marked(&lines[1]), ["new"]);
        // A span ends at each edge, so a view marks whole spans.
        let edges = |l: &DiffLine| {
            let mut at = 0;
            let mut ends = vec![0];
            for s in &l.spans {
                at += s.text.len() as u32;
                ends.push(at);
            }
            l.words
                .iter()
                .all(|w| ends.contains(&w.start) && ends.contains(&w.end))
        };
        assert!(edges(&lines[0]) && edges(&lines[1]));
    }

    #[test]
    fn only_a_paired_line_within_the_cap_carries_words() {
        let long = "x".repeat(WORD_DIFF_CAP + 1);
        let unified = format!("@@ -1,2 +1,3 @@\n ctx\n-{long}\n+{long}y\n+extra\n");
        let m = model(&diff("NOTES", &unified), "base16-ocean.dark");
        assert!(m.hunks[0].lines.iter().all(|l| l.words.is_empty()));
        let m = model(&diff("NOTES", "@@ -1 +1,2 @@\n-a b\n+a c\n+d\n"), "x");
        let words: Vec<_> = m.hunks[0].lines.iter().map(|l| l.words.len()).collect();
        assert_eq!(words, [1, 1, 0]);
    }

    #[test]
    fn align_zips_a_removed_run_against_the_added_run_after_it() {
        let (rows, _) = parse("@@ -1,3 +1,2 @@\n-a\n-b\n-c\n+x\n+y\n k\n");
        let pairs: Vec<_> = align(&rows)
            .into_iter()
            .map(|a| match a {
                Aligned::Meta(i) => (Some(i), None),
                Aligned::Pair(l, r) => (l, r),
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                (Some(0), None),
                (Some(1), Some(4)),
                (Some(2), Some(5)),
                (Some(3), None),
                (Some(6), Some(6)),
            ]
        );
    }

    #[test]
    fn a_file_without_an_extension_stays_plain() {
        let m = model(
            &diff("NOTES", "@@ -1 +1,2 @@\n-a\n+b\n+\n"),
            "base16-ocean.dark",
        );
        let lines = &m.hunks[0].lines;
        assert_eq!(lines[0].spans, [StyledSpan::plain("a")]);
        assert!(lines[2].spans.is_empty());
    }

    #[test]
    fn a_theme_pairs_with_its_sibling_and_an_unpaired_one_serves_both() {
        let pair = ["base16-ocean.dark", "base16-ocean.light"].map(String::from);
        assert_eq!(theme_variants("base16-ocean.dark"), pair);
        assert_eq!(theme_variants("base16-ocean.light"), pair);
        assert_eq!(theme_variants("no-such-theme"), pair);
        let solarized = ["Solarized (dark)", "Solarized (light)"].map(String::from);
        assert_eq!(theme_variants("Solarized (light)"), solarized);
        assert_eq!(
            theme_variants("InspiredGitHub"),
            ["InspiredGitHub"; 2].map(String::from)
        );
    }

    #[test]
    fn every_highlighted_run_carries_the_light_variant_too() {
        let m = model(
            &diff("x.rs", "@@ -1 +1 @@\n+fn main() {}\n"),
            "base16-ocean.light",
        );
        let spans = &m.hunks[0].lines[0].spans;
        assert!(spans.iter().all(|s| s.rgb.is_some() && s.light.is_some()));
        // The default foreground differs between the variants.
        assert!(spans.iter().any(|s| s.rgb != s.light));
        let plain = model(&diff("NOTES", "@@ -1 +1 @@\n+a\n"), "base16-ocean.dark");
        assert_eq!(plain.hunks[0].lines[0].spans, [StyledSpan::plain("a")]);
    }

    /// Twenty numbered lines with lines 2 and 18 replaced: two hunks, far
    /// enough apart that their context does not merge them.
    fn two_hunks() -> (String, String) {
        let before: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let now = before
            .replace("line 2\n", "line two\n")
            .replace("line 18\n", "line eighteen\n");
        (before, now)
    }

    #[test]
    fn revert_hunk_restores_only_that_hunk() {
        let (before, now) = two_hunks();
        let shown = between(Path::new("notes.txt"), &before, &now, "base16-ocean.dark");
        assert_eq!(shown.hunks.len(), 2);
        let reverted = revert_hunk(&before, &now, 1);
        assert_eq!(reverted, Ok(before.replace("line 2\n", "line two\n")));
        let reverted = revert_hunk(&before, &now, 0);
        assert_eq!(reverted, Ok(before.replace("line 18\n", "line eighteen\n")));
    }

    #[test]
    fn revert_hunk_of_an_added_file_region() {
        let before = "a\nb\n";
        let now = "a\nadded one\nadded two\nb\n";
        assert_eq!(revert_hunk(before, now, 0), Ok(before.to_owned()));
        // A file that did not exist at the checkpoint is all one added hunk.
        assert_eq!(revert_hunk("", now, 0), Ok(String::new()));
    }

    #[test]
    fn revert_hunk_refuses_a_stale_index() {
        let (before, now) = two_hunks();
        assert_eq!(revert_hunk(&before, &now, 2), Err(HunkError::Stale));
        // Reverted meanwhile: nothing is left to put back.
        assert_eq!(revert_hunk(&before, &before, 0), Err(HunkError::Stale));
    }

    #[test]
    fn revert_hunk_keeps_line_endings() {
        let before = "one\r\ntwo\r\nthree\r\n";
        let now = "one\r\nTWO\r\nthree\r\nfour";
        let reverted = revert_hunk(before, now, 0);
        // The one hunk holds both changes; the reverted text is `before`,
        // CRLF endings and all.
        assert_eq!(reverted, Ok(before.to_owned()));
        let mixed_before = "a\r\nb\nc\r\n";
        let mixed_now = "a\r\nB\nc\r\n";
        assert_eq!(
            revert_hunk(mixed_before, mixed_now, 0),
            Ok(mixed_before.to_owned())
        );
    }
}
