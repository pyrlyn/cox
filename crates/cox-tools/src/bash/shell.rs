// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Which program runs a `bash` command line (T57.2). Separate from the
//! runner because the choice is host policy, not process plumbing, and a
//! test drives it with an injected [`Lookup`] on any host.
//!
//! On Unix a shell is looked up only in [`SHELL_DIRS`], never on `PATH`:
//! what the sandbox spawns must not depend on an environment the workspace
//! can rewrite. On Windows there is no sandbox (D7) and no fixed shell
//! directory, so the lookup goes through `PATH` (R10.1.5). The default
//! there is Git Bash when Git for Windows is installed, else PowerShell —
//! `pwsh` 7 when present, else the Windows PowerShell every Windows 10+
//! ships (A128 (1); what Claude Code does, R10.5.2). Consequences:
//!
//! - the `bash` tool description names the default shell in use, so the
//!   model writes PowerShell syntax when there is no Git Bash;
//! - the risk classifier parses bash with tree-sitter, so a PowerShell or
//!   `cmd` command it cannot parse is classified as unknown (`Exec`, opaque
//!   segments) and asks;
//! - `Bash(...)` permission rules match the command text as on Unix;
//! - the same resolution serves `!` (a `bash` call on the model's path) and
//!   hooks ([`default_shell`], passed to the hook runner by the session).

use std::path::{Path, PathBuf};

/// Where a shell may live on Unix. Not `PATH`: see the header.
pub(super) const SHELL_DIRS: &[&str] = &["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"];

/// The host family resolution follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Host {
    Unix,
    Windows,
}

impl Host {
    pub(super) fn current() -> Self {
        if cfg!(windows) {
            Host::Windows
        } else {
            Host::Unix
        }
    }
}

/// What resolution may ask the machine; tests inject a fake one.
pub(super) trait Lookup {
    fn is_file(&self, path: &Path) -> bool;
    /// `name` in the first `PATH` directory that holds it.
    fn on_path(&self, name: &str) -> Option<PathBuf>;
}

/// The real filesystem and `PATH`.
pub(super) struct System;

impl Lookup for System {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn on_path(&self, name: &str) -> Option<PathBuf> {
        crate::lsp::on_path(name)
    }
}

/// The program for the shell called `name` (`sh` is the default): on Unix
/// the first [`SHELL_DIRS`] entry holding it; on Windows the default is
/// [`windows_default`], `bash` is Git Bash only, and any other name is
/// `<name>.exe` on `PATH`.
pub(super) fn resolve(name: &str, host: Host, lookup: &dyn Lookup) -> Option<PathBuf> {
    match (host, name) {
        (Host::Unix, _) => SHELL_DIRS
            .iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|path| lookup.is_file(path)),
        (Host::Windows, "sh") => windows_default(lookup),
        (Host::Windows, "bash") => git_bash(lookup),
        (Host::Windows, _) => lookup.on_path(&format!("{name}.exe")),
    }
}

/// Where [`resolve`] looked, for the error that names a missing shell.
pub(super) fn searched(host: Host) -> String {
    match host {
        Host::Unix => SHELL_DIRS.join(", "),
        Host::Windows => "PATH".to_string(),
    }
}

/// Windows' default shell: Git Bash, else `pwsh`, else Windows PowerShell.
fn windows_default(lookup: &dyn Lookup) -> Option<PathBuf> {
    git_bash(lookup)
        .or_else(|| lookup.on_path("pwsh.exe"))
        .or_else(|| lookup.on_path("powershell.exe"))
}

/// Git for Windows' `bash.exe`, found from `git.exe` on `PATH` (the
/// installer puts `<root>\cmd` there; `<root>\bin` and `<root>\mingw64\bin`
/// also hold a `git.exe`). Never `bash.exe` on `PATH` itself:
/// `C:\Windows\System32\bash.exe` is the WSL launcher, which runs the line
/// in a Linux VM with another view of the filesystem.
fn git_bash(lookup: &dyn Lookup) -> Option<PathBuf> {
    let git = lookup.on_path("git.exe")?;
    git.ancestors()
        .skip(2)
        .take(2)
        .map(|root| root.join("bin").join("bash.exe"))
        .find(|bash| lookup.is_file(bash))
}

