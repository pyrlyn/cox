// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! WASI preopens for one plugin (T33.14.2, PL§11): the only filesystem a
//! guest ever sees is what `capabilities.fs` grants, each root resolved
//! through the caller's `confine`. Its own module because it is a trust
//! guard: the mount list is computed here once, and `host.rs` hands it to
//! extism unchanged.
//!
//! `cox-plugin` may not depend on `cox-sandbox` (`crates/cox/tests/deps.rs`),
//! so the session injects `cox_tools::path::confine` as [`FsRoots::confine`]
//! rather than this crate carrying a second implementation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_plugin_api::FsCaps;

/// `confine(root, input)`: `input` resolved symlink-safely inside `root`, or
/// the reason it was refused.
pub type Confine = dyn Fn(&Path, &str) -> Result<PathBuf, String> + Send + Sync;

const WORKSPACE: &str = "$WORKSPACE";
const PLUGIN_DATA: &str = "$PLUGIN_DATA";
const GUEST_WORKSPACE: &str = "/workspace";
const GUEST_DATA: &str = "/data";
// Directories a writable mount must never be able to reach (D7).
const PROTECTED: [&str; 2] = [".git", ".cox"];
// A writable root is walked to prove it holds no protected directory; a
// tree too big to walk is refused instead of trusted.
const WALK_LIMIT: usize = 50_000;

/// What the session knows and `cox-plugin` must not: the workspace, where
/// per-plugin data lives, and the confinement guard.
#[derive(Clone)]
pub struct FsRoots {
    /// The session's workspace root (`$WORKSPACE`).
    pub workspace: PathBuf,
    /// Parent of every plugin's `$PLUGIN_DATA` directory (`<id>/` below it).
    pub data_base: PathBuf,
    /// `cox_tools::path::confine` with a single root.
    pub confine: Arc<Confine>,
}

/// The WASI setup for one plugin, in extism's terms.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Mounts {
    /// Whether the WASI imports link at all.
    pub wasi: bool,
    /// Extism `allowed_paths`: host path (`ro:` prefix when read-only) to
    /// guest path.
    pub paths: BTreeMap<String, PathBuf>,
}

/// Why a grant could not become a mount. Always a load refusal.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FsError {
    /// `fs` was granted but the session gave no roots to resolve it against.
    #[error("filesystem access is granted but this session has no workspace")]
    NoRoots,
    /// The root is not `$WORKSPACE` or `$PLUGIN_DATA` (validation catches it
    /// first; this keeps the guard whole when it is called alone).
    #[error("fs root {0:?} must start with $WORKSPACE or $PLUGIN_DATA")]
    Root(String),
    /// `confine` refused the resolved path.
    #[error("fs root {root:?} is refused: {why}")]
    Confined {
        /// The manifest spelling.
        root: String,
        /// `confine`'s reason.
        why: String,
    },
    /// A writable root is, or holds, `.git` or `.cox`.
    #[error("fs.write root {0:?} would make .git or .cox writable")]
    Protected(String),
    /// The directory to mount could not be created or is not UTF-8.
    #[error("fs root {0:?} cannot be mounted")]
    Mount(String),
}

/// Maps `fs` to WASI mounts. WASI is on when the manifest asks for it or any
/// root is granted; with neither, the guest gets no WASI at all.
pub fn mounts(
    id: &str,
    wasi: bool,
    fs: &FsCaps,
    roots: Option<&FsRoots>,
) -> Result<Mounts, FsError> {
    if fs.read.is_empty() && fs.write.is_empty() {
        return Ok(Mounts {
            wasi,
            paths: BTreeMap::new(),
        });
    }
    let roots = roots.ok_or(FsError::NoRoots)?;
    let mut out = Mounts {
        wasi: true,
        paths: BTreeMap::new(),
    };
    let mut writable = Vec::new();
    for decl in &fs.write {
        let (host, guest) = resolve(id, decl, roots, true)?;
        writable.push(host.clone());
        out.paths.insert(host, guest);
    }
    for decl in &fs.read {
        let (host, guest) = resolve(id, decl, roots, false)?;
        // A read-write mount already covers the same directory; two
        // preopens of it would let the read-only one shadow the other.
        if !writable.contains(&host) {
            out.paths.insert(format!("ro:{host}"), guest);
        }
    }
    Ok(out)
}

fn resolve(
    id: &str,
    decl: &str,
    roots: &FsRoots,
    writable: bool,
) -> Result<(String, PathBuf), FsError> {
    let data = roots.data_base.join(id);
    let (base, guest, is_data, rest) = if let Some(rest) = decl.strip_prefix(WORKSPACE) {
        (&roots.workspace, GUEST_WORKSPACE, false, rest)
    } else if let Some(rest) = decl.strip_prefix(PLUGIN_DATA) {
        (&data, GUEST_DATA, true, rest)
    } else {
        return Err(FsError::Root(decl.to_string()));
    };
    let target = format!("{}{rest}", base.display());
    // A preopen needs a directory to exist; only the plugin's own data
    // directory is ours to create.
    if is_data {
        std::fs::create_dir_all(&target).map_err(|_| FsError::Mount(decl.to_string()))?;
    }
    let host = (roots.confine)(base, &target).map_err(|why| FsError::Confined {
        root: decl.to_string(),
        why,
    })?;
    if writable && !is_data {
        let workspace = (roots.confine)(base, &base.display().to_string())
            .map_err(|_| FsError::Protected(decl.to_string()))?;
        if reaches_protected(&host, &workspace) {
            return Err(FsError::Protected(decl.to_string()));
        }
    }
    let host = host
        .to_str()
        .ok_or_else(|| FsError::Mount(decl.to_string()))?
        .to_string();
    Ok((host, PathBuf::from(format!("{guest}{rest}"))))
}

