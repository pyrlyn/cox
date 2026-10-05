// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox mcp`: picks which built-in tools to serve and plugs the permission
//! engine in as the gate (T6.2). The wire side lives in `cox_mcp::server`;
//! this is the only place the flags, the engine and the store meet.
//! `cox mcp login|logout <server>` (T22.5) run the OAuth flow for an HTTP
//! server outside a session, so a headless start never has to.

use std::path::Path;
use std::sync::Arc;

use cox_core::permission::{Engine, Outcome, why_text};
use cox_mcp::server::{CxTemplate, Gate, ToolServer};
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use cox_protocol::types::{ApprovalPolicy, PermissionMode, SandboxMode, ToolCall};
use cox_store::Store;

use crate::cli::{Cli, McpAction, McpArgs};
use crate::{config_load, session};

// No `outline` here: an outline is `read` with `mode = "outline"`, not a tool.
const READ_ONLY: &[&str] = &["read", "grep", "glob"];
const WRITE: &[&str] = &["edit", "write", "apply_patch"];

/// `policy = never`: an ask becomes a deny, since no one is there to answer.
struct EngineGate {
    engine: Engine,
    mode: PermissionMode,
    sandbox: SandboxMode,
}

impl Gate for EngineGate {
    fn check(&self, call: &ToolCall) -> Result<(), String> {
        match self
            .engine
            .decide(call, self.mode, ApprovalPolicy::Never, self.sandbox, &[])
        {
            Outcome::Allow { .. } => Ok(()),
            Outcome::Deny { reason, .. } => Err(reason),
            Outcome::Ask(why) => Err(why_text(&why)),
        }
    }
}

/// The tool names the flags select, in the order the plan lists them.
fn selected(args: &McpArgs) -> Vec<String> {
    match &args.tools {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        None => READ_ONLY
            .iter()
            .chain(args.allow_write.then_some(WRITE).into_iter().flatten())
            .map(|s| (*s).to_string())
            .collect(),
    }
}

pub fn run(cli: &Cli, args: &McpArgs, cwd: &Path) -> anyhow::Result<()> {
    let config = config_load::load(cwd, cli)?.config;
    match &args.action {
        Some(McpAction::Login { server }) => return login(&config, cwd, server),
        Some(McpAction::Logout { server }) => return logout(server),
        None => {}
    }
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let store = Arc::new(Store::open(&home)?);
    let names = selected(args);
    let mdir = session::memory_dir_for(&config, &home, cwd);
    let tools = session::tools(None, &store, mdir)
        .into_iter()
        .filter(|t| names.contains(&t.spec().name))
        .collect();
    let gate = EngineGate {
        engine: Engine::compile(&config.permissions, Some(&home), cwd)?,
        mode: config.permissions.mode,
        sandbox: config.sandbox.mode,
    };
    let mut roots = config.core.workspace_roots.clone();
    if roots.is_empty() {
        roots.push(cwd.to_path_buf());
    }
    let cx = CxTemplate {
        writable_roots: roots.clone(),
        roots,
        cwd: cwd.to_path_buf(),
        sandbox: crate::session::sandbox_policy(&config),
        archive: store,
        session: SessionId::new(),
    };
    let server = ToolServer::new(tools, Arc::new(gate), cx);
    tokio::runtime::Runtime::new()?.block_on(server.serve_stdio())?;
    Ok(())
}

/// The server's URL, or why there is none to log in to.
fn http_url(
    config: &cox_protocol::config::Config,
    cwd: &Path,
    server: &str,
) -> anyhow::Result<String> {
    let found = session::mcp_servers(config, cwd);
    let cfg = found
        .servers
        .get(server)
        .ok_or_else(|| anyhow::anyhow!("no MCP server named `{server}`"))?;
    cfg.url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("`{server}` is a stdio server; only HTTP servers use OAuth"))
}

fn login(config: &cox_protocol::config::Config, cwd: &Path, server: &str) -> anyhow::Result<()> {
    let url = http_url(config, cwd, server)?;
    let auth = session::mcp_auth(true);
    let store = auth.secrets.store(server);
    let prompt = auth
        .prompt
        .ok_or_else(|| anyhow::anyhow!("login needs a terminal"))?;
    tokio::runtime::Runtime::new()?.block_on(cox_mcp::auth::login(&url, store, None, &*prompt))?;
    println!("logged in to `{server}`; the token is in the keyring (cox/mcp/{server})");
    Ok(())
}

fn logout(server: &str) -> anyhow::Result<()> {
    let store = session::mcp_auth(false).secrets.store(server);
    tokio::runtime::Runtime::new()?.block_on(cox_mcp::auth::logout(store))?;
    println!("logged out of `{server}`");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selection_is_read_only_and_write_is_opt_in() {
        assert_eq!(selected(&McpArgs::default()), ["read", "grep", "glob"]);
        let with_write = selected(&McpArgs {
            action: None,
            allow_write: true,
            tools: None,
        });
        assert_eq!(with_write.len(), 6);
        assert!(with_write.contains(&"apply_patch".to_string()));
        assert!(!with_write.contains(&"bash".to_string()));
        let explicit = selected(&McpArgs {
            action: None,
            allow_write: false,
            tools: Some("bash, read".into()),
        });
        assert_eq!(explicit, ["bash", "read"]);
    }
}
