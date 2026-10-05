// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The session repo map (plan.md P43, `docs/design/v0.2-repomap.md`): every
//! workspace file the caller admits, recently changed first, each followed by
//! its `outline`, cut at a byte budget. Separate because it is the only
//! whole-tree reader in cox-tools; the core reaches it only through
//! `cox_protocol::traits::RepoMapper` (T43.4).
//!
//! The text is a pure function of the file bytes, the git order and the
//! budget — no timestamps, no mtimes, ties in path order — because it sits in
//! the byte-stable system[2] and a resumed session replays it.

use std::collections::HashSet;
use std::path::Path;

use crate::path::confine;
use crate::read::{BINARY_SNIFF_BYTES, VISIBLE_CAP_BYTES};
use crate::{git, outline};

/// How many commits back `git log` ranks files; older history adds little
/// over path order and costs a longer log on a big repository.
pub const RECENT_COMMITS: usize = 50;

/// What `outline` prints for a file with no definitions; the map shows the
/// path alone instead.
const NO_OUTLINE: &str = "(no outline entries found)";

/// The map for the workspace at `root`: one `path\n` line per file followed
/// by its outline indented two spaces, until the next file would pass
/// `budget_bytes`; then one `… N more files` line (outside the budget).
/// `admit` sees each file as `root.join(path)` — the path as a model's
/// `read` would name it, so relative and absolute permission rules both
/// match — and only after `confine` accepted it; a file it rejects is
/// neither shown nor counted. Binaries and files over `read`'s visible cap
/// are skipped. A zero budget is the empty map.
pub async fn build(
    root: &Path,
    budget_bytes: usize,
    admit: &(dyn Fn(&Path) -> bool + Send + Sync),
) -> String {
    if budget_bytes == 0 {
        return String::new();
    }
    let recent = git::recent_changes(root, RECENT_COMMITS).await;
    let walk = root.to_path_buf();
    let files = tokio::task::spawn_blocking(move || cox_search::glob::workspace_files(&walk))
        .await
        .unwrap_or_default();
    render(root, &order(&recent, &files), budget_bytes, admit)
}

/// `recent` ∩ `files` in `recent`'s order, then the rest of `files` (already
/// sorted by path), each once. `files` already excludes git's own `.git/`
/// store — the shared walk skips it (`cox_search::grep::walker`, T37.44.16).
fn order(recent: &[String], files: &[String]) -> Vec<String> {
    let known: HashSet<&str> = files.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    recent
        .iter()
        .chain(files)
        .filter(|p| known.contains(p.as_str()) && seen.insert(p.as_str()))
        .cloned()
        .collect()
}

fn render(
    root: &Path,
    order: &[String],
    budget: usize,
    admit: &(dyn Fn(&Path) -> bool + Send + Sync),
) -> String {
    let roots = [root.to_path_buf()];
    let mut admitted = order.iter().filter_map(|rel| {
        let abs = confine(&roots, root, rel).ok()?;
        admit(&root.join(rel)).then_some((rel, abs))
    });
    let mut out = String::new();
    while let Some((rel, abs)) = admitted.next() {
        let Some(block) = block(rel, &abs) else {
            continue;
        };
        if out.len() + block.len() > budget {
            let more = 1 + admitted.by_ref().count();
            out.push_str(&format!("… {more} more files\n"));
            break;
        }
        out.push_str(&block);
    }
    out
}

