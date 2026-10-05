// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP logins on the Settings screen (DT§5.7, T37.30.3): every MCP server in
//! effect for a directory with whether cox holds a token for it, and Log in /
//! Log out through `cox-mcp`'s OAuth with the login page handed to the host.
//! Here rather than in Swift so the status and the flow's wiring are tested
//! once, over `cox_mcp::auth`'s memory store; the OAuth flow itself, the
//! token store and discovery stay `cox-mcp`'s and `cox-session`'s.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use cox_mcp::auth::{self, Secrets, Status};
use cox_protocol::Config;
use rmcp::transport::auth::{AuthError, CredentialStore};
use serde::Serialize;

use crate::mcp_status::{self, McpRun, McpStatus};

/// Whether cox can reach a server without asking the person to log in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum McpLogin {
    /// A stdio server: it runs locally and has no login.
    Stdio,
    /// No token: the server may not ask for one, or the person has not
    /// logged in yet.
    LoggedOut,
    /// A token the server still accepts; `expires` is how long it has left
    /// (`3h`), `None` when the server gave no expiry.
    LoggedIn { expires: Option<String> },
    /// Expired with no refresh token: log in again.
    Expired,
    /// The token store could not be read (a locked keychain).
    Unreadable { error: String },
}

/// The button a login row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoginAction {
    LogIn,
    LogOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServer {
    pub name: String,
    /// Where it is configured: `config`, `.mcp.json`, `~/.claude.json`.
    pub source: String,
    pub login: McpLogin,
    /// The badge (T37.45.4).
    pub status: McpStatus,
    /// Why it failed, sanitized and capped; empty when nothing went wrong.
    pub log: Vec<String>,
    /// The login's line (`Logged in, expires in 3h`), worded here so no
    /// client words it (T58.4.1).
    pub detail: String,
    /// The button the row offers; `None` for a server with no login.
    pub action: Option<LoginAction>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("no MCP server named `{0}`")]
    Unknown(String),
    #[error("`{0}` is a stdio server; only HTTP servers log in")]
    Stdio(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
}

type Pending<'a> = Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>>;

/// How a login runs: [`Browser`] is `cox-mcp`'s authorization-code flow; a
/// test scripts the browser's callback instead.
pub trait Flow: Send + Sync {
    /// Logs in to the server at `url`, handing its login page to `open`,
    /// and leaves the token in `store`.
    fn login<'a>(
        &'a self,
        url: &'a str,
        store: Arc<dyn CredentialStore>,
        open: &'a (dyn Fn(&str) + Sync),
    ) -> Pending<'a>;
}

/// `cox_mcp::auth::login`: discovery, registration, PKCE and the loopback
/// callback, as `cox mcp login` runs them.
pub struct Browser;

impl Flow for Browser {
    fn login<'a>(
        &'a self,
        url: &'a str,
        store: Arc<dyn CredentialStore>,
        open: &'a (dyn Fn(&str) + Sync),
    ) -> Pending<'a> {
        Box::pin(auth::login(url, store, None, open))
    }
}

/// Where tokens live and how a login runs. The app's is the keyring the CLI
/// and every session use (`cox/mcp/<server>`) and the browser flow.
#[derive(Clone)]
pub struct McpAuth {
    pub secrets: Arc<dyn Secrets>,
    pub flow: Arc<dyn Flow>,
}

impl Default for McpAuth {
    fn default() -> Self {
        Self {
            secrets: Arc::new(auth::Keyring),
            flow: Arc::new(Browser),
        }
    }
}

