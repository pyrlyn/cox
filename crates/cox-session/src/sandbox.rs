// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The one argv wrap a host-spawned process runs under (T33.19, T33.42,
//! T35.2): a plugin's `[[mcp]]` servers and external agents, every other
//! stdio MCP server, and `doctor`'s probe of them. Separate so each of those
//! reaches the same `cox_tools::sandbox` guard `bash` runs under. Also D7
//! as code ([`effective_approval`], T57.3): what a host with no backend
//! does to the approval policy, applied once where every surface opens a
//! session.

use std::path::{Path, PathBuf};

use cox_protocol::Config;
use cox_protocol::types::{ApprovalPolicy, Level};
use cox_tools::sandbox::Backend;

/// The `[sandbox]` config as the policy a host-spawned process runs under:
/// `sandboxed_argv`'s wrap and the ACP client's `fs/*` checks for the same
/// external agent (T35.13) read one value.
pub fn sandbox_policy(config: &Config) -> cox_protocol::SandboxPolicy {
    cox_protocol::SandboxPolicy {
        mode: config.sandbox.mode,
        network: config.sandbox.network,
        writable: config.sandbox.writable.clone(),
        readonly_in_workspace: config.sandbox.readonly_in_workspace.clone(),
        linux_backend: config.sandbox.linux_backend,
    }
}

/// The policy an external agent runs under (T52.2, DT§3.3.1, the creator's
/// decision of 2026-09-29): the session's `[sandbox]` with network always
/// on, because every agent must reach its vendor's API, and every file
/// limit kept. The wrap (`agent_argv`) and the ACP client's `fs/*` and
/// `terminal/*` checks for the same agent read this one value.
pub fn agent_policy(config: &Config) -> cox_protocol::SandboxPolicy {
    cox_protocol::SandboxPolicy {
        network: true,
        ..sandbox_policy(config)
    }
}

/// `sandboxed_argv` under [`agent_policy`]: an external agent's program
/// (a plugin's `[[external_agents]]` entry or a user-config one), with
/// `writable` the session's writable roots plus any state directories the
/// entry lists.
pub fn agent_argv(
    program: &Path,
    args: &[String],
    config: &Config,
    writable: &[PathBuf],
) -> Result<Vec<String>, String> {
    wrap_argv(program, args, &agent_policy(config), config, writable)
}

/// `program args` under `sandbox::command`, the guard `bash` runs under. The
/// backend wraps a `<shell> -c <line>` triple last, so the line
/// `exec "$0" "$@"` with the argv appended runs the program with no shell
/// quoting to get wrong. Landlock confines in a pre-exec hook that no argv
/// can carry, so it — like a host with no backend — refuses the server
/// rather than run it bare; `danger-full-access` is the user's own choice.
/// Shared by a plugin's `[[mcp]]` servers (`plugin_mcp`), its external
/// agents (`plugin_agents`, T35.2) and, T33.42, every other stdio server
/// (`sandbox_stdio_servers`) — one wrap, not two. `pub(crate)` so
/// `doctor::check_external_agents` (T35.8) can probe a granted entry's
/// `--version` under the same wrap, without a second sandbox-wrap
/// implementation.
pub fn sandboxed_argv(
    program: &Path,
    args: &[String],
    config: &Config,
    writable: &[PathBuf],
) -> Result<Vec<String>, String> {
    wrap_argv(program, args, &sandbox_policy(config), config, writable)
}

