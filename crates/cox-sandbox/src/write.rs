// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Writing a file at a path `path::confine` already approved (T64.15.3). The
//! read side (`open`) checks a descriptor after opening it; a write cannot
//! wait that long, because `create_dir_all`, the temp file and the rename each
//! act on the path they are given, so a parent directory swapped for a
//! symlink after `confine` redirects all three. Here the parent directory is
//! opened once without following a symlink at its leaf, its real path is
//! checked against the canonical roots, and every later step — `mkdirat`, the
//! temp file's `openat`, `renameat` — is relative to that descriptor, which
//! keeps naming the same directory whatever happens to the path. Separate from
//! `open` because it owns a directory handle that outlives one call (a staged
//! write is committed later, for the all-or-nothing `rewrite_all`).

use std::path::{Path, PathBuf};

use cox_protocol::ToolError;

#[cfg(unix)]
pub use unix::{Staged, stage_confined};

#[cfg(not(unix))]
pub use portable::{Staged, stage_confined};

/// Writes `bytes` to `path` atomically: a sibling temp file in the same
/// directory, then a rename over the target. A rename within one filesystem is
/// atomic, so a crash mid-write never leaves a half-written file at `path`;
/// the temp file is a sibling (not `/tmp`) because a rename across
/// filesystems is neither atomic nor guaranteed to succeed. Missing parent
/// directories are created. A parent that resolves outside `roots` — by a
/// symlink swapped in since `confine` — is `Confined` with nothing created or
/// written there.
pub fn write_confined(roots: &[PathBuf], path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    stage_confined(roots, path, bytes)?.commit()
}

/// A sibling name no other writer picks: process id plus the clock, opened
/// with `O_EXCL` so a collision fails instead of overwriting. Public for the
/// callers that write a trusted, non-workspace path and so have no roots.
pub fn temp_name(name: &std::ffi::OsStr) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = name.to_str().unwrap_or("cox-write");
    format!(".{name}.cox-tmp-{}-{nanos}", std::process::id())
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::Write as _;
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};

    use cox_protocol::ToolError;
    use nix::errno::Errno;
    use nix::fcntl::{OFlag, open, openat, renameat};
    use nix::sys::stat::{Mode, mkdirat};
    use nix::unistd::{UnlinkatFlags, unlinkat};

    use super::temp_name;
    use crate::open::check_inside_roots;
    use crate::path::confined;

    /// `O_NOFOLLOW` only guards the leaf; the real-path check covers the rest.
    fn dir_flags() -> OFlag {
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
    }

    /// New files and directories get the same `0o666`/`0o777` (minus umask)
    /// the std calls this replaces gave them.
    fn mode(bits: u32) -> Mode {
        Mode::from_bits_truncate(bits as _)
    }

    /// A temp file written next to its target, not yet in place. Dropping it
    /// without [`Staged::commit`] removes the temp file, so an error between
    /// staging and committing leaves nothing behind.
    pub struct Staged {
        dir: OwnedFd,
        tmp: String,
        name: OsString,
        committed: bool,
    }

    impl Staged {
        /// Renames the temp file over the target, both relative to the
        /// checked directory descriptor.
        pub fn commit(mut self) -> Result<(), ToolError> {
            renameat(
                &self.dir,
                self.tmp.as_str(),
                &self.dir,
                self.name.as_os_str(),
            )
            .map_err(|_| ToolError::Io)?;
            self.committed = true;
            Ok(())
        }
    }

    impl Drop for Staged {
        fn drop(&mut self) {
            if !self.committed {
                let _ = unlinkat(&self.dir, self.tmp.as_str(), UnlinkatFlags::NoRemoveDir);
            }
        }
    }

    /// Writes `bytes` to a fresh sibling temp file of `path`, the first half
    /// of [`super::write_confined`]. `path` must be absolute, as `confine`
    /// returns it.
    pub fn stage_confined(
        roots: &[PathBuf],
        path: &Path,
        bytes: &[u8],
    ) -> Result<Staged, ToolError> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(confined(path, roots));
        };
        if !path.is_absolute() {
            return Err(confined(path, roots));
        }
        let dir = open_parent(roots, path, parent)?;
        let tmp = temp_name(name);
        let fd = openat(
            &dir,
            tmp.as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            mode(0o666),
        )
        .map_err(|_| ToolError::Io)?;
        let staged = Staged {
            dir,
            tmp,
            name: name.to_os_string(),
            committed: false,
        };
        File::from(fd).write_all(bytes).map_err(|_| ToolError::Io)?;
        Ok(staged)
    }

    /// Opens `parent`, creating the components that do not exist yet. The
    /// deepest existing ancestor is opened and checked against the roots
    /// before anything is created, then each missing component is made and
    /// entered relative to the descriptor in hand.
    fn open_parent(roots: &[PathBuf], path: &Path, parent: &Path) -> Result<OwnedFd, ToolError> {
        let mut existing = parent;
        let mut missing = Vec::new();
        let mut dir = loop {
            match open(existing, dir_flags(), Mode::empty()) {
                Ok(fd) => break fd,
                Err(Errno::ENOENT) => {
                    let (Some(up), Some(name)) = (existing.parent(), existing.file_name()) else {
                        return Err(ToolError::Io);
                    };
                    missing.push(name);
                    existing = up;
                }
                // A symlink leaf fails as ELOOP, or as ENOTDIR on Linux with
                // O_DIRECTORY; a plain file in the way is also ENOTDIR.
                Err(Errno::ELOOP | Errno::ENOTDIR)
                    if std::fs::symlink_metadata(existing)
                        .is_ok_and(|m| m.file_type().is_symlink()) =>
                {
                    return Err(confined(path, roots));
                }
                Err(Errno::ELOOP) => return Err(confined(path, roots)),
                Err(_) => return Err(ToolError::Io),
            }
        };
        check_inside_roots(&dir, roots)?;
        for name in missing.into_iter().rev() {
            match mkdirat(&dir, name, mode(0o777)) {
                Ok(()) | Err(Errno::EEXIST) => {}
                Err(_) => return Err(ToolError::Io),
            }
            // A symlink planted at the new name fails to open as a directory.
            let next = openat(&dir, name, dir_flags(), Mode::empty()).map_err(|e| match e {
                Errno::ELOOP | Errno::ENOTDIR => confined(path, roots),
                _ => ToolError::Io,
            })?;
            dir = next;
        }
        Ok(dir)
    }
}

