// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Opening a path `path::confine` already approved (T64.15). `confine`
//! returns a path, and the caller opens it later: anything that swaps a
//! component for a symlink in between (a background shell, another process)
//! redirects the open outside every root. This module closes that window at
//! the open itself — the leaf is opened without following a symlink, then the
//! opened descriptor's real path is checked against the same canonical roots
//! `confine` uses. Separate from `path` because `path` stays filesystem-pure
//! up to canonicalisation, while this owns descriptors and per-OS calls.

use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use cox_protocol::ToolError;

use crate::path::confined;
#[cfg(unix)]
use crate::path::{canonical_roots, path_starts_with};

/// Opens `path` — the result of `confine` — with `opts`, refusing it when a
/// symlink swapped in since `confine` would take the open outside `roots`.
///
/// On Unix the leaf is opened with `O_NOFOLLOW` (any custom flags in `opts`
/// are replaced), so a leaf turned into a symlink fails as `Confined`. A
/// swapped *parent* directory is still followed by the kernel, so the opened
/// descriptor's real path (Linux `/proc/self/fd`, macOS `F_GETPATH`) must
/// lie inside the canonical roots, or the file is closed and `Confined` is
/// returned. The check runs after the open: do not pass `truncate` (call
/// `File::set_len(0)` on the result instead), and know that `create` through
/// a swapped parent can leave an empty file outside the roots before the
/// refusal. This is the read-side open; writes go through
/// [`crate::write`], which checks the parent directory before creating
/// anything (T64.15.3).
///
/// On non-Unix targets this is a plain open: Windows has no `O_NOFOLLOW`
/// equivalent in `std` and no safe path-from-handle call here, so the swap
/// window `confine` leaves remains there.
pub fn open_confined(
    roots: &[PathBuf],
    path: &Path,
    opts: &OpenOptions,
) -> Result<File, ToolError> {
    let file = open_no_follow(path, opts).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::NotFound
        } else if is_symlink_refusal(&e) {
            confined(path, roots)
        } else {
            ToolError::Io
        }
    })?;
    #[cfg(unix)]
    check_inside_roots(&file, roots)?;
    Ok(file)
}

/// Requires the object `fd` refers to — a file or a directory — to lie inside
/// the canonical roots, by its real path. An unreadable real path fails
/// closed: without it the descriptor cannot be shown to have stayed inside the
/// roots. Shared with `write`, which checks the parent directory it then
/// works relative to.
#[cfg(unix)]
pub(crate) fn check_inside_roots(
    fd: &impl std::os::fd::AsFd,
    roots: &[PathBuf],
) -> Result<(), ToolError> {
    let real = opened_path(fd).map_err(|_| ToolError::Io)?;
    if canonical_roots(roots)
        .iter()
        .any(|r| path_starts_with(&real, r))
    {
        Ok(())
    } else {
        Err(confined(&real, roots))
    }
}

/// Reads the whole of `path` — the result of `confine` — through
/// [`open_confined`], so a symlink swapped in since `confine` is `Confined`
/// instead of being read. The read-side counterpart of `std::fs::read` for
/// every tool that opens a path after confining it (T64.15.2).
pub fn read_confined(roots: &[PathBuf], path: &Path) -> Result<Vec<u8>, ToolError> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let mut file = open_confined(roots, path, &opts)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| ToolError::Io)?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_no_follow(path: &Path, opts: &OpenOptions) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut opts = opts.clone();
    opts.custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
    opts.open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path, opts: &OpenOptions) -> std::io::Result<File> {
    opts.open(path)
}

/// `O_NOFOLLOW` on a symlink leaf fails with `ELOOP` on Linux and macOS.
#[cfg(unix)]
fn is_symlink_refusal(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(nix::libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_refusal(_: &std::io::Error) -> bool {
    false
}

/// A missing `/proc` (some containers) fails closed through the caller.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn opened_path(fd: &impl std::os::fd::AsFd) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd as _;
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()))
}

#[cfg(target_vendor = "apple")]
fn opened_path(fd: &impl std::os::fd::AsFd) -> std::io::Result<PathBuf> {
    let mut real = PathBuf::new();
    nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut real))?;
    Ok(real)
}

