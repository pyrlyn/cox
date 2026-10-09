// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Permission rule grammar (Claude Code's, verbatim — D4): one rule string
//! becomes a tool matcher plus a subject matcher. Separate from the engine
//! so the grammar is table-testable without a decision order around it.

use std::path::Path;

use globset::{Glob, GlobMatcher};

/// The subject half of a rule (`Tool(subject)`).
#[derive(Debug, Clone)]
pub enum Subject {
    /// `Tool` — any subject.
    Any,
    /// `Tool(exact text)`.
    Exact(String),
    /// `Tool(prefix:*)` — matches `prefix` alone or `prefix` followed by whitespace.
    Prefix(String),
    /// `WebFetch(domain:example.com)` — the URL's host or a subdomain of it.
    Domain(String),
    /// A path glob for file tools; relative globs also match under `cwd`.
    Path(Vec<GlobMatcher>),
}

/// One compiled rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The rule as written, for `Why::RuleAsk` and deny reasons.
    pub raw: String,
    /// The canonical cox tool name, or `mcp__server__*`-style prefix.
    pub tool: String,
    /// What the subject must look like.
    pub subject: Subject,
}

/// The canonical name of the off-machine cloud agent subject (T56.4,
/// `CloudAgent(<owner>/<repo>)`). `Engine` gives it its own decision path
/// because it sends the repository's code to a third party.
pub const CLOUD_AGENT: &str = "cloud_agent";

/// Whether `s` is a GitHub `owner/repo` pair: the only shape a cloud agent
/// subject may take. Strict on purpose, so a subject cannot carry
/// whitespace, extra path segments or control characters into a rule match
/// or an approval line.
pub fn is_github_repo(s: &str) -> bool {
    let Some((owner, repo)) = s.split_once('/') else {
        return false;
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = (1..=100).contains(&repo.len())
        && !matches!(repo, "." | "..")
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    owner_ok && repo_ok
}

/// Claude Code's tool names mapped onto cox's (plan.md §1.8); everything
/// else is lower-cased as-is. `project` (T59.5) is a shell command run by
/// `bash`'s executor, so it shares `bash`'s rules and session grants: a deny
/// rule `Bash(rm:*)` must not be bypassable by the same command arriving
/// under another tool name.
pub fn canonical_tool(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "project" => "bash".into(),
        "webfetch" => "web_fetch".into(),
        "cloudagent" => CLOUD_AGENT.into(),
        "websearch" => "web_search".into(),
        "multiedit" | "notebookedit" => "edit".into(),
        _ => lower,
    }
}

fn is_path_tool(tool: &str) -> bool {
    matches!(
        tool,
        "read" | "edit" | "write" | "grep" | "glob" | "outline" | "apply_patch"
    )
}

impl Rule {
    /// Parses one rule. `home` expands a leading `~/`; `cwd` anchors
    /// relative path globs.
    pub fn parse(raw: &str, home: Option<&Path>, cwd: &Path) -> Result<Rule, String> {
        let raw = raw.trim();
        let (tool, inner) = match raw.split_once('(') {
            None => (raw, None),
            Some((tool, rest)) => (
                tool,
                Some(rest.strip_suffix(')').ok_or("missing closing ')'")?),
            ),
        };
        if tool.trim().is_empty() {
            return Err("empty tool name".into());
        }
        let tool = canonical_tool(tool);
        let subject = match inner.map(str::trim) {
            None | Some("") => Subject::Any,
            // A prefix or domain says nothing about one repository, so a rule
            // that could not be exact is refused rather than half-honoured.
            Some(s) if tool == CLOUD_AGENT => {
                if !is_github_repo(s) {
                    return Err("CloudAgent takes a GitHub <owner>/<repo>".into());
                }
                Subject::Exact(s.into())
            }
            Some(s) => {
                if let Some(prefix) = s.strip_suffix(":*") {
                    Subject::Prefix(prefix.trim_end().into())
                } else if let Some(domain) = s.strip_prefix("domain:") {
                    Subject::Domain(domain.trim().to_ascii_lowercase())
                } else if is_path_tool(&tool) {
                    Subject::Path(path_globs(s, home, cwd)?)
                } else {
                    Subject::Exact(s.into())
                }
            }
        };
        Ok(Rule {
            raw: raw.into(),
            tool,
            subject,
        })
    }

    /// Whether the rule names one exact subject. A cloud agent allow only
    /// counts when it does: a bare `CloudAgent` would approve every repository.
    pub fn is_exact(&self) -> bool {
        matches!(self.subject, Subject::Exact(_))
    }

    /// Whether this rule is a `read` path rule. The cross-tool guard in
    /// `Engine::decide` matches these against the paths a read-only bash
    /// command names, so `Read(~/.ssh/**)` also stops `cat ~/.ssh/id_rsa`
    /// (T62.3).
    pub fn read_path_rule(&self) -> bool {
        self.tool == "read" && matches!(self.subject, Subject::Path(_))
    }

    /// Whether the rule's path globs cover `path` (already `~`-expanded,
    /// absolute). Only path rules can answer.
    pub fn covers_file(&self, path: &str) -> bool {
        match &self.subject {
            Subject::Path(globs) => globs.iter().any(|g| g.is_match(path)),
            _ => false,
        }
    }

