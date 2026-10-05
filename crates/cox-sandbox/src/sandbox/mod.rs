// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The sandbox front door (plan.md D7): turns a shell command, or any
//! program argv (`argv`, the desktop terminal pane's login shell), plus the
//! session's `SandboxPolicy` into what confines it on this host. Separate from `bash` so the tool only knows it runs *a* command,
//! and so the backends (`seatbelt` on macOS, `bwrap` or `landlock` on
//! Linux) share one policy-to-paths translation and `doctor` has one place
//! to ask. Seatbelt and bwrap wrap the argv; Landlock cannot, so it hooks
//! the child between fork and exec instead.

pub mod bwrap;
#[cfg(target_os = "linux")]
pub mod landlock;
pub mod seatbelt;

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use cox_protocol::{LinuxBackend, SandboxMode, SandboxPolicy};

/// Part of macOS since 10.5; not a PATH lookup because the sandbox must
/// not depend on what the user's shell resolves.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// What confines shell commands on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// macOS `sandbox-exec` profiles.
    Seatbelt,
    /// bubblewrap namespaces and bind mounts.
    Bwrap,
    /// Landlock rules plus a seccomp network filter, applied in the child.
    Landlock,
}

impl Backend {
    /// The name `doctor` and the status line print.
    pub fn name(self) -> &'static str {
        match self {
            Backend::Seatbelt => "seatbelt",
            Backend::Bwrap => "bwrap",
            Backend::Landlock => "landlock",
        }
    }
}

/// The backend that will confine commands here, or `None` when nothing
/// will (Windows, `linux_backend = none`, or a Linux host with neither
/// namespaces nor Landlock). D7: `cox_session::sandbox::effective_approval`
/// turns `None` into a security notice and forces `on-failure` to
/// `on-request` on every session open (T57.3).
pub fn backend(linux: LinuxBackend) -> Option<Backend> {
    if cfg!(target_os = "macos") {
        return Path::new(SANDBOX_EXEC)
            .is_file()
            .then_some(Backend::Seatbelt);
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    match linux {
        LinuxBackend::None => None,
        LinuxBackend::Bwrap => bwrap_works().then_some(Backend::Bwrap),
        LinuxBackend::Landlock => landlock_works().then_some(Backend::Landlock),
        LinuxBackend::Auto => {
            if bwrap_works() {
                Some(Backend::Bwrap)
            } else if landlock_works() {
                Some(Backend::Landlock)
            } else {
                None
            }
        }
    }
}

/// The command that runs `command` under `policy`: `<shell> -c` wrapped by
/// the backend, or bare for `danger-full-access` and hosts without one.
/// `shell` is an absolute path the caller already resolved from its own
/// allowlist — the sandbox never looks a program up on `PATH`.
// why: Command::pre_exec applies the sandbox guard in the forked child.
#[allow(unsafe_code)]
pub fn command(
    policy: &SandboxPolicy,
    roots: &[PathBuf],
    writable_roots: &[PathBuf],
    shell: &Path,
    command: &str,
) -> io::Result<Command> {
    let shell = [
        shell.to_string_lossy().into_owned(),
        "-c".to_string(),
        command.to_string(),
    ];
    let backend = active(policy);
    // Landlock confines the child in `pre_exec` below instead of wrapping
    // the argv, so only it bypasses `wrap`'s refusal.
    let argv = match backend {
        Some(Backend::Landlock) => shell.to_vec(),
        other => wrap(other, policy, roots, writable_roots, &shell)?,
    };
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    #[cfg(target_os = "linux")]
    if backend == Some(Backend::Landlock) {
        use std::os::unix::process::CommandExt;
        let scratch = scratch(policy.mode);
        let guard = landlock::prepare(policy, &writable(policy, writable_roots, &scratch))?;
        // SAFETY: `apply` only issues syscalls on state prepared before the
        // fork; nothing in it allocates or takes a lock.
        unsafe { cmd.pre_exec(move || guard.apply()) };
    }
    Ok(cmd)
}

/// The argv that runs `program` (an absolute path and its arguments, e.g.
/// an interactive login shell in a PTY) under `policy`: wrapped by
/// Seatbelt or bwrap, or unchanged for `danger-full-access` and hosts
/// without a backend, exactly as `command` wraps `<shell> -c`. Landlock is
/// refused: it confines the child in a `pre_exec` hook, which a caller that
/// only has an argv (a PTY spawn) cannot carry.
pub fn argv(
    policy: &SandboxPolicy,
    roots: &[PathBuf],
    writable_roots: &[PathBuf],
    program: &[String],
) -> io::Result<Vec<String>> {
    wrap(active(policy), policy, roots, writable_roots, program)
}

/// The backend that confines this policy's commands: none for
/// `danger-full-access`, otherwise the host's.
fn active(policy: &SandboxPolicy) -> Option<Backend> {
    (policy.mode != SandboxMode::DangerFullAccess)
        .then(|| backend(policy.linux_backend))
        .flatten()
}

/// The one place a policy becomes an argv; `backend` is passed in so the
/// Landlock refusal is testable on a host that has no Landlock.
fn wrap(
    backend: Option<Backend>,
    policy: &SandboxPolicy,
    roots: &[PathBuf],
    writable_roots: &[PathBuf],
    program: &[String],
) -> io::Result<Vec<String>> {
    if program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox: empty program argv",
        ));
    }
    let scratch = scratch(policy.mode);
    Ok(match backend {
        Some(Backend::Seatbelt) => {
            let profile = seatbelt::profile(policy, writable_roots, &scratch);
            let mut argv = vec![
                SANDBOX_EXEC.to_string(),
                "-p".to_string(),
                profile,
                "--".to_string(),
            ];
            argv.extend(program.iter().cloned());
            argv
        }
        Some(Backend::Bwrap) => bwrap::argv(policy, roots, writable_roots, &scratch, program),
        Some(Backend::Landlock) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sandbox: landlock cannot wrap an argv; it needs the child's pre_exec",
            ));
        }
        None => program.to_vec(),
    })
}