/// The servers a session in `cwd` would connect, sorted by name, each with
/// its login and, from `run` (the last session's), its status and log.
pub async fn servers(
    config: &Config,
    cwd: &Path,
    secrets: &dyn Secrets,
    run: Option<&McpRun>,
) -> Vec<McpServer> {
    let found = cox_session::mcp_servers(config, cwd);
    let mut names: Vec<&String> = found.servers.keys().collect();
    names.sort();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let login = match found.servers.get(name).and_then(|s| s.url.as_ref()) {
            None => McpLogin::Stdio,
            Some(_) => login_of(secrets.store(name).load().await),
        };
        let source = found.sources.get(name).cloned().unwrap_or_default();
        let (detail, action) = words(&login, &source);
        out.push(McpServer {
            name: name.clone(),
            status: mcp_status::status_of(config.mcp.enabled, name, &login, run),
            log: mcp_status::log_of(name, &login, run),
            source,
            login,
            detail,
            action,
        });
    }
    out
}

/// A login's line and its button; `source` is where a stdio server is
/// configured.
fn words(login: &McpLogin, source: &str) -> (String, Option<LoginAction>) {
    match login {
        McpLogin::Stdio => (format!("Runs locally from {source}; no login"), None),
        McpLogin::LoggedOut => ("Not logged in".into(), Some(LoginAction::LogIn)),
        McpLogin::LoggedIn {
            expires: Some(expires),
        } => (
            format!("Logged in, expires in {expires}"),
            Some(LoginAction::LogOut),
        ),
        McpLogin::LoggedIn { expires: None } => ("Logged in".into(), Some(LoginAction::LogOut)),
        McpLogin::Expired => ("Login expired".into(), Some(LoginAction::LogIn)),
        McpLogin::Unreadable { error } => (
            format!("Token store unreadable: {error}"),
            Some(LoginAction::LogIn),
        ),
    }
}

fn login_of(
    stored: Result<Option<rmcp::transport::auth::StoredCredentials>, AuthError>,
) -> McpLogin {
    match stored {
        Err(e) => McpLogin::Unreadable {
            error: e.to_string(),
        },
        Ok(creds) => match auth::status(creds.as_ref(), auth::now()) {
            Status::None => McpLogin::LoggedOut,
            Status::Ok { expires_in } => McpLogin::LoggedIn {
                expires: expires_in.map(auth::human),
            },
            Status::Expired => McpLogin::Expired,
        },
    }
}

/// Logs in to (`log_in`) or out of the HTTP server `name` in effect for
/// `cwd`; a login's page goes to `open`.
pub async fn set_login(
    config: &Config,
    cwd: &Path,
    name: &str,
    log_in: bool,
    mcp: &McpAuth,
    open: &(dyn Fn(&str) + Sync),
) -> Result<(), LoginError> {
    let found = cox_session::mcp_servers(config, cwd);
    let server = found
        .servers
        .get(name)
        .ok_or_else(|| LoginError::Unknown(name.to_string()))?;
    let url = server
        .url
        .as_deref()
        .ok_or_else(|| LoginError::Stdio(name.to_string()))?;
    let store = mcp.secrets.store(name);
    if log_in {
        mcp.flow.login(url, store, open).await?;
    } else {
        auth::logout(store).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logged_out_server_offers_log_in() {
        let words = words(&McpLogin::LoggedOut, "config");
        assert_eq!(words, ("Not logged in".into(), Some(LoginAction::LogIn)));
    }

    #[test]
    fn each_login_has_its_line_and_button() {
        let cases = [
            (
                McpLogin::Stdio,
                "Runs locally from .mcp.json; no login",
                None,
            ),
            (
                McpLogin::LoggedIn {
                    expires: Some("3h".into()),
                },
                "Logged in, expires in 3h",
                Some(LoginAction::LogOut),
            ),
            (
                McpLogin::LoggedIn { expires: None },
                "Logged in",
                Some(LoginAction::LogOut),
            ),
            (McpLogin::Expired, "Login expired", Some(LoginAction::LogIn)),
            (
                McpLogin::Unreadable {
                    error: "locked".into(),
                },
                "Token store unreadable: locked",
                Some(LoginAction::LogIn),
            ),
        ];
        for (login, detail, action) in cases {
            assert_eq!(words(&login, ".mcp.json"), (detail.to_string(), action));
        }
    }
}
