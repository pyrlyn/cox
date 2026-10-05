// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Seatbelt profile text for `sandbox-exec` (macOS, plan.md T4.1). Separate
//! from the front door because the profile is pure text: it is built and
//! unit-tested on every platform, only running it is macOS-only.

use std::path::{Path, PathBuf};

use cox_protocol::{SandboxMode, SandboxPolicy};

/// What a shell on a PTY needs to run at all. Everything else, including
/// every write and the network, is denied unless a later rule allows it.
const BASE: &str = "(version 1)
(deny default)
(allow file-read*)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow sysctl-read)
(allow mach-lookup)
(allow file-ioctl)
(allow pseudo-tty)
(allow ipc-posix*)
(allow process-info*)
(allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/ttys[0-9]+$\"))
";

/// The profile for `policy` over the workspace `roots`; `scratch` is
/// writable in every mode. Later rules win in Seatbelt, so the read-only
/// subpaths are denied after the roots are allowed.
pub fn profile(policy: &SandboxPolicy, roots: &[PathBuf], scratch: &[PathBuf]) -> String {
    let mut out = String::from(BASE);
    let writable = super::writable(policy, roots, scratch);
    if !writable.is_empty() {
        out.push_str(&rule("allow file-write*", &writable));
    }
    let readonly = super::readonly(policy, roots);
    if policy.mode == SandboxMode::WorkspaceWrite && !readonly.is_empty() {
        out.push_str(&rule("deny file-write*", &readonly));
    }
    if policy.network {
        out.push_str("(allow network*)\n");
    }
    out
}

fn rule(head: &str, paths: &[PathBuf]) -> String {
    let subpaths: String = paths
        .iter()
        .map(|p| format!(" (subpath \"{}\")", escape(&real(p))))
        .collect();
    format!("({head}{subpaths})\n")
}

/// Seatbelt matches the path the kernel sees, so symlinked roots (`/tmp`,
/// `/var`) must be resolved first. A path that does not exist yet stays as
/// given.
fn real(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: SandboxMode, network: bool) -> SandboxPolicy {
        SandboxPolicy {
            mode,
            network,
            writable: vec![PathBuf::from("/extra/out")],
            readonly_in_workspace: vec![PathBuf::from(".git"), PathBuf::from(".cox")],
            linux_backend: Default::default(),
        }
    }

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/ws/one")]
    }

    fn scratch() -> Vec<PathBuf> {
        vec![PathBuf::from("/scratch/tmp")]
    }

    #[test]
    fn seatbelt_workspace_write_allows_roots_then_denies_readonly_subpaths() {
        let p = profile(
            &policy(SandboxMode::WorkspaceWrite, false),
            &roots(),
            &scratch(),
        );
        let allow = "(allow file-write* (subpath \"/scratch/tmp\") (subpath \"/ws/one\") (subpath \"/extra/out\"))";
        let deny = "(deny file-write* (subpath \"/ws/one/.git\") (subpath \"/ws/one/.cox\"))";
        assert!(p.starts_with("(version 1)\n(deny default)\n"), "{p}");
        assert!(p.contains(allow), "{p}");
        assert!(p.contains(deny), "{p}");
        assert!(
            p.find(allow) < p.find(deny),
            "the deny must come last to win"
        );
        assert!(!p.contains("network"), "{p}");
    }

    #[test]
    fn seatbelt_read_only_writes_only_scratch() {
        let p = profile(&policy(SandboxMode::ReadOnly, false), &roots(), &scratch());
        assert!(
            p.contains("(allow file-write* (subpath \"/scratch/tmp\"))"),
            "{p}"
        );
        assert!(!p.contains("/ws/one"), "{p}");
        assert!(!p.contains("/extra/out"), "{p}");
    }

    #[test]
    fn seatbelt_network_flag_adds_the_rule_and_paths_are_escaped() {
        let root = vec![PathBuf::from("/ws/say \"hi\"")];
        let p = profile(&policy(SandboxMode::WorkspaceWrite, true), &root, &[]);
        assert!(p.ends_with("(allow network*)\n"), "{p}");
        assert!(p.contains("(subpath \"/ws/say \\\"hi\\\"\")"), "{p}");
    }
}
