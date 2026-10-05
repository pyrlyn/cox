// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A top-level session driven by an external ACP agent (T52.4, DT§3.3.1):
//! which agent a name means, how it is started, and what each intent does
//! to it. The process, the ACP connection and the event fold are
//! `cox_session::acp_session`'s; `live.rs` runs the result like any other
//! session, so the timeline, inbox and controller do not know the
//! difference (DT-7). The agent's permission asks reach the inbox the
//! same way, and an `Approve` intent answers them (T52.5).
//!
//! The session is stored as a cox session is (T52.6): a `sessions` row that
//! names the agent and its ACP session, and a rollout of the mapped events,
//! written before the view sees each one. It reopens through `session/load`
//! when the agent has it, and read-only otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::SessionId;
use cox_protocol::plugin::AgentMode;
use cox_protocol::types::{Event, ItemKind};
use cox_protocol::{Config, SessionRow, Store as _};
use cox_session::acp_session::{self, AcpOpenError, AcpResume, AcpSession, OpenedAcp};
use cox_store::{SessionAgent, Store};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::app::{App, AppError};
use crate::intent::{AgentDispatch, Intent, agent_dispatch};

/// The core's own event bound (DT§4.5).
const EVENTS: usize = 256;

/// One row of the New-session sheet's agent picker and of Settings' Agents
/// list (T52.7, DT§3.3.1, mockup 27): cox itself first, then every entry
/// that resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentChoice {
    /// What `OpenRequest.agent` takes; `None` for cox itself.
    pub name: Option<String>,
    /// What the UI calls it.
    pub label: String,
    /// `built-in`, `user config` or `plugin <id>`.
    pub origin: String,
    /// The program, its args, key, network and writable directories, as
    /// the Agents list shows them; empty for cox.
    pub launch: String,
    /// Why it cannot start here (EA§7); `None` when it can.
    pub unavailable: Option<String>,
}

/// The display names DT§3.3.1's launch table gives the known adapters, by
/// program. Claude's is "Claude Agent", never "Claude Code" (the creator's
/// decision 5). Any other entry is shown by its own name.
const LABELS: &[(&str, &str)] = &[
    ("claude-agent-acp", "Claude Agent"),
    ("codex-acp", "Codex"),
    ("gemini", "Gemini CLI"),
];

/// Every agent a new session in `cwd` could be driven by, each with why it
/// cannot start when it cannot; the key is looked up as [`start`] does.
pub(crate) fn choices(app: &App, config: &Config, cwd: &Path) -> Vec<AgentChoice> {
    let roots = writable_roots(config, cwd);
    let (agents, _) = acp_session::agents(config, &app.home, cwd, &roots);
    let path = std::env::var_os("PATH");
    let cox = AgentChoice {
        name: None,
        label: String::from("cox"),
        origin: String::from("built-in"),
        launch: String::new(),
        unavailable: None,
    };
    let external = agents.iter().map(|agent| {
        let keyed = std::env::var(agent.key_env()).is_ok_and(|v| !v.is_empty())
            || app.host.secret(agent.name()).is_some();
        let unavailable = if agent.mode() != AgentMode::Acp {
            Some(String::from("it speaks stream-json, not ACP"))
        } else if let Some(cli) = agent.missing_cli(path.as_deref()) {
            Some(format!("`{}` is not on PATH", cli.display()))
        } else if !keyed {
            Some(format!("{} is not set", agent.key_env()))
        } else {
            None
        };
        AgentChoice {
            name: Some(agent.name().to_string()),
            label: label(&agent.launch_line(), agent.name()),
            origin: agent.origin(),
            launch: agent.launch_line(),
            unavailable,
        }
    });
    std::iter::once(cox).chain(external).collect()
}

/// The label for the agent `name` whose launch line is `launch`: the
/// launch table's, by the program the line starts with, else its name.
fn label(launch: &str, name: &str) -> String {
    let program = launch.split(' ').next().unwrap_or_default();
    let program = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    LABELS
        .iter()
        .find(|(known, _)| *known == program)
        .map_or_else(|| name.to_string(), |(_, label)| label.to_string())
}

/// An agent session as `live.rs` runs it.
pub(crate) struct Opened {
    pub id: SessionId,
    /// The running agent, or why a stored session could only be read.
    pub acp: Result<AcpSession, String>,
    pub events: mpsc::Receiver<Event>,
    pub roots: Vec<PathBuf>,
}

/// Starts the agent `name` in `cwd`, or reopens session `resume` (whose
/// rollout already holds `turns` main turns) with it. A new session that
/// cannot start is one warning, and nothing opens; a stored one that
/// cannot be reattached opens read-only, with the reason.
pub(crate) async fn open(
    app: &App,
    config: &Config,
    cwd: &Path,
    name: &str,
    resume: Option<(SessionId, u32)>,
) -> Result<Opened, AppError> {
    let roots = writable_roots(config, cwd);
    let store = Store::open(&app.home)?;
    let Some((id, turns)) = resume else {
        let id = SessionId::new();
        let opened = start(app, config, cwd, name, &roots, id, None).await?;
        store.session_create(&SessionRow {
            id,
            created_at: String::new(),
            cwd: cwd.to_path_buf(),
            project_slug: String::new(),
            title: None,
            parent_id: None,
            rollout_path: PathBuf::new(),
        })?;
        let agent = SessionAgent {
            agent: name.to_string(),
            agent_session: Some(opened.agent_session),
        };
        store.session_agent_set(&id, &agent)?;
        return Ok(Opened {
            id,
            acp: Ok(opened.session),
            events: record(store, id, opened.events),
            roots,
        });
    };
    let stored = store.session_agent(&id)?.and_then(|a| a.agent_session);
    let started = match stored {
        Some(session) => {
            let resume = AcpResume { session, turns };
            start(app, config, cwd, name, &roots, id, Some(resume))
                .await
                .map_err(|e| e.to_string())
        }
        None => Err(format!("{name} left no ACP session id to reopen")),
    };
    Ok(match started {
        Ok(opened) => Opened {
            id,
            acp: Ok(opened.session),
            events: record(store, id, opened.events),
            roots,
        },
        // Nothing will ever send: the view shows the rollout and ends.
        Err(why) => Opened {
            id,
            acp: Err(why),
            events: mpsc::channel(1).1,
            roots,
        },
    })
}