#[cfg(not(unix))]
mod portable {
    use std::path::{Path, PathBuf};

    use cox_protocol::ToolError;

    use super::temp_name;

    /// A temp file written next to its target, not yet in place. Without a
    /// directory descriptor the swap window `confine` leaves remains, as for
    /// `open_confined` on these targets.
    pub struct Staged {
        tmp: PathBuf,
        path: PathBuf,
        committed: bool,
    }

    impl Staged {
        pub fn commit(mut self) -> Result<(), ToolError> {
            std::fs::rename(&self.tmp, &self.path).map_err(|_| ToolError::Io)?;
            self.committed = true;
            Ok(())
        }
    }

    impl Drop for Staged {
        fn drop(&mut self) {
            if !self.committed {
                let _ = std::fs::remove_file(&self.tmp);
            }
        }
    }

    pub fn stage_confined(
        _roots: &[PathBuf],
        path: &Path,
        bytes: &[u8],
    ) -> Result<Staged, ToolError> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|_| ToolError::Io)?;
        let name = path.file_name().unwrap_or_default();
        let staged = Staged {
            tmp: dir.join(temp_name(name)),
            path: path.to_path_buf(),
            committed: false,
        };
        std::fs::write(&staged.tmp, bytes).map_err(|_| ToolError::Io)?;
        Ok(staged)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::path::confine;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir(&root).expect("mkdir root");
        std::fs::create_dir(&outside).expect("mkdir outside");
        Fixture {
            _dir: dir,
            root,
            outside,
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_confined_creates_missing_parents_and_replaces_an_existing_file() {
        let f = fixture();
        let roots = [f.root.clone()];
        let nested = confine(&roots, &f.root, "a/b/c.txt").expect("confine");
        write_confined(&roots, &nested, b"one").expect("create");
        write_confined(&roots, &nested, b"two").expect("replace");

        assert_eq!(std::fs::read(&nested).expect("read"), b"two");
        assert_eq!(names(nested.parent().expect("parent")), ["c.txt"]);
    }

    #[test]
    fn write_confined_refuses_a_parent_swapped_for_a_symlink_and_writes_nothing() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::create_dir(f.root.join("sub")).expect("mkdir");
        let existing = confine(&roots, &f.root, "sub/new.txt").expect("confine");
        let missing = confine(&roots, &f.root, "deep/x/new.txt").expect("confine");
        std::fs::remove_dir(f.root.join("sub")).expect("rmdir");
        symlink(&f.outside, f.root.join("sub")).expect("symlink");
        symlink(&f.outside, f.root.join("deep")).expect("symlink");

        for path in [existing, missing] {
            let err = write_confined(&roots, &path, b"x").expect_err("must refuse");
            assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
        }
        assert!(names(&f.outside).is_empty(), "{:?}", names(&f.outside));
    }

    #[test]
    fn write_confined_replaces_a_symlink_leaf_instead_of_following_it() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::write(f.outside.join("secret.txt"), b"secret").expect("seed");
        let path = confine(&roots, &f.root, "a.txt").expect("confine");
        symlink(f.outside.join("secret.txt"), &path).expect("symlink");

        write_confined(&roots, &path, b"mine").expect("write");

        assert_eq!(std::fs::read(&path).expect("read"), b"mine");
        assert_eq!(
            std::fs::read(f.outside.join("secret.txt")).expect("secret"),
            b"secret"
        );
    }

    #[test]
    fn a_staged_write_dropped_without_commit_leaves_no_temp_file() {
        let f = fixture();
        let roots = [f.root.clone()];
        let path = confine(&roots, &f.root, "a.txt").expect("confine");

        let staged = stage_confined(&roots, &path, b"x").expect("stage");
        assert_eq!(names(&f.root).len(), 1);
        drop(staged);

        assert!(names(&f.root).is_empty());
    }
}
