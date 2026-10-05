// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The interactive surfaces' side of a session: `open` loads config from
//! the flags and hands it to `cox_session::open`, printing the warnings it
//! returns; the TUI loop (`run_tui`), `cox init`, `--worktree` and the
//! plugin dialogs and executors the TUI drives live here because they need
//! `Cli`, the terminal or `cox-tui`. Assembly itself is `cox-session`
//! (T37.1), so the TUI and `cox run -p` (T6.1) build the same session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_core::{History, Session};
use cox_protocol::Config;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use cox_protocol::types::{Event, Level, Submission};
use cox_store::Store;
use cox_tui::state::{Ask, GitStatus, Msg, PluginMgmtRequest, PluginNewRequest, State};

use crate::cli::Cli;
use crate::config_cmd;
use crate::config_load::{self, LoadedConfig};
use crate::resume;
use crate::status_line;

#[cfg(feature = "plugins")]
use cox_protocol::GrantScope;
#[cfg(feature = "plugins")]
pub(crate) use cox_session::sandboxed_argv;
use cox_session::{fork, handoff, project_root};
pub(crate) use cox_session::{lmstudio_model, mcp_servers, memory_dir_for, sandbox_policy, tools};

/// Loads config from `cli` for `cwd`, lets `tweak` adjust it, then builds
/// the session with `cox_session::open` and prints the warnings it returns.
/// `answer` is what `ask_user` returns when no one is there to ask;
/// `questions` (T22.1, DT G4) says the surface — only `run_tui` and
/// `--plain` are one — answers `Event::QuestionAsked` instead. `interactive` says a person is at
/// the terminal, so an MCP server's 401 may open a browser login (T22.5).
/// `plugin_ui` (T33.23, T33.44) — only `run_tui` has one — takes the TUI's
/// plugin feed and render requests, so the live plugins can be rendered
/// and ask for redraws.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    cli: &Cli,
    cwd: &Path,
    answer: Option<String>,
    questions: bool,
    tweak: impl FnOnce(&mut Config),
    resume: Option<(SessionId, History)>,
    interactive: bool,
    plugin_ui: Option<PluginUi>,
) -> anyhow::Result<(Session, LoadedConfig)> {
    let mut loaded = config_load::load(cwd, cli)?;
    tweak(&mut loaded.config);
    // T50.4: on resume an explicit `--permission-mode` wins, then the mode
    // the rollout recorded, then config. The resolved mode goes back into
    // the config too, so the TUI and `--plain` show the mode the session
    // actually runs in; `cox-core` records it when the session opens.
    let resume = resume.map(|(id, mut history)| {
        let mode = match (&cli.permission_mode, history.permission_mode) {
            (None, Some(recorded)) => recorded,
            _ => loaded.config.permissions.mode,
        };
        history.permission_mode = Some(mode);
        loaded.config.permissions.mode = mode;
        (id, history)
    });
    // T37.34: only the TUI renders plugins and only `--plain` and the TUI
    // ask questions, so these name the surface to a second opener.
    let surface = match (&plugin_ui, questions) {
        (Some(_), _) => "tui",
        (None, true) => "plain",
        (None, false) => "headless",
    };
    // Warnings come back through their own vector, never out of `opened`
    // (which holds the provider key); their text is already redacted.
    let mut warnings = Vec::new();
    let spec = cox_session::SessionSpec {
        config: loaded.config.clone(),
        cwd: cwd.to_path_buf(),
        home: cli.home.clone().unwrap_or_else(config_load::cox_home),
        worktree: cli.worktree.is_some(),
        answer,
        questions,
        resume,
        mcp_login: interactive.then(mcp_login),
        plugin_ui: serve_ui(plugin_ui),
        client: None,
        surface: surface.into(),
        tools: Vec::new(),
    };
    let opened = cox_session::open_reporting(spec, None, &mut warnings).await?;
    for warning in &warnings {
        eprintln!("cox: warning: {warning}");
    }
    loaded.config = opened.config;
    Ok((opened.session, loaded))
}

/// T22.5: an MCP server's 401 with a person at the terminal: print the
/// login URL and open the browser.
fn mcp_login() -> cox_mcp::client::Prompt {
    Arc::new(|url: &str| {
        eprintln!("cox: mcp login: open {url}");
        if !cox_mcp::auth::open_browser(url) {
            eprintln!("cox: no browser found; open the URL by hand");
        }
    })
}

/// T22.5: MCP auth for `cox mcp`: keyring tokens, and a browser login when
/// `interactive`.
pub fn mcp_auth(interactive: bool) -> cox_mcp::client::Auth {
    cox_session::mcp_auth(interactive.then(mcp_login))
}

/// `plugin_ui` as the renderer `cox_session::open` starts once the live
/// plugins exist (T33.44).
#[cfg(feature = "plugins")]
fn serve_ui(plugin_ui: Option<PluginUi>) -> Option<cox_session::ServeUi> {
    plugin_ui.map(|ui| {
        Box::new(move |live: &cox_plugin::LivePlugins| serve_plugin_ui(live, ui))
            as cox_session::ServeUi
    })
}

/// The slim build has no plugin to render.
#[cfg(not(feature = "plugins"))]
fn serve_ui(plugin_ui: Option<PluginUi>) -> Option<cox_session::ServeUi> {
    drop(plugin_ui);
    None
}

/// The TUI's ends of the plugin UI channels (T33.23): where `Msg::Plugin`
/// answers go, and the `Cmd::Plugin` requests to serve. `open` hands them
/// to the live plugins (T33.44).
#[cfg_attr(
    not(feature = "plugins"),
    expect(dead_code, reason = "the slim build has no plugin to render")
)]
pub struct PluginUi {
    /// The TUI's feed.
    pub feed: tokio::sync::mpsc::Sender<Msg>,
    /// The TUI's render requests.
    pub requests: tokio::sync::mpsc::Receiver<cox_tui::state::PluginRequest>,
}

/// T33.23's render server over the live hosts, and each plugin's granted
/// status slots, commands and keys (T33.25) and item renderers (T33.26)
/// declared on the feed, which renders/registers them the first time. A
/// plugin with any of them but no status slot still gets a `Declare`. Returns the tap's `Redraw`.
/// A server thread that fails to start only leaves plugin segments
/// unrendered.
#[cfg(feature = "plugins")]
fn serve_plugin_ui(live: &cox_plugin::LivePlugins, ui: PluginUi) -> cox_plugin::Redraw {
    use cox_tui::state::PluginUiMsg;

    let _ = crate::plugin_ui::serve(live.hosts(), ui.requests, ui.feed.clone());
    let declares: Vec<_> = live
        .plugins()
        .iter()
        .map(|p| {
            (
                p.id().to_string(),
                p.granted_status(),
                p.granted_commands(),
                p.granted_keys(),
                p.granted_renderers(),
            )
        })
        .filter(|(_, slots, commands, keys, renderers)| {
            !slots.is_empty() || !commands.is_empty() || !keys.is_empty() || !renderers.is_empty()
        })
        .collect();
    let feed = ui.feed.clone();
    tokio::spawn(async move {
        for (plugin, slots, commands, keys, renderers) in declares {
            let msg = Msg::Plugin(PluginUiMsg::Declare {
                plugin,
                slots,
                commands,
                keys,
                renderers,
            });
            if feed.send(msg).await.is_err() {
                break;
            }
        }
    });
    crate::plugin_ui::redraw(ui.feed)
}