/// Directories every command may write regardless of the workspace: the
/// temp dir always, plus the shared temp root and the user's cache in
/// `workspace-write` (cargo, pip and friends keep their caches there).
fn scratch(mode: SandboxMode) -> Vec<PathBuf> {
    let mut dirs = vec![std::env::temp_dir()];
    if mode == SandboxMode::WorkspaceWrite {
        dirs.push(PathBuf::from("/tmp"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".cache"));
        }
    }
    dirs
}

/// What the command may write: `scratch` in every mode, the workspace
/// explicitly writable roots and `[sandbox].writable` only in
/// `workspace-write`.
fn writable(
    policy: &SandboxPolicy,
    writable_roots: &[PathBuf],
    scratch: &[PathBuf],
) -> Vec<PathBuf> {
    let mut paths = scratch.to_vec();
    if policy.mode == SandboxMode::WorkspaceWrite {
        paths.extend(writable_roots.iter().chain(&policy.writable).cloned());
    }
    paths
}

/// Every root × `readonly_in_workspace`, the subpaths that stay read-only
/// inside a writable root.
fn readonly(policy: &SandboxPolicy, writable_roots: &[PathBuf]) -> Vec<PathBuf> {
    writable_roots
        .iter()
        .flat_map(|root| {
            policy
                .readonly_in_workspace
                .iter()
                .map(|sub| root.join(sub))
        })
        .collect()
}

