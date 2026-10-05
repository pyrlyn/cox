// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The single place a tool call is allowed, denied or escalated (AGENTS.md
//! trust boundaries; plan.md §1.8). Pure: rules compile once from config,
//! then `decide` is a function of the call, the mode, the policy and the
//! session grants — no I/O, so the 30-row table and the proptest need no
//! session around them. A tool never checks its own permission.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod policy;
pub mod rules;

use std::path::Path;

use cox_protocol::config::PermissionsConfig;
use cox_protocol::errors::CoreError;
use cox_protocol::types::{
    ApprovalPolicy, DecidedBy, PermissionMode, Risk, SandboxMode, ToolCall, Why,
};

use policy::{ExecPath, exec_path};
use rules::{Rule, canonical_tool};

/// What the engine concluded for one call.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Run it.
    Allow {
        /// What allowed it.
        by: DecidedBy,
    },
    /// Refuse it; the model sees `reason`.
    Deny {
        /// Shown in the tool result.
        reason: String,
        /// What denied it.
        by: DecidedBy,
    },
    /// The surface must ask the user.
    Ask(Why),
}

/// Compiled `allow`/`ask`/`deny` rules.
#[derive(Debug, Clone, Default)]
pub struct Engine {
    deny: Vec<Rule>,
    allow: Vec<Rule>,
    ask: Vec<Rule>,
}

