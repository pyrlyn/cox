// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `AGENTS.md`/`CLAUDE.md` chain (T7.1): which files load, in what
//! order, under what budget, and how `@path` includes expand. A pure
//! function over [`Roots`] so the order is testable on a fixture tree; the
//! caller resolves homes, git root and cwd — nothing here reads config or
//! the environment. The result is byte-stable for a given tree, which the
//! cache-stable prefix (plan.md §1.9) relies on.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Per-directory candidates, in load order. Claude's `CLAUDE.local.md` is
/// last so a personal file overrides the shared ones.
const PER_DIR: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".cox/AGENTS.md",
    ".claude/CLAUDE.md",
    "CLAUDE.local.md",
];

/// `@path` includes nest at most this deep (the top-level file is depth 0).
const MAX_DEPTH: usize = 3;

/// Where to look. `git_root` is `None` outside a repository, in which case
/// only `cwd` is searched.
#[derive(Debug, Clone)]
pub struct Roots {
    pub cox_home: Option<PathBuf>,
    pub claude_home: Option<PathBuf>,
    pub git_root: Option<PathBuf>,
    pub cwd: PathBuf,
}

/// The assembled block plus what to tell the user about it.
#[derive(Debug, Default, PartialEq)]
pub struct Loaded {
    /// `# Instructions\n## <path>\n<body>…`, empty when no file exists.
    pub block: String,
    /// Files that made it in, display paths in load order.
    pub files: Vec<String>,
    /// Dropped files, include cycles — one line each, for a `Notice`.
    pub notices: Vec<String>,
}

/// One token per four bytes: a deliberate rough cut for a budget guard, the
/// same heuristic the rest of cox uses before a provider reports real counts.
fn tokens(text: &str) -> u32 {
    u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Every candidate path in search order, before existence checks.
fn candidates(roots: &Roots) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = &roots.cox_home {
        out.push(home.join("AGENTS.md"));
    }
    if let Some(home) = &roots.claude_home {
        out.push(home.join("CLAUDE.md"));
    }
    let chain = directory_chain(roots);
    for dir in chain {
        out.extend(PER_DIR.iter().map(|name| dir.join(name)));
    }
    out
}

/// Git root down to cwd, inclusive; cwd alone outside a repository or when
/// cwd is not under the root.
fn directory_chain(roots: &Roots) -> Vec<PathBuf> {
    match &roots.git_root {
        Some(root) if roots.cwd.starts_with(root) => {
            let mut chain = vec![root.clone()];
            let mut cur = root.clone();
            if let Ok(rest) = roots.cwd.strip_prefix(root) {
                for part in rest.components() {
                    cur.push(part);
                    chain.push(cur.clone());
                }
            }
            chain
        }
        _ => vec![roots.cwd.clone()],
    }
}

/// Path as the block names it: relative to the git root when under it.
fn display(path: &Path, roots: &Roots) -> String {
    match &roots.git_root {
        Some(root) => pathdiff::diff_paths(path, root)
            .filter(|p| {
                !p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            })
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        None => path.display().to_string(),
    }
}

/// Loads the chain under `budget_tokens`.
pub fn load(roots: &Roots, budget_tokens: u32) -> Loaded {
    let mut loaded = Loaded::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut sections = Vec::new();
    let mut spent = 0u32;
    for path in candidates(roots) {
        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&canonical) else {
            continue;
        };
        let name = display(&path, roots);
        let mut stack = vec![canonical.clone()];
        let body = expand(&raw, &canonical, &mut stack, &mut loaded.notices);
        let section = format!("## {name}\n{}\n", body.trim_end());
        let cost = tokens(&section);
        if spent + cost > budget_tokens {
            loaded.notices.push(format!(
                "instruction file {name} dropped: {cost} tokens would exceed the {budget_tokens}-token budget"
            ));
            continue;
        }
        spent += cost;
        loaded.files.push(name);
        sections.push(section);
    }
    if !sections.is_empty() {
        loaded.block = format!("# Instructions\n{}", sections.join("\n"));
    }
    loaded
}

/// Expands `@path` words (Claude's include syntax) relative to the file
/// that contains them. Words that do not name a readable file stay as
/// written, so an e-mail address or a decorator is never touched.
fn expand(text: &str, file: &Path, stack: &mut Vec<PathBuf>, notices: &mut Vec<String>) -> String {
    let dir = file.parent().unwrap_or(file);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut rest = line;
        while let Some(at) = rest.find('@') {
            let word_ok = at == 0 || rest[..at].ends_with(char::is_whitespace);
            let end = rest[at..]
                .find(char::is_whitespace)
                .map(|n| at + n)
                .unwrap_or(rest.len());
            let word = &rest[at + 1..end];
            let target = include_target(word, dir);
            match target {
                Some(canonical) if word_ok => {
                    out.push_str(&rest[..at]);
                    out.push_str(&include(&canonical, word, stack, notices));
                }
                _ => out.push_str(&rest[..end]),
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
    }
    out
}

/// A readable file the word points at, resolved against `dir`.
fn include_target(word: &str, dir: &Path) -> Option<PathBuf> {
    if word.is_empty() {
        return None;
    }
    let raw = Path::new(word);
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        dir.join(raw)
    };
    fs::canonicalize(path).ok().filter(|p| p.is_file())
}

fn include(
    canonical: &Path,
    word: &str,
    stack: &mut Vec<PathBuf>,
    notices: &mut Vec<String>,
) -> String {
    if stack.iter().any(|p| p == canonical) {
        let chain: Vec<String> = stack
            .iter()
            .chain(std::iter::once(&canonical.to_path_buf()))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        notices.push(format!("instruction include cycle: {}", chain.join(" → ")));
        return format!("@{word}");
    }
    if stack.len() > MAX_DEPTH {
        notices.push(format!(
            "instruction include @{word} skipped: deeper than {MAX_DEPTH} levels"
        ));
        return format!("@{word}");
    }
    let Ok(text) = fs::read_to_string(canonical) else {
        return format!("@{word}");
    };
    stack.push(canonical.to_path_buf());
    let body = expand(&text, canonical, stack, notices);
    stack.pop();
    body.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_chain_runs_from_git_root_down_to_cwd() {
        let roots = Roots {
            cox_home: None,
            claude_home: None,
            git_root: Some(PathBuf::from("/r")),
            cwd: PathBuf::from("/r/a/b"),
        };
        let chain = directory_chain(&roots);
        assert_eq!(
            chain,
            [
                PathBuf::from("/r"),
                PathBuf::from("/r/a"),
                PathBuf::from("/r/a/b")
            ]
        );
    }

    #[test]
    fn instructions_cwd_outside_the_repo_searches_only_itself() {
        let roots = Roots {
            cox_home: None,
            claude_home: None,
            git_root: Some(PathBuf::from("/r")),
            cwd: PathBuf::from("/elsewhere"),
        };
        assert_eq!(directory_chain(&roots), [PathBuf::from("/elsewhere")]);
    }
}
