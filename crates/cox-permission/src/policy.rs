// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The approval-policy × sandbox-mode table for `Exec` calls (plan.md §1.8
//! step 8, T4.3). Its own module so the twelve cells are one function the
//! matrix test reads directly, instead of a condition buried in `by_risk`
//! and another in the loop's sandbox-denial path: both consult this.

use cox_protocol::types::{ApprovalPolicy, SandboxMode};

/// How an `Exec` call that no rule or grant settled proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecPath {
    /// Run it under the sandbox without asking; a sandbox denial comes back
    /// as `ApprovalRequired { SandboxDenied }` and an `Allow` reruns it
    /// unconfined.
    Confined,
    /// Ask first.
    Ask,
    /// Refuse: the policy never asks and nothing confines the call.
    Deny,
}

/// The line stream-json prints after `SessionStarted` and the TUI keeps in
/// its banner while `danger-full-access` is on.
pub const DANGER_FULL_ACCESS: &str =
    "sandbox off (danger-full-access): shell commands run unconfined with your full access";

/// `on-failure` is the only policy that trusts the sandbox instead of the
/// user, and only while there is a sandbox: with `danger-full-access` it
/// asks like `on-request`. `never` turns every ask into a denial.
pub fn exec_path(policy: ApprovalPolicy, sandbox: SandboxMode) -> ExecPath {
    match (policy, sandbox) {
        (ApprovalPolicy::OnFailure, SandboxMode::ReadOnly | SandboxMode::WorkspaceWrite) => {
            ExecPath::Confined
        }
        (ApprovalPolicy::Never, _) => ExecPath::Deny,
        _ => ExecPath::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_on_failure_is_confined_only_while_a_sandbox_exists() {
        assert_eq!(
            exec_path(ApprovalPolicy::OnFailure, SandboxMode::WorkspaceWrite),
            ExecPath::Confined
        );
        assert_eq!(
            exec_path(ApprovalPolicy::OnFailure, SandboxMode::DangerFullAccess),
            ExecPath::Ask
        );
        assert_eq!(
            exec_path(ApprovalPolicy::Never, SandboxMode::WorkspaceWrite),
            ExecPath::Deny
        );
        assert_eq!(
            exec_path(ApprovalPolicy::Untrusted, SandboxMode::ReadOnly),
            ExecPath::Ask
        );
    }
}
