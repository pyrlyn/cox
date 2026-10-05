// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! bubblewrap argv (plan.md T4.2 step 1): user and pid namespaces, `/`
//! bound read-only, the writable set bound read-write on top, the read-only
//! subpaths re-bound read-only over that, a private `/tmp`, and no network
//! namespace unless the policy allows one. Pure text like `seatbelt`, so it
//! is unit-tested on every platform; only running it needs Linux.

use std::path::{Path, PathBuf};

use cox_protocol::SandboxPolicy;

/// The namespaces and mounts every invocation starts with. `bwrap` sets
/// `PR_SET_NO_NEW_PRIVS` itself.
const BASE: &[&str] = &[
    "--unshare-user",
    "--unshare-pid",
    "--die-with-parent",
    "--ro-bind",
    "/",
    "/",
    "--tmpfs",
    "/tmp",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
];

/// A run that proves the host lets us use the namespaces `BASE` needs: a
/// `bwrap` binary on PATH is not enough (Docker, hardened kernels and
/// Ubuntu's AppArmor all refuse unprivileged user namespaces).
pub const PROBE: &[&str] = &[
    "--unshare-user",
    "--unshare-pid",
    "--die-with-parent",
    "--ro-bind",
    "/",
    "/",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
    "/bin/true",
];

/// The full argv, `bwrap` first, `shell` last. Bind sources must exist, so
/// missing ones are skipped. The private tmpfs hides the host `/tmp`, so a
/// writable path under it is not bound (that would put the whole host `/tmp`
/// back) unless it is a workspace root, which is bound so the command's cwd
/// exists. A program that itself lives under `/tmp` is mounted back by
/// [`expose_under_private_tmp`], one directory, not `/tmp`.
pub fn argv(
    policy: &SandboxPolicy,
    roots: &[PathBuf],
    writable_roots: &[PathBuf],
    scratch: &[PathBuf],
    shell: &[String],
) -> Vec<String> {
    let mut argv: Vec<String> = std::iter::once("bwrap")
        .chain(BASE.iter().copied())
        .map(str::to_string)
        .collect();
    let writable = super::writable(policy, writable_roots, scratch);
    for path in writable.iter().filter(|p| p.exists()) {
        let under_tmp = path.starts_with("/tmp") && !roots.contains(path);
        if !under_tmp {
            bind(&mut argv, "--bind", path);
        }
    }
    for path in roots
        .iter()
        .filter(|root| !writable_roots.contains(root) && root.exists())
    {
        bind(&mut argv, "--ro-bind", path);
    }
    for path in super::readonly(policy, writable_roots)
        .iter()
        .filter(|p| p.exists())
    {
        bind(&mut argv, "--ro-bind", path);
    }
    if !policy.network {
        argv.push("--unshare-net".to_string());
    }
    argv.push("--".to_string());
    argv.extend(shell.iter().cloned());
    argv
}

/// Mounts back the directory under the private `/tmp` that holds `program`,
/// so a plugin binary, an external agent or a `PATH` entry there can be
/// exec'd. The mount is the highest directory under `/tmp` that still
/// contains only that program's tree: `/tmp` itself is never bound (that
/// would expose every sibling), and the climb stops before a writable
/// `--bind` that already covers the program or that a wider mount would
/// hide. Inserts the `--ro-bind`s immediately before `--`.
pub fn expose_under_private_tmp(argv: &mut Vec<String>, programs: &[PathBuf]) {
    let Some(at) = argv.iter().position(|arg| arg == "--") else {
        return;
    };
    let mut mounts = Vec::new();
    for program in programs {
        let located = program.canonicalize().unwrap_or_else(|_| program.clone());
        let Some(mount) = expose_mount(&located, argv) else {
            continue;
        };
        if !mount.exists() || mounts.contains(&mount) {
            continue;
        }
        mounts.push(mount);
    }
    let mut extra = Vec::new();
    for mount in &mounts {
        bind(&mut extra, "--ro-bind", mount);
    }
    argv.splice(at..at, extra);
}

