// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Resolving a parsed [`Patch`] against real files: matching hunks and
//! staging the result.
//!
//! Two things make this separate from [`super::parse`]. First, every
//! failure here is a *stale context* failure — the model read the file, the
//! file moved on — so hunks are matched progressively (exact, then ignoring
//! trailing whitespace, then ignoring whitespace entirely) instead of being
//! rejected on the first mismatch. Second, [`stage`] is pure: it takes a
//! `read` closure rather than touching the filesystem, so the whole
//! resolution algorithm is testable, and so a patch that fails on its
//! fourth file cannot have already written its first three (plan.md T3.5
//! step 3, all-or-nothing).
//!
//! The `apply_patch` `Tool` impl — path confinement, the archive write, the
//! atomic rename — lives in `cox_tools::v4a::tool` (T32.6): everything in
//! this crate is filesystem-free.

use std::collections::BTreeMap;

use cox_protocol::ToolError;

use super::parse::{Hunk, HunkLine, Op, Patch};

/// One file's before/after, staged in memory. `after == None` is a
/// deletion; `before == None` is a creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The path the content ends up at.
    pub path: String,
    /// The path it came from, when the patch renamed it.
    pub from: Option<String>,
    /// Content before the patch, or `None` for a new file.
    pub before: Option<String>,
    /// Content after the patch, or `None` for a deletion.
    pub after: Option<String>,
}

impl Change {
    /// The single-letter status used in the tool's per-file summary.
    pub fn status(&self) -> char {
        match (&self.before, &self.after, &self.from) {
            (None, _, _) => 'A',
            (_, None, _) => 'D',
            (_, _, Some(_)) => 'R',
            _ => 'M',
        }
    }
}

/// Resolves every op against the content `read` returns, in patch order.
/// Returns the full change set or the first error — nothing is written
/// here, which is what makes the whole patch all-or-nothing.
pub fn stage(
    patch: &Patch,
    read: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<Change>, ToolError> {
    // Keyed so a later op sees an earlier op's result rather than the file
    // on disk, and so two ops on one path are caught as a conflict.
    let mut staged: BTreeMap<String, Change> = BTreeMap::new();

    for op in &patch.ops {
        let path = op.path().to_string();
        if staged.contains_key(&path) {
            return Err(ToolError::Denied {
                why: format!("patch touches {path} twice"),
            });
        }
        let change = match op {
            Op::Add { lines, .. } => {
                if read(&path).is_some() {
                    return Err(ToolError::Denied {
                        why: format!("`*** Add File: {path}` but the file already exists"),
                    });
                }
                Change {
                    path: path.clone(),
                    from: None,
                    before: None,
                    // A V4A add ends the file with a newline; the grammar
                    // has no way to express one that does not. Zero `+`
                    // lines is the one exception — that is an empty file,
                    // not a file containing a blank line.
                    after: Some(if lines.is_empty() {
                        String::new()
                    } else {
                        lines.join("\n") + "\n"
                    }),
                }
            }
            Op::Delete { .. } => Change {
                path: path.clone(),
                from: None,
                before: Some(require(read, &path)?),
                after: None,
            },
            Op::Update { move_to, hunks, .. } => {
                let before = require(read, &path)?;
                let after = apply_hunks(&before, hunks)?;
                Change {
                    path: move_to.clone().unwrap_or_else(|| path.clone()),
                    from: move_to.as_ref().map(|_| path.clone()),
                    before: Some(before),
                    after: Some(after),
                }
            }
        };
        staged.insert(path, change);
    }
    Ok(staged.into_values().collect())
}

fn require(read: &dyn Fn(&str) -> Option<String>, path: &str) -> Result<String, ToolError> {
    read(path).ok_or(ToolError::NotFound)
}

/// Applies every hunk in order. Each hunk is located in the region after
/// the previous one, so a patch that edits the same snippet twice still
/// resolves; the error names the 1-based hunk index (T3.5 step 2).
fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, ToolError> {
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let mut cursor = 0usize;

    for (n, hunk) in hunks.iter().enumerate() {
        let old: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Keep(t) | HunkLine::Del(t) => Some(t.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();
        let new: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                HunkLine::Keep(t) | HunkLine::Add(t) => Some(t.clone()),
                HunkLine::Del(_) => None,
            })
            .collect();

        let from = seek_context(&lines, &hunk.context, cursor);
        let at = locate(&lines, &old, from, hunk.eof).map_err(|why| ToolError::Denied {
            why: format!("hunk {} {why}", n + 1),
        })?;
        lines.splice(at..at + old.len(), new.iter().cloned());
        cursor = at + new.len();
    }
    Ok(lines.join("\n"))
}

/// Advances past each non-empty `@@` header the file still contains. A
/// header that is *not* found is skipped rather than fatal: it is a
/// locator hint, and the hunk body itself is the real anchor.
fn seek_context(lines: &[String], context: &[String], start: usize) -> usize {
    let mut at = start;
    for c in context.iter().filter(|c| !c.trim().is_empty()) {
        if let Some(hit) = lines[at..].iter().position(|l| l.trim() == c.trim()) {
            at += hit + 1;
        }
    }
    at
}

/// The three normalisers, tried in order (T3.5 step 2): exact, then
/// ignoring trailing whitespace, then ignoring whitespace altogether.
const LEVELS: [fn(&str) -> String; 3] = [
    |s| s.to_string(),
    |s| s.trim_end().to_string(),
    |s| s.chars().filter(|c| !c.is_whitespace()).collect(),
];

