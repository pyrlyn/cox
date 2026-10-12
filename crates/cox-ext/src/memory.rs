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
/// `memory_budget_tokens`, in name order. Descriptions only — fact bodies
/// stay out of this string. When the budget stops the list short, the
/// result ends with one line naming how many entries were left out, and
/// that line spends budget the same way an entry does. Pure over the
/// entries so the budget test needs no filesystem.
pub fn index_text(entries: &[Entry], budget_tokens: u32) -> String {
    // A zero budget has always returned the header and nothing else: every
    // candidate line fails the check immediately. Keep that result. The
    // elision line is a signal that some allowance was spent, not a
    // substitute for an empty one.
    if budget_tokens == 0 {
        return String::from("Memory index:\n");
    }
    let mut out = String::from("Memory index:\n");
    let mut kept = 0usize;
    for entry in entries {
        let line = format!("- {}: {}\n", entry.name, entry.description);
        if estimate_tokens(&out) + estimate_tokens(&line) > budget_tokens {
            break;
        }
        out.push_str(&line);
        kept += 1;
    }
    if kept == entries.len() {
        return out;
    }
    // Whole entries only, same boundary the loop already uses. Append the
    // elision line when it fits; otherwise give the last kept line's budget
    // to it. The line can be longer than the entry it replaces, so keep
    // peeling entries until it fits. If the header plus the line still
    // exceeds the budget, the line is the whole result.
    loop {
        let line = format!(
            "… {} more; memory_search to read them\n",
            entries.len() - kept
        );
        if estimate_tokens(&out) + estimate_tokens(&line) <= budget_tokens {
            out.push_str(&line);
            return out;
        }
        if kept == 0 {
            return line;
        }
        let end = out.trim_end_matches('\n').rfind('\n').map_or(0, |i| i + 1);
        out.truncate(end);
        kept -= 1;
    }
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

    // A line long enough that the elision banner is cheaper than a second entry.
    fn wide(name: &str) -> Entry {
        Entry {
            name: name.into(),
            description: format!("detail {name} {}", "x".repeat(180)),
        }
    }

    fn entry_line(entry: &Entry) -> String {
        format!("- {}: {}\n", entry.name, entry.description)
    }

    #[test]
    fn memory_index_text_budget_fitting_one_of_three_names_the_two_left_out() {
        let entries = [wide("alpha"), wide("bravo"), wide("charlie")];
        let mut one = String::from("Memory index:\n");
        one.push_str(&entry_line(&entries[0]));
        let banner = "… 2 more; memory_search to read them\n";
        let budget = estimate_tokens(&one) + estimate_tokens(banner);
        let two = estimate_tokens(&one) + estimate_tokens(&entry_line(&entries[1]));
        assert!(
            budget < two,
            "banner must fit where a second entry does not"
        );
        let text = index_text(&entries, budget);
        assert!(text.contains(entry_line(&entries[0]).trim_end()));
        assert!(!text.contains("- bravo:"), "{text}");
        assert!(!text.contains("- charlie:"), "{text}");
        assert!(text.ends_with(banner), "{text}");
        assert!(estimate_tokens(&text) <= budget, "{text}");
    }

    #[test]
    fn memory_index_text_budget_fitting_every_entry_has_no_more_line() {
        let entries = [wide("alpha"), wide("bravo"), wide("charlie")];
        let mut full = String::from("Memory index:\n");
        let mut budget = 0u32;
        for entry in &entries {
            let line = entry_line(entry);
            budget = estimate_tokens(&full) + estimate_tokens(&line);
            full.push_str(&line);
        }
        let text = index_text(&entries, budget);
        assert_eq!(text, full);
        assert!(!text.contains("more; memory_search"), "{text}");
    }

    #[test]
    fn memory_index_text_zero_budget_returns_the_header_and_a_sub_header_budget_is_banner_only() {
        let entries = [wide("alpha"), wide("bravo"), wide("charlie")];
        // Current zero-budget behavior: the header is emitted and the loop
        // adds nothing. A zero allowance does not become the elision line
        // and does not become empty.
        assert_eq!(index_text(&entries, 0), "Memory index:\n");
        let below_header = estimate_tokens("Memory index:\n") - 1;
        assert_eq!(
            index_text(&entries, below_header),
            "… 3 more; memory_search to read them\n"
        );
    }

    #[test]
    fn memory_index_text_replaces_the_last_kept_line_when_the_banner_does_not_fit() {
        let entries = [wide("alpha"), wide("bravo"), wide("charlie")];
        let mut one = String::from("Memory index:\n");
        one.push_str(&entry_line(&entries[0]));
        let budget = estimate_tokens(&one) + estimate_tokens(&entry_line(&entries[1]));
        let mut two = one.clone();
        two.push_str(&entry_line(&entries[1]));
        let beside_two = "… 1 more; memory_search to read them\n";
        assert!(
            estimate_tokens(&two) + estimate_tokens(beside_two) > budget,
            "banner must miss beside both entries"
        );
        let text = index_text(&entries, budget);
        assert!(text.contains(entry_line(&entries[0]).trim_end()), "{text}");
        assert!(!text.contains("- bravo:"), "{text}");
        assert!(
            text.ends_with("… 2 more; memory_search to read them\n"),
            "{text}"
        );
        assert!(estimate_tokens(&text) <= budget, "{text}");
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
