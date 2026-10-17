// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The supervisor's socket and who may reach it (T66.20, `docs/design/serve.md`
//! D4): `COX_HOME/run` is ours and owner-only, one supervisor holds
//! `run/serve.lock`, and only that holder replaces a stale `serve.sock`,
//! bound owner-only. Separate from `restart` because it is the trust
//! boundary: every check here runs before a peer's first line is read. Unix
//! only; Windows has no supervisor yet.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use cox_store::lock::{self, Holder, SessionLock};
use nix::sys::stat::{Mode, umask};

/// The `sun_path` size of this platform, NUL included: 104 on the BSDs and
/// macOS, 108 on Linux.
const SUN_PATH: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
    108
} else {
    104
};

/// Why the supervisor's socket could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// The socket path does not fit `sun_path`.
    #[error(
        "socket path {path} is {len} bytes; sun_path holds at most {limit} bytes with its NUL, \
         and there is no fallback to /tmp. Use a shorter COX_HOME"
    )]
    PathTooLong {
        /// The refused socket path.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
        /// The platform's `sun_path` size, NUL included.
        limit: usize,
    },
    /// The run directory exists but is not private to this user.
    #[error(
        "run directory {path} is unsafe: {reason} (mode {mode:04o}); \
         it must be a real directory you own with mode 0700"
    )]
    RunDir {
        /// The run directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// Another process holds `run/serve.lock`.
    #[error("another supervisor is running: {0}")]
    Busy(Holder),
    /// The lock file could not be opened or locked.
    #[error("could not take the supervisor lock")]
    Lock,
    /// A filesystem or socket call failed.
    #[error("{op} {path}: {source}")]
    Io {
        /// The call that failed.
        op: &'static str,
        /// The path it acted on.
        path: PathBuf,
        /// The OS error.
        source: io::Error,
    },
}

/// A bound supervisor socket. Dropping it closes the socket and releases the
/// singleton lock; the socket file stays and is replaced by the next holder.
#[derive(Debug)]
pub struct Bound {
    /// The listening socket, ready for `tokio::net::UnixListener::from_std`.
    pub listener: UnixListener,
    /// The socket file, `COX_HOME/run/serve.sock`.
    pub path: PathBuf,
    /// The effective uid every accepted peer is compared with.
    pub uid: u32,
    _lock: SessionLock,
}

/// Takes the singleton and binds `COX_HOME/run/serve.sock`, owner-only.
pub fn bind(home: &Path) -> Result<Bound, SocketError> {
    let path = path(home);
    let run = home.join("run");
    // Before anything is created, so a refusal leaves no trace.
    check_len(&path)?;
    let uid = own_uid();
    ensure_run_dir(home, &run, uid)?;
    let lock = match lock::claim_path(&run.join("serve.lock"), "serve") {
        Ok(Ok(lock)) => lock,
        Ok(Err(holder)) => return Err(SocketError::Busy(holder)),
        Err(_) => return Err(SocketError::Lock),
    };
    // Only the lock holder gets here, so a leftover file is nobody's.
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_err("remove", &path, source)),
    }
    let listener = {
        let _umask = Umask::restrict();
        UnixListener::bind(&path).map_err(|source| io_err("bind", &path, source))?
    };
    // The umask already did this; the explicit mode is the check that holds
    // on a platform whose bind ignores it.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|source| io_err("chmod", &path, source))?;
    Ok(Bound {
        listener,
        path,
        uid,
        _lock: lock,
    })
}

/// Where the supervisor of `home` (`COX_HOME`) listens.
pub fn path(home: &Path) -> PathBuf {
    home.join("run").join("serve.sock")
}

/// The effective uid of this process.
pub fn own_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

/// Whether a peer may attach: only the supervisor's own user. Pure, because a
/// test cannot fake a peer uid; root gets no exception.
pub fn admit(peer_uid: u32, own_uid: u32) -> bool {
    peer_uid == own_uid
}

/// Checks an accepted connection before its first line is read. A peer whose
/// credentials cannot be read is refused too: the check fails closed.
pub fn admit_peer(stream: &tokio::net::UnixStream, own_uid: u32) -> bool {
    match stream.peer_cred() {
        Ok(cred) if admit(cred.uid(), own_uid) => true,
        Ok(cred) => {
            tracing::warn!(
                peer_uid = cred.uid(),
                "dropped a connection from another uid"
            );
            false
        }
        Err(error) => {
            tracing::warn!(%error, "dropped a connection with unreadable peer credentials");
            false
        }
    }
}

fn check_len(path: &Path) -> Result<(), SocketError> {
    let len = path.as_os_str().as_bytes().len();
    if len >= SUN_PATH {
        return Err(SocketError::PathTooLong {
            path: path.to_path_buf(),
            len,
            limit: SUN_PATH,
        });
    }
    Ok(())
}

