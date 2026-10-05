// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox acp` (T11.1): Agent Client Protocol server on stdio for Zed,
//! JetBrains and neovim. Opens each session through `cox_session::open`
//! (T37.2), so an ACP session gets the same tools, MCP servers, skills,
//! hooks and plugins as the TUI; only the file/shell tools are
//! client-backed when the client offers `fs`/`terminal`.

use std::path::Path;
use std::sync::Arc;

use cox_protocol::types::Level;

use crate::cli::Cli;
use crate::config_load;

/// A [`cox_acp::SessionFactory`] over this machine's config and store.
struct AcpFactory {
    cli: Cli,
    answer: Option<String>,
}

#[async_trait::async_trait]
impl cox_acp::SessionFactory for AcpFactory {
    async fn create(&self, req: cox_acp::FactoryRequest) -> anyhow::Result<cox_core::Session> {
        let mut config = config_load::load(&req.cwd, &self.cli)?.config;
        // The client's extra directories join the default root rather than
        // replace it (§1.6: the git root of cwd, else cwd).
        if !req.roots.is_empty() && config.core.workspace_roots.is_empty() {
            config.core.workspace_roots =
                vec![config_load::find_git_root(&req.cwd).unwrap_or_else(|| req.cwd.clone())];
        }
        config
            .core
            .workspace_roots
            .extend(req.roots.iter().cloned());
        let opened = cox_session::open(cox_session::SessionSpec {
            config,
            cwd: req.cwd,
            home: self.cli.home.clone().unwrap_or_else(config_load::cox_home),
            worktree: false,
            answer: self.answer.clone(),
            questions: false,
            resume: None,
            // No terminal to print a login URL on: a 401 is a notice.
            mcp_login: None,
            plugin_ui: None,
            client: Some(cox_session::ClientTools {
                link: req.link,
                fs: req.client_fs,
                terminal: req.client_terminal,
            }),
            surface: "acp".into(),
            tools: Vec::new(),
        })
        .await?;
        // stdout carries the protocol: warnings reach the client as notices,
        // queued in the session's event channel ahead of any prompt.
        for warning in opened.warnings {
            opened
                .session
                .notice(Level::Warn, warning.to_string())
                .await?;
        }
        Ok(opened.session)
    }
}

/// Serves ACP on stdio until the client goes away.
pub fn run(cli: &Cli, _cwd: &Path) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(cox_acp::serve_stdio(Arc::new(AcpFactory {
        cli: cli.clone(),
        answer: None,
    })))?;
    Ok(())
}