/// The one argv wrap both entry points share, for a given policy.
fn wrap_argv(
    program: &Path,
    args: &[String],
    policy: &cox_protocol::SandboxPolicy,
    config: &Config,
    writable: &[PathBuf],
) -> Result<Vec<String>, String> {
    use cox_protocol::SandboxMode;
    use cox_tools::sandbox::{self, Backend};

    if policy.mode != SandboxMode::DangerFullAccess {
        match sandbox::backend(policy.linux_backend) {
            Some(Backend::Seatbelt | Backend::Bwrap) => {}
            Some(Backend::Landlock) => {
                return Err(String::from(
                    "the landlock sandbox cannot wrap a server's argv",
                ));
            }
            None => return Err(String::from("no sandbox backend on this host")),
        }
    }
    let line = r#"exec "$0" "$@""#;
    let cmd = sandbox::command(
        policy,
        &config.core.workspace_roots,
        writable,
        Path::new("/bin/sh"),
        line,
    )
    .map_err(|e| format!("sandbox: {e}"))?;
    let mut argv: Vec<String> = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // bwrap's private `/tmp` would hide a program, plugin package or
    // `COX_HOME` that lives there. Mount that directory back, read-only,
    // and leave every sibling in `/tmp` hidden.
    if matches!(sandbox::backend(policy.linux_backend), Some(Backend::Bwrap)) {
        sandbox::bwrap::expose_under_private_tmp(&mut argv, &[host_program(program)]);
    }
    argv.push(program.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    Ok(argv)
}

/// A bare `PATH` name resolved to the file the host would exec. Anything
/// with a directory stays as given: the sandbox still execs the original
/// string, and this path only decides which `/tmp` directory to mount back.
fn host_program(program: &Path) -> PathBuf {
    if program.components().count() != 1 {
        return program.to_path_buf();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return program.to_path_buf();
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return candidate;
        }
    }
    program.to_path_buf()
}

/// D7 as code (T57.3, A128 (3)): with no sandbox backend (Windows, or a
/// Linux host where `linux_backend` finds none) a command runs unconfined,
/// so `on-failure` — which runs `Exec` without asking because the sandbox
/// would catch it — becomes `on-request`. `untrusted` and `never` are
/// stricter already (`never` turns every `Ask` into `Deny`) and stay. The
/// notice is `Security`, so every surface pins it, and comes on every open:
/// the host has not become safer since the last one.
pub fn effective_approval(
    backend: Option<Backend>,
    configured: ApprovalPolicy,
) -> (ApprovalPolicy, Option<(Level, String)>) {
    if backend.is_some() {
        return (configured, None);
    }
    let unconfined = "no sandbox backend on this host: commands run unconfined";
    match configured {
        ApprovalPolicy::OnFailure => (
            ApprovalPolicy::OnRequest,
            Some((
                Level::Security,
                format!("{unconfined}; approval `on-failure` is forced to `on-request` (D7)"),
            )),
        ),
        policy => (
            policy,
            Some((Level::Security, format!("{unconfined} (D7)"))),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ApprovalPolicy; 4] = [
        ApprovalPolicy::Untrusted,
        ApprovalPolicy::OnRequest,
        ApprovalPolicy::OnFailure,
        ApprovalPolicy::Never,
    ];

    #[test]
    fn no_backend_forces_on_request() {
        let effective = |p| effective_approval(None, p).0;
        assert_eq!(
            effective(ApprovalPolicy::OnFailure),
            ApprovalPolicy::OnRequest
        );
        // The stricter policies stay as configured (A128 (3)).
        for p in [
            ApprovalPolicy::Untrusted,
            ApprovalPolicy::OnRequest,
            ApprovalPolicy::Never,
        ] {
            assert_eq!(effective(p), p);
        }
    }

    #[test]
    fn no_backend_emits_a_security_notice() {
        for p in ALL {
            let (level, text) = effective_approval(None, p).1.expect("a notice");
            assert_eq!(level, Level::Security, "{p:?}");
            assert!(text.contains("no sandbox backend"), "{text}");
        }
        let (_, text) = effective_approval(None, ApprovalPolicy::OnFailure)
            .1
            .expect("a notice");
        assert!(text.contains("forced to `on-request`"), "{text}");
    }

    #[test]
    fn backend_present_keeps_the_configured_policy() {
        for backend in [Backend::Seatbelt, Backend::Bwrap, Backend::Landlock] {
            for p in ALL {
                assert_eq!(effective_approval(Some(backend), p), (p, None));
            }
        }
    }
}
