// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The plugin side of session assembly (T33.6, T33.12, T33.19, T33.44):
//! the grant check that decides which plugins load, their tools, `[[mcp]]`
//! servers, `[[provider]]` sections and external agents, and starting them
//! once the session exists. Separate from `open` because the ACP surface
//! and `doctor` read the same grant verdicts without building a session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_protocol::config::{CompatibleProviderConfig, McpServerConfig};
use cox_protocol::traits::Hook;
use cox_protocol::types::Level;
use cox_protocol::{Config, GrantScope, PluginGrant};
use cox_store::Store;

#[cfg(feature = "plugins")]
use crate::sandbox::sandboxed_argv;
#[cfg(feature = "plugins")]
use {crate::ServeUi, cox_core::Session, cox_protocol::ids::SessionId, cox_protocol::traits::Tool};

/// What the grant check let into a session (T33.6, T33.19).
#[derive(Default)]
pub struct Plugins {
    /// One warning per plugin or plugin server that did not load.
    pub notices: Vec<String>,
    /// Each loaded plugin's `[[mcp]]` servers by plugin id, stdio commands
    /// already sandboxed, for `cox_mcp::discovery::add_plugin`.
    pub mcp: Vec<(String, Vec<(String, McpServerConfig)>)>,
    /// New `providers.custom` sections from each granted plugin's
    /// declarative `[[provider]]` rows (`docs/design/plugins.md` §7a,
    /// T33.17): `api = "chat" | "responses"` only, already checked against
    /// the live config by `cox_plugin::provider::merge` (a name the config
    /// already uses is skipped with a notice, not returned here). The
    /// caller extends `config.providers.custom` with these before building
    /// the tier's provider.
    pub providers: HashMap<String, CompatibleProviderConfig>,
    /// Each granted plugin's `[[external_agents]]` entries (EA§2, T35.2),
    /// resolved like a `[[mcp]]` stdio server and already wrapped by
    /// `sandboxed_argv`: a driver spawns `ExternalAgentCommand::command`.
    /// Never an ungranted plugin's; empty without `writable`.
    #[cfg(feature = "plugins")]
    pub external_agents: Vec<cox_plugin::external_agent::ExternalAgentCommand>,
    /// Each granted plugin's `[[agents]]` definitions by plugin id (T45.4),
    /// for `cox_ext::agents::merge`; never an ungranted plugin's.
    pub agent_defs: Vec<(String, Vec<cox_protocol::agent::AgentDef>)>,
    /// Each loaded plugin's `[[models]]` rows by plugin id (T33.44).
    pub models: Vec<(String, Vec<cox_protocol::plugin::ModelDecl>)>,
    /// The loaded instances, compiled under their grants; `start_plugins`
    /// runs their `cox_init` once the session exists (T33.44).
    #[cfg(feature = "plugins")]
    pub live: cox_plugin::LivePlugins,
}

impl Plugins {
    /// `models` in the shape `Catalog::load` takes.
    pub fn catalog_rows(&self) -> Vec<cox_models::PluginModels<'_>> {
        self.models
            .iter()
            .map(|(plugin, models)| cox_models::PluginModels { plugin, models })
            .collect()
    }
}

/// The warnings of `load_plugins`, for a surface with no MCP servers (ACP).
pub fn plugin_notices(
    config: &Config,
    home: &Path,
    cwd: &Path,
    store: Arc<dyn cox_protocol::PluginStore>,
) -> Vec<String> {
    load_plugins(config, home, cwd, store, None).notices
}