/// Finds the one place `old` sits in `lines[from..]`. `eof` anchors to the
/// end of the file instead of searching. `Err` carries the reason, which
/// the caller prefixes with the hunk index.
fn locate(lines: &[String], old: &[&str], from: usize, eof: bool) -> Result<usize, String> {
    if old.len() > lines.len().saturating_sub(from) {
        return Err("is longer than the remaining file".to_string());
    }
    if eof {
        // Two anchors, because `split('\n')` on a file that ends with a
        // newline leaves a trailing empty element the patch author never
        // wrote: prefer the last *content* line, fall back to the literal
        // end for a file with no final newline.
        let content_end = match lines.last() {
            Some(l) if l.is_empty() => lines.len() - 1,
            _ => lines.len(),
        };
        for end in [content_end, lines.len()] {
            let Some(at) = end.checked_sub(old.len()) else {
                continue;
            };
            if at >= from && LEVELS.iter().any(|&norm| window_eq(lines, old, at, norm)) {
                return Ok(at);
            }
        }
        return Err("does not match the end of the file".to_string());
    }
    for norm in LEVELS {
        let hits: Vec<usize> = (from..=lines.len() - old.len())
            .filter(|&i| window_eq(lines, old, i, norm))
            .collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(hits[0]),
            n => {
                return Err(format!(
                    "matches {n} places (lines {}); add context to make it unique",
                    hits.iter()
                        .map(|i| (i + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    Err("does not match the file; re-read it and rebuild the hunk".to_string())
}

fn window_eq(lines: &[String], old: &[&str], at: usize, norm: fn(&str) -> String) -> bool {
    lines[at..at + old.len()]
        .iter()
        .zip(old)
        .all(|(have, want)| norm(have) == norm(want))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn staged(before: &[(&str, &str)], src: &str) -> Result<Vec<Change>, ToolError> {
        let files: BTreeMap<String, String> = before
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        stage(&parse(src).expect("parse"), &|p| files.get(p).cloned())
    }

    fn after(before: &[(&str, &str)], src: &str) -> String {
        let changes = staged(before, src).expect("stage");
        changes[0].after.clone().unwrap_or_default()
    }

    #[test]
    fn v4a_applies_an_exact_hunk() {
        let out = after(
            &[("a.rs", "one\ntwo\nthree\n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@\n one\n-two\n+TWO\n three\n*** End Patch",
        );
        assert_eq!(out, "one\nTWO\nthree\n");
    }

    #[test]
    fn v4a_falls_back_through_the_three_match_levels() {
        // Level 2: the file has trailing whitespace the model did not copy.
        let out = after(
            &[("a.rs", "let x = 1;   \n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@\n-let x = 1;\n+let x = 2;\n*** End Patch",
        );
        assert_eq!(out, "let x = 2;\n");

        // Level 3: interior whitespace differs too.
        let out = after(
            &[("a.rs", "let    x  =  1;\n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@\n-let x = 1;\n+let x = 2;\n*** End Patch",
        );
        assert_eq!(out, "let x = 2;\n");
    }

    #[test]
    fn v4a_ambiguous_hunk_names_the_hunk_and_the_lines() {
        let err = staged(
            &[("a.rs", "dup\nmid\ndup\n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@\n-dup\n+one\n*** End Patch",
        )
        .expect_err("two candidates must be refused");
        let ToolError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        assert!(why.starts_with("hunk 1 matches 2 places"), "got {why}");
        assert!(why.contains("lines 1, 3"), "got {why}");
    }

    #[test]
    fn v4a_context_disambiguates_a_repeated_snippet() {
        let out = after(
            &[("a.rs", "fn a() {\n  x\n}\nfn b() {\n  x\n}\n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@ fn b() {\n-  x\n+  y\n*** End Patch",
        );
        assert_eq!(out, "fn a() {\n  x\n}\nfn b() {\n  y\n}\n");
    }

    #[test]
    fn v4a_end_of_file_anchors_to_the_tail() {
        let out = after(
            &[("a.rs", "x\nlast\n")],
            "*** Begin Patch\n*** Update File: a.rs\n@@\n last\n+added\n*** End of File\n*** End Patch",
        );
        assert_eq!(out, "x\nlast\nadded\n");
    }

    #[test]
    fn v4a_a_failing_hunk_stages_nothing() {
        let err = staged(
            &[("a.rs", "ok\n"), ("b.rs", "ok\n")],
            "*** Begin Patch\n*** Delete File: a.rs\n*** Update File: b.rs\n@@\n-absent\n+x\n*** End Patch",
        )
        .expect_err("the second op must fail");
        assert!(
            matches!(&err, ToolError::Denied { why } if why.starts_with("hunk 1 does not match")),
            "got {err:?}"
        );
    }

    #[test]
    fn v4a_move_reports_both_paths() {
        let changes = staged(
            &[("a.rs", "x\n")],
            "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n@@\n-x\n+y\n*** End Patch",
        )
        .expect("stage");
        assert_eq!(changes[0].path, "b.rs");
        assert_eq!(changes[0].from.as_deref(), Some("a.rs"));
        assert_eq!(changes[0].status(), 'R');
    }

    #[test]
    fn v4a_add_refuses_to_clobber_and_delete_needs_the_file() {
        assert!(
            staged(
                &[("a.rs", "x\n")],
                "*** Begin Patch\n*** Add File: a.rs\n+y\n*** End Patch"
            )
            .is_err(),
            "adding over an existing file must be refused"
        );
        assert!(matches!(
            staged(
                &[],
                "*** Begin Patch\n*** Delete File: gone.rs\n*** End Patch"
            ),
            Err(ToolError::NotFound)
        ));
    }
}