/// Resolves `name` to an agent and opens its ACP session. A name no entry
/// resolves to is one warning, with the reason its entry was refused when
/// there is one.
async fn start(
    app: &App,
    config: &Config,
    cwd: &Path,
    name: &str,
    roots: &[PathBuf],
    id: SessionId,
    resume: Option<AcpResume>,
) -> Result<OpenedAcp, AcpOpenError> {
    let (agents, warnings) = acp_session::agents(config, &app.home, cwd, roots);
    let Some(agent) = agents.into_iter().find(|a| a.name() == name) else {
        let why: Vec<String> = warnings.into_iter().filter(|w| w.contains(name)).collect();
        let why = match why.is_empty() {
            true => format!("no external agent is named {name}"),
            false => why.join("; "),
        };
        return Err(AcpOpenError::Unavailable(why));
    };
    let host = Arc::clone(&app.host);
    // The entry's own variable wins, as for a provider key; then the key
    // the app stored under the agent's name. Never cox's provider keys.
    let key = move |env: &str, section: &str| {
        std::env::var(env)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| host.secret(section))
            .ok_or(ProviderError::Auth)
    };
    let path = std::env::var_os("PATH");
    acp_session::open(agent, config, cwd, roots, path.as_deref(), key, id, resume).await
}

/// Appends each event to session `id`'s rollout, then passes it on: the
/// lossless rule's order, so a reopen shows what the view showed. The
/// store also counts the turn and keeps the title from it. A failed write
/// never stops the session; the prompt is indexed for search as the core's
/// are, best effort.
fn record(store: Store, id: SessionId, mut from: mpsc::Receiver<Event>) -> mpsc::Receiver<Event> {
    let (tx, rx) = mpsc::channel(EVENTS);
    tokio::spawn(async move {
        let mut turn = 0;
        while let Some(event) = from.recv().await {
            let _ = store.rollout_append(&id, &event);
            match &event {
                Event::TurnStarted { seq, .. } => turn = *seq,
                Event::ItemStarted {
                    kind: ItemKind::UserMessage { text, .. },
                    ..
                } => {
                    let _ = store.rollout_index(&id, turn, text);
                }
                _ => {}
            }
            // Err only once the view and the inbox closed.
            let _ = tx.send(event).await;
        }
    });
    rx
}

/// Runs `intent` against the agent's session; `acp` is the reason instead
/// when the session could only be reopened read-only, where a rename is
/// all that still applies.
pub(crate) fn send(
    app: &App,
    id: SessionId,
    agent: &str,
    acp: Result<&AcpSession, &str>,
    intent: Intent,
) -> Result<(), AppError> {
    let dispatch = agent_dispatch(intent)?;
    if let AgentDispatch::Rename(title) = dispatch {
        return app.rename(id, &title).map(|_| ());
    }
    let acp = acp.map_err(|why| AppError::ReadOnly {
        agent: agent.to_string(),
        why: why.to_string(),
    })?;
    let delivered = match dispatch {
        AgentDispatch::Prompt(text) => acp.prompt(text),
        AgentDispatch::Cancel => acp.cancel(),
        // A stale answer (the ask timed out, or was answered from another
        // window) finds nothing waiting; the inbox already shows it decided.
        AgentDispatch::Approve { call, decision } => {
            acp.approve(call, decision);
            true
        }
        AgentDispatch::Rename(_) => true,
        AgentDispatch::Refused(intent) => {
            return Err(AppError::Unsupported {
                agent: agent.to_string(),
                intent,
            });
        }
    };
    match delivered {
        true => Ok(()),
        false => Err(AppError::Core(CoreError::ExternalAgent {
            agent: agent.to_string(),
            message: String::from("the agent has exited"),
        })),
    }
}

/// §1.6, as `cox-session` resolves it for a cox session: the configured
/// workspace roots, else the git root of `cwd`, else `cwd`.
fn writable_roots(config: &Config, cwd: &Path) -> Vec<PathBuf> {
    match config.core.workspace_roots.is_empty() {
        true => vec![cox_config::load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())],
        false => config.core.workspace_roots.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_adapter_is_labelled_claude_agent_never_claude_code() {
        let launch = "/opt/bin/claude-agent-acp --hide-claude-auth · key ANTHROPIC_API_KEY";
        assert_eq!(label(launch, "claude"), "Claude Agent");
        assert_eq!(label("codex-acp · key CODEX_API_KEY", "codex"), "Codex");
        assert_eq!(label("my-agent --acp · key K", "mine"), "mine");
    }
}
