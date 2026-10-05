// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One process drives a session (T37.34, A67, DT§11 Q6): the process that
//! runs session `<id>` holds an OS advisory lock on `sessions/<id>.lock`,
//! next to its rollout. The kernel drops the lock when that process exits
//! or crashes, so there is no lease to go stale; the holder written into the
//! file is only there to name it in a refusal — the lock, not the text, is
//! the truth. Separate from the rollout writer because a lock outlives any
//! one `Store`: claims within one process share it, so a surface that
//! reopens its own session (a failed `/fork` falling back to the parent) is
//! never refused by itself. The file is left behind on release; unlinking
//! it would race a second process that has it open.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use cox_protocol::{SessionId, StoreError};
use serde::{Deserialize, Serialize};

/// Who drives a session, as its lock file says. `Default` stands for a
/// holder that has the lock but has not written itself yet, or a file this
/// platform will not let a second process read (Windows locks the bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    /// The surface that opened it: `tui`, `plain`, `headless`, `app`.
    pub surface: String,
    /// RFC 3339, when the lock was taken.
    pub since: String,
}

impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.pid == 0 {
            return f.write_str("another cox process");
        }
        write!(
            f,
            "cox {} (pid {}, since {})",
            self.surface, self.pid, self.since
        )
    }
}

/// A held session lock; the last clone dropped (or the process gone)
/// releases it.
#[derive(Debug, Clone)]
pub struct SessionLock {
    _file: Arc<File>,
}

/// This process's live locks, so a second claim of the same session here
/// shares the first instead of meeting it as another writer (flock locks an
/// open file description, so a second `open` would conflict with the first).
static HELD: Mutex<Vec<(PathBuf, Weak<File>)>> = Mutex::new(Vec::new());