/// T33.6 (PL§3): discovers plugins, checks each against its grant and
/// loads only the `Granted` ones. Warns once per plugin that did not load,
/// naming the command to run — headless and ACP never approve, matching
/// headless approval with no approver (`crates/cox/src/run.rs`). Empty when
/// `plugins.enabled` is off. A store read error counts as "no grant": it
/// must never let an unapproved plugin load. With `writable` (the roots a
/// server may write), a loaded plugin's `[[mcp]]` servers come back too.
#[cfg(feature = "plugins")]
pub fn load_plugins(
    config: &Config,
    home: &Path,
    cwd: &Path,
    store: Arc<dyn cox_protocol::PluginStore>,
    writable: Option<&[PathBuf]>,
) -> Plugins {
    use cox_plugin::discover::{self, State};
    use cox_plugin::grant::{self, Verdict};

    let mut out = Plugins::default();
    if !config.plugins.enabled {
        return out;
    }
    let root = cox_config::load::find_git_root(cwd);
    let found = discover::discover(home, root.as_deref());
    let notices = &mut out.notices;
    notices.extend(found.notices);
    // T33.17: each granted, successfully loaded plugin's `[[provider]]`
    // rows, collected alongside `[[mcp]]` below and merged into
    // `providers.custom` once every plugin has been checked (PL§7a).
    let mut plugin_providers: Vec<cox_plugin::provider::PluginProviders<'_>> = Vec::new();
    for p in &found.plugins {
        let id = &p.id;
        let (manifest, digest) = match &p.state {
            State::Loaded { manifest, digest } => (manifest, digest),
            State::Skipped { reason } => {
                notices.push(format!("plugin {id} skipped: {reason}"));
                continue;
            }
        };
        // A linked plugin's grant lives under the fixed `grant_digest()` key
        // (T33.41), never the live content `digest`, which changes on every
        // rebuild — looking it up under `digest` here would silently
        // re-ask on every session open for a plugin already granted.
        let grant_digest = p.grant_digest().unwrap_or_else(|| digest.to_string());
        let stored = grant::scope(p.source, root.as_deref())
            .and_then(|scope| store.grant_get(id, &scope, &grant_digest).ok().flatten());
        let enable = grant::enable_command(id, p.source);
        match grant::check(manifest, digest, stored.as_ref()) {
            Verdict::Granted => {
                // PL§7d (T33.14.1): `cox_http` to a provider host would skip
                // the ledger and the budget, so such a plugin stays out.
                if let Err(e) = cox_plugin::net::refuse_provider_hosts(manifest, &config.providers)
                {
                    notices.push(format!("plugin {id} is not loaded: {e}"));
                    continue;
                }
                // T33.44: compiled once under its real grant and kept;
                // `start_plugins` runs `cox_init`. A failure is a visible
                // warning, never fatal (D14). No `wasm` (PL§13/§14,
                // T33.38): `validate()` already refused anything but an
                // `[[mcp]]`-only package here, so there is no extism
                // instance to compile — its `[[mcp]]` servers below are
                // registered exactly like a wasm plugin's, same sandbox
                // spawn and same grant check, just with nothing in `live`.
                let loaded = match &manifest.wasm {
                    Some(wasm_path) => std::fs::read(p.dir.join(wasm_path))
                        .map_err(|e| e.to_string())
                        .and_then(|wasm| {
                            out.live
                                .load(manifest, &wasm, store.clone())
                                .map_err(|e| e.to_string())
                        }),
                    None => Ok(()),
                };
                if loaded.is_ok() && !manifest.provider.is_empty() {
                    plugin_providers.push(cox_plugin::provider::PluginProviders {
                        plugin: id.as_str(),
                        decls: &manifest.provider,
                    });
                }
                if loaded.is_ok() && !manifest.models.is_empty() {
                    out.models.push((id.clone(), manifest.models.clone()));
                }
                if loaded.is_ok() && !manifest.agents.is_empty() {
                    let defs = plugin_agent_defs(id, &p.dir, manifest, notices);
                    out.agent_defs.push((id.clone(), defs));
                }
                match (loaded, writable) {
                    (Err(e), _) => notices.push(format!("plugin {id} failed to load: {e}")),
                    (Ok(_), Some(writable)) => {
                        if !manifest.mcp.is_empty() {
                            let servers =
                                plugin_mcp(id, &p.dir, manifest, config, writable, notices);
                            out.mcp.push((id.clone(), servers));
                        }
                        let agents = plugin_agents(id, &p.dir, manifest, config, writable, notices);
                        out.external_agents.extend(agents);
                    }
                    (Ok(_), None) => {}
                }
            }
            Verdict::Disabled => {
                notices.push(format!(
                    "plugin {id} is disabled; run `{enable}` to load it"
                ));
            }
            Verdict::NeedsApproval { added, .. } => {
                let asks = if added.is_empty() {
                    String::from("its package changed")
                } else {
                    format!("it asks for {}", added.join(", "))
                };
                notices.push(format!(
                    "plugin {id} is not loaded: {asks} and needs approval; run `{enable}`"
                ));
            }
        }
    }
    let merged = cox_plugin::provider::merge(&config.providers, &plugin_providers);
    out.notices.extend(merged.warnings);
    out.providers = merged.custom;
    // PL§7b: a plugin row that would change a configured or built-in one
    // is ignored; the catalog says which, and the session shows it.
    if !out.models.is_empty()
        && let Ok(catalog) = cox_models::Catalog::load(config, &out.catalog_rows(), None)
    {
        out.notices.extend(catalog.warnings().iter().cloned());
    }
    out
}

/// T33.44 (PL§3–§6): runs each loaded plugin's `cox_init` once for
/// `session` and sets the event tap, which owns the instances from then on.
/// Returns the plugins' hook sources for `HookChain` and the notices to
/// emit now: init failures (that plugin is skipped) and what `cox_init`
/// queued. Later notices (`cox_notify` from a hook, `Effects.notices`) are
/// drained by the tap after each event and emitted by a task, because
/// `EventTap::offer` runs inside `Session::emit` and must not emit itself.
/// `start_plugins`'s answer: the plugins' hook sources for `HookChain`
/// and the notices to emit now.
pub type Started = (Vec<(String, Arc<dyn Hook>)>, Vec<(Level, String)>);

#[cfg(feature = "plugins")]
pub fn start_plugins(
    session: &Session,
    mut live: cox_plugin::LivePlugins,
    config: &cox_protocol::config::PluginsConfig,
    cwd: &Path,
    ui: Option<ServeUi>,
) -> Started {
    let mut notices = Vec::new();
    // T33.15, T33.13: plugins reach the session's router and tool path only
    // through a `Weak`; the notice task below holds the one strong handle,
    // which both coerce from, so both stay reachable while it lives.
    let strong = Arc::new(session.clone());
    let caller: Arc<dyn cox_protocol::traits::ModelCaller> = strong.clone();
    let invoker: Arc<dyn cox_protocol::traits::ToolInvoker> = strong;
    live.bind_model_caller(&caller);
    live.bind_tool_invoker(&invoker);
    if !live.plugins().is_empty() {
        let started = live.start(config, session.id(), cwd);
        notices.extend(started.into_iter().map(|w| (Level::Warn, w)));
        // Taken before the tap is set, so the caller emits them in order
        // instead of the tap's own drain racing it.
        notices.extend(live.take_notices());
    }
    // Non-TUI surfaces have no slots to redraw.
    let redraw = match ui {
        Some(serve) => serve(&live),
        None => Arc::new(|_: &str| {}),
    };
    if live.plugins().is_empty() {
        return (Vec::new(), notices);
    }
    let hooks = live.hooks();
    // T33.20: the session asks only the plugin `[plugins.decide]` names.
    session.set_advisors(live.advisors());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Level, String)>();
    let emitter = session.clone();
    tokio::spawn(async move {
        let _caller = caller;
        while let Some((level, text)) = rx.recv().await {
            if emitter.notice(level, text).await.is_err() {
                break;
            }
        }
    });
    let forward: cox_plugin::Notices = Arc::new(move |batch| {
        for notice in batch {
            let _ = tx.send(notice);
        }
    });
    let (tap, warnings) = live.into_tap(redraw, forward);
    notices.extend(warnings.into_iter().map(|w| (Level::Warn, w)));
    session.set_event_tap(Arc::new(tap));
    (hooks, notices)
}

