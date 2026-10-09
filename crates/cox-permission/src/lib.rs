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
use rules::{CLOUD_AGENT, Rule, canonical_tool, is_github_repo};

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
    /// The home the rules were compiled with, to expand `~` in the paths a
    /// read-only command names (the same expansion `Rule::parse` does).
    home: Option<std::path::PathBuf>,
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
            home: home.map(Path::to_path_buf),
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
        // A denied read path is denied however it arrives (T62.3). A bash
        // `cat ~/.ssh/id_rsa` classifies ReadOnly and no Bash command rule
        // can enumerate every reader, so the read path denies also match the
        // paths a read-only command names.
        if call.risk == Risk::ReadOnly
            && let Some(rule) = self.denied_read_path(call)
        {
            return Outcome::Deny {
                reason: format!("denied by rule {rule}"),
                by: DecidedBy::Rule,
            };
        }
        // Before `bypass` and `auto`: code leaving the machine is not a risk
        // class those modes may waive (T56.4, A123 (5)).
        if rules::tool_matches(CLOUD_AGENT, &call.name) {
            return self.decide_cloud_agent(call, mode, policy);
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

impl Engine {
    /// `CloudAgent(<owner>/<repo>)` (T56.4): the repository's code is read and
    /// edited on someone else's machine, so every mode asks, `bypass` and
    /// `auto` included. Only an exact allow rule from the loaded config lifts
    /// the question; `plan` mode never does. A project config cannot hold
    /// such a rule: the config loader reverts its `allow` (A122), so what
    /// reaches the engine is the user's own. Session grants do not count, since
    /// the user approved a command there, not a repository.
    fn decide_cloud_agent(
        &self,
        call: &ToolCall,
        mode: PermissionMode,
        policy: ApprovalPolicy,
    ) -> Outcome {
        let deny = |reason: String| Outcome::Deny {
            reason,
            by: DecidedBy::Policy,
        };
        // The subject is matched byte for byte against rules and printed in
        // the approval, so only a clean `owner/repo` gets that far.
        if !is_github_repo(&call.subject) {
            return deny("a cloud agent needs a GitHub <owner>/<repo> subject".into());
        }
        if mode == PermissionMode::Plan {
            return deny("plan mode: code does not leave this machine".into());
        }
        if self
            .allow
            .iter()
            .any(|r| r.is_exact() && r.matches(&call.name, &call.subject))
        {
            return Outcome::Allow {
                by: DecidedBy::Rule,
            };
        }
        let why = Why::RuleAsk {
            rule: format!("CloudAgent({})", call.subject),
        };
        if policy == ApprovalPolicy::Never {
            return deny(format!(
                "{} and the approval policy is `never`",
                why_text(&why)
            ));
        }
        Outcome::Ask(why)
    }
}

/// The approval text for a cloud agent call (T56.4): the repository (the
/// call's subject), the remote and the starting ref (the call's `remote` and
/// `ref` input fields), and that the code leaves this machine. Control and
/// bidi characters are dropped because the remote and the ref come from a
/// repository's git config and refs, which are untrusted.
pub fn cloud_agent_approval_text(call: &ToolCall) -> String {
    let field = |key: &str| {
        call.input[key]
            .as_str()
            .map(clean)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "unknown".into())
    };
    format!(
        "Run a cloud agent on {repo}?\n\
         Remote: {remote}\n\
         Starting ref: {git_ref}\n\
         The code of this repository is read and edited off this machine.",
        repo = clean(&call.subject),
        remote = field("remote"),
        git_ref = field("ref"),
    )
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
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

impl Engine {
    /// The raw text of the first read-path deny rule whose globs cover a
    /// path the call names, or `None`. Scans the whole subject and every
    /// simple command of a split line, so `echo hi && cat ~/.ssh/id` is
    /// caught by its second segment.
    fn denied_read_path(&self, call: &ToolCall) -> Option<String> {
        if !self.deny.iter().any(|r| r.read_path_rule()) {
            return None;
        }
        let mut lines = vec![call.subject.as_str()];
        if let Some(segments) = &call.segments {
            lines.extend(segments.commands.iter().map(String::as_str));
        }
        for line in lines {
            for token in line.split_whitespace() {
                let Some(path) = token_path(token, self.home.as_deref()) else {
                    continue;
                };
                if let Some(rule) = self
                    .deny
                    .iter()
                    .find(|r| r.read_path_rule() && r.covers_file(&path))
                {
                    return Some(rule.raw.clone());
                }
            }
        }
        None
    }
}

/// The absolute path a command token names, `~` expanded, with surrounding
/// shell punctuation and quotes trimmed; `None` when the token cannot name
/// a file (a flag, a bare word, a relative path — read rules anchor
/// absolutely, so relatives cannot match one anyway).
fn token_path(token: &str, home: Option<&Path>) -> Option<String> {
    let trimmed = token.trim_matches(|c: char| {
        !(c.is_ascii_alphanumeric() || matches!(c, '/' | '~' | '.' | '-' | '_' | '='))
    });
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home.map(|h| h.join(rest).to_string_lossy().into_owned());
    }
    if trimmed.starts_with('/') && trimmed.len() > 1 {
        return Some(trimmed.to_string());
    }
    None
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
    use cox_protocol::{CallId, types::Segments};

    const ALL_MODES: [PermissionMode; 4] = [
        PermissionMode::Plan,
        PermissionMode::Default,
        PermissionMode::Auto,
        PermissionMode::Bypass,
    ];

    fn bash_call(line: &str, commands: &[&str], opaque: bool) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: serde_json::json!({ "command": line }),
            risk: Risk::ReadOnly,
            subject: line.into(),
            segments: Some(Segments {
                commands: commands.iter().map(|c| c.to_string()).collect(),
                opaque,
            }),
        }
    }

    /// T62.3: the default `Read(~/.ssh/**)` deny must also stop a bash
    /// command that names the same files, whatever the reader is.
    #[test]
    fn a_read_only_bash_command_cannot_read_a_denied_path() {
        let home = Path::new("/home/alice");
        let engine = Engine::compile(
            &PermissionsConfig::default(),
            Some(home),
            Path::new("/repo"),
        )
        .expect("default config compiles");
        for line in [
            "cat ~/.ssh/id_ed25519",
            "head -n 1 /home/alice/.ssh/config",
            "grep alice '~/.ssh/known_hosts'",
            "echo hi && cat ~/.ssh/id_ed25519",
            "tail -f ~/.aws/credentials",
        ] {
            let call = bash_call(line, &[line], false);
            let outcome = engine.decide(
                &call,
                PermissionMode::Default,
                ApprovalPolicy::OnRequest,
                SandboxMode::WorkspaceWrite,
                &[],
            );
            assert!(
                matches!(outcome, Outcome::Deny { .. }),
                "{line} must be denied, got {outcome:?}"
            );
        }
    }

    /// The guard only answers what a read rule denies: ordinary read-only
    /// commands, and commands under the workspace, are untouched.
    #[test]
    fn a_read_only_bash_command_elsewhere_is_untouched() {
        let home = Path::new("/home/alice");
        let engine = Engine::compile(
            &PermissionsConfig::default(),
            Some(home),
            Path::new("/repo"),
        )
        .expect("default config compiles");
        for line in ["cat README.md", "ls -la /repo", "grep foo src/main.rs"] {
            let call = bash_call(line, &[line], false);
            let outcome = engine.decide(
                &call,
                PermissionMode::Default,
                ApprovalPolicy::OnRequest,
                SandboxMode::WorkspaceWrite,
                &[],
            );
            assert!(
                matches!(outcome, Outcome::Allow { .. }),
                "{line} must stay allowed, got {outcome:?}"
            );
        }
    }

    /// An opaque line (substitution, `eval`) still cannot smuggle the path
    /// past the deny: the whole subject is scanned when segments cannot be
    /// trusted.
    #[test]
    fn an_opaque_line_naming_a_denied_path_is_denied() {
        let home = Path::new("/home/alice");
        let engine = Engine::compile(
            &PermissionsConfig::default(),
            Some(home),
            Path::new("/repo"),
        )
        .expect("default config compiles");
        let line = "eval \"cat ~/.ssh/id_ed25519\"";
        let call = bash_call(line, &[line], true);
        let outcome = engine.decide(
            &call,
            PermissionMode::Default,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            &[],
        );
        assert!(matches!(outcome, Outcome::Deny { .. }), "got {outcome:?}");
    }

    fn cloud_call(repo: &str) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "cloud_agent".into(),
            input: serde_json::json!({
                "remote": "https://github.com/acme/widgets.git",
                "ref": "main",
            }),
            risk: Risk::Exec,
            subject: repo.into(),
            segments: None,
        }
    }

    fn engine_with(allow: &[&str], ask: &[&str], deny: &[&str]) -> Engine {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cfg = PermissionsConfig {
            allow: strings(allow),
            ask: strings(ask),
            deny: strings(deny),
            ..PermissionsConfig::default()
        };
        Engine::compile(&cfg, Some(Path::new("/home/u")), Path::new("/repo"))
            .expect("rules compile")
    }

    fn decide_cloud(engine: &Engine, repo: &str, mode: PermissionMode) -> Outcome {
        engine.decide(
            &cloud_call(repo),
            mode,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            &[],
        )
    }

    fn is_cloud_ask(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Ask(Why::RuleAsk { rule }) if rule.starts_with("CloudAgent("))
    }

    #[test]
    fn cloud_agent_asks_in_auto_and_bypass() {
        let engine = engine_with(&[], &[], &[]);
        for mode in [
            PermissionMode::Default,
            PermissionMode::Auto,
            PermissionMode::Bypass,
        ] {
            let outcome = decide_cloud(&engine, "acme/widgets", mode);
            assert!(is_cloud_ask(&outcome), "{mode:?} gave {outcome:?}");
        }
        // A session grant approved a command, not a repository.
        let grants = [("cloud_agent".to_string(), "acme/widgets".to_string())];
        let granted = engine.decide(
            &cloud_call("acme/widgets"),
            PermissionMode::Auto,
            ApprovalPolicy::OnRequest,
            SandboxMode::WorkspaceWrite,
            &grants,
        );
        assert!(is_cloud_ask(&granted), "got {granted:?}");
        // Nobody can answer under `never`, as for every other ask.
        let never = engine.decide(
            &cloud_call("acme/widgets"),
            PermissionMode::Bypass,
            ApprovalPolicy::Never,
            SandboxMode::WorkspaceWrite,
            &[],
        );
        assert!(matches!(never, Outcome::Deny { .. }), "got {never:?}");
    }

    #[test]
    fn cloud_agent_is_denied_in_plan_mode() {
        let engine = engine_with(&["CloudAgent(acme/widgets)"], &[], &[]);
        let outcome = decide_cloud(&engine, "acme/widgets", PermissionMode::Plan);
        assert!(matches!(outcome, Outcome::Deny { .. }), "got {outcome:?}");
        // The subject must be a clean `owner/repo` in every mode.
        for bad in ["acme", "acme/widgets extra", "acme/../x", ""] {
            let outcome = decide_cloud(&engine, bad, PermissionMode::Bypass);
            assert!(
                matches!(outcome, Outcome::Deny { .. }),
                "{bad:?} gave {outcome:?}"
            );
        }
    }

    #[test]
    fn cloud_agent_user_allow_rule_matches_one_repo() {
        let engine = engine_with(&["CloudAgent(acme/widgets)"], &[], &[]);
        for mode in [
            PermissionMode::Default,
            PermissionMode::Auto,
            PermissionMode::Bypass,
        ] {
            assert_eq!(
                decide_cloud(&engine, "acme/widgets", mode),
                Outcome::Allow {
                    by: DecidedBy::Rule
                },
                "{mode:?}"
            );
            let other = decide_cloud(&engine, "acme/other", mode);
            assert!(is_cloud_ask(&other), "{mode:?} gave {other:?}");
        }
        // GitHub names ignore case; the allow follows.
        assert!(matches!(
            decide_cloud(&engine, "ACME/Widgets", PermissionMode::Auto),
            Outcome::Allow { .. }
        ));
        // A bare `CloudAgent` allow would approve every repository: ignored.
        let bare = engine_with(&["CloudAgent"], &[], &[]);
        let outcome = decide_cloud(&bare, "acme/widgets", PermissionMode::Auto);
        assert!(is_cloud_ask(&outcome), "got {outcome:?}");
        // A deny rule still beats the allow, whatever the mode or case.
        let denied = engine_with(
            &["CloudAgent(acme/widgets)"],
            &[],
            &["CloudAgent(Acme/Widgets)"],
        );
        let outcome = decide_cloud(&denied, "acme/widgets", PermissionMode::Bypass);
        assert!(matches!(outcome, Outcome::Deny { .. }), "got {outcome:?}");
    }

    /// A repository must not pre-approve sending its own code away: the real
    /// loader reverts the project's `allow` (A122), so the engine built from
    /// the effective config still asks.
    #[test]
    fn cloud_agent_project_allow_is_reverted() {
        let home = tempfile::tempdir().expect("tempdir");
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join(".git")).expect("mkdir .git");
        std::fs::create_dir_all(root.path().join(".cox")).expect("mkdir .cox");
        std::fs::write(
            root.path().join(".cox/config.toml"),
            "[permissions]\nallow = [\"CloudAgent(acme/widgets)\"]\n",
        )
        .expect("write project config");
        let loaded = cox_config::load::load_in(
            &home.path().join("config.toml"),
            root.path(),
            &serde_json::json!({}),
            |_| None,
        )
        .expect("config loads");
        assert!(
            loaded
                .violations
                .iter()
                .any(|v| v.key == "permissions.allow"),
            "the project allow must be reported: {:?}",
            loaded.violations
        );
        let engine = Engine::compile(&loaded.config.permissions, Some(home.path()), root.path())
            .expect("effective rules compile");
        for mode in [PermissionMode::Auto, PermissionMode::Bypass] {
            let outcome = decide_cloud(&engine, "acme/widgets", mode);
            assert!(is_cloud_ask(&outcome), "{mode:?} gave {outcome:?}");
        }
    }

    #[test]
    fn cloud_agent_approval_text_names_repo_ref_and_off_machine() {
        let text = cloud_agent_approval_text(&cloud_call("acme/widgets"));
        assert!(text.contains("acme/widgets"), "{text}");
        assert!(
            text.contains("https://github.com/acme/widgets.git"),
            "{text}"
        );
        assert!(text.contains("Starting ref: main"), "{text}");
        assert!(text.contains("off this machine"), "{text}");
        // The remote and ref are repository-controlled: no escape sequences.
        let mut hostile = cloud_call("acme/widgets");
        hostile.input = serde_json::json!({
            "remote": "https://github.com/a/b\u{1b}[2J",
            "ref": "main\u{202E}",
        });
        let text = cloud_agent_approval_text(&hostile);
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{202E}'),
            "{text:?}"
        );
        let bare = ToolCall {
            input: serde_json::Value::Null,
            ..cloud_call("acme/widgets")
        };
        assert!(cloud_agent_approval_text(&bare).contains("Starting ref: unknown"));
    }

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