/// Where `program` should be read back from the host `/tmp`. A path that
/// does not exist is treated as a file, so callers can test the shape
/// without creating it; [`expose_under_private_tmp`] drops missing mounts.
fn expose_mount(program: &Path, argv: &[String]) -> Option<PathBuf> {
    let tmp = Path::new("/tmp");
    if !program.starts_with(tmp) || program == tmp {
        return None;
    }
    let start = if program.is_dir() {
        program.to_path_buf()
    } else if program.parent() == Some(tmp) {
        if covered_by_writable_bind(argv, program) {
            return None;
        }
        return Some(program.to_path_buf());
    } else {
        program.parent()?.to_path_buf()
    };
    if covered_by_writable_bind(argv, &start) {
        return None;
    }
    let mut dir = start;
    while let Some(parent) = dir.parent() {
        if parent == tmp
            || covered_by_writable_bind(argv, parent)
            || writable_bind_inside(argv, parent)
        {
            break;
        }
        dir = parent.to_path_buf();
    }
    Some(dir)
}

fn covered_by_writable_bind(argv: &[String], mount: &Path) -> bool {
    argv.windows(3).any(|w| {
        if w[0] != "--bind" {
            return false;
        }
        let bound = Path::new(&w[1]);
        mount == bound || mount.starts_with(bound)
    })
}

fn writable_bind_inside(argv: &[String], parent: &Path) -> bool {
    argv.windows(3).any(|w| {
        if w[0] != "--bind" {
            return false;
        }
        let bound = Path::new(&w[1]);
        bound.starts_with(parent) && bound != parent
    })
}

