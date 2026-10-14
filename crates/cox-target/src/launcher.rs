// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Where each launcher program lives (T68.1): a fixed list of absolute paths
//! per kind and OS, first existing one wins. Separate from `command` so the
//! existence check is injected and a test touches no real path. `PATH` is
//! never consulted: a repository or a compromised `PATH` entry must not
//! choose the program that receives the session.

use std::path::{Path, PathBuf};

use crate::{Kind, TargetError};

/// The absolute paths `kind`'s launcher may be at, most preferred first.
pub fn candidates(kind: Kind) -> Vec<PathBuf> {
    let program = kind.program();
    match kind {
        Kind::Ssh if cfg!(windows) => {
            // why: `SystemRoot` is the only place Windows says where it is
            // installed; the default is right on every stock machine.
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            vec![
                PathBuf::from(root)
                    .join("System32")
                    .join("OpenSSH")
                    .join("ssh.exe"),
            ]
        }
        Kind::Ssh => vec![PathBuf::from("/usr/bin/ssh")],
        Kind::Docker if cfg!(windows) => {
            vec![PathBuf::from(
                "C:\\Program Files\\Docker\\Docker\\resources\\bin\\docker.exe",
            )]
        }
        Kind::Docker => [
            "/opt/homebrew/bin/docker",
            "/usr/local/bin/docker",
            "/usr/bin/docker",
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        ]
        .map(PathBuf::from)
        .to_vec(),
        // Lima and Colima have no Windows build.
        Kind::Lima | Kind::Colima if cfg!(windows) => Vec::new(),
        Kind::Lima | Kind::Colima => [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/home/linuxbrew/.linuxbrew/bin",
        ]
        .map(|dir| Path::new(dir).join(program))
        .to_vec(),
    }
}

/// The first of `kind`'s candidates `exists` accepts.
pub fn find(kind: Kind, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates(kind).into_iter().find(|p| exists(p))
}

/// `kind`'s launcher on this machine.
pub fn launcher(kind: Kind) -> Result<PathBuf, TargetError> {
    find(kind, Path::is_file).ok_or(TargetError::NoLauncher(kind.program()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [Kind; 4] = [Kind::Ssh, Kind::Docker, Kind::Lima, Kind::Colima];

    #[test]
    #[allow(unsafe_code)]
    fn launcher_lookup_never_uses_path() {
        let dir = tempfile::tempdir().expect("a temp dir");
        // why: env::set_var is unsafe in edition 2024; the test would have
        // `find` read this directory if it consulted PATH.
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("PATH", dir.path()) };
        for kind in KINDS {
            for program in [kind.program(), "ssh"] {
                std::fs::write(dir.path().join(program), "").expect("a fake launcher");
            }
            for candidate in candidates(kind) {
                assert!(candidate.is_absolute(), "{candidate:?} is relative");
            }
            assert_eq!(find(kind, |p| p.starts_with(dir.path())), None);
            assert_eq!(find(kind, |_| false), None);
        }
    }

    #[test]
    fn first_existing_candidate_wins() {
        for kind in KINDS {
            let all = candidates(kind);
            let Some(last) = all.last() else { continue };
            assert_eq!(find(kind, |p| p == last).as_ref(), Some(last));
            assert_eq!(find(kind, |_| true), all.first().cloned());
        }
    }
}