    /// Whether this rule covers `(tool, subject)`.
    pub fn matches(&self, tool: &str, subject: &str) -> bool {
        if !tool_matches(&self.tool, tool) {
            return false;
        }
        match &self.subject {
            Subject::Any => true,
            // GitHub names are case-insensitive, so a deny for `Acme/Widgets`
            // must also stop `acme/widgets`.
            Subject::Exact(s) if self.tool == CLOUD_AGENT => s.eq_ignore_ascii_case(subject),
            Subject::Exact(s) => s == subject,
            Subject::Prefix(p) => p.is_empty() || word_prefix(p, subject),
            Subject::Domain(d) => {
                host(subject).is_some_and(|h| h == *d || h.ends_with(&format!(".{d}")))
            }
            Subject::Path(globs) => globs.iter().any(|g| g.is_match(subject)),
        }
    }

    /// Whether this rule covers the whole command line on its own: only an
    /// `Any` or `Exact` rule may, since a prefix says nothing about what is
    /// chained after it (T36.1).
    pub fn matches_line(&self, tool: &str, line: &str) -> bool {
        matches!(self.subject, Subject::Any | Subject::Exact(_)) && self.matches(tool, line)
    }
}

/// `subject` is `prefix` alone or `prefix` followed by whitespace, so
/// `npm run test` covers `npm run test -- --watch` but not `npm run tests`.
pub(crate) fn word_prefix(prefix: &str, subject: &str) -> bool {
    subject
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// `mcp__server__*` matches by prefix; everything else by canonical name.
pub(crate) fn tool_matches(rule_tool: &str, call_tool: &str) -> bool {
    let call = canonical_tool(call_tool);
    match rule_tool.strip_suffix('*') {
        Some(prefix) => call.starts_with(prefix),
        None => rule_tool == call,
    }
}

fn path_globs(pattern: &str, home: Option<&Path>, cwd: &Path) -> Result<Vec<GlobMatcher>, String> {
    let mut patterns = Vec::new();
    match (pattern.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => patterns.push(home.join(rest).to_string_lossy().into_owned()),
        (Some(_), None) => patterns.push(pattern.to_owned()),
        (None, _) if pattern.starts_with('/') => patterns.push(pattern.to_owned()),
        (None, _) => {
            patterns.push(pattern.to_owned());
            patterns.push(cwd.join(pattern).to_string_lossy().into_owned());
        }
    }
    patterns
        .iter()
        .map(|p| {
            Glob::new(p)
                .map(|g| g.compile_matcher())
                .map_err(|e| e.to_string())
        })
        .collect()
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn rule(raw: &str) -> Rule {
        Rule::parse(raw, Some(Path::new("/home/u")), Path::new("/repo")).expect("parses")
    }

    #[test]
    fn permission_rule_grammar_matches_claude_code_forms() {
        assert!(rule("Bash").matches("bash", "anything"));
        assert!(rule("Bash(npm run test:*)").matches("bash", "npm run test -- --watch"));
        assert!(rule("Bash(npm run test:*)").matches("bash", "npm run test"));
        assert!(!rule("Bash(npm run test:*)").matches("bash", "npm run tests"));
        assert!(rule("Bash(rm -rf /*)").matches("bash", "rm -rf /*"));
        assert!(!rule("Bash(rm -rf /*)").matches("bash", "rm -rf /tmp"));
        assert!(rule("Read(~/.ssh/**)").matches("read", "/home/u/.ssh/id_rsa"));
        assert!(rule("Edit(src/**)").matches("edit", "/repo/src/a.rs"));
        assert!(rule("Edit(src/**)").matches("edit", "src/a.rs"));
        assert!(!rule("Edit(src/**)").matches("write", "src/a.rs"));
        assert!(
            rule("WebFetch(domain:example.com)").matches("web_fetch", "https://api.example.com/x")
        );
        assert!(
            !rule("WebFetch(domain:example.com)")
                .matches("web_fetch", "https://example.com.evil/x")
        );
        assert!(rule("mcp__gh__*").matches("mcp__gh__issues", ""));
        assert!(!rule("mcp__gh__*").matches("mcp__slack__post", ""));
        assert!(rule("mcp__gh__issues").matches("mcp__gh__issues", ""));
        assert!(rule("read").matches("Read", "/x"));
        assert!(Rule::parse("Bash(", None, &PathBuf::from("/")).is_err());
        assert!(Rule::parse("", None, &PathBuf::from("/")).is_err());
    }

    #[test]
    fn cloud_agent_rules_take_a_bare_tool_or_one_repository() {
        assert!(rule("CloudAgent").matches("cloud_agent", "acme/widgets"));
        assert!(rule("CloudAgent(Acme/Widgets)").matches("cloud_agent", "acme/widgets"));
        assert!(!rule("CloudAgent(acme/widgets)").matches("cloud_agent", "acme/other"));
        assert!(rule("CloudAgent(acme/widgets)").is_exact());
        assert!(!rule("CloudAgent").is_exact());
        for bad in [
            "CloudAgent(acme)",
            "CloudAgent(acme/widgets/x)",
            "CloudAgent(acme/..)",
            "CloudAgent(acme/wid gets)",
            "CloudAgent(acme/:*)",
            "CloudAgent(domain:github.com)",
        ] {
            assert!(
                Rule::parse(bad, None, Path::new("/")).is_err(),
                "{bad} must not parse"
            );
        }
    }
}
