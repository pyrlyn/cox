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

/// Claude Code's tool names mapped onto cox's (plan.md §1.8); everything
/// else is lower-cased as-is.
pub fn canonical_tool(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "webfetch" => "web_fetch".into(),
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

    /// Whether this rule covers `(tool, subject)`.
    pub fn matches(&self, tool: &str, subject: &str) -> bool {
        if !tool_matches(&self.tool, tool) {
            return false;
        }
        match &self.subject {
            Subject::Any => true,
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
}