/// How the tool description names the default shell (`sh` in `resolve`).
pub(super) fn default_label(host: Host, lookup: &dyn Lookup) -> String {
    if host == Host::Unix {
        return "`sh`".to_string();
    }
    let program = windows_default(lookup);
    let file = program
        .as_deref()
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase);
    match file.as_deref() {
        Some("bash") => "Git Bash (`bash`)".to_string(),
        Some(stem) => format!("PowerShell (`{stem}`; write PowerShell syntax)"),
        None => "no shell: neither Git Bash nor PowerShell is installed".to_string(),
    }
}

/// The default shell on this host, for callers outside the `bash` tool
/// (hooks), so they spawn the same program a `!` line does.
pub fn default_shell() -> Option<PathBuf> {
    resolve("sh", Host::current(), &System)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A machine with these files and this `PATH` (name → program).
    struct Fake {
        files: HashSet<PathBuf>,
        path: HashMap<&'static str, PathBuf>,
    }

    impl Fake {
        fn new(files: &[&str], path: &[(&'static str, &str)]) -> Self {
            Fake {
                files: files.iter().map(PathBuf::from).collect(),
                path: path.iter().map(|(n, p)| (*n, PathBuf::from(p))).collect(),
            }
        }
    }

    impl Lookup for Fake {
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn on_path(&self, name: &str) -> Option<PathBuf> {
            self.path.get(name).cloned()
        }
    }

    const GIT: &str = "C:/Program Files/Git/cmd/git.exe";
    const GIT_BASH: &str = "C:/Program Files/Git/bin/bash.exe";
    const PWSH: &str = "C:/Program Files/PowerShell/7/pwsh.exe";
    const WINPS: &str = "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
    const WSL: &str = "C:/Windows/System32/bash.exe";

    #[test]
    fn windows_shell_resolution_follows_the_chosen_order() {
        let win = |fake: &Fake| resolve("sh", Host::Windows, fake);
        let all = Fake::new(
            &[GIT_BASH],
            &[
                ("git.exe", GIT),
                ("pwsh.exe", PWSH),
                ("powershell.exe", WINPS),
                ("bash.exe", WSL),
            ],
        );
        assert_eq!(win(&all), Some(PathBuf::from(GIT_BASH)));
        assert_eq!(
            resolve("bash", Host::Windows, &all),
            Some(PathBuf::from(GIT_BASH))
        );
        // `git.exe` under `mingw64\bin` finds the same `bash.exe`.
        let mingw = Fake::new(
            &[GIT_BASH],
            &[("git.exe", "C:/Program Files/Git/mingw64/bin/git.exe")],
        );
        assert_eq!(win(&mingw), Some(PathBuf::from(GIT_BASH)));
        // No Git Bash: `pwsh`, then Windows PowerShell; the WSL launcher on
        // `PATH` is never taken for Git Bash.
        let no_git = Fake::new(
            &[],
            &[
                ("pwsh.exe", PWSH),
                ("powershell.exe", WINPS),
                ("bash.exe", WSL),
            ],
        );
        assert_eq!(win(&no_git), Some(PathBuf::from(PWSH)));
        assert_eq!(resolve("bash", Host::Windows, &no_git), None);
        let only_winps = Fake::new(&[], &[("powershell.exe", WINPS)]);
        assert_eq!(win(&only_winps), Some(PathBuf::from(WINPS)));
        assert_eq!(win(&Fake::new(&[], &[])), None);
        // Any other named shell is `<name>.exe` on `PATH`.
        assert_eq!(
            resolve("pwsh", Host::Windows, &no_git),
            Some(PathBuf::from(PWSH))
        );
        assert_eq!(resolve("zsh", Host::Windows, &no_git), None);
        assert_eq!(searched(Host::Windows), "PATH");
    }

    #[test]
    fn unix_shell_resolution_ignores_path() {
        let fake = Fake::new(&["/usr/bin/zsh"], &[("zsh", "/work/evil/zsh")]);
        assert_eq!(
            resolve("zsh", Host::Unix, &fake),
            Some(PathBuf::from("/usr/bin/zsh"))
        );
        assert_eq!(resolve("fish", Host::Unix, &fake), None);
    }

    #[test]
    fn tool_description_names_the_windows_default_shell() {
        let git = Fake::new(&[GIT_BASH], &[("git.exe", GIT), ("powershell.exe", WINPS)]);
        assert_eq!(default_label(Host::Windows, &git), "Git Bash (`bash`)");
        let ps = Fake::new(&[], &[("powershell.exe", WINPS)]);
        assert!(
            default_label(Host::Windows, &ps).contains("PowerShell (`powershell`"),
            "{}",
            default_label(Host::Windows, &ps)
        );
        assert_eq!(default_label(Host::Unix, &ps), "`sh`");
    }
}