/// Claims session `id` under `sessions` for `surface`: the lock, or who
/// holds it when another process does.
pub fn claim(
    sessions: &Path,
    id: &SessionId,
    surface: &str,
) -> Result<Result<SessionLock, Holder>, StoreError> {
    let path = sessions.join(format!("{id}.lock"));
    let mut held = HELD.lock().unwrap_or_else(|poison| poison.into_inner());
    held.retain(|(_, weak)| weak.strong_count() > 0);
    if let Some(file) = held
        .iter()
        .find(|(p, _)| *p == path)
        .and_then(|(_, weak)| weak.upgrade())
    {
        return Ok(Ok(SessionLock { _file: file }));
    }
    std::fs::create_dir_all(sessions).map_err(|_| StoreError::Io)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|_| StoreError::Io)?;
    match file.try_lock() {
        Ok(()) => {
            let me = Holder {
                pid: std::process::id(),
                surface: surface.to_string(),
                since: crate::now_rfc3339(),
            };
            let text = serde_json::to_vec(&me).map_err(|_| StoreError::Io)?;
            file.set_len(0).map_err(|_| StoreError::Io)?;
            file.write_all(&text).map_err(|_| StoreError::Io)?;
            let file = Arc::new(file);
            held.push((path, Arc::downgrade(&file)));
            Ok(Ok(SessionLock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => {
            let mut text = String::new();
            let holder = match file.read_to_string(&mut text) {
                Ok(_) => serde_json::from_str(&text).unwrap_or_default(),
                Err(_) => Holder::default(),
            };
            Ok(Err(holder))
        }
        Err(TryLockError::Error(_)) => Err(StoreError::Io),
    }
}

/// Who drives session `id` from another process, without claiming it
/// (T37.10: the desktop's "busy elsewhere" row). A shared probe, released
/// at once, so it neither writes the holder file nor keeps a lock; a
/// session this process holds is not "elsewhere".
pub fn holder(sessions: &Path, id: &SessionId) -> Result<Option<Holder>, StoreError> {
    let path = sessions.join(format!("{id}.lock"));
    let held = HELD.lock().unwrap_or_else(|poison| poison.into_inner());
    if held
        .iter()
        .any(|(p, weak)| *p == path && weak.strong_count() > 0)
    {
        return Ok(None);
    }
    let Ok(mut file) = File::open(&path) else {
        return Ok(None);
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            Ok(Some(serde_json::from_str(&text).unwrap_or_default()))
        }
        Err(TryLockError::Error(_)) => Err(StoreError::Io),
    }
}

#[cfg(test)]
mod tests {
    //! The holder is a second process: this test binary re-executed to run
    //! only the ignored `holder_process` (the `watch.rs` pattern), because
    //! within one process claims share the lock by design.

    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    use super::*;

    const HOLD_DIR: &str = "COX_STORE_TEST_HOLD_DIR";
    const HOLD_ID: &str = "COX_STORE_TEST_HOLD_ID";

    fn spawn_holder(dir: &Path, id: &SessionId) -> Child {
        Command::new(std::env::current_exe().expect("current_exe"))
            .args([
                "lock::tests::holder_process",
                "--exact",
                "--ignored",
                "--test-threads=1",
                "-q",
            ])
            .env(HOLD_DIR, dir)
            .env(HOLD_ID, id.to_string())
            .spawn()
            .expect("spawn holder")
    }

    fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let start = Instant::now();
        while !ok() {
            assert!(start.elapsed() < Duration::from_secs(30), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The child side: claims the session, says so with a `ready` file,
    /// holds it until a `release` file appears, then exits without running
    /// destructors, as a crash would: only the kernel releases the lock.
    #[test]
    #[ignore = "run by spawn_holder in a child process"]
    fn holder_process() {
        let dir = PathBuf::from(std::env::var_os(HOLD_DIR).expect("dir"));
        let id: SessionId = std::env::var(HOLD_ID).expect("id").parse().expect("id");
        let lock = claim(&dir, &id, "test").expect("claim").expect("free");
        std::fs::write(dir.join("ready"), "").expect("ready");
        wait_for("release", || dir.join("release").exists());
        std::mem::forget(lock);
        std::process::exit(0);
    }

    #[test]
    fn second_opener_gets_session_busy() {
        let dir = tempfile::tempdir().expect("dir");
        let id = SessionId::new();
        let mut child = spawn_holder(dir.path(), &id);
        wait_for("child claimed", || dir.path().join("ready").exists());
        let holder = claim(dir.path(), &id, "tui")
            .expect("claim")
            .expect_err("the child drives it");
        assert_eq!(holder.pid, child.id());
        assert_eq!(holder.surface, "test");
        assert!(holder.to_string().starts_with("cox test (pid "), "{holder}");
        std::fs::write(dir.path().join("release"), "").expect("release");
        assert!(child.wait().expect("child").success());
    }

    #[test]
    fn lock_released_when_holder_exits() {
        let dir = tempfile::tempdir().expect("dir");
        let id = SessionId::new();
        std::fs::write(dir.path().join("release"), "").expect("release");
        let mut child = spawn_holder(dir.path(), &id);
        assert!(child.wait().expect("child").success());
        assert!(dir.path().join("ready").exists(), "the child held it first");
        let lock = claim(dir.path(), &id, "tui").expect("claim");
        assert!(lock.is_ok(), "the kernel dropped the exited child's lock");
    }

    #[test]
    fn holder_names_another_process_without_claiming() {
        let dir = tempfile::tempdir().expect("dir");
        let id = SessionId::new();
        assert_eq!(holder(dir.path(), &id).expect("probe"), None, "no file yet");
        let mut child = spawn_holder(dir.path(), &id);
        wait_for("child claimed", || dir.path().join("ready").exists());
        let busy = holder(dir.path(), &id).expect("probe").expect("busy");
        assert_eq!(busy.pid, child.id());
        std::fs::write(dir.path().join("release"), "").expect("release");
        assert!(child.wait().expect("child").success());
        assert_eq!(holder(dir.path(), &id).expect("probe"), None, "released");
        let _mine = claim(dir.path(), &id, "app").expect("claim").expect("free");
        assert_eq!(
            holder(dir.path(), &id).expect("probe"),
            None,
            "ours is not elsewhere"
        );
    }

    #[test]
    fn claims_in_one_process_share_the_lock() {
        let dir = tempfile::tempdir().expect("dir");
        let id = SessionId::new();
        let first = claim(dir.path(), &id, "tui").expect("claim");
        let second = claim(dir.path(), &id, "tui").expect("claim");
        assert!(first.is_ok() && second.is_ok());
    }
}