fn is_protected(name: &std::ffi::OsStr) -> bool {
    PROTECTED.iter().any(|p| name == *p)
}

/// Whether a writable mount of `root` could write `.git` or `.cox`: it lies
/// inside one, or one is anywhere beneath it. WASI has no way to carve a
/// directory out of a mount, so the whole tree is checked up front.
fn reaches_protected(root: &Path, workspace: &Path) -> bool {
    if root
        .strip_prefix(workspace)
        .is_ok_and(|rel| rel.iter().any(is_protected))
    {
        return true;
    }
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // Unreadable means unverifiable.
            return true;
        };
        for entry in entries {
            seen += 1;
            let Ok(entry) = entry else { return true };
            if seen > WALK_LIMIT || is_protected(&entry.file_name()) {
                return true;
            }
            // A symlink is not followed: WASI itself refuses one that
            // leaves the mount, and one that stays inside is walked as the
            // real directory it points at.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confine_in(root: &Path, input: &str) -> Result<PathBuf, String> {
        let real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let path = PathBuf::from(input);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.starts_with(&real) {
            Ok(path)
        } else {
            Err("outside the root".into())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        roots: FsRoots,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(dir.path()).unwrap().join("ws");
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join("out")).unwrap();
        let roots = FsRoots {
            workspace,
            data_base: dir.path().join("data"),
            confine: Arc::new(confine_in),
        };
        Fixture { _dir: dir, roots }
    }

    fn caps(read: &[&str], write: &[&str]) -> FsCaps {
        FsCaps {
            read: read.iter().map(|s| s.to_string()).collect(),
            write: write.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn fs_write_to_dot_git_is_refused() {
        let f = fixture();
        for bad in ["$WORKSPACE", "$WORKSPACE/.git", "$WORKSPACE/.git/hooks"] {
            let err = mounts("p", false, &caps(&[], &[bad]), Some(&f.roots));
            assert!(matches!(err, Err(FsError::Protected(_))), "{bad}: {err:?}");
        }
        // `.cox` and a nested repository count too.
        std::fs::create_dir_all(f.roots.workspace.join("out/sub/.cox")).unwrap();
        let err = mounts("p", false, &caps(&[], &["$WORKSPACE/out"]), Some(&f.roots));
        assert!(matches!(err, Err(FsError::Protected(_))), "{err:?}");
        // A sibling directory with no protected entry is fine.
        std::fs::create_dir_all(f.roots.workspace.join("clean")).unwrap();
        let ok = mounts(
            "p",
            false,
            &caps(&[], &["$WORKSPACE/clean"]),
            Some(&f.roots),
        );
        assert!(ok.is_ok(), "{ok:?}");
    }

    #[test]
    fn reads_mount_read_only_and_writes_do_not() {
        let f = fixture();
        let m = mounts(
            "p",
            false,
            &caps(&["$WORKSPACE"], &["$WORKSPACE/out"]),
            Some(&f.roots),
        )
        .unwrap();
        let ws = f.roots.workspace.display().to_string();
        assert!(m.wasi);
        assert_eq!(
            m.paths.get(&format!("ro:{ws}")),
            Some(&PathBuf::from("/workspace"))
        );
        assert_eq!(
            m.paths.get(&format!("{ws}/out")),
            Some(&PathBuf::from("/workspace/out"))
        );
        assert_eq!(m.paths.len(), 2);
    }

    #[test]
    fn plugin_data_is_created_and_writable_even_under_dot_cox() {
        let f = fixture();
        let m = mounts(
            "p",
            false,
            &caps(&[], &["$PLUGIN_DATA/cache"]),
            Some(&f.roots),
        )
        .unwrap();
        assert!(f.roots.data_base.join("p/cache").is_dir());
        assert_eq!(m.paths.values().next(), Some(&PathBuf::from("/data/cache")));
        assert!(m.paths.keys().all(|k| !k.starts_with("ro:")));
    }

    #[test]
    fn wasi_is_on_only_for_the_flag_or_a_grant() {
        let f = fixture();
        let off = mounts("p", false, &FsCaps::default(), Some(&f.roots)).unwrap();
        assert_eq!(off, Mounts::default());
        let flag = mounts("p", true, &FsCaps::default(), None).unwrap();
        assert!(flag.wasi && flag.paths.is_empty());
        let granted = mounts("p", false, &caps(&["$WORKSPACE"], &[]), Some(&f.roots)).unwrap();
        assert!(granted.wasi);
    }

    #[test]
    fn a_grant_without_roots_or_outside_confine_is_refused() {
        let f = fixture();
        assert_eq!(
            mounts("p", false, &caps(&["$WORKSPACE"], &[]), None),
            Err(FsError::NoRoots)
        );
        let deny = FsRoots {
            confine: Arc::new(|_, _| Err("nope".into())),
            ..f.roots.clone()
        };
        let err = mounts("p", false, &caps(&["$WORKSPACE"], &[]), Some(&deny));
        assert!(matches!(err, Err(FsError::Confined { .. })), "{err:?}");
        let err = mounts("p", false, &caps(&["/etc"], &[]), Some(&f.roots));
        assert_eq!(err, Err(FsError::Root("/etc".into())));
    }
}