/// `PluginMgmtRequest::New`'s executor (T33.30): the same
/// `plugin_new::scaffold`/`write` pair `cox plugin new` calls (`main.rs`),
/// so there is only one implementation of the name/language/capability
/// mapping. Maps the picker's or `--lang`'s string back to `Lang`, and each
/// `--with` word back to `Capability`, here — the one place that happens,
/// since `cox-tui` cannot depend on this crate's types. Writes under
/// `cwd`/`name`; `write` already refuses an existing directory.
#[cfg(feature = "plugins")]
fn run_plugin_new(cwd: &Path, request: &PluginNewRequest) -> Result<String, String> {
    use clap::ValueEnum;

    let lang = crate::plugin_new::Lang::from_str(&request.lang, true)
        .map_err(|_| format!("unknown plugin language {:?}", request.lang))?;
    let with = request
        .with
        .iter()
        .map(|w| crate::plugin_new::Capability::from_str(w, true))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("unknown capability in --with: {:?}", request.with))?;
    let dir = cwd.join(&request.name);
    let files =
        crate::plugin_new::scaffold(&request.name, lang, &with).map_err(|e| e.to_string())?;
    crate::plugin_new::write(&dir, &files).map_err(|e| e.to_string())?;
    Ok(format!("scaffolded {} in {}", request.name, dir.display()))
}

/// The slim build has no `plugin_new` module (T33.30).
#[cfg(not(feature = "plugins"))]
fn run_plugin_new(_cwd: &Path, _request: &PluginNewRequest) -> Result<String, String> {
    Err("this build has no plugin support (built without --features plugins)".to_string())
}

/// `Cmd::PluginMgmt`'s executor (T33.30 `New`; T33.33 `Update`/`Remove`/
/// `List`): each arm calls the same `plugin_new`/`plugin_cmd` function
/// `cox plugin ...` calls (`main.rs`), so there is only one implementation.
/// `Remove` additionally sends `PluginRequest::Stop` on `plugin_tx` — the
/// same channel `Cmd::Plugin` already reaches `plugin_ui::serve` on, the one
/// place that holds this session's live hosts — so the removed plugin's
/// frozen `WasmTool`s answer `Denied` for the rest of the session instead of
/// still calling into an instance `cox plugin remove` just deleted on disk.
#[cfg(feature = "plugins")]
fn run_plugin_mgmt(
    cli: &Cli,
    cwd: &Path,
    plugin_tx: &tokio::sync::mpsc::Sender<cox_tui::state::PluginRequest>,
    request: PluginMgmtRequest,
) -> Result<String, String> {
    match request {
        PluginMgmtRequest::New(req) => run_plugin_new(cwd, &req),
        PluginMgmtRequest::Update {
            ids,
            all,
            check,
            rollback,
        } => crate::plugin_cmd::update_for_tui(cli, &ids, all, check, rollback)
            .map_err(|e| e.to_string()),
        PluginMgmtRequest::Remove { id, keep_data } => {
            let text = crate::plugin_cmd::remove_for_tui(cli, cwd, &id, keep_data)
                .map_err(|e| e.to_string())?;
            let _ = plugin_tx.try_send(cox_tui::state::PluginRequest::Stop { plugin: id });
            Ok(text)
        }
        PluginMgmtRequest::List { json } => Ok(crate::plugin_cmd::list(cli, cwd, json)),
    }
}

/// The slim build has no `plugin_cmd` module (T33.30, T33.33); `New` still
/// goes through `run_plugin_new` above so its own slim message stays the one
/// place that text is written.
#[cfg(not(feature = "plugins"))]
fn run_plugin_mgmt(
    _cli: &Cli,
    cwd: &Path,
    _plugin_tx: &tokio::sync::mpsc::Sender<cox_tui::state::PluginRequest>,
    request: PluginMgmtRequest,
) -> Result<String, String> {
    match request {
        PluginMgmtRequest::New(req) => run_plugin_new(cwd, &req),
        _ => Err("this build has no plugin support (built without --features plugins)".to_string()),
    }
}

/// T33.8 (PL§3): the `NeedsApproval` plugins from the same discovery walk
/// `plugin_notices` performs, shaped for `Modal::PluginGrant` instead of a
/// warning line — the TUI is the only surface with somewhere interactive to
/// put the choice (`run_tui`, below); headless and ACP still get the text
/// notice `plugin_notices` already emits, unchanged. A store read error,
/// like `plugin_notices`, counts as no grant: it must never let a dialog
/// offer to widen a grant it cannot actually confirm.
#[cfg(feature = "plugins")]
fn plugin_grant_requests(
    config: &Config,
    home: &Path,
    cwd: &Path,
    store: &dyn cox_protocol::PluginStore,
) -> Vec<cox_tui::modal::PluginGrantDialog> {
    use cox_plugin::discover::{self, State};
    use cox_plugin::grant::{self, Verdict};

    if !config.plugins.enabled {
        return Vec::new();
    }
    let root = config_load::find_git_root(cwd);
    let found = discover::discover(home, root.as_deref());
    let mut out = Vec::new();
    for p in &found.plugins {
        let State::Loaded { manifest, digest } = &p.state else {
            continue;
        };
        // A project plugin discovered with no git root has nowhere to
        // write a grant, so it stays a `plugin_notices` text warning only.
        let Some(scope) = grant::scope(p.source, root.as_deref()) else {
            continue;
        };
        // A linked plugin's grant is keyed to `link_digest()`, not its
        // (rebuild-volatile) package digest (T33.41) — `p.grant_digest()`
        // picks the right key; `grant::check` still gets the real
        // `digest` so it can tell a genuine mismatch from a linked one.
        let grant_digest = p.grant_digest().unwrap_or_else(|| digest.to_string());
        let stored = store.grant_get(&p.id, &scope, &grant_digest).ok().flatten();
        let Verdict::NeedsApproval { added, removed } =
            grant::check(manifest, digest, stored.as_ref())
        else {
            continue;
        };
        let repo = match &scope {
            GrantScope::Project(root) => Some(root.display().to_string()),
            GrantScope::User => None,
        };
        // Carries `grant_digest`, not the live `digest`: a `y` here goes
        // through `GrantDecision` to `write_plugin_grant` below, which
        // must write the same key `p.grant_digest()` looks up under next
        // time, or a linked plugin's approval would never be found again
        // (T33.41). Identical to `digest` for a non-linked plugin, so
        // this changes nothing for the existing path.
        out.push(cox_tui::modal::PluginGrantDialog::new(
            p.id.clone(),
            grant_digest.clone(),
            scope,
            manifest.name.clone(),
            manifest.description.clone(),
            grant::capability_list(manifest),
            added,
            removed,
            repo,
            p.dev,
        ));
    }
    out
}

/// The slim build never discovers a plugin, so there is never a dialog to
/// queue.
#[cfg(not(feature = "plugins"))]
fn plugin_grant_requests(
    _config: &Config,
    _home: &Path,
    _cwd: &Path,
    _store: &dyn cox_protocol::PluginStore,
) -> Vec<cox_tui::modal::PluginGrantDialog> {
    Vec::new()
}

/// Writes one plugin grant (T33.8): forwards to `plugin_cmd::write_grant`,
/// the single place a `PluginGrant` row is assembled — T33.7's `cox plugin
/// enable`/`install` write through the same function, so this module never
/// grows its own copy of that literal or of `cox_store::now_rfc3339`'s
/// timestamp. Feature-gated like `plugin_grant_requests`: the slim build
/// has no `plugin_cmd` module to forward to, and there is never a decision
/// to write in that build anyway.
#[cfg(feature = "plugins")]
fn write_plugin_grant(
    store: &Store,
    decision: cox_tui::state::GrantDecision,
) -> Result<(), cox_protocol::StoreError> {
    crate::plugin_cmd::write_grant(
        store,
        &decision.plugin_id,
        &decision.scope,
        &decision.digest,
        decision.capabilities,
        serde_json::json!({}),
    )
}

#[cfg(not(feature = "plugins"))]
fn write_plugin_grant(
    _store: &Store,
    _decision: cox_tui::state::GrantDecision,
) -> Result<(), cox_protocol::StoreError> {
    Ok(())
}