/// Creates `run` as `0700`, or checks the one that exists. Checked even after
/// creating it, so a directory another user raced into place is refused too.
fn ensure_run_dir(home: &Path, run: &Path, uid: u32) -> Result<(), SocketError> {
    std::fs::create_dir_all(home).map_err(|source| io_err("create", home, source))?;
    match std::fs::DirBuilder::new().mode(0o700).create(run) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(io_err("create", run, source)),
    }
    // `symlink_metadata`: a symlink must be seen as one, not followed.
    let meta = std::fs::symlink_metadata(run).map_err(|source| io_err("stat", run, source))?;
    let mode = meta.mode() & 0o7777;
    let unsafe_dir = |reason| SocketError::RunDir {
        path: run.to_path_buf(),
        mode,
        reason,
    };
    if !meta.file_type().is_dir() {
        return Err(unsafe_dir("not a directory (or a symlink)"));
    }
    if meta.uid() != uid {
        return Err(unsafe_dir("owned by another user"));
    }
    if mode & 0o077 != 0 {
        return Err(unsafe_dir("open to group or other"));
    }
    Ok(())
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> SocketError {
    SocketError::Io {
        op,
        path: path.to_path_buf(),
        source,
    }
}

/// Umask `077` for the span of a bind, restored on drop. Process-wide, so it
/// is held only around the one call that creates the socket file.
struct Umask(Mode);

impl Umask {
    fn restrict() -> Self {
        Self(umask(Mode::from_bits_truncate(0o077)))
    }
}

impl Drop for Umask {
    fn drop(&mut self) {
        umask(self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).expect("stat").mode() & 0o777
    }

    /// Another process holding `run/serve.lock`: a second open file
    /// description, which flock treats as a separate owner even in this
    /// process (within one process, claims share the lock by design).
    fn hold_serve_lock(run: &Path, text: &[u8]) -> std::fs::File {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(run.join("serve.lock"))
            .expect("open");
        (&file).write_all(text).expect("write");
        file.lock().expect("lock");
        file
    }

    #[test]
    fn a_second_supervisor_is_refused_naming_the_holder() {
        let home = tempfile::tempdir().expect("home");
        let run = home.path().join("run");
        ensure_run_dir(home.path(), &run, own_uid()).expect("run dir");
        let text = br#"{"pid":4242,"surface":"serve","since":"2026-10-17T00:00:00Z"}"#;
        let _other = hold_serve_lock(&run, text);
        let error = bind(home.path()).expect_err("the lock is held");
        let SocketError::Busy(holder) = &error else {
            panic!("expected Busy, got {error:?}");
        };
        assert_eq!(holder.pid, 4242);
        assert!(error.to_string().contains("pid 4242"), "{error}");
        assert!(!run.join("serve.sock").exists(), "a loser binds nothing");
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let home = tempfile::tempdir().expect("home");
        let run = home.path().join("run");
        ensure_run_dir(home.path(), &run, own_uid()).expect("run dir");
        // A crashed supervisor's socket: the file outlives its listener.
        drop(UnixListener::bind(run.join("serve.sock")).expect("stale"));
        let bound = bind(home.path()).expect("replaced");
        assert!(
            std::os::unix::net::UnixStream::connect(&bound.path).is_ok(),
            "the new listener answers, not the stale file"
        );
    }

    #[test]
    fn a_run_dir_open_to_others_is_refused() {
        let home = tempfile::tempdir().expect("home");
        let run = home.path().join("run");
        std::fs::create_dir(&run).expect("run");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o750)).expect("chmod");
        let error = bind(home.path()).expect_err("group bits");
        let message = error.to_string();
        assert!(message.contains(run.to_str().expect("utf8")), "{message}");
        assert!(message.contains("0750"), "{message}");
    }

    #[test]
    fn a_symlinked_run_dir_is_refused() {
        let home = tempfile::tempdir().expect("home");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        std::fs::set_permissions(elsewhere.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod");
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("run")).expect("link");
        let error = bind(home.path()).expect_err("a symlink");
        assert!(matches!(error, SocketError::RunDir { .. }), "{error:?}");
        assert!(
            std::fs::read_dir(elsewhere.path())
                .expect("dir")
                .next()
                .is_none(),
            "nothing was written through the link"
        );
    }

    #[test]
    fn a_path_over_sun_path_is_refused() {
        let base = tempfile::tempdir().expect("base");
        let home = base.path().join("h".repeat(SUN_PATH));
        let error = bind(&home).expect_err("too long");
        assert!(
            matches!(error, SocketError::PathTooLong { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains(&SUN_PATH.to_string()), "{error}");
        assert!(
            !home.exists(),
            "a refusal creates nothing, and never falls back to /tmp"
        );
    }

    #[test]
    fn the_socket_is_owner_only() {
        let home = tempfile::tempdir().expect("home");
        let bound = bind(home.path()).expect("bind");
        assert_eq!(mode_of(&bound.path), 0o600);
        assert_eq!(mode_of(&home.path().join("run")), 0o700);
    }

    #[test]
    fn admit_refuses_another_uid() {
        assert!(admit(501, 501));
        assert!(!admit(502, 501));
        assert!(!admit(0, 501), "root gets no exception");
    }

    #[tokio::test]
    async fn admit_peer_checks_the_connected_uid() {
        let (a, _b) = tokio::net::UnixStream::pair().expect("pair");
        assert!(admit_peer(&a, own_uid()));
        assert!(!admit_peer(&a, own_uid().wrapping_add(1)));
    }
}
