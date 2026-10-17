// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Content-hash line anchors (plan.md T69.7): `read` prints `N:abc→line`, and
//! `edit` takes `{op, anchor, content}` batches that name lines by that
//! anchor instead of quoting them. Separate from `edit.rs` because `read`
//! renders the same anchors and the two must hash identically.
//!
//! The hash is a 3-letter code (26^3 buckets) of the line with its whitespace
//! collapsed, so re-indenting a line keeps its anchor while any content change
//! almost always breaks it. A collision lets a changed line through about once
//! in 17 576 edits; the cost of a miss is the same as `old_string` matching
//! the wrong text, and the archived pre-image makes it recoverable.

use cox_protocol::ToolError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Distinct 3-letter codes: `26^3`.
const BUCKETS: u32 = 26 * 26 * 26;

/// Lines of context around a stale line: the one before it and the two after.
const NEIGHBOURS_BEFORE: usize = 1;
const NEIGHBOURS_AFTER: usize = 2;

/// One anchored edit. `content` may span lines; an empty `content` deletes the
/// anchored line (`replace`) or adds nothing (`insert_after`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Replace { anchor: String, content: String },
    InsertAfter { anchor: String, content: String },
}

impl Op {
    fn anchor(&self) -> &str {
        match self {
            Self::Replace { anchor, .. } | Self::InsertAfter { anchor, .. } => anchor,
        }
    }

