// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Session assembly (T37.1, DT§4.2): turns an effective `Config` into a
//! live [`Session`] — provider, built-in and MCP tools, skills, subagent
//! definitions, hooks, plugins, the checkpointer and worktrees. Separate
//! from `crates/cox` so every surface (TUI, `run -p`, ACP, the desktop app)
//! builds a session the same way without `clap`, `anyhow` or a terminal:
//! the caller loads config from its own flags, and what went wrong on the
//! way comes back as [`Warning`]s for it to show, never printed here.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_core::{History, Session};
use cox_protocol::Config;
use cox_protocol::errors::{CoreError, ProviderError};
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{Hook, Store as _, Tool};
use cox_protocol::types::Level;
use cox_store::Store;
use cox_tools::send_message::SendMessageTool;

#[cfg(feature = "plugins")]
pub mod acp_session;
pub mod doctor;
pub mod env;
#[cfg(feature = "plugins")]
pub mod external_agents;
pub mod lineage;
pub mod mcp;
#[cfg(feature = "plugins")]
pub mod plugin_ui;
pub mod plugins;
pub mod provider;
pub mod sandbox;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
pub mod tools;

pub use lineage::{Follow, fork, handoff, resume};
pub use mcp::{mcp_auth, mcp_servers};
#[cfg(feature = "plugins")]
pub use plugins::start_plugins;
pub use plugins::{Plugins, load_plugins, plugin_notices, write_grant};
pub use provider::{lmstudio_model, provider_for};
pub use sandbox::{agent_argv, agent_policy, sandbox_policy, sandboxed_argv};
pub use tools::{tools, with_client_tools};

/// Why a session could not be built. Anything an extension breaks is a
/// [`Warning`] instead (D14); these stop the session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Store(#[from] cox_protocol::StoreError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Price(#[from] cox_provider::usage::PriceError),
    /// T30.16: LM Studio did not answer for the session's model before any
    /// turn. `error` is not a `source`, so the message stays one line.
    #[error("LM Studio at {base_url}: {error}")]
    LmStudio {
        base_url: String,
        error: ProviderError,
    },
    #[error("unknown provider `{0}` in tiers.code")]
    UnknownProvider(String),
    #[error("unknown api `{api}` for provider `{owner}` (want \"chat\" or \"responses\")")]
    UnknownApi { api: String, owner: String },
    /// T37.34 (A67): another process drives session `id`; one writer per
    /// rollout. The caller may [`Follow`] it read-only or [`fork`] it.
    #[error(
        "session {id} is open in {holder}; one process drives a session. \
         Follow it read-only or fork it into a new session \
         (taking it over is not available yet)"
    )]
    SessionBusy {
        id: SessionId,
        holder: cox_store::lock::Holder,
    },
}

/// Something skipped while the session was built; the session runs
/// without it (D14). The caller decides how to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A `SKILL.md` that did not parse (T22.2).
    Skill(String),
    /// A subagent definition that did not parse (T34.1).
    Agent(String),
    /// An MCP server that did not start, or runs unsandboxed (T7.6, T33.42).
    Mcp(String),
    /// The login shell's environment could not be read; the process's own
    /// environment stands in (T37.11, DT§4.8).
    Env(String),
    /// An `AGENTS.md`/`CLAUDE.md` file or include dropped from `system[2]`
    /// (T50.1).
    Instruction(String),
}

impl std::fmt::Display for Warning {
    /// Secret-shaped runs (API keys, bearer tokens, PEM blocks) are masked
    /// with `cox_sanitize::redact::scrub`: a warning may quote an error or a
    /// config line, and every surface shows or logs this text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Skill(text)
        | Self::Agent(text)
        | Self::Mcp(text)
        | Self::Env(text)
        | Self::Instruction(text)) = self;
        f.write_str(&cox_sanitize::redact::scrub(text))
    }
}

/// A surface's plugin renderer (T33.23, T33.44): given the live plugins,
/// starts serving their render requests and returns the event tap's
/// redraw. Only the TUI has one.
#[cfg(feature = "plugins")]
pub type ServeUi = Box<dyn FnOnce(&cox_plugin::LivePlugins) -> cox_plugin::Redraw + Send>;

/// The slim build has no plugin to render, so there is never a renderer.
#[cfg(not(feature = "plugins"))]
pub type ServeUi = std::convert::Infallible;