/// `--worktree <name>` (T27.3): creates or reuses the worktree, then makes
/// the rest of the run see it as `--cwd <worktree>`; [`open`] adds the main
/// checkout as a read-only root after config loading. Returns the new cwd.
/// Runs before config is loaded because the
/// project config is read from the worktree like everything else.
pub fn enter_worktree(cli: &mut Cli, cwd: &Path) -> anyhow::Result<PathBuf> {
    let Some(name) = cli.worktree.clone() else {
        return Ok(cwd.to_path_buf());
    };
    let owner = format!("cox / pid {}", std::process::id());
    let rt = tokio::runtime::Runtime::new()?;
    let wt = rt.block_on(cox_tools::git::worktree_add(cwd, &name, &owner))?;
    // A new session has no record yet, so a fresh id excludes nobody.
    Ok(switch_to_worktree(cli, &rt, wt.path, &SessionId::new()))
}

/// What `--worktree` and a resumed worktree session share once the tree
/// exists: the holder warning, then `--cwd <worktree>` for the rest of the
/// run. `cli.worktree` being set is what makes `open` add the main checkout
/// as a read-only root, keep only the worktree writable and put the
/// worktree in the presence record (`SessionSpec::worktree`).
fn switch_to_worktree(
    cli: &mut Cli,
    rt: &tokio::runtime::Runtime,
    worktree: PathBuf,
    me: &SessionId,
) -> PathBuf {
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    warn_worktree_holder(rt, &home, &worktree, me);
    cli.cwd = Some(worktree.clone());
    worktree
}

/// `cox --resume <id>` of a session that ran in a linked worktree of this
/// project (T44.4, A75): the run moves into that worktree exactly as
/// `--worktree` would, so the sandbox roots follow. A worktree that is gone
/// is refused with a hint — cox never runs `git worktree add` on resume.
/// Any other recorded cwd (the main checkout, another project, no row)
/// leaves `cwd` as it was. Returns the cwd to use.
pub fn resume_worktree(cli: &mut Cli, cwd: &Path) -> anyhow::Result<PathBuf> {
    let (Some(id), None) = (cli.resume.clone(), cli.worktree.as_ref()) else {
        return Ok(cwd.to_path_buf());
    };
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let Some(recorded) = resume::recorded_cwd(&home, &id) else {
        return Ok(cwd.to_path_buf());
    };
    let rt = tokio::runtime::Runtime::new()?;
    let main = rt.block_on(project_root(cwd));
    if recorded == main || recorded == cwd {
        return Ok(cwd.to_path_buf());
    }
    let repo = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir_name = recorded
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `worktree_add` names the tree `<repo>-<name>`; the rest is the name.
    let name = dir_name
        .strip_prefix(&format!("{repo}-"))
        .unwrap_or(dir_name.as_str())
        .to_string();
    if !recorded.is_dir() {
        // Gone, so git cannot say whose it was: cox's own layout
        // (`_worktrees/<repo>-<name>`) is the evidence it was ours.
        let ours = recorded
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|p| p == "_worktrees")
            && dir_name.starts_with(&format!("{repo}-"));
        if !ours {
            return Ok(cwd.to_path_buf());
        }
        anyhow::bail!(
            "session {id} ran in worktree {}, which no longer exists; \
             `git worktree list` shows the worktrees there are, and \
             `cox --worktree {name} --resume {id}` recreates it on branch {name}",
            recorded.display()
        );
    }
    let linked_here = crate::sessions::is_linked_worktree(&recorded)
        && rt.block_on(project_root(&recorded)) == main;
    if !linked_here {
        return Ok(cwd.to_path_buf());
    }
    cli.worktree = Some(name);
    // The resumed session's own record is not another holder.
    let me: SessionId = id.parse()?;
    Ok(switch_to_worktree(cli, &rt, recorded, &me))
}

/// T44.2 (A75): another live session of this project already running in
/// `worktree` is warned about by id before this one starts there — warn,
/// not block (fail open, no lock). Printed on the same `cox: warning:` path
/// as `open`'s warnings, which stays above the TUI's inline viewport. `me`
/// is left out, so a resumed session never warns about its own record.
fn warn_worktree_holder(
    rt: &tokio::runtime::Runtime,
    home: &Path,
    worktree: &Path,
    me: &SessionId,
) {
    let project = rt.block_on(project_root(worktree));
    let now = cox_ext::presence::now_secs();
    if let Some(other) = cox_ext::presence::holder(home, &project, worktree, me, now) {
        eprintln!(
            "cox: warning: session {} (pid {}) is already working in worktree {}; \
             two sessions editing one tree will overwrite each other's changes",
            other.session,
            other.pid,
            worktree.display()
        );
    }
}

/// Push-to-talk for one TUI session (T54.7): `[voice] key` and
/// `auto_submit` into `state`, and the `Dictation` `app::run` takes. Every
/// reason there is none (the build, `enabled`, a missing model) leaves the
/// key showing how to turn it on; only a missing model is also a warning.
fn voice(
    state: &mut State,
    config: &Config,
    #[cfg_attr(not(feature = "voice"), allow(unused_variables))] home: &Path,
) -> Option<Box<dyn cox_protocol::traits::Dictation>> {
    state.voice.auto_submit = config.voice.auto_submit;
    let default = cox_tui::keymap::parse("alt+v");
    if cox_tui::keymap::parse(&config.voice.key) != default
        && let Err(text) = state.keymap.rebind(
            cox_tui::keymap::Action::Voice,
            std::slice::from_ref(&config.voice.key),
        )
    {
        state.transcript.push(cox_tui::state::Cell::Notice {
            level: Level::Warn,
            text: format!("voice.key: {text}"),
        });
    }
    #[cfg(feature = "voice")]
    match dictation(config, home) {
        Ok(dictation) => return dictation,
        Err(text) => state.transcript.push(cox_tui::state::Cell::Notice {
            level: Level::Warn,
            text,
        }),
    }
    None
}

/// `[voice] enabled` with the model on disk: the dictation; enabled
/// without it: the warning naming the download command.
#[cfg(feature = "voice")]
fn dictation(
    config: &Config,
    home: &Path,
) -> Result<Option<Box<dyn cox_protocol::traits::Dictation>>, String> {
    let voice = &config.voice;
    if !voice.enabled {
        return Ok(None);
    }
    let Some(path) = crate::voice_cmd::model_path(home, &voice.model) else {
        return Err(format!(
            "voice: no pinned model named {}; `cox voice model list` shows them",
            voice.model
        ));
    };
    if !path.is_file() {
        return Err(format!(
            "voice: model {m} is not downloaded; run `cox voice model download {m}`",
            m = voice.model
        ));
    }
    let language = match voice.language.trim() {
        "" | "auto" => None,
        code => Some(code.to_string()),
    };
    let max = std::time::Duration::from_secs(u64::from(voice.max_seconds));
    Ok(Some(Box::new(cox_voice::PushToTalk::new(
        path, language, max,
    ))))
}

/// After `/quit` in a worktree session: a clean tree is offered for
/// removal on the terminal the TUI just gave back; a dirty one is kept and
/// said so. The branch always stays — merging is the user's action.
fn offer_worktree_removal(rt: &tokio::runtime::Runtime, path: &Path) {
    if rt.block_on(cox_tools::git::is_clean(path)) != Some(true) {
        eprintln!(
            "cox: worktree {} kept: it has uncommitted or untracked files",
            path.display()
        );
        return;
    }
    eprint!(
        "cox: worktree {} is clean; remove it? [y/N] ",
        path.display()
    );
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        eprintln!("cox: worktree {} kept", path.display());
        return;
    }
    match rt.block_on(cox_tools::git::worktree_remove(
        path,
        cox_tools::git::OWNER_PREFIX,
    )) {
        Ok(()) => eprintln!(
            "cox: worktree {} removed; its branch is kept for you to merge",
            path.display()
        ),
        Err(e) => eprintln!("cox: worktree {} kept: {e}", path.display()),
    }
}