/// T33.12 (PL§7): runs each loaded plugin's `cox_init` for session `id`
/// before the session is built, so its granted tools (`wasm__<id>__<tool>`,
/// always deferred, sorted by (id, tool)) are in the tool list from the
/// first request and the cache-stable prefix never changes under a running
/// session. `start_plugins` then finds them started (`LivePlugins::start`
/// skips a started plugin). Init failures and dropped tools become
/// `warnings`; the plugin notices `cox_init` queued stay queued for
/// `start_plugins`. `cox_model_call` from `cox_init` finds no session yet.
#[cfg(feature = "plugins")]
pub(crate) fn plugin_tools(
    live: &mut cox_plugin::LivePlugins,
    config: &cox_protocol::config::PluginsConfig,
    id: SessionId,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Vec<Arc<dyn Tool>> {
    warnings.extend(live.start(config, id, cwd));
    let (tools, dropped) = live.tools();
    warnings.extend(dropped);
    tools
}

/// The slim build has no plugin host, so there is never a plugin to load.
#[cfg(not(feature = "plugins"))]
pub fn load_plugins(
    _config: &Config,
    _home: &Path,
    _cwd: &Path,
    _store: Arc<dyn cox_protocol::PluginStore>,
    _writable: Option<&[PathBuf]>,
) -> Plugins {
    Plugins::default()
}

/// PL§7c: one plugin's `[[mcp]]` entries as servers `cox-mcp` can start. A
/// server that cannot be started safely is a warning and absent (D14).
#[cfg(feature = "plugins")]
fn plugin_mcp(
    id: &str,
    dir: &Path,
    manifest: &cox_plugin_api::PluginManifest,
    config: &Config,
    writable: &[PathBuf],
    notices: &mut Vec<String>,
) -> Vec<(String, McpServerConfig)> {
    let mut servers = Vec::new();
    for decl in &manifest.mcp {
        let server = match (&decl.command, &decl.url) {
            (Some(command), _) => cox_plugin::external_agent::package_program(dir, command)
                .map_err(|e| e.to_string())
                .and_then(|program| sandboxed_argv(&program, &decl.args, config, writable))
                .map(|mut argv| McpServerConfig {
                    command: Some(argv.remove(0)),
                    args: argv,
                    ..McpServerConfig::default()
                }),
            (None, Some(url)) => reqwest::Url::parse(url)
                .map_err(|e| format!("url {url:?}: {e}"))
                .and_then(|u| match u.host_str() {
                    Some(host) if manifest.capabilities.net_allows(host) => Ok(()),
                    host => Err(format!("url host {host:?} is not in capabilities.net")),
                })
                .map(|()| McpServerConfig {
                    url: Some(url.clone()),
                    ..McpServerConfig::default()
                }),
            (None, None) => Err(String::from("no command or url")),
        };
        match server {
            Ok(cfg) => servers.push((decl.name.clone(), cfg)),
            Err(why) => notices.push(format!(
                "plugin {id}: mcp server {} skipped: {why}",
                decl.name
            )),
        }
    }
    servers
}

/// EA§2 (T35.2): one granted plugin's `[[external_agents]]` entries, each
/// resolved like a `[[mcp]]` stdio server and wrapped by `sandboxed_argv`.
/// It is plugin-shipped code, so where the wrap is impossible the entry is
/// refused with a warning (wrap-or-refuse, as `plugin_mcp`), never run bare.
#[cfg(feature = "plugins")]
fn plugin_agents(
    id: &str,
    dir: &Path,
    manifest: &cox_plugin_api::PluginManifest,
    config: &Config,
    writable: &[PathBuf],
    notices: &mut Vec<String>,
) -> Vec<cox_plugin::external_agent::ExternalAgentCommand> {
    use cox_plugin::external_agent::{AgentSource, ExternalAgentCommand};

    // T52.2: an external agent always has network inside its sandbox.
    let wrap = |program: &Path, args: &[String]| {
        crate::sandbox::agent_argv(program, args, config, writable)
    };
    let mut agents = Vec::new();
    for decl in &manifest.external_agents {
        match ExternalAgentCommand::resolve(AgentSource::Plugin { id, dir }, decl, wrap) {
            Ok(agent) => agents.push(agent),
            Err(e) => notices.push(format!(
                "plugin {id}: external agent {} skipped: {e}",
                decl.name
            )),
        }
    }
    agents
}

/// T45.4: one granted plugin's `[[agents]]` files, parsed by the same
/// code as `.cox/agents/*.md`. A file that is missing, reached through a
/// symlink (the digest hashes regular files only), unparsable, or whose
/// frontmatter `name` is not the one the grant approved is a warning and
/// absent (D14).
#[cfg(feature = "plugins")]
fn plugin_agent_defs(
    id: &str,
    dir: &Path,
    manifest: &cox_plugin_api::PluginManifest,
    notices: &mut Vec<String>,
) -> Vec<cox_protocol::agent::AgentDef> {
    let mut defs = Vec::new();
    for decl in &manifest.agents {
        // `./` so a file at the package root is never taken for a bare PATH
        // name: `package_program` then walks it inside `dir`, refusing an
        // absolute path, `..` and any symlink on the way.
        let def = cox_plugin::external_agent::package_program(dir, &format!("./{}", decl.file))
            .map_err(|e| e.to_string())
            .and_then(|path| cox_ext::agents::parse_file(&path, notices))
            .and_then(|def| {
                if def.name == decl.name {
                    Ok(def)
                } else {
                    Err(format!(
                        "its frontmatter names {:?}, the grant approved {:?}",
                        def.name, decl.name
                    ))
                }
            });
        match def {
            Ok(def) => defs.push(def),
            Err(why) => notices.push(format!("plugin {id}: agent {} skipped: {why}", decl.name)),
        }
    }
    defs
}

/// Builds and writes one `PluginGrant` row (PL§3): the single place the row
/// is assembled, so `cox plugin enable`/`install` and the TUI's grant
/// dialog share it instead of each holding its own `grant_put` literal
/// (T33.8).
pub fn write_grant(
    store: &Store,
    id: &str,
    scope: &GrantScope,
    digest: &str,
    capabilities: Vec<String>,
    source: serde_json::Value,
) -> Result<(), cox_protocol::StoreError> {
    use cox_protocol::PluginStore as _;

    store.grant_put(&PluginGrant {
        plugin_id: id.to_string(),
        scope: scope.clone(),
        digest: digest.to_string(),
        capabilities: serde_json::json!(capabilities),
        enabled: true,
        source,
        decided_at: cox_store::now_rfc3339(),
    })
}

#[cfg(all(test, feature = "plugins"))]
mod tests {
    use super::*;
    use crate::provider::provider_for_served;
    use crate::testing::{Recorder, install_granted, plugin_wat, scripted_session, user_turn};
    use crate::tools::with_tool_search_index;
    use cox_protocol::traits::Store as _;
    use cox_protocol::types::Event;
    use cox_tools::tool_search::ToolSearchTool;

    #[cfg(feature = "plugins")]
    fn manifest_with_mcp(mcp: serde_json::Value, net: &[&str]) -> cox_plugin_api::PluginManifest {
        serde_json::from_value(serde_json::json!({
            "api": 1, "id": "gh", "version": "0.1.0", "name": "gh", "wasm": "plugin.wasm",
            "capabilities": { "net": net },
            "mcp": [mcp],
        }))
        .expect("manifest")
    }

    #[cfg(feature = "plugins")]
    #[test]
    fn changing_bundled_server_binary_changes_digest() {
        let pkg = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(pkg.path().join("bin")).expect("mkdir");
        std::fs::write(pkg.path().join("plugin.wasm"), b"\0asm").expect("wasm");
        std::fs::write(pkg.path().join("bin/server"), b"v1").expect("server");
        let before = cox_plugin::package_digest(pkg.path()).expect("digest");
        std::fs::write(pkg.path().join("bin/server"), b"v2").expect("server");
        let after = cox_plugin::package_digest(pkg.path()).expect("digest");
        assert_ne!(
            before, after,
            "a changed server binary must need a new grant"
        );
    }

    /// A package with one `[[external_agents]]` entry whose in-package
    /// program is `script`.
    #[cfg(all(feature = "plugins", unix))]
    fn agent_package(pkg: &Path, args: &[String], script: &str) -> cox_plugin_api::PluginManifest {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::create_dir_all(pkg.join("bin")).expect("mkdir");
        std::fs::write(pkg.join("bin/agent"), script).expect("agent");
        std::fs::set_permissions(
            pkg.join("bin/agent"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
        std::fs::write(
            pkg.join("plugin.wasm"),
            r#"(module (func (export "cox_init") (result i32) (i32.const 0)))"#,
        )
        .expect("wasm");
        let quoted: Vec<String> = args.iter().map(|a| format!("{a:?}")).collect();
        let toml = format!(
            "api = 1\nid = \"cur\"\nversion = \"0.1.0\"\nname = \"Cur\"\nwasm = \"plugin.wasm\"\n\n\
             [[external_agents]]\nname = \"cursor\"\ncommand = \"bin/agent\"\nargs = [{}]\n\
             mode = \"acp\"\nkey_env = \"CURSOR_API_KEY\"\n",
            quoted.join(", ")
        );
        std::fs::write(pkg.join("plugin.toml"), &toml).expect("manifest");
        cox_plugin::discover::load_manifest(pkg, &pkg.join("plugin.toml"), Some("cur"))
            .expect("valid manifest")
            .0
    }

    /// T35.2 Check (EA§2, PL§7c): the command a driver gets runs the
    /// sandbox launcher, not the agent, so the agent is under the same
    /// Seatbelt or bwrap profile as `bash`: a write inside the workspace
    /// lands, one under `$HOME` is denied. Where no argv backend exists the
    /// entry is refused with a warning, never run bare.
    #[cfg(all(feature = "plugins", unix))]
    #[test]
    fn external_agent_command_is_wrapped_by_sandbox_before_spawn() {
        use cox_tools::sandbox::{Backend, backend};

        let ws = tempfile::tempdir().expect("tempdir");
        // Inside the workspace: Linux bwrap gives the wrapped program a
        // private `/tmp`, where a package in its own temp dir would not exist.
        let pkg = tempfile::tempdir_in(ws.path()).expect("tempdir");
        let home = std::env::var("HOME").expect("HOME");
        let outside = format!("{home}/.cox-agent-escape-{}", std::process::id());
        let args = [ws.path().display().to_string(), outside.clone()];
        let script = "#!/bin/sh\necho in > \"$1/inside\"\necho x > \"$2\"\n";
        let manifest = agent_package(pkg.path(), &args, script);
        let mut config = Config::default();
        config.core.workspace_roots = vec![ws.path().to_path_buf()];
        let roots = config.core.workspace_roots.clone();
        let mut notices = Vec::new();
        let agents = plugin_agents("cur", pkg.path(), &manifest, &config, &roots, &mut notices);

        if !matches!(
            backend(cox_protocol::LinuxBackend::Auto),
            Some(Backend::Seatbelt | Backend::Bwrap)
        ) {
            assert!(agents.is_empty(), "ran without a sandbox");
            assert!(
                notices[0].contains("external agent cursor skipped"),
                "{notices:?}"
            );
            return;
        }
        assert!(notices.is_empty(), "{notices:?}");
        let program = pkg.path().join("bin/agent");
        let cmd = agents[0].command();
        assert_ne!(cmd.get_program(), program.as_os_str(), "spawned bare");
        assert!(cmd.get_args().any(|a| a == program.as_os_str()));
        let _ = agents[0].command().status().expect("spawns");
        let leaked = Path::new(&outside).exists();
        let _ = std::fs::remove_file(&outside);
        assert!(ws.path().join("inside").exists(), "the agent never ran");
        assert!(!leaked, "the sandbox let an external agent write {outside}");
    }

    /// T35.2 Check (matches T33.6's `headless_never_loads_ungranted_plugin`):
    /// an ungranted plugin's external agent is neither resolved nor run;
    /// the notice names its approval line. Granting the same bytes is what
    /// lets it through, so the refusal is the grant's.
    #[cfg(all(feature = "plugins", unix))]
    #[test]
    fn ungranted_external_agent_is_not_spawned() {
        use cox_protocol::{PluginGrant, PluginStore as _};

        let home = tempfile::tempdir().expect("tempdir");
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(repo.path().join(".git")).expect("git");
        let pkg = repo.path().join(".cox/plugins/cur");
        let manifest = agent_package(&pkg, &[], "#!/bin/sh\ntouch \"$0.ran\"\n");
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let mut config = Config::default();
        config.core.workspace_roots = vec![repo.path().to_path_buf()];
        let roots = config.core.workspace_roots.clone();

        let out = load_plugins(
            &config,
            home.path(),
            repo.path(),
            store.clone(),
            Some(&roots),
        );
        assert!(out.external_agents.is_empty());
        let line = "agent:cursor bin/agent key=CURSOR_API_KEY";
        assert!(
            out.notices
                .iter()
                .any(|n| n.contains("plugin cur is not loaded") && n.contains(line)),
            "{:?}",
            out.notices
        );
        assert!(
            !pkg.join("bin/agent.ran").exists(),
            "an ungranted agent ran"
        );

        let root = cox_config::load::find_git_root(repo.path());
        let scope = cox_plugin::grant::scope(cox_plugin::Source::Project, root.as_deref())
            .expect("project scope");
        store
            .grant_put(&PluginGrant {
                plugin_id: "cur".into(),
                scope,
                digest: cox_plugin::package_digest(&pkg).expect("digest"),
                capabilities: serde_json::json!(cox_plugin::grant::capability_list(&manifest)),
                enabled: true,
                source: serde_json::json!({}),
                decided_at: "2026-09-26T00:00:00Z".into(),
            })
            .expect("grant");
        let out = load_plugins(
            &config,
            home.path(),
            repo.path(),
            store.clone(),
            Some(&roots),
        );
        let resolved = out.external_agents.len()
            + out
                .notices
                .iter()
                .filter(|n| {
                    n.contains("external agent cursor skipped: cannot run under the sandbox")
                })
                .count();
        assert_eq!(resolved, 1, "{:?}", out.notices);
        assert!(
            !pkg.join("bin/agent.ran").exists(),
            "loading spawned the agent"
        );
    }

    /// T33.14.1 (PL§7d): a granted plugin whose `net` covers a configured
    /// provider host is not loaded, with a notice naming the host.
    #[cfg(feature = "plugins")]
    #[test]
    fn granted_plugin_with_provider_host_in_net_is_not_loaded() {
        use cox_protocol::{PluginGrant, PluginStore as _};

        let home = tempfile::tempdir().expect("tempdir");
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(repo.path().join(".git")).expect("git");
        let pkg = repo.path().join(".cox/plugins/leak");
        std::fs::create_dir_all(&pkg).expect("mkdir");
        std::fs::write(
            pkg.join("plugin.wasm"),
            r#"(module (func (export "cox_init") (result i32) (i32.const 0)))"#,
        )
        .expect("wasm");
        std::fs::write(
            pkg.join("plugin.toml"),
            "api = 1\nid = \"leak\"\nversion = \"0.1.0\"\nname = \"Leak\"\n\
             wasm = \"plugin.wasm\"\n\n[capabilities]\nnet = [\"api.anthropic.com\"]\n",
        )
        .expect("manifest");
        let manifest =
            cox_plugin::discover::load_manifest(&pkg, &pkg.join("plugin.toml"), Some("leak"))
                .expect("valid manifest")
                .0;
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let root = cox_config::load::find_git_root(repo.path());
        let scope = cox_plugin::grant::scope(cox_plugin::Source::Project, root.as_deref())
            .expect("project scope");
        store
            .grant_put(&PluginGrant {
                plugin_id: "leak".into(),
                scope,
                digest: cox_plugin::package_digest(&pkg).expect("digest"),
                capabilities: serde_json::json!(cox_plugin::grant::capability_list(&manifest)),
                enabled: true,
                source: serde_json::json!({}),
                decided_at: "2026-10-03T00:00:00Z".into(),
            })
            .expect("grant");
        let out = load_plugins(&Config::default(), home.path(), repo.path(), store, None);
        assert!(out.live.plugins().is_empty(), "the plugin loaded");
        assert!(
            out.notices
                .iter()
                .any(|n| n.contains("plugin leak is not loaded")
                    && n.contains("provider host api.anthropic.com")),
            "{:?}",
            out.notices
        );
    }

    #[cfg(feature = "plugins")]
    #[test]
    fn plugin_http_server_needs_its_host_in_net() {
        let mcp = serde_json::json!({"name": "api", "url": "https://mcp.example.com/mcp"});
        let config = Config::default();
        let mut notices = Vec::new();
        let denied = manifest_with_mcp(mcp.clone(), &["api.github.com"]);
        assert!(plugin_mcp("gh", Path::new("."), &denied, &config, &[], &mut notices).is_empty());
        assert!(
            notices[0].contains("not in capabilities.net"),
            "{notices:?}"
        );
        let allowed = manifest_with_mcp(mcp, &["*.example.com"]);
        let servers = plugin_mcp("gh", Path::new("."), &allowed, &config, &[], &mut notices);
        assert_eq!(
            servers[0].1.url.as_deref(),
            Some("https://mcp.example.com/mcp")
        );
    }

    /// PL§7c, D7: a plugin's stdio server is spawned by `cox-mcp` from the
    /// argv `plugin_mcp` built, so it runs under the same Seatbelt or bwrap
    /// profile as `bash` (T4.1/T4.2): a write inside the workspace lands,
    /// one under `$HOME` is denied.
    #[cfg(all(feature = "plugins", unix))]
    #[tokio::test]
    async fn plugin_stdio_server_runs_under_sandbox() {
        use cox_tools::sandbox::{Backend, backend};
        use std::os::unix::fs::PermissionsExt as _;

        match backend(cox_protocol::LinuxBackend::Auto) {
            Some(Backend::Seatbelt | Backend::Bwrap) => {}
            other => {
                eprintln!("skipped: no argv sandbox backend here ({other:?})");
                return;
            }
        }
        let ws = tempfile::tempdir().expect("tempdir");
        // Inside the workspace, as in the external-agent test above.
        let pkg = tempfile::tempdir_in(ws.path()).expect("tempdir");
        std::fs::create_dir(pkg.path().join("bin")).expect("mkdir");
        let server = pkg.path().join("bin/server");
        std::fs::write(
            &server,
            "#!/bin/sh\necho in > \"$1/inside\"\necho x > \"$2\"\n",
        )
        .expect("server");
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let home = std::env::var("HOME").expect("HOME");
        let outside = format!("{home}/.cox-plugin-escape-{}", std::process::id());
        let ws_arg = ws.path().display().to_string();
        let mcp =
            serde_json::json!({"name": "s", "command": "bin/server", "args": [ws_arg, outside]});
        let mut config = Config::default();
        config.core.workspace_roots = vec![ws.path().to_path_buf()];
        let roots = config.core.workspace_roots.clone();
        let mut notices = Vec::new();
        let manifest = manifest_with_mcp(mcp, &[]);
        let servers = plugin_mcp("gh", pkg.path(), &manifest, &config, &roots, &mut notices);
        assert!(notices.is_empty(), "{notices:?}");
        let mut found = cox_mcp::discovery::Discovered::default();
        cox_mcp::discovery::add_plugin(&mut found, "gh", servers);

        // Not an MCP server, so the handshake fails once the script exits,
        // after both writes were tried.
        let timeout = std::time::Duration::from_secs(10);
        let auth = cox_mcp::client::Auth::none();
        let _ = cox_mcp::client::McpClient::connect("gh-s", &found.servers["gh-s"], timeout, &auth)
            .await;

        let leaked = Path::new(&outside).exists();
        let _ = std::fs::remove_file(&outside);
        assert!(ws.path().join("inside").exists(), "the server never ran");
        assert!(!leaked, "the sandbox let a plugin server write {outside}");
    }

    /// T33.38 Check (PL§13/§14): a wasm-less `[[mcp]]`-only package —
    /// staged, discovered and granted exactly as `install_granted` does for
    /// a wasm plugin, just with no `plugin.wasm` — installs, is granted,
    /// and its `[[mcp]]` server comes back from `load_plugins` and gets
    /// named `count-count` by `cox_mcp::discovery::add_plugin`, the same
    /// `<id>-<name>` a wasm plugin's server gets. `load_plugins` must not
    /// try to `fs::read` a `wasm` this manifest does not have.
    #[cfg(all(feature = "plugins", unix))]
    #[test]
    fn wasm_less_mcp_only_plugin_installs_grants_and_registers_its_server() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::tempdir().expect("tempdir");
        let staged = home.path().join("plugins/count/versions/staged");
        std::fs::create_dir_all(staged.join("bin")).expect("plugin dir");
        let server = staged.join("bin/server");
        std::fs::write(&server, "#!/bin/sh\nexit 0\n").expect("server");
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::write(
            staged.join("plugin.toml"),
            "api = 1\nid = \"count\"\nversion = \"0.1.0\"\nname = \"count\"\n\n\
             [[mcp]]\nname = \"count\"\ncommand = \"bin/server\"\n",
        )
        .expect("plugin.toml");
        let digest = cox_plugin::package_digest(&staged).expect("digest");
        std::fs::rename(&staged, staged.with_file_name(&digest[..12])).expect("stage");
        std::fs::write(home.path().join("plugins/count/current"), &digest[..12]).expect("current");

        let found = cox_plugin::discover::discover(home.path(), None);
        let plugin = found
            .plugins
            .iter()
            .find(|p| p.id == "count")
            .expect("discovered");
        let cox_plugin::State::Loaded { manifest, digest } = &plugin.state else {
            panic!("count did not load: {:?}", found.notices);
        };
        assert_eq!(manifest.wasm, None, "the fixture ships no plugin.wasm");
        let grant_store = Store::open(home.path()).expect("store");
        crate::write_grant(
            &grant_store,
            "count",
            &GrantScope::User,
            digest,
            cox_plugin::grant::capability_list(manifest),
            serde_json::json!({}),
        )
        .expect("grant");

        let config = Config::default();
        let work = tempfile::tempdir().expect("tempdir");
        let roots = vec![work.path().to_path_buf()];
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let out = load_plugins(&config, home.path(), work.path(), store, Some(&roots));
        assert!(
            out.notices.iter().all(|n| !n.contains("failed to load")),
            "{:?}",
            out.notices
        );
        assert_eq!(out.mcp.len(), 1, "{:?}", out.mcp);
        assert_eq!(out.mcp[0].0, "count");

        let mut discovered = cox_mcp::discovery::Discovered::default();
        cox_mcp::discovery::add_plugin(&mut discovered, "count", out.mcp[0].1.clone());
        assert!(
            discovered.servers.contains_key("count-count"),
            "{:?}",
            discovered.servers.keys().collect::<Vec<_>>()
        );
    }

    /// `open`'s plugin steps over a scripted session: discover and load,
    /// build, `start_plugins`, the hook chain, then the notices in order.
    #[cfg(feature = "plugins")]
    async fn session_with_plugins(home: &Path, work: &Path, turns: usize) -> (Session, Arc<Store>) {
        let scenario: String = (0..turns)
            .map(|i| format!("[[turn]]\ntext = \"reply {i}\"\n"))
            .collect();
        let (session, store) = scripted_session(home, work, &scenario);
        let config = Config::default();
        let plugins = load_plugins(&config, home, work, store.clone(), None);
        let (hooks, started) = start_plugins(&session, plugins.live, &config.plugins, work, None);
        session.set_hook(Arc::new(cox_ext::hooks::HookChain::new(None, hooks)));
        let loaded = plugins.notices.into_iter().map(|w| (Level::Warn, w));
        for (level, text) in loaded.chain(started) {
            session.notice(level, text).await.expect("notice");
        }
        (session, store)
    }

    #[cfg(feature = "plugins")]
    fn notices(store: &Store, session: &Session) -> Vec<String> {
        store
            .rollout_read(&session.id())
            .expect("rollout")
            .into_iter()
            .filter_map(|ev| match ev {
                Event::Notice { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Polls the rollout until a notice satisfies `want`; the tap's notices
    /// reach it through a task, after the event that drained them.
    #[cfg(feature = "plugins")]
    async fn wait_for_notice(
        store: &Store,
        session: &Session,
        want: impl Fn(&str) -> bool,
    ) -> String {
        for _ in 0..250 {
            if let Some(found) = notices(store, session).into_iter().find(|n| want(n)) {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("no such notice in {:?}", notices(store, session));
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn granted_plugin_runs_cox_init_once_per_session() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        install_granted(home.path(), "once", "", &plugin_wat("init ran", "{}", ""));
        let (session, store) = session_with_plugins(home.path(), work.path(), 2).await;
        user_turn(&session, "one").await;
        user_turn(&session, "two").await;
        let inits = notices(&store, &session)
            .into_iter()
            .filter(|n| n == "plugin once: init ran")
            .count();
        assert_eq!(inits, 1, "{:?}", notices(&store, &session));
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn plugin_notify_reaches_the_transcript() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let caps = "[capabilities]\nhooks = [\"UserPromptSubmit\"]\n";
        install_granted(
            home.path(),
            "tell",
            caps,
            &plugin_wat("", "{}", r"\u001b[2Jfrom a hook"),
        );
        let (session, store) = session_with_plugins(home.path(), work.path(), 1).await;
        user_turn(&session, "hi").await;
        // Sanitized on the way (T33.9), attributed to the plugin here.
        let got = wait_for_notice(&store, &session, |n| n.contains("from a hook")).await;
        assert_eq!(got, "plugin tell: from a hook");
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn hooks_and_event_tap_share_one_plugin_instance() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let caps = "[capabilities]\nhooks = [\"UserPromptSubmit\"]\nevents = [\"turn_done\"]\n";
        let init_out = r#"{"subscribe":["turn_done"]}"#;
        install_granted(home.path(), "ctr", caps, &plugin_wat("", init_out, ""));
        let (session, store) = session_with_plugins(home.path(), work.path(), 8).await;
        // The tap drains after an event, so a later turn carries the notice
        // `cox_on_event` answered for an earlier `turn_done`.
        let mut seen = None;
        for i in 0..8 {
            user_turn(&session, &format!("turn {i}")).await;
            seen = notices(&store, &session)
                .into_iter()
                .find(|n| n.starts_with("plugin ctr: hooks"));
            if seen.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Two instances would leave the event side's count at 0.
        let seen = seen.expect("a notice from cox_on_event");
        assert_ne!(seen, "plugin ctr: hooks 0");
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn plugin_init_failure_is_skipped_with_a_warning() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let caps = "[capabilities]\nhooks = [\"UserPromptSubmit\"]\n";
        let trap = r#"(module (func (export "cox_init") (result i32) unreachable)
                      (func (export "cox_hook") (result i32) unreachable))"#;
        install_granted(home.path(), "boom", caps, trap);
        let (session, store) = session_with_plugins(home.path(), work.path(), 1).await;
        // The session goes on and the dropped plugin's hook never runs:
        // it would trap, which the core reports as a hook warning.
        user_turn(&session, "hi").await;
        let all = notices(&store, &session);
        assert!(
            all.iter()
                .any(|n| n.starts_with("plugin boom failed to start")),
            "{all:?}"
        );
        assert_eq!(all.len(), 1, "{all:?}");
    }

    #[cfg(feature = "plugins")]
    #[test]
    fn granted_plugin_models_join_the_catalog() {
        fn fake_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
            Ok("sk-test".to_string())
        }
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let init = r#"(module (func (export "cox_init") (result i32) (i32.const 0)))"#;
        let row =
            |window: u32| format!("[[models]]\nid = \"plug-big\"\ncontext_window = {window}\n");
        install_granted(home.path(), "aa", &row(1_000_000), init);
        install_granted(home.path(), "zz", &row(5), init);
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let mut cfg = Config::default();
        cfg.tiers.code.model = "plug-big".into();
        let plugins = load_plugins(&cfg, home.path(), work.path(), store, None);
        // The lower id keeps the row; the other is shown, not applied.
        assert!(
            plugins
                .notices
                .iter()
                .any(|n| n == "plugin zz also defines model plug-big; plugin aa's row is kept"),
            "{:?}",
            plugins.notices
        );
        let p = provider_for_served(&cfg, fake_key, None, &plugins.catalog_rows())
            .expect("anthropic builds");
        assert_eq!(p.capabilities().max_context, 1_000_000);
    }

    /// Bytes the `dump` tool of `tool_plugin_wat` answers.
    #[cfg(feature = "plugins")]
    const DUMP_BYTES: usize = 20_000;

    /// T33.12 fixture: `cox_init` declares one read-only tool, `dump`;
    /// `cox_tool_call` answers `DUMP_BYTES` of `a`, past the visible cap.
    #[cfg(feature = "plugins")]
    fn tool_plugin_wat() -> String {
        let init = r#"{"tools":[{"name":"dump","description":"dump the quarterly report","risk":"read_only","input_schema":{"type":"object"}}]}"#;
        format!(
            r##"(module
              (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
              (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
              (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
              (memory 1)
              (data (i32.const 0) "{data}")
              (data (i32.const 1024) "{{\"text\":\"")
              (data (i32.const 1040) "\"}}")
              (func $put (param $off i64) (param $p i32) (param $len i32) (local $i i32)
                (block $end (loop $next
                  (br_if $end (i32.ge_u (local.get $i) (local.get $len)))
                  (call $store (i64.add (local.get $off) (i64.extend_i32_u (local.get $i)))
                    (i32.load8_u (i32.add (local.get $p) (local.get $i))))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br $next))))
              (func (export "cox_init") (result i32) (local $off i64)
                (local.set $off (call $alloc (i64.const {init_len})))
                (call $put (local.get $off) (i32.const 0) (i32.const {init_len}))
                (call $output_set (local.get $off) (i64.const {init_len})) (i32.const 0))
              (func (export "cox_tool_call") (result i32) (local $off i64) (local $i i64)
                (local.set $off (call $alloc (i64.const {total})))
                (call $put (local.get $off) (i32.const 1024) (i32.const 9))
                (block $end (loop $next
                  (br_if $end (i64.ge_u (local.get $i) (i64.const {n})))
                  (call $store (i64.add (local.get $off) (i64.add (i64.const 9) (local.get $i)))
                    (i32.const 97))
                  (local.set $i (i64.add (local.get $i) (i64.const 1)))
                  (br $next)))
                (call $put (i64.add (local.get $off) (i64.const {tail})) (i32.const 1040) (i32.const 2))
                (call $output_set (local.get $off) (i64.const {total})) (i32.const 0)))"##,
            data = init.replace('"', "\\\""),
            init_len = init.len(),
            n = DUMP_BYTES,
            tail = DUMP_BYTES + 9,
            total = DUMP_BYTES + 11,
        )
    }

    /// `open`'s order for a session with plugin tools: load, `cox_init`
    /// under the chosen id (`plugin_tools`), build with the full tool list
    /// and a `tool_search` indexed over it, then `start_plugins`.
    #[cfg(feature = "plugins")]
    async fn session_with_plugin_tools(
        home: &Path,
        work: &Path,
        scenario: &str,
    ) -> (Session, Arc<Store>, Arc<Recorder>) {
        install_granted(
            home,
            "big",
            "[capabilities]\ntools = [\"dump\"]\n",
            &tool_plugin_wat(),
        );
        let store = Arc::new(Store::open(home).expect("store"));
        let config = Config::default();
        let plugins = load_plugins(&config, home, work, store.clone(), None);
        let (mut live, mut warnings) = (plugins.live, plugins.notices);
        let id = SessionId::new();
        let mut all: Vec<Arc<dyn Tool>> = vec![Arc::new(ToolSearchTool::new(vec![]))];
        all.extend(plugin_tools(
            &mut live,
            &config.plugins,
            id,
            work,
            &mut warnings,
        ));
        assert!(warnings.is_empty(), "{warnings:?}");
        let recorder = Arc::new(Recorder {
            inner: cox_provider::scripted::Scripted::from_toml(scenario, "").expect("scenario"),
            sent: std::sync::Mutex::default(),
        });
        let session = Session::new_with_id(
            id,
            config.clone(),
            recorder.clone(),
            with_tool_search_index(all),
            store.clone(),
            store.clone(),
            work.to_path_buf(),
        )
        .expect("session");
        let (hooks, _) = start_plugins(&session, live, &config.plugins, work, None);
        session.set_hook(Arc::new(cox_ext::hooks::HookChain::new(None, hooks)));
        (session, store, recorder)
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn prefix_bytes_identical_between_turns_with_plugin_tool_discovered() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let scenario = r#"
[[turn]]
[[turn.tool_calls]]
name = "tool_search"
input = { query = "quarterly report" }
[[turn]]
text = "found it"
[[turn]]
text = "two"
[[turn]]
text = "three"
"#;
        let (session, _store, recorder) =
            session_with_plugin_tools(home.path(), work.path(), scenario).await;
        for text in ["one", "two", "three"] {
            user_turn(&session, text).await;
        }
        let sent = recorder.sent.lock().expect("sent").clone();
        assert_eq!(sent.len(), 4);
        let prefix = |r: &cox_protocol::types::Request| {
            serde_json::to_vec(&(&r.system[0..=2], &r.tools)).expect("prefix")
        };
        let has_dump =
            |r: &cox_protocol::types::Request| r.tools.iter().any(|t| t.name == "wasm__big__dump");
        // Deferred until `tool_search` finds it; then the prefix changes
        // once and stays byte-identical for every later request.
        assert!(!has_dump(&sent[0]) && has_dump(&sent[1]));
        assert_ne!(prefix(&sent[0]), prefix(&sent[1]));
        assert_eq!(prefix(&sent[1]), prefix(&sent[2]));
        assert_eq!(prefix(&sent[2]), prefix(&sent[3]));
    }

    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn plugin_tool_output_is_archived_before_truncation() {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let scenario = r#"
[[turn]]
[[turn.tool_calls]]
name = "wasm__big__dump"
input = {}
[[turn]]
text = "done"
"#;
        let (session, store, recorder) =
            session_with_plugin_tools(home.path(), work.path(), scenario).await;
        user_turn(&session, "dump it").await;
        let result = store
            .rollout_read(&session.id())
            .expect("rollout")
            .into_iter()
            .find_map(|ev| match ev {
                Event::ToolCallDone { result, .. } => Some(result),
                _ => None,
            })
            .expect("the dump call finished");
        let archive = result.archive.expect("archived");
        let full = "a".repeat(DUMP_BYTES);
        assert_eq!(
            store.archive_get(&archive.id).expect("row"),
            full.as_bytes()
        );
        // What the model saw is shortened and points at the archive row.
        let pointer = format!("expand #{}", archive.id);
        assert!(result.visible.len() < full.len(), "{}", result.visible);
        assert!(result.visible.contains(&pointer), "{}", result.visible);
        let sent = recorder.sent.lock().expect("sent").clone();
        let seen = sent[1]
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .find_map(|c| match c {
                cox_protocol::types::Content::ToolResult { content, .. } => Some(content),
                _ => None,
            })
            .expect("a tool result");
        assert!(seen.contains(&pointer), "{seen}");
    }
}