/// Everything [`open`] needs. `config` is the effective config the surface
/// loaded from its own flags; [`open`] fills in what an empty config means.
pub struct SessionSpec {
    pub config: Config,
    /// The session's working directory (the worktree, for a worktree run).
    pub cwd: PathBuf,
    /// `COX_HOME`: the store, skills, plugins and checkpoints live here.
    pub home: PathBuf,
    /// `cwd` is a worktree cox made (T27.3): the main checkout joins as a
    /// read-only root and only the worktree is writable.
    pub worktree: bool,
    /// What `ask_user` returns when no one is there to ask.
    pub answer: Option<String>,
    /// T22.1, DT G4: the surface answers `Event::QuestionAsked` with
    /// `Submission::Answer`, so `ask_user` asks it instead of using `answer`.
    pub questions: bool,
    /// The session to reopen and its rebuilt history.
    pub resume: Option<(SessionId, History)>,
    /// T22.5: with a person present, how an MCP server's 401 hands them the
    /// login URL; `None` makes a 401 a notice.
    pub mcp_login: Option<cox_mcp::client::Prompt>,
    /// T33.23, T33.44: renders the live plugins; only the TUI has one.
    pub plugin_ui: Option<ServeUi>,
    /// T37.2: an ACP client that offers `fs`/`terminal` backs the file and
    /// shell tools; only `cox acp` has one.
    pub client: Option<ClientTools>,
    /// T37.34: who drives the session (`tui`, `plain`, `headless`, `acp`,
    /// `app`), named to a second process that tries to open it.
    pub surface: String,
    /// T51.7: tools only this surface can back (the app's `browser_*`),
    /// added before the `tool_search` index is built so the deferred ones
    /// are found. Fixed at open, so the tool set stays byte-stable.
    pub tools: Vec<Arc<dyn Tool>>,
}

/// The ACP client's side of a session (T11.1): the link its proxy tools
/// call back through and which of `fs`/`terminal` it offers.
pub struct ClientTools {
    pub link: cox_acp::ClientLink,
    pub fs: bool,
    pub terminal: bool,
}

/// A built session.
pub struct Opened {
    pub session: Session,
    /// `spec.config` with the defaults [`open`] filled in, before granted
    /// plugins' provider sections joined it.
    pub config: Config,
    /// In the order they happened; empty from [`open_reporting`].
    pub warnings: Vec<Warning>,
}