/// One file's entry, or `None` for a binary, an oversized or unreadable file.
fn block(rel: &str, abs: &Path) -> Option<String> {
    let meta = std::fs::metadata(abs).ok()?;
    if !meta.is_file() || meta.len() > VISIBLE_CAP_BYTES as u64 {
        return None;
    }
    let bytes = std::fs::read(abs).ok()?;
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0) {
        return None;
    }
    let content = String::from_utf8_lossy(&bytes);
    let mut out = format!("{rel}\n");
    let outline = outline::outline(abs, &content);
    if outline != NO_OUTLINE {
        for line in outline.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    fn allow_all(_: &Path) -> bool {
        true
    }

    /// Runs git with a fixed identity and no signing, so the developer's
    /// global config never decides the result; `false` when git is missing.
    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write");
    }

    /// `a.rs`, `b.rs`, `m.rs`, `z.rs` committed; then `z.rs` changed in a
    /// second commit and `m.rs` left edited.
    fn repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        if !git(p, &["init", "-q", "--initial-branch=trunk"]) {
            return None;
        }
        for name in ["a", "b", "m", "z"] {
            write(p, &format!("{name}.rs"), &format!("pub fn {name}() {{}}\n"));
        }
        git(p, &["add", "."]).then_some(())?;
        git(p, &["commit", "-q", "-m", "one"]).then_some(())?;
        write(p, "z.rs", "pub fn z() {}\npub fn z2() {}\n");
        git(p, &["commit", "-q", "-am", "two"]).then_some(())?;
        write(p, "m.rs", "pub fn m() {}\npub fn m2() {}\n");
        Some(dir)
    }

    fn position(map: &str, path: &str) -> usize {
        map.lines()
            .position(|l| l == path)
            .unwrap_or_else(|| panic!("{path} missing from:\n{map}"))
    }

    #[tokio::test]
    async fn repomap_lists_recently_changed_files_first() {
        let Some(dir) = repo() else {
            return; // no usable `git` here
        };
        let map = build(dir.path(), 10_000, &allow_all).await;
        let (m, z, a, b) = (
            position(&map, "m.rs"),
            position(&map, "z.rs"),
            position(&map, "a.rs"),
            position(&map, "b.rs"),
        );
        assert!(m < z && z < a && a < b, "{map}");
        assert!(map.contains("  2: pub fn m2()"), "outline follows: {map}");
    }

    #[tokio::test]
    async fn repomap_leaves_out_the_git_dir() {
        let Some(dir) = repo() else {
            return;
        };
        let map = build(dir.path(), 100_000, &allow_all).await;
        assert!(!map.contains(".git/"), "{map}");
    }

    #[tokio::test]
    async fn repomap_is_byte_identical_for_the_same_tree() {
        let Some(dir) = repo() else {
            return;
        };
        let first = build(dir.path(), 10_000, &allow_all).await;
        let second = build(dir.path(), 10_000, &allow_all).await;
        assert!(!first.is_empty());
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn repomap_respects_the_byte_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..20 {
            write(
                dir.path(),
                &format!("f{i:02}.rs"),
                &format!("pub fn f{i}() {{}}\n"),
            );
        }
        let budget = 200;
        let map = build(dir.path(), budget, &allow_all).await;
        let (body, tail) = map
            .trim_end_matches('\n')
            .rsplit_once('\n')
            .expect("a body and a tail line");
        assert!(body.len() < budget, "{} bytes: {map}", body.len());
        assert!(
            tail.starts_with("… ") && tail.ends_with(" more files"),
            "{tail}"
        );
        let shown = map.lines().filter(|l| l.ends_with(".rs")).count();
        let more: usize = tail
            .trim_start_matches("… ")
            .trim_end_matches(" more files")
            .parse()
            .expect("count");
        assert_eq!(shown + more, 20, "{map}");
        assert_eq!(build(dir.path(), 0, &allow_all).await, "");
    }

    #[tokio::test]
    async fn repomap_skips_files_the_filter_rejects() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "open.rs", "pub fn open() {}\n");
        write(dir.path(), "secret.rs", "pub fn secret() {}\n");
        let deny_secret = |p: &Path| !p.ends_with("secret.rs");
        let map = build(dir.path(), 10_000, &deny_secret).await;
        assert!(map.contains("open.rs"), "{map}");
        assert!(!map.contains("secret"), "{map}");
    }

    #[tokio::test]
    async fn repomap_without_git_falls_back_to_path_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "c.rs", "pub fn c() {}\n");
        write(dir.path(), "a.rs", "pub fn a() {}\n");
        write(dir.path(), "b/x.rs", "pub fn x() {}\n");
        write(dir.path(), "blob.bin", "\0\0\0");
        let map = build(dir.path(), 10_000, &allow_all).await;
        let paths: Vec<&str> = map.lines().filter(|l| !l.starts_with(' ')).collect();
        assert_eq!(paths, ["a.rs", "b/x.rs", "c.rs"], "binary skipped: {map}");
    }
}