impl Engine {
    /// Compiles the three rule lists; a malformed rule is a config error,
    /// never a silently skipped guard.
    pub fn compile(
        cfg: &PermissionsConfig,
        home: Option<&Path>,
        cwd: &Path,
    ) -> Result<Self, CoreError> {
        let compile = |key: &str, raw: &[String]| {
            raw.iter()
                .map(|r| {
                    Rule::parse(r, home, cwd).map_err(|message| CoreError::Config {
                        key: format!("permissions.{key}"),
                        message: format!("{r:?}: {message}"),
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            deny: compile("deny", &cfg.deny)?,
            allow: compile("allow", &cfg.allow)?,
            ask: compile("ask", &cfg.ask)?,
        })
    }

    /// Plan.md §1.8 steps 1–9, in order. `sandbox` only matters for `Exec`
    /// under `on-failure`: that policy trusts the sandbox, so without one
    /// it asks (T4.3).
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::path::Path;
    /// use cox_permission::{Engine, Outcome};
    /// use cox_protocol::{CallId, config::PermissionsConfig, types::*};
    /// use serde_json::json;
    ///
    /// let home = Path::new("/home/alice");
    /// let engine = Engine::compile(&PermissionsConfig::default(), Some(home), Path::new("/repo"))?;
    /// // A deny rule beats the ReadOnly default: the default config denies
    /// // `Read(~/.ssh/**)`, so this never runs, whatever else matches.
    /// let ssh = ToolCall {
    ///     id: CallId::new(),
    ///     name: "read".into(),
    ///     input: json!({"path": "/home/alice/.ssh/id_ed25519"}),
    ///     risk: Risk::ReadOnly,
    ///     subject: "/home/alice/.ssh/id_ed25519".into(),
    ///     segments: None,
    /// };
    /// let outcome = engine.decide(
    ///     &ssh,
    ///     PermissionMode::Default,
    ///     ApprovalPolicy::OnRequest,
    ///     SandboxMode::WorkspaceWrite,
    ///     &[],
    /// );
    /// assert!(matches!(outcome, Outcome::Deny { .. }));
    /// # Ok::<(), cox_protocol::CoreError>(())
    /// ```
    pub fn decide(
        &self,
        call: &ToolCall,
        mode: PermissionMode,
        policy: ApprovalPolicy,
        sandbox: SandboxMode,
        grants: &[(String, String)],
    ) -> Outcome {
        // Deny and ask need one hit: the whole line or any of its commands.
        let first = |rules: &[Rule]| {
            rules
                .iter()
                .find(|r| {
                    r.matches(&call.name, &call.subject)
                        || call
                            .segments
                            .as_ref()
                            .is_some_and(|s| s.commands.iter().any(|c| r.matches(&call.name, c)))
                })
                .map(|r| r.raw.clone())
        };
        if let Some(rule) = first(&self.deny) {
            return Outcome::Deny {
                reason: format!("denied by rule {rule}"),
                by: DecidedBy::Rule,
            };
        }
        if mode == PermissionMode::Bypass {
            return Outcome::Allow {
                by: DecidedBy::Policy,
            };
        }
        if mode == PermissionMode::Plan {
            return if call.risk == Risk::ReadOnly {
                Outcome::Allow {
                    by: DecidedBy::Policy,
                }
            } else {
                Outcome::Deny {
                    reason: "plan mode: only read-only tools run; describe the change instead"
                        .into(),
                    by: DecidedBy::Policy,
                }
            };
        }
        if covered(
            call,
            |line| self.allow.iter().any(|r| r.matches_line(&call.name, line)),
            |c| self.allow.iter().any(|r| r.matches(&call.name, c)),
        ) {
            return Outcome::Allow {
                by: DecidedBy::Rule,
            };
        }
        let why = if let Some(rule) = first(&self.ask) {
            Some(Why::RuleAsk { rule })
        } else if granted(call, grants) {
            return Outcome::Allow {
                by: DecidedBy::Session,
            };
        } else if policy == ApprovalPolicy::Untrusted && call.risk != Risk::ReadOnly {
            Some(Why::Policy { policy })
        } else {
            by_risk(call.risk, mode, policy, sandbox)
        };
        match why {
            None => Outcome::Allow {
                by: DecidedBy::Policy,
            },
            Some(why) if policy == ApprovalPolicy::Never => Outcome::Deny {
                reason: format!("{} and the approval policy is `never`", why_text(&why)),
                by: DecidedBy::Policy,
            },
            Some(why) => Outcome::Ask(why),
        }
    }
}

/// Whether allow-side matchers cover `call`. A call without segments is one
/// unit, matched by `each`. A split command line is covered by `line` on its
/// whole text (an exact or bare rule), or by `each` on every one of its
/// commands — never when the split is opaque (T36.1).
fn covered(call: &ToolCall, line: impl Fn(&str) -> bool, each: impl Fn(&str) -> bool) -> bool {
    match &call.segments {
        None => each(&call.subject),
        Some(s) => {
            line(&call.subject)
                || (!s.opaque && !s.commands.is_empty() && s.commands.iter().all(|c| each(c)))
        }
    }
}

/// Step 6: an `AllowForSession` grant. Grants are recorded per command by
/// [`grants_for`]; a grant covers a command it prefixes at a word boundary,
/// and an opaque line only when the user approved that exact line.
fn granted(call: &ToolCall, grants: &[(String, String)]) -> bool {
    let mine = || {
        grants
            .iter()
            .filter(|(tool, _)| rules::tool_matches(&canonical_tool(tool), &call.name))
            .map(|(_, subject)| subject.as_str())
    };
    // A call without segments is one subject (a path, a URL, an MCP name).
    // The same word boundary as a split command: `/repo/a.rs` does not
    // cover `/repo/a.rs.bak`, and `https://example.com` does not cover
    // `https://example.com.evil`. An empty grant is not a prefix of every
    // subject (`starts_with("")` is true for every string).
    covered(
        call,
        |line| mine().any(|g| g == line),
        |c| mine().any(|g| rules::word_prefix(g, c)),
    )
}

/// The `(tool, subject)` grants an `AllowForSession` answer to `call`
/// records: one per command of a split line, so approving `git status &&
/// npm test` later covers `npm test` alone and never `npm test; rm -rf ~`.
/// An opaque line or a call without segments records its whole subject.
pub fn grants_for(call: &ToolCall) -> Vec<(String, String)> {
    match &call.segments {
        Some(s) if !s.opaque && !s.commands.is_empty() => s
            .commands
            .iter()
            .map(|c| (call.name.clone(), c.clone()))
            .collect(),
        _ => vec![(call.name.clone(), call.subject.clone())],
    }
}

/// Step 7: what the risk alone requires. `Exec` marked safe by the T3.7
/// classifier arrives here as `ReadOnly` (A5: risk is per call).
fn by_risk(
    risk: Risk,
    mode: PermissionMode,
    policy: ApprovalPolicy,
    sandbox: SandboxMode,
) -> Option<Why> {
    match risk {
        Risk::ReadOnly => None,
        Risk::Write if mode == PermissionMode::Auto => None,
        Risk::Exec if exec_path(policy, sandbox) == ExecPath::Confined => None,
        _ => Some(Why::Risk { risk }),
    }
}

/// One line for a `Deny` reason or a notice.
pub fn why_text(why: &Why) -> String {
    match why {
        Why::RuleAsk { rule } => format!("rule {rule} requires approval"),
        Why::Risk { risk } => format!("{risk:?} calls require approval"),
        Why::SandboxDenied { detail } => format!("the sandbox denied it: {detail}"),
        Why::Policy { policy } => format!("approval policy {policy:?} requires approval"),
    }
}

/// `Shift+Tab`: default → plan → auto → default (§1.13); bypass is never
/// cycled into, only left. Here, beside the modes' meaning, so the TUI and
/// the desktop composer (T37.24.7) cycle in the same order.
pub fn next_mode(mode: PermissionMode) -> PermissionMode {
    match mode {
        PermissionMode::Default => PermissionMode::Plan,
        PermissionMode::Plan => PermissionMode::Auto,
        PermissionMode::Auto | PermissionMode::Bypass => PermissionMode::Default,
    }
}

/// The narrower of two permission modes (P42, A73): a mode preset may only
/// tighten what `permissions.mode` allows, never widen it.
pub fn narrower(a: PermissionMode, b: PermissionMode) -> PermissionMode {
    if rank(a) <= rank(b) { a } else { b }
}

/// How much a mode lets through without asking: `Plan < Default < Auto <
/// Bypass`. The one definition of that order, so `narrower` cannot drift.
fn rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Default => 1,
        PermissionMode::Auto => 2,
        PermissionMode::Bypass => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MODES: [PermissionMode; 4] = [
        PermissionMode::Plan,
        PermissionMode::Default,
        PermissionMode::Auto,
        PermissionMode::Bypass,
    ];

    #[test]
    fn narrower_never_returns_the_wider_mode() {
        for a in ALL_MODES {
            for b in ALL_MODES {
                let n = narrower(a, b);
                assert!(n == a || n == b, "{a:?} ∧ {b:?} gave a third mode {n:?}");
                assert_eq!(rank(n), rank(a).min(rank(b)), "{a:?} ∧ {b:?} gave {n:?}");
            }
        }
        assert_eq!(
            narrower(PermissionMode::Bypass, PermissionMode::Plan),
            PermissionMode::Plan
        );
        assert_eq!(
            narrower(PermissionMode::Auto, PermissionMode::Default),
            PermissionMode::Default
        );
    }

    #[test]
    fn narrower_is_commutative() {
        for a in ALL_MODES {
            for b in ALL_MODES {
                assert_eq!(narrower(a, b), narrower(b, a), "{a:?}, {b:?}");
            }
        }
    }

    #[test]
    fn shift_tab_cycles_default_plan_auto_and_leaves_bypass() {
        let from = [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::Bypass,
        ];
        let to = from.map(next_mode);
        assert_eq!(
            to,
            [
                PermissionMode::Plan,
                PermissionMode::Auto,
                PermissionMode::Default,
                PermissionMode::Default,
            ]
        );
    }
}