/// T37.14: where a surface keeps provider keys instead of the OS keyring
/// (the macOS app asks the Keychain through Swift): section → key. The
/// section's env var still wins, as with the keyring.
pub type Keys = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Opens the store under `spec.home`, picks the provider (`COX_PROVIDER`
/// test doubles first) and builds the session with every tool, hook and
/// plugin it gets.
pub async fn open(spec: SessionSpec) -> Result<Opened, SessionError> {
    open_with_keys(spec, None).await
}

/// [`open`], with the main provider's key looked up in `keys` rather than
/// the OS keyring when given.
pub async fn open_with_keys(spec: SessionSpec, keys: Option<Keys>) -> Result<Opened, SessionError> {
    let mut warnings = Vec::new();
    let mut opened = open_reporting(spec, keys, &mut warnings).await?;
    opened.warnings = warnings;
    Ok(opened)
}

/// [`open_with_keys`], with the warnings pushed into `warnings` as they
/// happen and [`Opened::warnings`] left empty. A caller that logs them
/// uses this, so what it prints never comes out of the value that holds
/// the session and its provider key.
pub async fn open_reporting(
    spec: SessionSpec,
    keys: Option<Keys>,
    warnings: &mut Vec<Warning>,
) -> Result<Opened, SessionError> {
    let SessionSpec {
        config: mut base,
        cwd,
        home,
        worktree,
        answer,
        questions,
        resume,
        mcp_login,
        plugin_ui,
        client,
        surface,
        tools: surface_tools,
    } = spec;
    let cwd = cwd.as_path();
    // §1.6: empty `workspace_roots` means the git root of cwd, else cwd.
    if base.core.workspace_roots.is_empty() {
        base.core.workspace_roots =
            vec![cox_config::load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())];
    }
    // T57.3: D7 applied before anything reads the policy, so the core,
    // external agents and `Opened::config` all see the effective one.
    let (approval, unsandboxed) = sandbox::effective_approval(
        cox_tools::sandbox::backend(base.sandbox.linux_backend),
        base.permissions.approval,
    );
    base.permissions.approval = approval;
    let worktree_main = if worktree {
        let main = project_root(cwd).await;
        add_read_root(&mut base, &main);
        Some(main)
    } else {
        None
    };
    // T9.1 step 4 (generalised): a non-first-party `tiers.code.provider`
    // maps every tier to the same server; the router then pins each tier to
    // that provider's section model, so a `--provider deepseek` flip works
    // without editing every tier model.
    if !["anthropic", "openai"].contains(&base.tiers.code.provider.as_str()) {
        for tier in [&mut base.tiers.cheap, &mut base.tiers.think] {
            tier.provider = base.tiers.code.provider.clone();
        }
    }
    let mut config = base.clone();
    let store = Arc::new(Store::open(&home)?);
    // T37.34: claimed before anything is built or written, so a busy
    // session costs the second opener nothing.
    let id = resume.as_ref().map_or_else(SessionId::new, |(id, _)| *id);
    if let Some(holder) = store.claim_session(&id, &surface)? {
        return Err(SessionError::SessionBusy { id, holder });
    }
    // T33.19: granted plugins load before the provider is built and before
    // MCP discovery: a granted plugin's declarative `[[provider]]` rows
    // (T33.17) must already be in `config.providers.custom` when
    // `provider_for`/`provider_for_served` below pick the tier's client,
    // and its `[[mcp]]` servers join MCP discovery further down. A
    // worktree session's plugin server may write only the worktree, like
    // its `bash` (`set_writable_roots` below).
    let writable = match worktree_main {
        Some(_) => vec![cwd.to_path_buf()],
        None => config.core.workspace_roots.clone(),
    };
    let mut plugins = load_plugins(&config, &home, cwd, store.clone(), Some(&writable));
    // T45.4: taken now, before `catalog_rows` borrows `plugins`; merged
    // into the discovered definitions below.
    let plugin_agent_defs = std::mem::take(&mut plugins.agent_defs);
    config.providers.custom.extend(plugins.providers.clone());
    // T33.44: granted plugins' `[[models]]` join the catalog the provider
    // reads its context window from (PL§7b).
    let plugin_models = plugins.catalog_rows();
    // T30.16: ask LM Studio what it runs before the provider is built, so
    // the loaded context becomes the session's window. The key resolved
    // here is reused for the chat client: one keyring read, not two.
    let served = provider::lmstudio_served(&config).await?;
    let provider = match &served {
        Some(s) => {
            let key = s.api_key.clone();
            provider::provider_for_served(
                &config,
                move |_, _| key.ok_or(ProviderError::Auth),
                Some(&s.model),
                &plugin_models,
            )?
        }
        None => match keys {
            Some(keys) => provider::provider_for_served(
                &config,
                |var, section| cox_provider::http::resolve_key_with(var, section, |s| keys(s)),
                None,
                &plugin_models,
            )?,
            None => provider::provider_for_served(
                &config,
                cox_provider::http::resolve_key,
                None,
                &plugin_models,
            )?,
        },
    };
    let mdir = memory_dir_for(&base, &home, cwd);
    // T27.3: a worktree session's project is still the main checkout, so
    // the sessions of one repository see each other whatever tree they edit.
    let project = project_root(cwd).await;
    // T22.2: `SKILL.md` files are discovered once per session build; the
    // `skill` tool hands bodies out on demand, and a broken skill is a
    // warning and skipped, never fatal (D14).
    let claude_home = cox_config::load::home_dir().join(".claude");
    let found = cox_ext::skills::discover(&cox_ext::skills::skill_dirs(
        Some(&home),
        Some(&claude_home),
        Some(&project),
    ));
    warnings.extend(found.notices.into_iter().map(Warning::Skill));
    // T34.1: subagent definitions are discovered once here, at session
    // build, the same roots `cox ext list` reads — never inside `cox-core`,
    // which does no filesystem I/O of its own (`agent_defs` on `Session`
    // is set below, after construction, like `set_worktrees`).
    let mut agents_found = cox_ext::agents::discover(&cox_ext::agents::agent_dirs(
        Some(&home),
        Some(&claude_home),
        Some(&project),
    ));
    // T45.4: granted plugins' definitions join after the files, so a local
    // definition of the same name wins (PL§14 decision 15).
    for (id, defs) in plugin_agent_defs {
        cox_ext::agents::merge(&mut agents_found, defs, &id);
    }
    warnings.extend(agents_found.notices.into_iter().map(Warning::Agent));
    let (instructions, skills_index, dropped) = prefix_texts(
        &home,
        &claude_home,
        cwd,
        config.context.instruction_budget_tokens,
        &found.skills,
    );
    warnings.extend(dropped.into_iter().map(Warning::Instruction));
    let mut all = tools(answer, &store, mdir);
    // T47.3: made before MCP connects, so each server's handshake already
    // declares (or not) the elicitation capability.
    let (asker, asks) = mcp::question_channel(questions).unzip();
    if questions {
        all = tools::with_question_surface(all);
    }
    if let Some(c) = client {
        all = with_client_tools(all, c.link, c.fs, c.terminal);
    }
    // T34.6: stateless — the session that builds each call's own `ToolCx`
    // (`cox-core/src/turn.rs`) stamps `ToolCx.relay` with itself, so this
    // one shared instance still reaches each caller's own session, never a
    // handle fixed at construction time (SM§4; no preset grants it to a
    // child yet, but a shared instance must be safe if one someday does).
    all.push(Arc::new(SendMessageTool));
    // T22.2: the deferred `skill` tool hands skill bodies out on demand
    // (its spec is `deferred`, `ReadOnly`; broken skills are skipped above,
    // D14); `with_tool_search_index` below makes it discoverable.
    all.push(Arc::new(cox_ext::skills::SkillTool::new(found.skills)));
    if config.mcp.enabled {
        let (mcp, notices) =
            mcp::mcp_tools(&config, cwd, mcp_login, asker, plugins.mcp, &writable).await;
        all.extend(mcp);
        warnings.extend(notices.into_iter().map(Warning::Mcp));
    }
    #[cfg_attr(not(feature = "plugins"), allow(unused_mut))]
    let mut plugin_warnings = plugins.notices;
    #[cfg(feature = "plugins")]
    let mut live = plugins.live;
    #[cfg(feature = "plugins")]
    all.extend(plugins::plugin_tools(
        &mut live,
        &base.plugins,
        id,
        cwd,
        &mut plugin_warnings,
    ));
    all.extend(surface_tools);
    let all = tools::with_lsp(all, &config, &writable);
    let all = tools::with_tool_search_index(all);
    let session = match resume {
        Some((id, history)) => Session::resume(
            config,
            provider,
            all,
            store.clone(),
            store,
            cwd.to_path_buf(),
            id,
            history,
        )?,
        None => Session::new_with_id(
            id,
            config,
            provider,
            all,
            store.clone(),
            store,
            cwd.to_path_buf(),
        )?,
    };
    if worktree_main.is_some() {
        session.set_writable_roots(vec![cwd.to_path_buf()]);
    }
    if let Some(asks) = asks {
        mcp::bridge_questions(&session, asks);
    }
    session.set_agent_defs(agents_found.agents);
    session.set_instructions(instructions, skills_index);
    // T35.13: one driver per granted `[[external_agents]]` entry; an entry
    // whose CLI or key is missing is left out with one warning (EA§7).
    #[cfg(feature = "plugins")]
    let plugin_warnings = {
        let (drivers, left_out) = external_agents::drivers(
            plugins.external_agents,
            &base,
            cwd,
            &writable,
            std::env::var_os("PATH").as_deref(),
            cox_provider::http::resolve_key,
        );
        session.set_external_agents(drivers);
        plugin_warnings
            .into_iter()
            .chain(left_out)
            .collect::<Vec<_>>()
    };
    // T33.44: each granted plugin's `cox_init` ran once, in `plugin_tools`
    // above; its instance is shared by its tools, its hooks (below) and the
    // event tap `start_plugins` sets.
    #[cfg(feature = "plugins")]
    let (plugin_hooks, plugin_started) =
        start_plugins(&session, live, &base.plugins, cwd, plugin_ui);
    #[cfg(not(feature = "plugins"))]
    let (plugin_hooks, plugin_started): plugins::Started = {
        let _ = plugin_ui;
        (Vec::new(), Vec::new())
    };
    // A14: the presence hook wraps the user's shell hooks so the other
    // sessions of this workspace see every surface, `--no-hooks` or not;
    // PL§6: plugin hooks follow the shell's in one chain, and `--no-hooks`
    // turns off only the shell's.
    // T57.2: hooks run in the shell a `!` line runs in.
    let shell: Option<Arc<dyn Hook>> = base.hooks.enabled.then(|| {
        let hooks = cox_ext::hooks::ShellHooks::new(&base.hooks, cwd.to_path_buf());
        let hooks = match cox_tools::bash::default_shell() {
            Some(program) => hooks.with_shell(program),
            None => hooks,
        };
        Arc::new(hooks) as Arc<dyn Hook>
    });
    session.set_hook(Arc::new(
        cox_ext::presence::PresenceHook::new(
            home.clone(),
            session.id(),
            cwd.to_path_buf(),
            project,
            Some(Arc::new(cox_ext::hooks::HookChain::new(
                shell,
                plugin_hooks,
            ))),
        )
        .with_worktree(worktree.then(|| cwd.to_path_buf())),
    ));
    // T26.1: pre-images for `/rewind` live in private git dirs under home.
    session.set_checkpointer(Arc::new(cox_tools::checkpoint::GitCheckpointer::new(
        home.clone(),
    )));
    // T27.3: `agent(isolation: "worktree")` gets real worktrees on every surface.
    session.set_worktrees(Arc::new(cox_tools::git::GitWorktrees));
    // P43: the repo map, built once on the first submit when
    // `context.repomap_budget_tokens` is on.
    session.set_repo_mapper(Arc::new(ToolsRepoMapper));
    for warning in served.iter().flat_map(|s| s.model.warnings()) {
        session.notice(Level::Warn, warning).await?;
    }
    for warning in plugin_warnings {
        session.notice(Level::Warn, warning).await?;
    }
    for (level, text) in plugin_started {
        session.notice(level, text).await?;
    }
    if let Some((level, text)) = unsandboxed {
        session.notice(level, text).await?;
    }
    Ok(Opened {
        session,
        config: base,
        warnings: Vec::new(),
    })
}