/// Runs `bwrap` once with the namespaces the real argv uses and remembers
/// the answer for the process; see `bwrap::PROBE` for why PATH is not enough.
fn bwrap_works() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        Command::new("bwrap")
            .args(bwrap::PROBE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn landlock_works() -> bool {
    #[cfg(target_os = "linux")]
    {
        landlock::supported()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: SandboxMode) -> SandboxPolicy {
        SandboxPolicy {
            mode,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        }
    }

    fn cmd_argv(cmd: &Command) -> Vec<String> {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn sandbox_danger_full_access_runs_the_shell_bare() {
        let cmd = command(
            &policy(SandboxMode::DangerFullAccess),
            &[],
            &[],
            Path::new("/bin/sh"),
            "echo hi",
        )
        .expect("command");
        assert_eq!(cmd_argv(&cmd), ["/bin/sh", "-c", "echo hi"]);
    }

    #[test]
    fn sandbox_writable_is_scratch_plus_roots_only_in_workspace_write() {
        let roots = vec![PathBuf::from("/ws")];
        let scratch = vec![PathBuf::from("/scratch")];
        let rw = writable(&policy(SandboxMode::WorkspaceWrite), &roots, &scratch);
        assert_eq!(rw, [PathBuf::from("/scratch"), PathBuf::from("/ws")]);
        let ro = writable(&policy(SandboxMode::ReadOnly), &roots, &scratch);
        assert_eq!(ro, [PathBuf::from("/scratch")]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_macos_backend_is_seatbelt_and_wraps_the_shell() {
        assert_eq!(backend(LinuxBackend::Auto), Some(Backend::Seatbelt));
        let cmd = command(
            &policy(SandboxMode::WorkspaceWrite),
            &[],
            &[],
            Path::new("/bin/zsh"),
            "echo hi",
        )
        .expect("command");
        let argv = cmd_argv(&cmd);
        assert_eq!(argv[0], SANDBOX_EXEC);
        assert_eq!(argv[1], "-p");
        assert!(argv[2].starts_with("(version 1)"));
        // The chosen shell is what the profile wraps, not always /bin/sh.
        assert_eq!(&argv[argv.len() - 3..], ["/bin/zsh", "-c", "echo hi"]);
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn command_argv_is_unchanged_by_the_extraction() {
        let roots = vec![PathBuf::from("/ws")];
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess,
        ] {
            let policy = policy(mode);
            let shell = strings(&["/bin/zsh", "-c", "echo hi"]);
            let cmd = command(&policy, &roots, &roots, Path::new("/bin/zsh"), "echo hi")
                .expect("command");
            // The argv `command` built inline before `argv` existed.
            let scratch = scratch(mode);
            let expected = match active(&policy) {
                Some(Backend::Seatbelt) => {
                    let mut v = vec![
                        SANDBOX_EXEC.to_string(),
                        "-p".to_string(),
                        seatbelt::profile(&policy, &roots, &scratch),
                        "--".to_string(),
                    ];
                    v.extend(shell.clone());
                    v
                }
                Some(Backend::Bwrap) => bwrap::argv(&policy, &roots, &roots, &scratch, &shell),
                Some(Backend::Landlock) | None => shell.clone(),
            };
            assert_eq!(cmd_argv(&cmd), expected, "{mode:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn argv_wraps_an_interactive_login_shell_in_seatbelt() {
        let roots = vec![PathBuf::from("/ws")];
        let policy = policy(SandboxMode::WorkspaceWrite);
        let login = strings(&["/bin/zsh", "-l", "-i"]);
        let argv = argv(&policy, &roots, &roots, &login).expect("argv");
        assert_eq!(argv[..2], [SANDBOX_EXEC, "-p"]);
        assert_eq!(
            argv[2],
            seatbelt::profile(&policy, &roots, &scratch(policy.mode))
        );
        assert_eq!(argv[3], "--");
        assert_eq!(argv[4..], login[..]);
    }

    #[test]
    fn argv_refuses_landlock() {
        let policy = policy(SandboxMode::WorkspaceWrite);
        let login = strings(&["/bin/bash", "-l", "-i"]);
        let err = wrap(Some(Backend::Landlock), &policy, &[], &[], &login)
            .expect_err("landlock cannot wrap an argv");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn argv_leaves_danger_full_access_bare() {
        let login = strings(&["/bin/zsh", "-l", "-i"]);
        let argv = argv(
            &policy(SandboxMode::DangerFullAccess),
            &[PathBuf::from("/ws")],
            &[PathBuf::from("/ws")],
            &login,
        )
        .expect("argv");
        assert_eq!(argv, login);
    }
}
