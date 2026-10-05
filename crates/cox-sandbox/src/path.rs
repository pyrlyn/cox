// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Path confinement: the single trust-boundary guard for every filesystem
//! path a tool receives from the model (AGENTS.md: "`cox_tools::path::confine`
//! — every path from the model passes through it; rejects escapes from the
//! workspace roots"). Kept as its own module, not folded into `read`/`edit`/
//! `write`, so every file tool calls the exact same function — two slightly
//! different confinement implementations is how a sandbox escape ships.

use std::env;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use cox_protocol::ToolError;

/// Confines `input` (a path argument from the model) to one of `roots`.
///
/// 1. Rejects NUL bytes and Windows drive/UNC/ADS syntax (`C:`, `\\?\`,
///    `file:stream`) outright — none of that is meaningful on the Unix
///    filesystems cox runs on, so any occurrence is treated as an escape
///    attempt. `ponytail:` this bans literal `:` in every path outright
///    (simplest rule that also blocks ADS on any future Windows target)
///    rather than pattern-matching each syntax; revisit only if a real
///    Unix path legitimately needs a colon.
/// 2. Expands a leading `~` to `$HOME`, then joins relative to `cwd`.
/// 3. Lexically normalises `.`/`..` and checks containment against `roots`
///    — a cheap, filesystem-free rejection of a plain `..` escape.
/// 4. Canonicalises the deepest *existing* ancestor of the (still
///    un-normalised) joined path and re-checks containment against
///    canonicalised roots. This step is what actually matters:
///    canonicalising the *raw* path, not the lexically-normalised one,
///    lets the OS resolve `..` against wherever a symlink in the existing
///    prefix really points — the case a purely lexical check misses
///    (`link/../x` where `link` points outside every root).
///
/// A non-existent leaf inside an existing, confined directory is allowed
/// (`write` targets a path that doesn't exist yet); the root itself is
/// allowed.
pub fn confine(roots: &[PathBuf], cwd: &Path, input: &str) -> Result<PathBuf, ToolError> {
    reject_unsafe_syntax(input, roots)?;

    let joined = cwd.join(expand_tilde(input)?);

    let canon_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| lexically_normalize(r)))
        .collect();

    // Step 3: fast, symlink-free rejection. A root is matched as given and
    // canonical, so an already-canonical path under a symlinked root (a
    // checkpoint row's path when cwd is `/var/…` on macOS) is not refused
    // here; step 4 still decides.
    let lexical = lexically_normalize(&joined);
    if !roots
        .iter()
        .map(|r| lexically_normalize(r))
        .chain(canon_roots.iter().cloned())
        .any(|r| lexical.starts_with(r))
    {
        return Err(confined(&lexical, roots));
    }

    // Step 4: authoritative check through the deepest existing ancestor.
    let (existing, tail) = deepest_existing_ancestor(&joined);
    let canon_ancestor = std::fs::canonicalize(&existing).map_err(|_| ToolError::Io)?;

    let mut resolved = canon_ancestor;
    for part in &tail {
        resolved.push(part);
    }
    let resolved = lexically_normalize(&resolved);

    if canon_roots.iter().any(|r| resolved.starts_with(r)) {
        Ok(resolved)
    } else {
        Err(confined(&resolved, roots))
    }
}

/// Rejects syntax that only means something on Windows filesystems (drive
/// letters, `\\?\` device paths, `:stream` alternate data streams) and NUL,
/// which terminates a C string and would truncate the path the OS actually
/// sees.
fn reject_unsafe_syntax(input: &str, roots: &[PathBuf]) -> Result<(), ToolError> {
    let suspicious = input.contains('\0') || input.contains(':') || input.starts_with("\\\\");
    if suspicious {
        Err(confined(Path::new(input), roots))
    } else {
        Ok(())
    }
}

/// Expands a leading `~` (alone, or `~/rest`) to `$HOME`. Any other input
/// is returned unchanged for the caller to join against `cwd`.
fn expand_tilde(input: &str) -> Result<PathBuf, ToolError> {
    if input == "~" || input.starts_with("~/") {
        let home = env::var_os("HOME").ok_or(ToolError::Io)?;
        let rest = input.strip_prefix('~').unwrap_or(input);
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let mut home_path = PathBuf::from(home);
        if !rest.is_empty() {
            home_path.push(rest);
        }
        Ok(home_path)
    } else {
        Ok(PathBuf::from(input))
    }
}