/// Where the `AGENTS.md`/`CLAUDE.md` chain is read from for `cwd`: one
/// place, so `cox ext` lists exactly the files a session sends (T50.1).
pub fn instruction_roots(
    cox_home: &Path,
    claude_home: &Path,
    cwd: &Path,
) -> cox_ext::instructions::Roots {
    cox_ext::instructions::Roots {
        cox_home: Some(cox_home.to_path_buf()),
        claude_home: Some(claude_home.to_path_buf()),
        git_root: cox_config::load::find_git_root(cwd),
        cwd: cwd.to_path_buf(),
    }
}

/// T50.1: the two `system[2]` texts a session opens with — the
/// `AGENTS.md`/`CLAUDE.md` chain under the instruction budget and the skills
/// index — read once here, since `cox-core` reads no files — plus what the
/// load dropped (a file or include cycle), a warning, never fatal (D14).
fn prefix_texts(
    home: &Path,
    claude_home: &Path,
    cwd: &Path,
    budget_tokens: u32,
    skills: &[cox_ext::skills::Skill],
) -> (String, String, Vec<String>) {
    let roots = instruction_roots(home, claude_home, cwd);
    let loaded = cox_ext::instructions::load(&roots, budget_tokens);
    (loaded.block, cox_ext::skills::index(skills), loaded.notices)
}