/// `/sessions` and `/resume` rows: this project's sessions, newest first,
/// forks and handoffs indented under their parent (T26.3). Row zero is the
/// project header from the one SQL aggregate in `Store::project_totals`
/// (T28.2). A store that will not open is an empty list, not a failed start.
fn project_sessions(home: &Path, cwd: &Path) -> Vec<(String, String)> {
    let project = config_load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let now = crate::sessions::now_secs();
    let store = Store::open(home).ok();
    let Some(store) = store.as_ref() else {
        return Vec::new();
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    let tree = store.sessions_tree(200).unwrap_or_default();
    let kept: Vec<&cox_store::queries::TreeRow> = tree
        .iter()
        .filter(|row| Path::new(&row.info.cwd).starts_with(&project))
        .collect();
    if !kept.is_empty() {
        let slug = cox_ext::memory::slug_for(cwd);
        let totals = store.project_totals(&slug).ok();
        let (sessions, cost) =
            totals.map_or((kept.len() as i64, 0.0), |t| (t.sessions, t.cost_usd));
        rows.push((
            String::new(),
            cox_tui::picker::project_header(&slug, sessions, cost),
        ));
    }
    rows.extend(kept.into_iter().map(|row| {
        let entry = cox_tui::picker::session_entry(
            row.depth,
            row.info.title.as_deref(),
            &row.info.cwd,
            &crate::sessions::age_of(&row.info.updated_at, now),
            row.info.cost_usd,
        );
        (row.info.id.clone(), entry)
    }));
    rows
}

/// `Ctrl+R`'s other-session rows (T25.8): the prompts typed in this
/// project's other sessions, newest first and each text once, as
/// `(picker row, full text)`. Best-effort like `project_sessions`.
fn project_prompts(home: &Path, cwd: &Path, me: SessionId) -> Vec<(String, String)> {
    let project = config_load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let Ok(store) = Store::open(home) else {
        return Vec::new();
    };
    let now = crate::sessions::now_secs();
    let me = me.to_string();
    let ages: HashMap<String, String> = store
        .list_sessions(1000)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.id != me && Path::new(&s.cwd).starts_with(&project))
        .map(|s| (s.id, crate::sessions::age_of(&s.updated_at, now)))
        .collect();
    let mut seen = HashSet::new();
    store
        .user_prompts(5000)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| Some((ages.get(&p.session_id)?, p.text)))
        .filter(|(_, text)| seen.insert(text.clone()))
        .take(500)
        .map(|(age, text)| (cox_tui::picker::prompt_entry(age, &text), text))
        .collect()
}

/// `--continue` / `--resume <id>` for the interactive surfaces (the TUI and
/// `--plain`, T29.1): the session to reopen and its rebuilt history.
pub(crate) fn resume_from_flags(
    cli: &Cli,
    home: &Path,
    cwd: &Path,
) -> anyhow::Result<Option<(SessionId, History)>> {
    if cli.r#continue {
        let id = Store::open(home)?.latest_session_for_cwd(cwd)?;
        let history = resume::from_home(home, &id.to_string())?;
        Ok(Some((id, history)))
    } else if let Some(id_str) = &cli.resume {
        let id: SessionId = id_str.parse()?;
        let history = resume::from_home(home, id_str)?;
        Ok(Some((id, history)))
    } else {
        Ok(None)
    }
}

/// `cox init [--force]` (T25.6): scaffold `AGENTS.md` headlessly over the
/// same core path the interactive `/init` drives, then print where it
/// landed. Exit 0 when the file was written, 1 when it was refused (an
/// existing `AGENTS.md` without `--force`) or denied.
pub fn run_init(cli: &Cli, cwd: &Path, force: bool) -> anyhow::Result<i32> {
    let rt = tokio::runtime::Runtime::new()?;
    let (session, _) = rt.block_on(open(cli, cwd, None, false, |_| {}, None, false, None))?;
    let mut events = session
        .events()
        .ok_or_else(|| anyhow::anyhow!("session events already taken"))?;
    let running = {
        let session = session.clone();
        rt.spawn(async move { session.run_init(force).await })
    };
    let mut written = false;
    let mut refused = false;
    while let Some(ev) = rt.block_on(events.recv()) {
        match &ev {
            Event::Notice { text, .. } if text.starts_with("wrote AGENTS.md") => {
                println!("{text}");
                written = true;
            }
            Event::Notice {
                level: Level::Warn,
                text,
            } if text.contains("already exists") => {
                println!("cox init: {text}");
                refused = true;
            }
            Event::ApprovalRequired { call, .. } => {
                let session = session.clone();
                let call_id = call.id;
                rt.block_on(session.submit(Submission::Approve {
                    call_id,
                    decision: cox_protocol::types::Decision::Allow,
                }))?;
            }
            _ => {}
        }
        if written || refused {
            break;
        }
    }
    drop(running);
    Ok(i32::from(!written))
}

