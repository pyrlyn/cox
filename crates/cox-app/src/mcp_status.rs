// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP server status and log on the Settings screen (T37.45.4, DT§5.7
//! "status per server", A120): each server's badge — connected, needs
//! login, failed, disabled, or unknown before any session tried it — from
//! the config, the token store and what the last session opened in the
//! project made of its servers, and the lines that say why, sanitized and
//! capped here so Swift only draws them. Separate from `mcp_login.rs`,
//! which owns the token store and the login flow this only reads.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::mcp_login::McpLogin;

/// A log keeps at most this many lines, the latest.
pub const LOG_LINES: usize = 200;
/// A log line is cut to this many columns.
pub const LOG_COLUMNS: usize = 400;

/// What a session made of its MCP servers: the names it had in effect and
/// why each one it could not start was skipped. `App` keeps the latest per
/// project; the next session opened there replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpRun {
    tried: BTreeSet<String>,
    skipped: BTreeMap<String, String>,
}

impl McpRun {
    /// `servers`: the names in effect when the session opened; `notices`:
    /// its MCP warnings, of which only `cox_mcp`'s skip notices count.
    pub fn new(servers: impl IntoIterator<Item = String>, notices: &[String]) -> Self {
        let skipped = notices
            .iter()
            .filter_map(|n| cox_mcp::client::skipped_server(n))
            .map(|(name, reason)| (name.to_string(), reason.to_string()))
            .collect();
        Self {
            tried: servers.into_iter().collect(),
            skipped,
        }
    }
}

/// The badge a server row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpStatus {
    /// The last session here started it.
    Connected,
    /// Its token expired, or it failed with no token held.
    NeedsLogin,
    /// It would not start, or its token store cannot be read.
    Failed,
    /// MCP is off in the config.
    Disabled,
    /// No session here has tried it yet.
    Unknown,
}

/// The badge of server `name`. Fails open: anything cox cannot tell is
/// `Unknown`, never an error.
pub fn status_of(enabled: bool, name: &str, login: &McpLogin, run: Option<&McpRun>) -> McpStatus {
    if !enabled {
        return McpStatus::Disabled;
    }
    match login {
        McpLogin::Unreadable { .. } => return McpStatus::Failed,
        McpLogin::Expired => return McpStatus::NeedsLogin,
        _ => {}
    }
    let Some(run) = run.filter(|r| r.tried.contains(name)) else {
        return McpStatus::Unknown;
    };
    match (run.skipped.contains_key(name), login) {
        (false, _) => McpStatus::Connected,
        // An HTTP server cox holds no token for: logging in is the first fix.
        (true, McpLogin::LoggedOut) => McpStatus::NeedsLogin,
        (true, _) => McpStatus::Failed,
    }
}

/// What cox knows went wrong with server `name`, oldest first: the token
/// store's error, then why the last session skipped it. A server's own
/// output is untrusted, so every line is sanitized, secrets are scrubbed,
/// and the log is capped at [`LOG_LINES`] lines of [`LOG_COLUMNS`] columns.
pub fn log_of(name: &str, login: &McpLogin, run: Option<&McpRun>) -> Vec<String> {
    let mut text = String::new();
    if let McpLogin::Unreadable { error } = login {
        text.push_str(&format!("token store: {error}\n"));
    }
    if let Some(reason) = run.and_then(|r| r.skipped.get(name)) {
        text.push_str(&format!("skipped: {reason}\n"));
    }
    let clean = cox_sanitize::sanitize(&cox_sanitize::redact::scrub(&text));
    let lines: Vec<String> = clean
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| cox_sanitize::truncate(l, LOG_COLUMNS, "…"))
        .collect();
    let skip = lines.len().saturating_sub(LOG_LINES);
    lines.into_iter().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(skipped: &[(&str, &str)]) -> McpRun {
        let notices: Vec<String> = skipped
            .iter()
            .map(|(n, r)| cox_mcp::client::skipped(n, r))
            .chain(["mcp: .mcp.json skipped: bad json".to_string()])
            .collect();
        McpRun::new(["docs", "local", "gone"].map(String::from), &notices)
    }

    #[test]
    fn client_states_map_to_badges() {
        let failed = run(&[("local", "spawn nope: not found"), ("docs", "401")]);
        let clean = run(&[]);
        let logged_in = McpLogin::LoggedIn { expires: None };
        let cases = [
            (
                false,
                "local",
                McpLogin::Stdio,
                Some(&clean),
                McpStatus::Disabled,
            ),
            (true, "local", McpLogin::Stdio, None, McpStatus::Unknown),
            (
                true,
                "new",
                McpLogin::Stdio,
                Some(&clean),
                McpStatus::Unknown,
            ),
            (
                true,
                "local",
                McpLogin::Stdio,
                Some(&clean),
                McpStatus::Connected,
            ),
            (
                true,
                "local",
                McpLogin::Stdio,
                Some(&failed),
                McpStatus::Failed,
            ),
            (
                true,
                "docs",
                McpLogin::LoggedOut,
                Some(&failed),
                McpStatus::NeedsLogin,
            ),
            (
                true,
                "docs",
                logged_in.clone(),
                Some(&failed),
                McpStatus::Failed,
            ),
            (true, "docs", logged_in, Some(&clean), McpStatus::Connected),
            (
                true,
                "docs",
                McpLogin::Expired,
                Some(&clean),
                McpStatus::NeedsLogin,
            ),
            (
                true,
                "docs",
                McpLogin::Unreadable {
                    error: "locked".into(),
                },
                None,
                McpStatus::Failed,
            ),
        ];
        for (enabled, name, login, run, want) in cases {
            assert_eq!(
                status_of(enabled, name, &login, run),
                want,
                "{name} {login:?}"
            );
        }
    }

    #[test]
    fn log_text_is_sanitized_scrubbed_and_capped() {
        let reason =
            "spawn: \u{1b}]0;pwned\u{7}exit 127\n\u{202e}evil\nkey sk-abcdefghijklmnopqrstuvwxyz";
        let lines = log_of("local", &McpLogin::Stdio, Some(&run(&[("local", reason)])));
        assert_eq!(lines[0], "skipped: spawn: exit 127");
        assert_eq!(lines[1], "evil");
        assert!(!lines[2].contains("abcdefghijklmnop"), "{}", lines[2]);
        assert!(lines.iter().all(|l| !l.contains('\u{1b}')));

        let long = format!("{}\n", "x".repeat(LOG_COLUMNS * 2)).repeat(LOG_LINES + 5);
        let capped = log_of("local", &McpLogin::Stdio, Some(&run(&[("local", &long)])));
        assert_eq!(capped.len(), LOG_LINES);
        assert!(capped.iter().all(|l| l.chars().count() <= LOG_COLUMNS));
    }

    #[test]
    fn a_server_with_nothing_wrong_has_an_empty_log() {
        assert!(log_of("docs", &McpLogin::LoggedOut, Some(&run(&[]))).is_empty());
        let unreadable = McpLogin::Unreadable {
            error: "keychain locked".into(),
        };
        assert_eq!(
            log_of("docs", &unreadable, None),
            ["token store: keychain locked"]
        );
    }
}
