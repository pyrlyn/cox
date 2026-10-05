// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Ripgrep-equivalent content search (plan.md T3.3): walks a root with
//! `ignore::WalkBuilder` (`.gitignore` honoured, hidden files included),
//! searches each file with `grep-regex` + `grep-searcher`, and formats
//! `-n --no-heading`-style output (`path:line:text`, context lines as
//! `path-line-text` with a bare `--` between non-contiguous groups — the
//! same shapes `rg` prints). Pure: [`search`] takes a root and returns the
//! formatted [`Line`]s; the caller (`cox_tools::grep`) owns `ToolCx`,
//! `path::confine` and archiving the over-cap tail.

use std::path::{Path, PathBuf};

use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;

/// One formatted output line plus whether it counts toward a caller's
/// match cap (context/`--` break lines don't).
pub struct Line {
    /// The line as `rg -n --no-heading` prints it, `path` prefix included.
    pub text: String,
    /// Whether this is a match line (counted toward the cap) rather than a
    /// context or `--` break line.
    pub is_match: bool,
}

/// A `grep_searcher::Sink` that formats matched/context lines the way `rg
/// -n --no-heading` does, prefixed with `path`.
struct GrepSink<'a> {
    path: &'a Path,
    lines: Vec<Line>,
}

impl Sink for GrepSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        let Some(line_number) = mat.line_number() else {
            return Ok(true); // line numbers are always requested; skip defensively
        };
        let text = String::from_utf8_lossy(mat.bytes());
        self.lines.push(Line {
            text: format!(
                "{}:{}:{}",
                self.path.display(),
                line_number,
                text.trim_end_matches(['\n', '\r'])
            ),
            is_match: true,
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, std::io::Error> {
        let Some(line_number) = ctx.line_number() else {
            return Ok(true);
        };
        let text = String::from_utf8_lossy(ctx.bytes());
        self.lines.push(Line {
            text: format!(
                "{}-{}-{}",
                self.path.display(),
                line_number,
                text.trim_end_matches(['\n', '\r'])
            ),
            is_match: false,
        });
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, std::io::Error> {
        self.lines.push(Line {
            text: "--".to_string(),
            is_match: false,
        });
        Ok(true)
    }
}

/// A file's glob filter matches either its basename (`*.rs` at any depth,
/// gitignore-style) or its full path (patterns that spell out a directory).
pub(crate) fn glob_allows(
    glob: &globset::GlobMatcher,
    entry_path: &Path,
    file_name: &std::ffi::OsStr,
) -> bool {
    glob.is_match(file_name) || glob.is_match(entry_path)
}

/// An invalid regex or glob pattern handed to [`search`].
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The regex pattern did not compile.
    #[error("invalid pattern: {0}")]
    Pattern(grep_regex::Error),
    /// The glob filter did not parse.
    #[error("invalid glob: {0}")]
    Glob(globset::Error),
}

/// Ripgrep-equivalent content search over `root`: `ignore::WalkBuilder`
/// (`.gitignore` honoured, hidden files included) + `grep-regex`/
/// `grep-searcher`. `glob` filters which files are searched (matched
/// against the basename or the full path); `context` adds that many lines
/// of context before and after each match.
pub fn search(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    context: Option<usize>,
) -> Result<Vec<Line>, SearchError> {
    let matcher = RegexMatcher::new(pattern).map_err(SearchError::Pattern)?;
    let glob_matcher = match glob {
        Some(g) => Some(
            globset::Glob::new(g)
                .map_err(SearchError::Glob)?
                .compile_matcher(),
        ),
        None => None,
    };

    let walker = walker(&root.to_path_buf());

    let mut all: Vec<Line> = Vec::new();
    for entry in walker.build() {
        let Ok(entry) = entry else { continue }; // unreadable dir entry: skip, not fatal
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let entry_path = entry.path();
        if let Some(gm) = &glob_matcher
            && !glob_allows(gm, entry_path, entry.file_name())
        {
            continue;
        }

        let mut builder = SearcherBuilder::new();
        builder.line_number(true);
        if let Some(n) = context {
            builder.before_context(n).after_context(n);
        }
        let mut searcher = builder.build();
        let mut sink = GrepSink {
            path: entry_path,
            lines: Vec::new(),
        };
        // A search error (binary content, unreadable file) just skips
        // that file rather than failing the whole call.
        if searcher
            .search_path(&matcher, entry_path, &mut sink)
            .is_ok()
        {
            all.extend(sink.lines);
        }
    }

    Ok(all)
}

/// The gitignore-aware walk shared by `grep` and `glob`: `.gitignore`
/// honoured, hidden files included. `require_git(false)`: a `.gitignore`
/// states intent whether or not a `.git` directory happens to sit above it,
/// and a worktree the agent is handed may not be a repository at all.
/// `filter_entry` skips a `.git` directory or file at any depth: it is
/// git's own store (or, in a worktree, a pointer to it), never a source a
/// blind walk should list. A path the model or user names explicitly still
/// reaches it — `read`, `write` and friends resolve through
/// `cox_sandbox::path::confine` directly and never call this walker.
pub(crate) fn walker(root: &PathBuf) -> WalkBuilder {
    let mut w = WalkBuilder::new(root);
    w.hidden(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|entry| entry.file_name() != ".git");
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nested `.git` directory (hooks, objects — anything git owns) never
    /// reaches the walk, at any depth; a sibling file with `.git` only as a
    /// substring of its name is unaffected.
    #[test]
    fn walker_skips_a_git_directory_at_any_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".git/hooks")).expect("mkdir");
        std::fs::write(dir.path().join(".git/hooks/pre-commit"), "#!/bin/sh").expect("write");
        std::fs::create_dir_all(dir.path().join("src/nested/.git")).expect("mkdir");
        std::fs::write(dir.path().join("src/nested/.git/HEAD"), "ref: x").expect("write");
        std::fs::write(dir.path().join("src/gitignore.rs"), "// not git's own file")
            .expect("write");

        let paths: Vec<String> = walker(&dir.path().to_path_buf())
            .build()
            .filter_map(Result::ok)
            .map(|e| e.path().display().to_string())
            .collect();

        assert!(
            !paths.iter().any(|p| p.contains(".git")),
            "walk listed a .git entry: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("gitignore.rs")),
            "walk dropped an unrelated file: {paths:?}"
        );
    }

    /// `search`'s content search must not descend into `.git/` either — it
    /// used to build its own `WalkBuilder` instead of the shared `walker`.
    #[test]
    fn search_does_not_match_inside_the_git_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".git")).expect("mkdir");
        std::fs::write(dir.path().join(".git/config"), "needle").expect("write");
        std::fs::write(dir.path().join("real.txt"), "needle").expect("write");

        let lines = search(dir.path(), "needle", None, None).expect("search");

        assert_eq!(
            lines.len(),
            1,
            "{:?}",
            lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        assert!(lines[0].text.contains("real.txt"), "{}", lines[0].text);
    }
}