fn bind(argv: &mut Vec<String>, flag: &str, path: &Path) {
    let path = path.to_string_lossy().into_owned();
    argv.push(flag.to_string());
    argv.push(path.clone());
    argv.push(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::SandboxMode;

    fn policy(mode: SandboxMode, network: bool) -> SandboxPolicy {
        SandboxPolicy {
            mode,
            network,
            writable: vec![],
            readonly_in_workspace: vec![PathBuf::from(".git")],
            linux_backend: Default::default(),
        }
    }

    fn shell() -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), "echo hi".into()]
    }

    fn triple(flag: &str, path: &Path) -> String {
        let p = path.to_string_lossy();
        format!("{flag} {p} {p}")
    }

    #[test]
    fn bwrap_workspace_write_binds_the_root_and_rebinds_git_read_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir");
        let root = dir.path().to_path_buf();
        let argv = argv(
            &policy(SandboxMode::WorkspaceWrite, false),
            std::slice::from_ref(&root),
            std::slice::from_ref(&root),
            &[],
            &shell(),
        );
        let joined = argv.join(" ");
        assert!(
            joined
                .starts_with("bwrap --unshare-user --unshare-pid --die-with-parent --ro-bind / /"),
            "{joined}"
        );
        assert!(joined.contains(&triple("--bind", &root)), "{joined}");
        assert!(
            joined.contains(&triple("--ro-bind", &root.join(".git"))),
            "{joined}"
        );
        assert!(joined.contains("--unshare-net"), "{joined}");
        assert!(joined.ends_with("-- /bin/sh -c echo hi"), "{joined}");
        let bind = joined.find(&triple("--bind", &root));
        let ro = joined.find(&triple("--ro-bind", &root.join(".git")));
        assert!(bind < ro, "the read-only bind must come last to win");
    }

    #[test]
    fn bwrap_read_only_binds_nothing_writable_and_network_flag_drops_unshare_net() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let argv = argv(
            &policy(SandboxMode::ReadOnly, true),
            std::slice::from_ref(&root),
            std::slice::from_ref(&root),
            &[],
            &shell(),
        );
        let joined = argv.join(" ");
        assert!(!joined.contains("--bind "), "{joined}");
        assert!(!joined.contains("--unshare-net"), "{joined}");
    }

    #[test]
    fn bwrap_keeps_non_writable_workspace_roots_read_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("main");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&main).expect("main");
        std::fs::create_dir_all(&worktree).expect("worktree");
        let roots = vec![worktree.clone(), main.clone()];
        let argv = argv(
            &policy(SandboxMode::WorkspaceWrite, false),
            &roots,
            std::slice::from_ref(&worktree),
            &[],
            &shell(),
        );
        let joined = argv.join(" ");
        assert!(joined.contains(&triple("--bind", &worktree)), "{joined}");
        assert!(joined.contains(&triple("--ro-bind", &main)), "{joined}");
        assert!(!joined.contains(&triple("--bind", &main)), "{joined}");
    }

    #[test]
    fn bwrap_skips_missing_sources_and_scratch_under_tmp() {
        let missing = PathBuf::from("/definitely/not/here");
        let argv = argv(
            &policy(SandboxMode::WorkspaceWrite, false),
            std::slice::from_ref(&missing),
            std::slice::from_ref(&missing),
            &[PathBuf::from("/tmp")],
            &shell(),
        );
        let joined = argv.join(" ");
        assert!(!joined.contains("/definitely/not/here"), "{joined}");
        assert!(!joined.contains("--bind /tmp /tmp"), "{joined}");
    }

    #[test]
    fn expose_mount_is_the_program_tree_under_tmp_and_not_tmp_itself() {
        assert_eq!(
            expose_mount(Path::new("/tmp/pkg/bin/server"), &[]),
            Some(PathBuf::from("/tmp/pkg"))
        );
        assert_eq!(
            expose_mount(Path::new("/tmp/server"), &[]),
            Some(PathBuf::from("/tmp/server"))
        );
        assert_eq!(expose_mount(Path::new("/tmp"), &[]), None);
        assert_eq!(expose_mount(Path::new("/usr/bin/sh"), &[]), None);
    }

    #[test]
    fn expose_mount_does_not_cover_or_hide_a_writable_bind() {
        let writable = vec![
            "--bind".to_string(),
            "/tmp/foo/nested".to_string(),
            "/tmp/foo/nested".to_string(),
        ];
        assert_eq!(
            expose_mount(Path::new("/tmp/foo/nested/bin/server"), &writable),
            None,
            "a program inside a writable root is already visible"
        );
        assert_eq!(
            expose_mount(Path::new("/tmp/foo/elsewhere/server"), &writable),
            Some(PathBuf::from("/tmp/foo/elsewhere")),
            "a wider mount would hide the writable bind"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_runs_a_program_from_a_private_tmp_dir_and_hides_siblings() {
        let probe = std::process::Command::new("bwrap")
            .args(PROBE)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !probe.map(|s| s.success()).unwrap_or(false) {
            eprintln!("skipped: bwrap namespaces are unavailable");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cox-t3514-{}", std::process::id()));
        let sibling = std::env::temp_dir().join(format!("cox-t3514-sib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&sibling);
        std::fs::create_dir_all(&dir).expect("program dir");
        let program = dir.join("run");
        std::fs::write(
            &program,
            "#!/bin/sh\nif [ -r \"$1\" ]; then echo leaked; exit 2; fi\necho ok\n",
        )
        .expect("script");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::write(&sibling, "secret").expect("sibling");

        let mut built = argv(
            &policy(SandboxMode::ReadOnly, false),
            &[],
            &[],
            &[],
            &["/bin/sh".into(), "-c".into(), r#"exec "$0" "$@""#.into()],
        );
        expose_under_private_tmp(&mut built, std::slice::from_ref(&program));
        built.push(program.display().to_string());
        built.push(sibling.display().to_string());
        let out = std::process::Command::new(&built[0])
            .args(&built[1..])
            .output()
            .expect("bwrap runs");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&sibling);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "status {:?} stdout {stdout} stderr {stderr}",
            out.status
        );
        assert!(stdout.contains("ok"), "{stdout}");
        assert!(!stdout.contains("leaked"), "{stdout}");
    }
}
