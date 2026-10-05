// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Find files by name (plan.md T3.3): walks a root with the same
//! `ignore::WalkBuilder` configuration `grep` uses (`.gitignore` honoured,
//! hidden files included — see [`crate::grep::walker`]) and keeps the
//! entries a `globset` pattern matches. [`find`] is pure — it returns
//! unordered candidates with their mtime; the caller (`cox_tools::glob`)
//! owns `ToolCx`/`path::confine` and picks the mtime- or fuzzy-ranked order
//! and the result limit.
//!
//! [`rank_by_query`] re-ranks a candidate set by `nucleo`'s fuzzy score
//! instead of mtime, which is what makes "the auth handler, wherever it
//! lives" a single call rather than a guess at the path.

use std::path::Path;
use std::time::SystemTime;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32String};

use crate::grep::{glob_allows, walker};

/// A candidate path with the two keys it can be ordered by.
pub struct Candidate {
    /// The path as shown to the caller.
    pub display: String,
    /// Last modification time, the key for newest-first ordering.
    pub mtime: SystemTime,
}

/// An invalid glob pattern handed to [`find`].
#[derive(Debug, thiserror::Error)]
#[error("invalid glob: {0}")]
pub struct GlobError(globset::Error);

/// Name-based file lookup over the gitignore-aware walk: every file under
/// `root` that `pattern` matches (basename or full path), unordered, each
/// with its mtime for the caller to sort or rank by.
pub fn find(root: &Path, pattern: &str) -> Result<Vec<Candidate>, GlobError> {
    let matcher = globset::Glob::new(pattern)
        .map_err(GlobError)?
        .compile_matcher();

    let mut found: Vec<Candidate> = Vec::new();
    for entry in walker(&root.to_path_buf()).build() {
        let Ok(entry) = entry else { continue }; // unreadable dir entry: skip, not fatal
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if !glob_allows(&matcher, path, entry.file_name()) {
            continue;
        }
        // A file that vanished between the walk and the stat, or whose
        // mtime the platform withholds, still belongs in the list; it
        // just sorts as oldest.
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified().map_err(Into::into))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        found.push(Candidate {
            display: path.display().to_string(),
            mtime,
        });
    }

    Ok(found)
}

/// Reorders `found` by `nucleo`'s fuzzy score, best first, dropping paths the
/// query does not match at all.
pub fn rank_by_query(found: &mut Vec<Candidate>, query: &str) {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

    let mut scored: Vec<(u32, Candidate)> = std::mem::take(found)
        .into_iter()
        .filter_map(|c| {
            let haystack = Utf32String::from(c.display.as_str());
            pattern
                .score(haystack.slice(..), &mut matcher)
                .map(|score| (score, c))
        })
        .collect();
    // Ties broken by path so the order is stable across runs.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.display.cmp(&b.1.display)));
    *found = scored.into_iter().map(|(_, c)| c).collect();
}

/// Every file under `root` the ignore rules allow, relative to it and
/// sorted: the TUI's `@` picker candidates. Same walk `find` uses, so what
/// the picker offers is what `glob` would find. The binary calls this and
/// hands the list to `cox-tui`, which may not depend on this crate
/// (plan.md §1.1).
pub fn workspace_files(root: &Path) -> Vec<String> {
    let mut files: Vec<String> = walker(&root.to_path_buf())
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.display().to_string())
        })
        .collect();
    files.sort();
    files
}