/// One worktree-aware project identity for presence writes and polling.
pub async fn project_root(cwd: &Path) -> PathBuf {
    cox_tools::git::project_root(cwd).await.unwrap_or_else(|_| {
        cox_config::load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
    })
}

/// Adds `root` to the workspace roots once (T27.3: the main checkout of a
/// worktree session, readable, not writable).
pub fn add_read_root(config: &mut Config, root: &Path) {
    if !config.core.workspace_roots.iter().any(|r| r == root) {
        config.core.workspace_roots.push(root.to_path_buf());
    }
}

/// Where a session's memory facts live: `config.memory.dir` wins, else
/// `<home>/projects/<slug>/memory` (T10.1).
pub fn memory_dir_for(config: &Config, home: &Path, cwd: &Path) -> PathBuf {
    if config.memory.dir.is_empty() {
        cox_ext::memory::memory_dir(home, cwd)
    } else {
        PathBuf::from(&config.memory.dir)
    }
}

/// `cox_tools::repomap` behind the core's `RepoMapper` (P43): `cox-core`
/// may not walk the tree or run git itself.
struct ToolsRepoMapper;

#[async_trait::async_trait]
impl cox_protocol::traits::RepoMapper for ToolsRepoMapper {
    async fn build(
        &self,
        root: &Path,
        budget_bytes: usize,
        admit: &(dyn for<'p> Fn(&'p Path) -> bool + Send + Sync),
    ) -> String {
        cox_tools::repomap::build(root, budget_bytes, admit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key-shaped run in a warning's text never reaches a surface.
    #[test]
    fn warning_display_masks_secrets() {
        let w = Warning::Mcp("server x: 401 for Bearer tok123secret, key sk-abcdefghijk".into());
        let shown = w.to_string();
        assert!(
            !shown.contains("tok123secret") && !shown.contains("sk-abcdefghijk"),
            "{shown}"
        );
        assert!(shown.starts_with("server x: 401 for "), "{shown}");
    }

    /// T37.1: a broken `SKILL.md` comes back from `open` as a
    /// `Warning::Skill`, for the caller to show, and the session still opens.
    #[test]
    fn open_returns_skill_warnings_as_data() {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let bad = home.path().join("skills/bad");
        std::fs::create_dir_all(&bad).expect("skill dir");
        std::fs::write(bad.join("SKILL.md"), "no front matter\n").expect("skill");
        let scenario = work.path().join("scenario.toml");
        std::fs::write(&scenario, "[[turn]]\ntext = \"hi\"\n").expect("scenario");
        let mut config = Config::default();
        // Nothing from the developer's own MCP servers, plugins or hooks.
        config.mcp.enabled = false;
        config.plugins.enabled = false;
        config.hooks.enabled = false;
        let spec = SessionSpec {
            config,
            cwd: work.path().to_path_buf(),
            home: home.path().to_path_buf(),
            worktree: false,
            answer: None,
            questions: false,
            resume: None,
            mcp_login: None,
            plugin_ui: None,
            client: None,
            surface: "test".into(),
            tools: Vec::new(),
        };
        let scenario = scenario.display().to_string();
        let vars = [
            ("COX_PROVIDER", Some("scripted")),
            ("COX_SCENARIO", Some(scenario.as_str())),
        ];
        cox_config::load::temp_env(&vars, || {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            let opened = rt.block_on(open(spec)).expect("the session opens");
            // Only this home's skill: `~/.claude/skills` is read too.
            let ours: Vec<_> = opened
                .warnings
                .iter()
                .filter(|w| w.to_string().contains("skills/bad/SKILL.md skipped"))
                .collect();
            assert!(
                matches!(ours.as_slice(), [Warning::Skill(_)]),
                "expected exactly one skill warning for skills/bad/SKILL.md"
            );
            assert_eq!(
                opened.config.core.workspace_roots,
                vec![work.path().to_path_buf()],
                "an empty root list became cwd"
            );
        });
    }

    /// T50.1: the `AGENTS.md` chain and the skills index a session opens
    /// with reach `system[2]`, and the next turn sends the same prefix bytes.
    #[tokio::test]
    async fn instruction_files_and_skills_index_reach_system_two() {
        use crate::testing::{Recorder, user_turn};
        let (home, claude, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("claude"),
            tempfile::tempdir().expect("work"),
        );
        std::fs::write(work.path().join("AGENTS.md"), "Answer in haiku.\n").expect("agents");
        let skill = work.path().join(".cox").join("skills").join("greet");
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: greet\ndescription: Greet the user first.\n---\nSay hello.\n",
        )
        .expect("skill");
        let found = cox_ext::skills::discover(&cox_ext::skills::skill_dirs(
            Some(home.path()),
            Some(claude.path()),
            Some(work.path()),
        ));
        let config = Config::default();
        let (block, index, _) = prefix_texts(
            home.path(),
            claude.path(),
            work.path(),
            config.context.instruction_budget_tokens,
            &found.skills,
        );
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let recorder = Arc::new(Recorder {
            inner: cox_provider::scripted::Scripted::from_toml(
                "[[turn]]\ntext = \"one\"\n[[turn]]\ntext = \"two\"\n",
                "",
            )
            .expect("scenario"),
            sent: std::sync::Mutex::default(),
        });
        let session = Session::new_with_id(
            SessionId::new(),
            config,
            recorder.clone(),
            vec![],
            store.clone(),
            store,
            work.path().to_path_buf(),
        )
        .expect("session");
        session.set_instructions(block, index);
        user_turn(&session, "one").await;
        user_turn(&session, "two").await;
        let sent = recorder.sent.lock().expect("sent").clone();
        assert_eq!(sent.len(), 2);
        let two = &sent[0].system[2].text;
        assert!(two.contains("# Instructions\n"), "{two}");
        assert!(two.contains("Answer in haiku."), "{two}");
        assert!(two.contains("- greet: Greet the user first."), "{two}");
        let prefix = |r: &cox_protocol::types::Request| {
            serde_json::to_vec(&r.system[0..=2]).expect("prefix")
        };
        assert_eq!(prefix(&sent[0]), prefix(&sent[1]));
    }
}