/// T46.4 (P46): starts the `[tui.status_line]` runner for one TUI session
/// and turns its row on, or does nothing when no command is set. A host
/// whose sandbox cannot wrap the command gets one warning and no row; the
/// command never runs bare. Each answer rides `feed` as `Msg::StatusLine`.
fn start_status_line(
    rt: &tokio::runtime::Runtime,
    config: &Config,
    cwd: &Path,
    state: &mut State,
    feed: &tokio::sync::mpsc::Sender<Msg>,
) -> Option<tokio::sync::mpsc::Sender<(serde_json::Value, u16)>> {
    let cfg = &config.tui.status_line;
    if cfg.command.trim().is_empty() {
        return None;
    }
    let roots = match config.core.workspace_roots.as_slice() {
        [] => vec![cwd.to_path_buf()],
        roots => roots.to_vec(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let feed = feed.clone();
    let out = move |line: Option<String>| {
        let feed = feed.clone();
        tokio::spawn(async move {
            let _ = feed.send(Msg::StatusLine(line)).await;
        });
    };
    let _runtime = rt.enter();
    match status_line::spawn(cfg.clone(), sandbox_policy(config), roots, rx, out) {
        Ok(_) => {
            state.status_script = Some(cox_tui::status::StatusScript {
                enabled: true,
                ..Default::default()
            });
            Some(tx)
        }
        Err(e) => {
            state.transcript.push(cox_tui::state::Cell::Notice {
                level: Level::Warn,
                text: format!("tui.status_line is off: {e}"),
            });
            None
        }
    }
}

/// T46.7: a theme file stem the editor may write: `^[a-z0-9][a-z0-9._-]{0,63}$`
/// and no `..`. Not a model path, so `confine` does not apply; this check
/// is the only thing keeping the write inside `<home>/themes`.
fn theme_stem_ok(stem: &str) -> bool {
    let mut chars = stem.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && stem.len() <= 64
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && !stem.contains("..")
}

/// T46.7: answers `Ask::SaveTheme`. Starts from the existing
/// `<dir>/<stem>.toml`, else from the built-in a `-custom` stem was named
/// after, sets each token with `theme::set_token` (comments and other keys
/// kept) and writes it through `config_cmd`'s one writer.
fn save_theme(
    dir: &Path,
    stem: &str,
    dark: bool,
    tokens: &[(String, String)],
) -> anyhow::Result<cox_tui::theme::ThemeFile> {
    use cox_tui::theme;
    if !theme_stem_ok(stem) {
        anyhow::bail!("{stem:?} is not a theme file name");
    }
    let path = dir.join(format!("{stem}.toml"));
    let mut src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => stem
            .strip_suffix("-custom")
            .and_then(theme::builtin_source)
            .unwrap_or_default()
            .to_string(),
        Err(e) => return Err(e.into()),
    };
    for (token, value) in tokens {
        let color = theme::parse_color(value)
            .ok_or_else(|| anyhow::anyhow!("{value:?} is not a colour"))?;
        src = theme::set_token(&src, token, dark, color)?;
    }
    let file = theme::parse_theme_file(&src)?;
    cox_config::cmd::write_file(&path, &src)?;
    Ok(file)
}

/// Runs the interactive TUI until the user quits.
pub fn run_tui(cli: &Cli, cwd: &Path) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let project = rt.block_on(project_root(cwd));
    let themes_dir = home.join("themes");
    // T24.2: a `/theme` picker choice reaches here as `(key, value)`
    // because only `crates/cox` owns `config_cmd::set`; a write failing
    // (a full disk, a bad permission) is not fatal, just not persisted —
    // the picker already applied the theme to `state` either way.
    let (persist_tx, mut persist_rx) = tokio::sync::mpsc::channel::<(String, String)>(4);
    rt.spawn(async move {
        while let Some((key, value)) = persist_rx.recv().await {
            let _ = config_cmd::set(&key, &value);
        }
    });
    let mut resume_spec = resume_from_flags(cli, &home, cwd)?;
    let mut first = true;
    // What `/fork`/`/handoff` did, shown atop the next session's transcript.
    let mut announce: Option<(Level, String)> = None;
    loop {
        let seed = resume_spec.as_ref().map(|(_, history)| history.clone());
        // T33.23/T33.44: made before `open`, which hands the feed and the
        // `Cmd::Plugin` requests to the live plugins it starts.
        let (feed, feed_rx) = tokio::sync::mpsc::channel(4);
        let (plugin_tx, plugin_rx) = tokio::sync::mpsc::channel(16);
        let plugin_ui = PluginUi {
            feed: feed.clone(),
            requests: plugin_rx,
        };
        // T33.30, T33.33: `/plugin new|update|remove|list`'s executor,
        // spawned per session like the plugin UI channel above — its answer
        // rides this session's `feed`. `plugin_tx` is cloned because
        // `app::run` below takes the original; a successful `remove` sends
        // `PluginRequest::Stop` on it (`run_plugin_mgmt`'s doc comment).
        let (plugin_mgmt_tx, mut plugin_mgmt_rx) =
            tokio::sync::mpsc::channel::<PluginMgmtRequest>(4);
        {
            let cli = cli.clone();
            let cwd = cwd.to_path_buf();
            let feed = feed.clone();
            let plugin_tx = plugin_tx.clone();
            rt.spawn(async move {
                while let Some(request) = plugin_mgmt_rx.recv().await {
                    let result = run_plugin_mgmt(&cli, &cwd, &plugin_tx, request);
                    if feed.send(Msg::PluginMgmt(result)).await.is_err() {
                        break;
                    }
                }
            });
        }
        let (session, loaded) = rt.block_on(open(
            cli,
            cwd,
            None,
            true,
            |_| {},
            resume_spec.take(),
            true,
            Some(plugin_ui),
        ))?;
        let config = &loaded.config;
        let mut state = State::new(config.permissions.mode, config.sandbox.mode);
        // T28.1: the status line names the spend over the session cap.
        state.status.budget_cap_usd = config.budget.session_usd;
        state.status.budget_warn_at = config.budget.warn_at;
        // T22.2: markdown commands from `.claude/commands`/`.cox/commands`
        // join the `/` palette after the built-ins; a broken file is a
        // warning and skipped (D14).
        let cmds = cox_ext::commands::discover(&cox_ext::commands::command_dirs(
            Some(&home),
            Some(&config_load::home_dir().join(".claude")),
            Some(&project),
        ));
        for notice in &cmds.notices {
            eprintln!("cox: warning: {notice}");
        }
        state.commands.extend(cmds.commands.iter().map(|c| {
            let usage = match &c.argument_hint {
                Some(hint) => format!("/{} <{hint}>", c.name),
                None => format!("/{}", c.name),
            };
            (
                c.name.clone(),
                usage,
                c.description.clone().unwrap_or_default(),
            )
        }));
        // T25.5: rebound keys drive dispatch, hints, `?` and `/help` alike.
        let keys = config_load::keymap(&home, &config_load::home_dir().join(".claude"));
        for text in keys.warnings {
            state.transcript.push(cox_tui::state::Cell::Notice {
                level: cox_protocol::types::Level::Warn,
                text,
            });
        }
        for skipped in &keys.skipped {
            tracing::debug!("{skipped}");
        }
        state.keymap = keys.keymap;
        let dictation = voice(&mut state, config, &home);
        if let Some(history) = seed {
            state.transcript_from_history(&history);
        }
        if let Some((level, text)) = announce.take() {
            state
                .transcript
                .push(cox_tui::state::Cell::Notice { level, text });
        }
        state.files = cox_tools::glob::workspace_files(cwd);
        // T45.6: what `@name task` may dispatch, as the `agent` tool resolves it.
        state.agent_names = session.agent_names();
        state.cwd = cwd.to_path_buf();
        state.git_branches = rt.block_on(cox_tools::git::branches(cwd));
        state.worktree = cli.worktree.clone();
        // A113: a resumed session's title, including a rename the app wrote
        // straight to the store; a `TitleSet` later replaces it.
        state.title = Store::open(&home)
            .and_then(|store| store.session_info(&session.id()))
            .ok()
            .and_then(|info| info.title);
        state.sessions = project_sessions(&home, cwd);
        state.past_prompts = project_prompts(&home, cwd, session.id());
        state.composer.set_vim(config.tui.vim);
        // T22.6/T24.2: `"auto"` and a named theme both query the terminal's
        // OSC 11 background once, before raw mode; `"light"`/`"dark"` are
        // an explicit choice and skip it, same as before T24.2 existed.
        let needs_background = !matches!(config.tui.theme.as_str(), "light" | "dark");
        let background_dark = needs_background
            .then(|| cox_tui::color::detect_dark(cox_tui::color::OSC11_TIMEOUT))
            .flatten();
        let catalog = cox_tui::theme::catalog(&themes_dir);
        let resolved = cox_tui::theme::resolve(&config.tui.theme, background_dark, &catalog);
        state.dark = resolved.dark;
        state.glyphs = cox_tui::glyph::resolve(&config.tui);
        state.depth = cox_tui::color::resolve(&config.tui);
        // T23.1: the env heuristic only, not `query()` — a live `CSI ?u`
        // round trip needs exclusive use of stdin for its reply, and
        // `app.rs`'s own input thread starts reading it moments later; the
        // two racing is exactly how `doctor` (which owns the terminal
        // outright and prints the query's own verdict) gets away with
        // `query()` and an interactive session should not risk it. A
        // heuristic miss is what `[tui.caps]` (surfaced by `doctor`) is for.
        let env_fn = |key: &str| std::env::var(key).ok();
        let mut caps = cox_tui::term::Caps::detect(&env_fn);
        caps.apply(&config.tui.caps);
        state.caps = caps;
        // `NO_COLOR` (T24.1) wins over whatever `tui.theme` picked: every
        // token resets so only `Modifier::BOLD`/`DIM` carry hierarchy.
        state.theme = match state.depth {
            cox_tui::color::Depth::None => cox_tui::theme::Theme::mono(),
            _ => resolved.theme,
        };
        if let Some(warning) = resolved.warning {
            state.transcript.push(cox_tui::state::Cell::Notice {
                level: cox_protocol::types::Level::Warn,
                text: warning,
            });
        }
        // T24.2 step 4: `.tmTheme` files merge into the bundled syntect set
        // once per process, like the theme name leak below.
        cox_tui::markdown::load_user_themes(&themes_dir);
        let syntax_names: Vec<&'static str> = cox_tui::theme::tm_theme_names(&themes_dir)
            .into_iter()
            .map(|name| -> &'static str { String::leak(name) })
            .collect();
        // The theme name outlives every render; one leak per process buys a
        // `Copy` `Look` instead of a clone on each line. A theme file's own
        // `syntax` wins over `tui.syntax_theme` when it names one.
        let syntax_theme_cfg = resolved
            .syntax
            .unwrap_or_else(|| config.tui.syntax_theme.clone());
        state.syntax_theme = String::leak(syntax_theme_cfg);
        if !state.syntax_theme.is_empty()
            && cox_tui::markdown::theme_name(state.dark, state.syntax_theme) != state.syntax_theme
        {
            state.transcript.push(cox_tui::state::Cell::Notice {
                level: cox_protocol::types::Level::Warn,
                text: format!(
                    "unknown tui.syntax_theme {:?}; using the default. Available: {}",
                    config.tui.syntax_theme,
                    cox_tui::markdown::themes().join(", ")
                ),
            });
        }
        // `/theme` (T24.2 step 3): built-ins and user files, then every
        // `.tmTheme` under a `syntax: ` row.
        state.theme_rows = catalog
            .iter()
            .map(|(name, _)| name.clone())
            .chain(syntax_names.iter().map(|name| format!("syntax: {name}")))
            .collect();
        state.theme_catalog = catalog;
        state.syntax_names = syntax_names;
        state.show_thinking = config.tui.show_thinking == "full";
        state.diff_mode = cox_tui::diff::Mode::parse(&config.tui.diff);
        state.still = config.tui.motion == "reduced";
        state.notify = cox_tui::state::Notify::parse(&config.tui.notify);
        // T22.4: the only switch for mouse capture is this config key.
        state.mouse = config.tui.mouse;
        state.marks = cli.verbose > 0;
        // T33.8, PL§3: one `Modal::PluginGrant` per `NeedsApproval` plugin,
        // queued in `pending_grants` since the TUI has one modal slot. A
        // fresh `Store::open` here, like the poll task's own reads below —
        // `open()` already moved its store into `session`. A read error
        // (a locked or missing file) leaves the queue empty rather than
        // failing the whole session open (D14: fail open on extensions).
        let mut pending_grants: VecDeque<_> = Store::open(&home)
            .map(|gs| plugin_grant_requests(config, &home, cwd, &gs))
            .unwrap_or_default()
            .into();
        state.modal = pending_grants
            .pop_front()
            .map(cox_tui::state::Modal::PluginGrant);
        state.pending_grants = pending_grants;
        let status_tx = start_status_line(&rt, config, cwd, &mut state, &feed);
        // T46.4: 4, not 1 — a status-line ask must not crowd out a
        // `Ctrl+G` that arrives while the poll below is inside `git status`.
        let (ask, mut ask_rx) = tokio::sync::mpsc::channel(4);
        let (grant_tx, mut grant_rx) =
            tokio::sync::mpsc::channel::<cox_tui::state::GrantDecision>(4);
        // The poller lives here, not in cox-tui: the TUI never touches the disk.
        let poll = {
            let home = home.clone();
            let project = project.clone();
            let me = session.id();
            let git = config.tui.git;
            let dir = cwd.to_path_buf();
            rt.spawn(async move {
                let mut every = tokio::time::interval(std::time::Duration::from_secs(2));
                loop {
                    tokio::select! {
                        _ = every.tick() => {
                            let now = cox_ext::presence::now_secs();
                            let agents = cox_ext::presence::others(&home, &project, &me, now);
                            if feed.send(Msg::Agents(agents)).await.is_err() {
                                break;
                            }
                            if !git {
                                continue;
                            }
                            // T15.2: the same poll carries the branch and counts.
                            let status = cox_tools::git::status(&dir).await.map(|s| GitStatus {
                                branch: s.branch,
                                added: s.added,
                                removed: s.removed,
                            });
                            if feed.send(Msg::Git(status)).await.is_err() {
                                break;
                            }
                        }
                        // T15.3: `Ctrl+G` asks for the diff; the answer rides the feed.
                        ask = ask_rx.recv() => match ask {
                            Some(Ask::GitDiff) => {
                                let diff = cox_tools::git::diff(&dir).await;
                                if feed.send(Msg::Diff(diff)).await.is_err() {
                                    break;
                                }
                            }
                            // T27.5: `Enter` on an `/agents` sibling-session
                            // row asks for that session's rollout, the same
                            // read `crates/cox/src/resume.rs` does for
                            // `--resume`; a read error (store missing, id
                            // stale) answers empty rather than killing the
                            // poll loop the rest of `/agents` still needs.
                            Some(Ask::Rollout(id)) => {
                                let events = Store::open(&home)
                                    .and_then(|store| store.rollout_read(&id))
                                    .unwrap_or_default();
                                if feed.send(Msg::Rollout(events)).await.is_err() {
                                    break;
                                }
                            }
                            // T46.4: the runner debounces; a full channel
                            // only means a newer input is already queued.
                            Some(Ask::StatusLine { input, columns }) => {
                                if let Some(tx) = &status_tx {
                                    let input = status_line::input(input, me, &dir, &project);
                                    let _ = tx.try_send((input, columns));
                                }
                            }
                            // T46.7: never fatal; a refused stem or a failed
                            // write is a warning and the old theme stays.
                            Some(Ask::SaveTheme { stem, dark, tokens }) => {
                                let msg = match save_theme(&home.join("themes"), &stem, dark, &tokens) {
                                    Ok(file) => Msg::ThemeSaved(stem, file),
                                    Err(e) => Msg::Event(Event::Notice {
                                        level: Level::Warn,
                                        text: format!("theme {stem:?} not saved: {e}"),
                                    }),
                                };
                                if feed.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        },
                        // T33.8: `Modal::PluginGrant`'s `y`, written here —
                        // the one place in this surface that opens the
                        // store, same reasoning as `Ask::Rollout` above.
                        // Best-effort: a write that fails (a locked or
                        // full store) leaves the plugin ungranted, same as
                        // any other `plugin_notices` warning.
                        Some(decision) = grant_rx.recv() => {
                            if let Ok(gs) = Store::open(&home) {
                                let _ = write_plugin_grant(&gs, decision);
                            }
                        }
                    }
                }
            })
        };
        if first {
            if let Some(prompt) = cli.prompt.clone().filter(|p| !p.is_empty()) {
                let starter = session.clone();
                rt.spawn(async move {
                    let _ = starter
                        .submit(Submission::UserTurn {
                            text: prompt,
                            attachments: vec![],
                            confirm_think: false,
                        })
                        .await;
                });
            }
            first = false;
        }
        let quit = session.clone();
        let outcome = rt.block_on(cox_tui::app::run(
            session,
            state,
            feed_rx,
            ask,
            persist_tx.clone(),
            grant_tx,
            plugin_tx,
            plugin_mgmt_tx,
            dictation,
        ))?;
        poll.abort();
        // `/handoff`'s summary is the parent's `compact` call, so it runs
        // while the parent still has its provider and ledger.
        let summary = match &outcome {
            cox_tui::app::TuiOutcome::Handoff { objective } => {
                rt.block_on(quit.handoff_summary(objective))
            }
            _ => None,
        };
        // The TUI never shut the core down, so `SessionEnd` hooks and the
        // presence record outlived the window (T16.2).
        rt.block_on(quit.submit(Submission::Shutdown))?;
        // T34.11: the same kill headless `run` does before it exits (see
        // its comment there): a detached `bash` still running would
        // otherwise outlive this session — and, at quit, cox itself as an
        // orphan. Every outcome leaves this session, so every one reaps it;
        // `end`, not `interrupt`, so a shell detached in an older turn is
        // reached too (T38.2).
        quit.end();
        rt.block_on(quit.wait_tasks_cleared(crate::run::SHELL_CANCEL_GRACE));
        let parent = quit.id();
        let (child, what) = match outcome {
            cox_tui::app::TuiOutcome::Clear => continue,
            cox_tui::app::TuiOutcome::Quit => break,
            cox_tui::app::TuiOutcome::Fork { turn } => {
                let at = turn.map_or_else(|| "the latest turn".into(), |t| format!("T{t}"));
                (fork(&home, cwd, parent, turn), format!("fork at {at}"))
            }
            cox_tui::app::TuiOutcome::Handoff { objective } => {
                let child = handoff(&home, cwd, parent, &objective, summary.as_deref());
                let what = match summary {
                    Some(_) => "handoff".to_string(),
                    None => "handoff (no summary: the summariser returned nothing)".into(),
                };
                (child, what)
            }
        };
        // Fail open: a child that cannot be built puts the user back in the
        // parent rather than ending the program.
        (resume_spec, announce) = match child {
            Ok((id, history)) => (
                Some((id, history)),
                Some((
                    Level::Info,
                    format!("{what}: session {id}, child of {parent}"),
                )),
            ),
            Err(e) => (
                resume::from_home(&home, &parent.to_string())
                    .ok()
                    .map(|history| (parent, history)),
                Some((
                    Level::Warn,
                    format!("{what} failed: {e}; still in {parent}"),
                )),
            ),
        };
    }
    if cli.worktree.is_some() {
        offer_worktree_removal(&rt, cwd);
    }
    // T34.11: `rt`'s own `Drop` waits for every blocking task, including a
    // shell `wait_tasks_cleared` gave up on, which hung quit for as long as
    // that shell ran; same reasoning as `run`'s `shutdown_background`.
    rt.shutdown_background();
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use cox_session::add_read_root;
    #[cfg(feature = "plugins")]
    use cox_session::testing::{Recorder, install_granted, plugin_wat, user_turn};
    #[cfg(feature = "plugins")]
    use cox_session::{load_plugins, start_plugins};

    use super::*;

    /// T46.4: without `tui.status_line.command` nothing starts and the TUI
    /// draws no row; with one, on a host that can sandbox it, the row is on.
    #[test]
    fn status_line_starts_only_with_a_command() {
        use cox_protocol::types::{PermissionMode, SandboxMode};

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (feed, _feed_rx) = tokio::sync::mpsc::channel(4);
        let cwd = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        assert!(start_status_line(&rt, &config, cwd.path(), &mut state, &feed).is_none());
        assert!(state.status_script.is_none());

        config.tui.status_line.command = "echo hi".into();
        let sandboxed = cox_tools::sandbox::backend(config.sandbox.linux_backend).is_some();
        let tx = start_status_line(&rt, &config, cwd.path(), &mut state, &feed);
        assert_eq!(tx.is_some(), sandboxed);
        assert_eq!(
            state.status_script.as_ref().map(|s| s.enabled),
            sandboxed.then_some(true)
        );
        if !sandboxed {
            assert!(
                matches!(state.transcript.last(), Some(cox_tui::state::Cell::Notice { level: Level::Warn, text }) if text.contains("tui.status_line")),
                "one warning instead of a row"
            );
        }
    }

    /// T27.3: `--worktree t9` from a repository puts the session in
    /// `_worktrees/<repo>-t9` with the main checkout as its second root.
    #[test]
    fn worktree_flag_sets_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(repo) = git_repo(tmp.path()) else {
            return; // no usable git here
        };
        // A scratch home: `enter_worktree` reads (and sweeps) presence records.
        let home = tmp.path().join("home");
        let home_arg = home.display().to_string();
        let mut cli = Cli::parse_from(["cox", "--worktree", "T9", "--home", &home_arg]);
        let cwd = enter_worktree(&mut cli, &repo).expect("enter");
        let root = std::fs::canonicalize(tmp.path()).expect("canon");
        assert_eq!(cwd, root.join("_worktrees").join("repo-t9"));
        assert!(cwd.join("a.txt").is_file(), "the worktree is checked out");
        let mut loaded = config_load::load(&cwd, &cli).expect("load");
        assert_eq!(loaded.config.core.workspace_roots, vec![cwd.clone()]);
        assert!(cli.add_dir.is_empty(), "the main checkout is not writable");
        let project = rt_project_root(&cwd);
        assert_eq!(project, repo);
        add_read_root(&mut loaded.config, &project);
        assert_eq!(loaded.config.core.workspace_roots, vec![cwd.clone(), repo]);
        assert_eq!(cli.cwd.as_deref(), Some(cwd.as_path()));
    }

    /// A repository with one commit at `<tmp>/repo`, canonical; `None`
    /// where no usable git is installed.
    fn git_repo(tmp: &Path) -> Option<PathBuf> {
        let repo = tmp.join("repo");
        std::fs::create_dir(&repo).expect("mkdir");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q", "--initial-branch=trunk"]) {
            return None;
        }
        std::fs::write(repo.join("a.txt"), "a\n").expect("write");
        assert!(git(&["add", "a.txt"]));
        assert!(git(&[
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "first"
        ]));
        Some(std::fs::canonicalize(&repo).expect("canon"))
    }

    /// A stored session that ran in `cwd`, as `session_create` records it.
    fn recorded_session(home: &Path, cwd: &Path) -> String {
        let store = Store::open(home).expect("store");
        let id = SessionId::new();
        store
            .session_create(&cox_protocol::SessionRow {
                id,
                created_at: String::new(),
                cwd: cwd.to_path_buf(),
                project_slug: "repo".into(),
                title: None,
                parent_id: None,
                rollout_path: home.join(format!("{id}.jsonl")),
            })
            .expect("session row");
        id.to_string()
    }

    /// `cox --worktree <name> --home <home>` from `repo`: the worktree path.
    fn made_worktree(repo: &Path, home: &str, name: &str) -> PathBuf {
        let mut cli = Cli::parse_from(["cox", "--worktree", name, "--home", home]);
        enter_worktree(&mut cli, repo).expect("enter")
    }

    /// T44.4: resuming a worktree session moves the run into that worktree
    /// the way `--worktree` does; a main-checkout session stays put.
    #[test]
    fn resume_uses_recorded_worktree_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(repo) = git_repo(tmp.path()) else {
            return; // no usable git here
        };
        let home = tmp.path().join("home");
        let home_arg = home.display().to_string();
        let wt = made_worktree(&repo, &home_arg, "t9");
        let id = recorded_session(&home, &wt);
        let mut cli = Cli::parse_from(["cox", "--resume", &id, "--home", &home_arg]);
        let cwd = resume_worktree(&mut cli, &repo).expect("resume");
        assert_eq!(cwd, wt);
        assert_eq!(cli.cwd.as_deref(), Some(wt.as_path()));
        // What makes `open` add the main checkout as a read root and put
        // the worktree in the presence record.
        assert_eq!(cli.worktree.as_deref(), Some("t9"));
        let main_id = recorded_session(&home, &repo);
        let mut cli = Cli::parse_from(["cox", "--resume", &main_id, "--home", &home_arg]);
        assert_eq!(resume_worktree(&mut cli, &repo).expect("resume"), repo);
        assert!(cli.worktree.is_none() && cli.cwd.is_none());
    }

    /// T44.4: a worktree that is gone is refused with the way back, and
    /// resume never recreates it.
    #[test]
    fn resume_refuses_missing_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(repo) = git_repo(tmp.path()) else {
            return; // no usable git here
        };
        let home = tmp.path().join("home");
        let home_arg = home.display().to_string();
        let wt = made_worktree(&repo, &home_arg, "t9");
        let id = recorded_session(&home, &wt);
        std::fs::remove_dir_all(&wt).expect("remove the worktree");
        let mut cli = Cli::parse_from(["cox", "--resume", &id, "--home", &home_arg]);
        let err = resume_worktree(&mut cli, &repo)
            .expect_err("a missing worktree is refused")
            .to_string();
        assert!(err.contains("git worktree list"), "{err}");
        assert!(err.contains("--worktree t9"), "{err}");
        assert!(!wt.exists(), "resume never runs `git worktree add`");
        assert!(cli.cwd.is_none());
    }

    fn rt_project_root(cwd: &Path) -> PathBuf {
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(project_root(cwd))
    }

    #[cfg(feature = "plugins")]
    fn rollout_json(store: &Store, session: &Session) -> String {
        let events = store.rollout_read(&session.id()).expect("rollout");
        serde_json::to_string(&events).expect("rollout json")
    }

    /// Two turns over a granted `item:assistant_message` renderer that
    /// always paints `PAINTED`. With `tui`, a TUI `State` consumes turn one
    /// and its render request is served by the real host before turn two.
    /// Asserts the render left the rollout byte-identical; returns every
    /// `Request` as JSON, the final rollout and the first reply as drawn.
    #[cfg(feature = "plugins")]
    async fn render_run(work: &Path, tui: bool) -> (Vec<String>, String, String) {
        use cox_protocol::types::{PermissionMode, SandboxMode};
        use cox_tui::state::{Cell, Cmd, update};

        let home = tempfile::tempdir().expect("home");
        let init = r#"{"renderers":["item:assistant_message"]}"#;
        let widget = r#"{"text":[[{"text":"PAINTED"}]]}"#;
        let base = plugin_wat("", init, "");
        let wat = format!(
            r#"{} (data (i32.const 1536) "{}")
              (func (export "cox_render_item") (result i32)
                (call $out (i32.const 1536) (i32.const {})) (i32.const 0)))"#,
            base.trim_end().strip_suffix(')').expect("module"),
            widget.replace('"', "\\\""),
            widget.len()
        );
        let caps = "[capabilities]\nui = { render = [\"item:assistant_message\"] }\n";
        install_granted(home.path(), "look", caps, &wat);
        let scenario = "[[turn]]\ntext = \"reply 0\"\n[[turn]]\ntext = \"reply 1\"\n";
        let recorder = Arc::new(Recorder {
            inner: cox_provider::scripted::Scripted::from_toml(scenario, "").expect("scenario"),
            sent: std::sync::Mutex::default(),
        });
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let config = Config::default();
        let session = Session::new(
            config.clone(),
            recorder.clone(),
            vec![],
            store.clone(),
            store.clone(),
            work.to_path_buf(),
        )
        .expect("session");
        let plugins = load_plugins(&config, home.path(), work, store.clone(), None);
        let (req_tx, req_rx) = tokio::sync::mpsc::channel(8);
        let (feed_tx, mut feed_rx) = tokio::sync::mpsc::channel(8);
        let ui = tui.then(|| PluginUi {
            feed: feed_tx,
            requests: req_rx,
        });
        start_plugins(&session, plugins.live, &config.plugins, work, serve_ui(ui));
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        if tui {
            let declare = feed_rx.recv().await.expect("declare");
            update(&mut state, declare);
        }
        user_turn(&session, "one").await;
        let before = rollout_json(&store, &session);
        let mut drawn = String::new();
        if tui {
            let events = store.rollout_read(&session.id()).expect("rollout");
            let asks: Vec<_> = events
                .into_iter()
                .flat_map(|ev| update(&mut state, Msg::Event(ev)))
                .filter_map(|cmd| match cmd {
                    Cmd::Plugin(request) => Some(request),
                    _ => None,
                })
                .collect();
            assert_eq!(asks.len(), 1, "{asks:?}");
            for request in asks {
                req_tx.send(request).await.expect("request");
                let answer = feed_rx.recv().await.expect("answer");
                update(&mut state, answer);
            }
            let look = state.look(40);
            let cell = state
                .transcript
                .iter()
                .find(|c| matches!(c, Cell::Assistant { .. }))
                .expect("reply cell");
            drawn = cox_tui::cells::cell_lines(cell, &look)
                .iter()
                .map(ToString::to_string)
                .collect();
        }
        let after = rollout_json(&store, &session);
        assert_eq!(before, after, "the render wrote nothing to the rollout");
        user_turn(&session, "two").await;
        let requests = recorder
            .sent
            .lock()
            .expect("sent")
            .iter()
            .map(|r| serde_json::to_string(r).expect("request json"))
            .collect();
        (requests, rollout_json(&store, &session), drawn)
    }

    /// T33.26, PL§8: renderer output is display-only.
    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn renderer_output_never_reaches_rollout_or_model() {
        let work = tempfile::tempdir().expect("work");
        let (with, with_rollout, drawn) = render_run(work.path(), true).await;
        let (without, _, plain) = render_run(work.path(), false).await;
        // The renderer really ran: the reply cell draws the plugin's widget.
        assert_eq!(drawn, "PAINTED");
        assert!(plain.is_empty());
        // Every `Request`, the second turn's included, is byte-identical.
        assert_eq!(with.len(), 2);
        assert_eq!(with, without);
        assert!(!with_rollout.contains("PAINTED"));
        assert!(with.iter().all(|r| !r.contains("PAINTED")));
    }

    /// T46.7: the stem is the only guard on where the editor writes, so a
    /// separator, a `..` or an absolute path is refused and nothing lands
    /// outside `<home>/themes`.
    #[test]
    fn save_theme_rejects_a_path_stem() {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("themes");
        let tokens = vec![("accent".to_string(), "#ff0000".to_string())];
        for stem in ["../evil", "a/b", "/abs", "a..b", "", "Upper", ".hidden"] {
            assert!(
                save_theme(&dir, stem, true, &tokens).is_err(),
                "{stem:?} was accepted"
            );
        }
        assert!(!home.path().join("evil.toml").exists());
        assert!(!dir.exists(), "a refused stem created nothing");
    }

    /// T46.7: a `-custom` stem starts from its built-in's own text, and the
    /// written file parses to what the TUI is told it holds.
    #[test]
    fn save_theme_writes_a_custom_copy_of_a_builtin() {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("themes");
        let tokens = vec![("accent".to_string(), "#ff0000".to_string())];
        let file = save_theme(&dir, "cox-dark-custom", true, &tokens).expect("saved");
        let written = std::fs::read_to_string(dir.join("cox-dark-custom.toml")).expect("written");
        assert_eq!(
            cox_tui::theme::parse_theme_file(&written).expect("parses"),
            file
        );
        assert_eq!(file.dark.accent, cox_tui::theme::parse_color("#ff0000"));
    }

    /// T54.7: `[voice] enabled = false` hands the TUI no dictation and no
    /// warning; enabled without the model, one warning naming the download;
    /// with the model on disk, a dictation (no microphone opens until the
    /// key). A non-default `[voice] key` rebinds the action.
    #[cfg(feature = "voice")]
    #[test]
    fn voice_disabled_passes_no_dictation() {
        use cox_tui::state::Cell;

        let home = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        let fresh = || {
            State::new(
                cox_protocol::types::PermissionMode::Default,
                cox_protocol::types::SandboxMode::WorkspaceWrite,
            )
        };
        let mut state = fresh();
        assert!(voice(&mut state, &config, home.path()).is_none());
        assert!(state.transcript.is_empty(), "off is not a warning");

        config.voice.enabled = true;
        assert!(voice(&mut state, &config, home.path()).is_none());
        assert!(matches!(
            state.transcript.as_slice(),
            [Cell::Notice { level: Level::Warn, text }]
                if text.contains("cox voice model download base.en")
        ));

        let model = crate::voice_cmd::model_path(home.path(), "base.en").expect("pinned");
        std::fs::create_dir_all(model.parent().expect("dir")).expect("mkdir");
        std::fs::write(&model, b"ggml").expect("model");
        config.voice.key = "f5".into();
        let mut state = fresh();
        assert!(voice(&mut state, &config, home.path()).is_some());
        assert!(state.transcript.is_empty());
        let f5 = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(5),
            crossterm::event::KeyModifiers::NONE,
        );
        assert_eq!(
            state.keymap.resolve(f5, cox_tui::commands::Context::Idle),
            Some(cox_tui::keymap::Action::Voice)
        );
    }
}
