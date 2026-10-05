// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Project memory files (T10.1): Claude Code's layout under
//! `~/.cox/projects/<slug>/memory/` — a `MEMORY.md` index plus one file per
//! fact with `name`/`description`/`type` frontmatter. This module owns the
//! layout, slug/dir resolution and the token-budgeted index text; the
//! `memory_*` tools (cox-tools, which may not depend on this crate) mirror
//! the file format, and `memory_upsert` on the store keeps FTS in sync.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The index file listing every fact.
pub const INDEX_NAME: &str = "MEMORY.md";

/// One index entry: what `system[3]` may carry per fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Fact slug (`<name>.md`).
    pub name: String,
    /// One-line description.
    pub description: String,
}

#[derive(Deserialize)]
struct Header {
    name: Option<String>,
    description: Option<String>,
}

/// The project slug for `cwd`: the git root's directory name, else `cwd`'s,
/// lowercased to `[a-z0-9-]`.
pub fn slug_for(cwd: &Path) -> String {
    let base = git_root(cwd)
        .or_else(|| Some(cwd.to_path_buf()))
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "default".to_string());
    sanitize(&base)
}

/// The memory directory for a project: `<home>/projects/<slug>/memory`.
pub fn memory_dir(home: &Path, cwd: &Path) -> PathBuf {
    home.join("projects").join(slug_for(cwd)).join("memory")
}

/// Fact names double as file stems, so they stay machine-shaped.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The relative file name for a fact (validated first).
pub fn file_name(name: &str) -> String {
    format!("{name}.md")
}

/// Writes `<name>.md` and rebuilds the index from the directory scan, so a
/// concurrent save can only delay an entry, never corrupt the index.
pub fn save_fact(
    dir: &Path,
    name: &str,
    description: &str,
    kind: &str,
    body: &str,
) -> Result<PathBuf, String> {
    if !is_valid_name(name) {
        return Err(format!("invalid memory name {name:?}: use [a-z0-9-]"));
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let description = one_line(description);
    let kind = one_line(kind);
    let text = format!(
        "---\nname: {name}\ndescription: {description}\ntype: {kind}\n---\n{}",
        body.trim()
    );
    fs::write(dir.join(file_name(name)), text).map_err(|e| e.to_string())?;
    rebuild_index(dir)?;
    Ok(PathBuf::from(file_name(name)))
}

/// Rewrites `MEMORY.md` from the facts on disk; returns the entry count.
pub fn rebuild_index(dir: &Path) -> Result<usize, String> {
    let entries = list_facts(dir);
    let mut out = String::from("# Memory\n");
    for entry in &entries {
        out.push_str(&format!(
            "- [{}]({}) — {}\n",
            entry.name,
            file_name(&entry.name),
            entry.description
        ));
    }
    fs::write(dir.join(INDEX_NAME), out).map_err(|e| e.to_string())?;
    Ok(entries.len())
}

/// Every parseable fact in `dir`, sorted by name; broken files are skipped
/// (a later save rebuilds their index line away only if fixed — the file
/// itself is left alone for the user to repair).
pub fn list_facts(dir: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Ok(files) = fs::read_dir(dir) else {
        return entries;
    };
    let mut paths: Vec<PathBuf> = files
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "md")
                && p.file_name().is_some_and(|n| n != INDEX_NAME)
                && p.is_file()
        })
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok((header, _)) = crate::frontmatter::parse::<Header>(&text) else {
            continue;
        };
        let (Some(name), Some(description)) = (header.name, header.description) else {
            continue;
        };
        if !is_valid_name(name.trim()) {
            continue;
        }
        entries.push(Entry {
            name: name.trim().to_string(),
            description: one_line(&description),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Parses `MEMORY.md` back into entries (missing file → empty).
pub fn load_index(dir: &Path) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(dir.join(INDEX_NAME)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("- [")?;
            let (name, rest) = rest.split_once("](")?;
            let (_, description) = rest.split_once(") — ")?;
            if !is_valid_name(name) {
                return None;
            }
            Some(Entry {
                name: name.to_string(),
                description: description.to_string(),
            })
        })
        .collect()
}

/// Rough token count (bytes/4, the same heuristic the loop budgets by).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4 + 1) as u32
}

/// Index text for `system[3]`: as many entries as fit under
/// `memory_budget_tokens`, in name order. Pure over the entries so the
/// budget test needs no filesystem.
pub fn index_text(entries: &[Entry], budget_tokens: u32) -> String {
    let mut out = String::from("Memory index:\n");
    for entry in entries {
        let line = format!("- {}: {}\n", entry.name, entry.description);
        if estimate_tokens(&out) + estimate_tokens(&line) > budget_tokens {
            break;
        }
        out.push_str(&line);
    }
    out
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn sanitize(base: &str) -> String {
    let slug: String = base
        .to_ascii_lowercase()
        .bytes()
        .map(|b| {
            if b.is_ascii_lowercase() || b.is_ascii_digit() {
                b as char
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            description: format!("Fact {name} for the budget test."),
        }
    }

    #[test]
    fn memory_index_stays_under_budget_with_40_facts() {
        let entries: Vec<Entry> = (0..40).map(|i| entry(&format!("fact-{i:02}"))).collect();
        let text = index_text(&entries, 800);
        assert!(estimate_tokens(&text) <= 800, "{text}");
        for i in 0..40 {
            assert!(text.contains(&format!("fact-{i:02}")), "all 40 fit");
        }
    }

    #[test]
    fn memory_index_text_stops_at_the_budget() {
        let entries: Vec<Entry> = (0..100).map(|i| entry(&format!("fact-{i:02}"))).collect();
        let text = index_text(&entries, 100);
        assert!(estimate_tokens(&text) <= 100);
        assert!(!text.contains("fact-99"), "tail cut off");
    }

    #[test]
    fn memory_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mem = dir.path().join("memory");
        save_fact(
            &mem,
            "auth-flow",
            "Login goes through auth.rs.",
            "decision",
            "Details.",
        )
        .unwrap();
        save_fact(&mem, "widget-api", "Canvas holds widgets.", "fact", "More.").unwrap();
        assert!(mem.join("auth-flow.md").exists());
        let loaded = load_index(&mem);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "auth-flow");
        assert_eq!(rebuild_index(&mem).unwrap(), 2);
        assert!(!slug_for(dir.path()).is_empty());
    }

    #[test]
    fn memory_invalid_names_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "../x", "UPPER", "has space", "a/b", "dot.name"] {
            assert!(save_fact(dir.path(), bad, "d", "f", "b").is_err(), "{bad}");
            assert!(!is_valid_name(bad), "{bad}");
        }
        assert!(is_valid_name("fine-09"));
    }
}