/// Collapses `.` and `..` components without touching the filesystem.
/// `PathBuf::pop` is a no-op once `out` is already at the root, so a `..`
/// chain longer than the path's depth clamps at `/` instead of underflowing
/// — the same behaviour `realpath(3)` gives for `..` above the root.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Walks `path` up to the first ancestor that exists on disk, returning
/// that ancestor plus the (still-unresolved) trailing components. The
/// trailing components may still contain `..` — `path` is deliberately not
/// lexically normalised first, so an existing prefix that is itself a
/// symlink gets resolved by the OS (via `exists`) instead of being
/// collapsed by us before the symlink is ever looked at.
///
/// Built from the raw component list (not `Path::pop`/`file_name`, which
/// return `None` once the last component is `..` or `.` and would cut the
/// walk short whenever a trailing `..` shows up mid-walk).
fn deepest_existing_ancestor(path: &Path) -> (PathBuf, Vec<OsString>) {
    let components: Vec<Component> = path.components().collect();
    let mut end = components.len();
    while end > 0 {
        let candidate: PathBuf = components[..end].iter().copied().collect();
        if candidate.exists() {
            let tail = components[end..]
                .iter()
                .map(|c| c.as_os_str().to_os_string())
                .collect();
            return (candidate, tail);
        }
        end -= 1;
    }
    // Every prefix failed to exist, including the filesystem root itself
    // (should not happen in practice) — fall back to it anyway so
    // `canonicalize` still has something real to try.
    let root_only: PathBuf = components
        .first()
        .into_iter()
        .map(|c| c.as_os_str())
        .collect();
    let tail = components
        .iter()
        .skip(1)
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    (root_only, tail)
}

/// Builds a `ToolError::Confined`, reporting whichever root shares the
/// longest path prefix with `path` (the most useful one to show the model
/// when several roots are configured).
fn confined(path: &Path, roots: &[PathBuf]) -> ToolError {
    let root = roots
        .iter()
        .max_by_key(|r| common_prefix_len(r, path))
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/"));
    ToolError::Confined {
        path: path.to_path_buf(),
        root,
    }
}

fn common_prefix_len(a: &Path, b: &Path) -> usize {
    a.components()
        .zip(b.components())
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test kept alongside the exhaustive table in
    /// `tests/confine.rs`: a plain in-root relative path resolves to an
    /// absolute path under the root.
    #[test]
    fn confine_plain_relative_path_stays_in_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        std::fs::write(root.join("a.txt"), b"hi").expect("write fixture");

        let got = confine(std::slice::from_ref(&root), &root, "a.txt").expect("confine");
        assert_eq!(got, root.join("a.txt"));
    }

    #[test]
    fn confine_rejects_dotdot_escape_above_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        std::fs::create_dir(&root).expect("mkdir");

        let err = confine(std::slice::from_ref(&root), &root, "../outside.txt")
            .expect_err("must confine");
        assert!(matches!(err, ToolError::Confined { .. }));
    }

    /// A rewind restores the canonical paths its checkpoint rows hold,
    /// while the root is the cwd as the session was opened.
    #[cfg(unix)]
    #[test]
    fn a_canonical_path_under_a_symlinked_root_is_confined() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        std::fs::create_dir(real.join("real")).expect("mkdir");
        std::os::unix::fs::symlink(real.join("real"), real.join("link")).expect("symlink");
        let (root, file) = (real.join("link"), real.join("real/a.txt"));
        std::fs::write(&file, b"hi").expect("write fixture");

        let input = file.to_string_lossy();
        let got = confine(std::slice::from_ref(&root), &root, &input).expect("confine");
        assert_eq!(got, file);
        let outside = real.join("b.txt").to_string_lossy().into_owned();
        let err = confine(std::slice::from_ref(&root), &root, &outside).expect_err("outside");
        assert!(matches!(err, ToolError::Confined { .. }));
    }
}
