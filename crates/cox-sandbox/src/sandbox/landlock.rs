// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Landlock + seccomp for Linux hosts without bubblewrap (plan.md T4.2
//! step 2). Separate from `bwrap` because this backend cannot wrap an argv:
//! the restriction is applied inside the child between fork and exec, so
//! everything that allocates (opening rule fds, compiling the filter) is
//! prepared here in the parent and only *applied* in `pre_exec`.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use cox_protocol::SandboxPolicy;
use landlock::{
    ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr,
    path_beneath_rules,
};
use nix::libc;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// Best effort at ABI 3 (Linux 6.2, adds truncate); older kernels enforce
/// what they have, which `backend()` accepts as long as Landlock exists.
const WANTED: ABI = ABI::V3;

/// Whether this kernel has Landlock at all.
// why: raw landlock_create_ruleset syscall to probe the ABI version (FFI).
#[allow(unsafe_code)]
pub fn supported() -> bool {
    // SAFETY: the version probe passes no attribute (null, size 0) and
    // touches no memory; the kernel only reports its ABI number.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32, // LANDLOCK_CREATE_RULESET_VERSION
        )
    };
    abi > 0
}

/// Everything the child needs, built in the parent.
pub struct Guard {
    ruleset: RulesetCreated,
    net: Option<BpfProgram>,
}

/// Read everywhere, write on `writable`. Landlock rules only grant, so a
/// read-only subpath inside a writable root cannot be expressed here; that
/// guarantee needs `bwrap`.
pub fn prepare(policy: &SandboxPolicy, writable: &[PathBuf]) -> io::Result<Guard> {
    let ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(WANTED))
        .map_err(io::Error::other)?
        .create()
        .map_err(io::Error::other)?
        .add_rules(path_beneath_rules(["/"], AccessFs::from_read(WANTED)))
        .map_err(io::Error::other)?
        .add_rules(path_beneath_rules(writable, AccessFs::from_all(WANTED)))
        .map_err(io::Error::other)?;
    let net = if policy.network {
        None
    } else {
        Some(net_filter()?)
    };
    Ok(Guard { ruleset, net })
}

impl Guard {
    /// Runs in the child after `fork`: `restrict_self` (which also sets
    /// `PR_SET_NO_NEW_PRIVS`) and the seccomp load are plain syscalls.
    pub fn apply(&self) -> io::Result<()> {
        self.ruleset
            .try_clone()?
            .restrict_self()
            .map_err(io::Error::other)?;
        if let Some(net) = &self.net {
            seccompiler::apply_filter(net).map_err(io::Error::other)?;
        }
        Ok(())
    }
}

/// `connect` and `socket(AF_INET | AF_INET6)` fail with `EPERM`; unix
/// sockets can still be created but never connected, as the plan says.
fn net_filter() -> io::Result<BpfProgram> {
    let family = |af: i32| -> io::Result<SeccompRule> {
        let cond = SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, af as u64)
            .map_err(io::Error::other)?;
        SeccompRule::new(vec![cond]).map_err(io::Error::other)
    };
    let rules = BTreeMap::from([
        (libc::SYS_connect, vec![]),
        (
            libc::SYS_socket,
            vec![family(libc::AF_INET)?, family(libc::AF_INET6)?],
        ),
    ]);
    let arch: TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(io::Error::other)?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(io::Error::other)?;
    BpfProgram::try_from(filter).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::SandboxMode;

    #[test]
    fn landlock_prepare_builds_a_guard_with_a_network_filter_when_offline() {
        if !supported() {
            eprintln!("skipped: no Landlock on this kernel");
            return;
        }
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        };
        let guard = prepare(&policy, &[std::env::temp_dir()]).expect("prepare");
        assert!(guard.net.is_some());
    }
}