    /// Applying higher ranks first at one line keeps `replace` from shifting
    /// the position `insert_after` targets.
    fn rank(&self) -> u8 {
        match self {
            Self::Replace { .. } => 0,
            Self::InsertAfter { .. } => 1,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnchorError {
    #[error("`ops` is empty; pass at least one {{op, anchor, content}}")]
    Empty,
    #[error("malformed anchor {0:?}; expected `N:abc` as printed by `read` with `anchors: true`")]
    Malformed(String),
    #[error("stale anchor, nothing was changed; re-anchor from the fresh lines below:\n{0}")]
    Stale(String),
    #[error("two `{op}` ops target line {line}; merge them into one")]
    Duplicate { op: &'static str, line: usize },
}

impl From<AnchorError> for ToolError {
    fn from(e: AnchorError) -> Self {
        // `ToolError` has no variant that carries a message besides `Denied`
        // (see the deviation note in `edit.rs`).
        ToolError::Denied { why: e.to_string() }
    }
}

/// The 3-letter code of `line`, ignoring how its whitespace is laid out.
pub fn line_hash(line: &str) -> String {
    let mut h = Sha256::new();
    for (i, word) in line.split_whitespace().enumerate() {
        if i > 0 {
            h.update(b" ");
        }
        h.update(word.as_bytes());
    }
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % BUCKETS;
    // `n < 26^3`, so each digit is below 26 and the cast cannot truncate.
    [n / 676, n / 26 % 26, n % 26]
        .iter()
        .map(|&d| char::from(b'a' + d as u8))
        .collect()
}

/// `N:abc→line` for the 1-based line number `n`.
pub fn render(n: usize, line: &str) -> String {
    format!("{n}:{}→{line}", line_hash(line))
}

fn parse_anchor(anchor: &str) -> Result<(usize, &str), AnchorError> {
    let bad = || AnchorError::Malformed(anchor.to_string());
    let (n, hash) = anchor.split_once(':').ok_or_else(bad)?;
    let n: usize = n.parse().map_err(|_| bad())?;
    if n == 0 || hash.len() != 3 || !hash.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(bad());
    }
    Ok((n, hash))
}

fn content_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// What the model needs to re-anchor a rejected line: its fresh anchor and the
/// lines around it, or the file length when the line is gone.
fn describe_stale(lines: &[String], n: usize, anchor: &str) -> String {
    let Some(line) = lines.get(n - 1) else {
        return format!(
            "{anchor}: line {n} is past the end of the file ({} lines)",
            lines.len()
        );
    };
    let from = n.saturating_sub(1 + NEIGHBOURS_BEFORE);
    let to = (n + NEIGHBOURS_AFTER).min(lines.len());
    let nearby: Vec<String> = (from..to)
        .filter(|&i| i != n - 1)
        .map(|i| render(i + 1, &lines[i]))
        .collect();
    format!(
        "{anchor}: line {n} is now {}\nnearby:\n{}",
        render(n, line),
        nearby.join("\n")
    )
}

/// Applies `ops` to `content` (already `\n`-normalised). Every anchor is
/// checked against the original lines first, so one stale anchor rejects the
/// whole batch; the edits then run bottom-up so each keeps the line numbers
/// the model saw.
pub fn apply(content: &str, ops: &[Op]) -> Result<String, AnchorError> {
    if ops.is_empty() {
        return Err(AnchorError::Empty);
    }
    let trailing_newline = content.ends_with('\n');
    let mut lines = content_lines(content);

    let mut stale = Vec::new();
    let mut resolved: Vec<(usize, &Op)> = Vec::new();
    for op in ops {
        let (n, hash) = parse_anchor(op.anchor())?;
        match lines.get(n - 1) {
            Some(line) if line_hash(line) == hash => resolved.push((n - 1, op)),
            _ => stale.push(describe_stale(&lines, n, op.anchor())),
        }
    }
    if !stale.is_empty() {
        return Err(AnchorError::Stale(stale.join("\n\n")));
    }

    resolved.sort_by_key(|&(idx, op)| (idx, op.rank()));
    if let Some(pair) = resolved
        .windows(2)
        .find(|w| w[0].0 == w[1].0 && w[0].1.rank() == w[1].1.rank())
    {
        return Err(AnchorError::Duplicate {
            op: if pair[0].1.rank() == 0 {
                "replace"
            } else {
                "insert_after"
            },
            line: pair[0].0 + 1,
        });
    }

    for &(idx, op) in resolved.iter().rev() {
        match op {
            Op::Replace { content, .. } => {
                lines.splice(idx..=idx, content_lines(content));
            }
            Op::InsertAfter { content, .. } => {
                lines.splice(idx + 1..idx + 1, content_lines(content));
            }
        }
    }

    let mut out = lines.join("\n");
    if trailing_newline && !lines.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(anchor: &str, content: &str) -> Op {
        Op::Replace {
            anchor: anchor.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn hash_ignores_whitespace_layout_but_not_content() {
        assert_eq!(line_hash("    let  x = 1;"), line_hash("\tlet x = 1;  "));
        assert_ne!(line_hash("let x = 1;"), line_hash("let x = 2;"));
        assert_eq!(line_hash("a").len(), 3);
    }

    #[test]
    fn anchors_parse_only_in_the_printed_shape() {
        assert_eq!(parse_anchor("12:abc"), Ok((12, "abc")));
        for bad in [
            "", "abc", "0:abc", "1:ab", "1:ABC", "x:abc", "1:abcd", "-1:abc",
        ] {
            assert!(parse_anchor(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn empty_replace_deletes_the_line() {
        let a = format!("2:{}", line_hash("b"));
        assert_eq!(
            apply("a\nb\nc\n", &[replace(&a, "")]).as_deref(),
            Ok("a\nc\n")
        );
    }

    #[test]
    fn same_line_replace_and_insert_after_both_apply() {
        let a = format!("1:{}", line_hash("a"));
        let ops = [
            replace(&a, "A"),
            Op::InsertAfter {
                anchor: a.clone(),
                content: "a2".into(),
            },
        ];
        assert_eq!(apply("a\nb\n", &ops).as_deref(), Ok("A\na2\nb\n"));
    }

    #[test]
    fn two_ops_on_one_line_of_the_same_kind_are_rejected() {
        let a = format!("1:{}", line_hash("a"));
        let err = apply("a\n", &[replace(&a, "x"), replace(&a, "y")]).expect_err("duplicate");
        assert_eq!(
            err,
            AnchorError::Duplicate {
                op: "replace",
                line: 1
            }
        );
    }

    #[test]
    fn line_past_the_end_is_stale_with_the_file_length() {
        let err = apply("a\n", &[replace("5:aaa", "x")]).expect_err("stale");
        assert!(
            err.to_string()
                .contains("past the end of the file (1 lines)")
        );
    }

    #[test]
    fn empty_batch_is_rejected() {
        assert_eq!(apply("a\n", &[]), Err(AnchorError::Empty));
    }
}