/// cox ships only Linux and macOS among Unix targets; elsewhere there is no
/// path-from-descriptor call wired up, so every open fails closed.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn opened_path(_: &impl std::os::fd::AsFd) -> std::io::Result<PathBuf> {
    Err(std::io::ErrorKind::Unsupported.into())
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
        std::fs::write(outside.join("secret.txt"), b"secret").expect("write secret");
        Fixture {
            _dir: dir,
            root,
            outside,
        }
    }

    fn read_opts() -> OpenOptions {
        let mut opts = OpenOptions::new();
        opts.read(true);
        opts
    }

    fn read_all(mut file: File) -> String {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut file, &mut s).expect("read");
        s
    }

    #[test]
    fn open_confined_refuses_a_leaf_swapped_for_a_symlink_that_std_read_follows() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::write(f.root.join("a.txt"), b"mine").expect("write");
        let path = confine(&roots, &f.root, "a.txt").expect("confine");

        std::fs::remove_file(&path).expect("rm");
        symlink(f.outside.join("secret.txt"), &path).expect("symlink");

        // The bug: a plain re-open of the confined path reads outside.
        assert_eq!(std::fs::read(&path).expect("std read"), b"secret");
        let err = open_confined(&roots, &path, &read_opts()).expect_err("must refuse");
        assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
    }

    #[test]
    fn open_confined_refuses_a_parent_swapped_for_a_symlink_that_std_read_follows() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::create_dir(f.root.join("sub")).expect("mkdir sub");
        std::fs::write(f.root.join("sub/secret.txt"), b"mine").expect("write");
        let path = confine(&roots, &f.root, "sub/secret.txt").expect("confine");

        std::fs::rename(f.root.join("sub"), f.root.join("moved")).expect("rename");
        symlink(&f.outside, f.root.join("sub")).expect("symlink");

        // O_NOFOLLOW guards only the leaf; the kernel follows `sub`.
        assert_eq!(std::fs::read(&path).expect("std read"), b"secret");
        let err = open_confined(&roots, &path, &read_opts()).expect_err("must refuse");
        assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
    }

    #[test]
    fn open_confined_opens_a_plain_file_inside_the_root() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::write(f.root.join("a.txt"), b"mine").expect("write");
        let path = confine(&roots, &f.root, "a.txt").expect("confine");

        let file = open_confined(&roots, &path, &read_opts()).expect("open");
        assert_eq!(read_all(file), "mine");
    }

    #[test]
    fn open_confined_opens_an_in_root_path_confine_resolved_through_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Not canonicalised: on macOS the tempdir itself sits under the
        // `/var` -> `/private/var` symlink, like a real symlinked root.
        let root = dir.path().to_path_buf();
        std::fs::create_dir(root.join("real")).expect("mkdir");
        std::fs::write(root.join("real/f.txt"), b"mine").expect("write");
        symlink(root.join("real"), root.join("alias")).expect("symlink");
        let roots = [root.clone()];
        let path = confine(&roots, &root, "alias/f.txt").expect("confine");

        let file = open_confined(&roots, &path, &read_opts()).expect("open");
        assert_eq!(read_all(file), "mine");
    }

    #[test]
    fn open_confined_creates_a_new_file_inside_the_root() {
        let f = fixture();
        let roots = [f.root.clone()];
        let path = confine(&roots, &f.root, "new.txt").expect("confine");
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);

        let mut file = open_confined(&roots, &path, &opts).expect("create");
        std::io::Write::write_all(&mut file, b"fresh").expect("write");
        assert_eq!(std::fs::read(&path).expect("read back"), b"fresh");
    }

    #[test]
    fn open_confined_reports_a_missing_file_as_not_found() {
        let f = fixture();
        let roots = [f.root.clone()];
        let path = confine(&roots, &f.root, "absent.txt").expect("confine");

        let err = open_confined(&roots, &path, &read_opts()).expect_err("missing");
        assert!(matches!(err, ToolError::NotFound), "{err:?}");
    }

    #[test]
    fn read_confined_refuses_a_swapped_leaf_and_a_swapped_parent() {
        let f = fixture();
        let roots = [f.root.clone()];
        std::fs::create_dir(f.root.join("sub")).expect("mkdir sub");
        std::fs::write(f.root.join("sub/secret.txt"), b"mine").expect("write");
        std::fs::write(f.root.join("a.txt"), b"mine").expect("write");
        let leaf = confine(&roots, &f.root, "a.txt").expect("confine");
        let nested = confine(&roots, &f.root, "sub/secret.txt").expect("confine");
        assert_eq!(read_confined(&roots, &leaf).expect("plain"), b"mine");

        std::fs::remove_file(&leaf).expect("rm");
        symlink(f.outside.join("secret.txt"), &leaf).expect("symlink");
        std::fs::rename(f.root.join("sub"), f.root.join("moved")).expect("rename");
        symlink(&f.outside, f.root.join("sub")).expect("symlink");

        for path in [&leaf, &nested] {
            let err = read_confined(&roots, path).expect_err("must refuse");
            assert!(matches!(err, ToolError::Confined { .. }), "{err:?}");
        }
    }
}
