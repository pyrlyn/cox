// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Config loading and provenance (plan.md §1.6/D13/T0.3): layers
//! `config/default.toml` < `~/.cox/config.toml` < `<git root>/.cox/config.toml`
//! < `COX_<SECTION>_<KEY>` env vars < CLI flags via `figment`, enforces the
//! project-config guard list, and records which layer last set each key so
//! `cox config show --sources` can print it.
//!
//! `.claude/settings.json` (T7.5) is one more layer above project config (plan.md §1.6
//! "Out of scope"). A repository's own `.claude` files count as project
//! config for the guard list (T22.11, A122), the user's `~/.claude` file does
//! not.
//!
//! T32.16: moved here from `crates/cox`. The two inputs that come from the
//! binary's side stay there and are passed in: the CLI-flag layer (built
//! from clap's `Cli` by `crates/cox`'s `config_load::flag_overrides`) and the
//! `.claude/settings.json` reader (`cox-ext`), so this crate depends only on
//! `cox-protocol`. It prints nothing: the caller reports the guard
//! violations `load` returns.

use std::env;
use std::path::{Path, PathBuf};

use cox_protocol::config::DEFAULT_CONFIG_TOML;
use cox_protocol::{Config, CoreError, PermissionMode, SandboxMode};
use figment::providers::{Env, Format, Serialized, Toml};
use figment::value::{Dict, Map as FigMap};
use figment::{Figment, Metadata, Profile, Provider};
use serde_json::Value as JsonValue;

/// A `Provider` adapter that reports a fixed layer name as its `Metadata`,
/// so `Figment::find_metadata` tells us which layer produced a value —
/// `figment`'s own provider names (`"TOML file"`, `"environment
/// variable(s)"`, ...) aren't the `default|user|project|env|flag` labels
/// `cox config show --sources` needs.
struct Named<P> {
    name: &'static str,
    inner: P,
}

impl<P: Provider> Provider for Named<P> {
    fn metadata(&self) -> Metadata {
        Metadata::named(self.name)
    }

    fn data(&self) -> Result<FigMap<Profile, Dict>, figment::Error> {
        self.inner.data()
    }

    fn profile(&self) -> Option<Profile> {
        self.inner.profile()
    }
}

fn named<P: Provider>(name: &'static str, inner: P) -> Named<P> {
    Named { name, inner }
}

/// Where `cox` looks for its home directory. `COX_HOME` overrides `~/.cox`
/// (plan.md §1.6 `core.home` comment) for the whole `~/.cox` tree, not just
/// the `core.home` config value — this is what every task's `COX_HOME=...`
/// scratch-tree invocation relies on.
pub fn cox_home() -> PathBuf {
    match env::var_os("COX_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(".cox"),
    }
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The user config file: `<cox_home>/config.toml`.
pub fn user_config_path() -> PathBuf {
    cox_home().join("config.toml")
}

/// Walks up from `start` looking for a `.git` entry (a directory for a
/// normal clone, a file for a worktree), returning the first ancestor that
/// has one.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The project config file, if `cwd` is inside a git checkout:
/// `<git root>/.cox/config.toml`.
pub fn project_config_path(cwd: &Path) -> Option<PathBuf> {
    find_git_root(cwd).map(|root| root.join(".cox").join("config.toml"))
}

/// One project-config guard violation (plan.md §1.6): the project layer set
/// a key it isn't allowed to, so the loader reverted it and is reporting why.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardViolation {
    /// The dotted key the project layer tried to set.
    pub key: &'static str,
    /// What the project layer set it to.
    pub project_value: String,
    /// What it was reverted to (the value without the project layer).
    pub reverted_to: String,
}

/// What a guard reason says for a key the list does not name; the test
/// below keeps every guarded key off it.
const GUARD_REASON: &str = "A project may not weaken this setting";

impl GuardViolation {
    /// Why the project may not set this key, one line for the desktop
    /// Settings screen (T37.30.4), next to the guard that enforces it.
    pub fn reason(&self) -> &'static str {
        match self.key {
            "budget.session_usd" | "budget.monthly_usd" | "budget.warn_at" => {
                "A project may not raise a budget above your own"
            }
            "core.max_concurrent_subagents" => {
                "A project may not run more subagents at once than you allow"
            }
            "permissions.mode" => "A project may not turn on bypass mode",
            "permissions.allow" => "A project may not allow a tool call",
            "sandbox.mode" => "A project may not turn the sandbox off",
            "plugins.enabled" => "A project may not turn plugins back on",
            "tiers.think.confirm" => "A project may not skip the think tier's confirmation",
            "mcp.servers.*.sandbox" => "A project may not run an MCP server unsandboxed",
            "lsp.servers" => "A project may not choose which language servers run",
            "voice" => "A project may not turn on the microphone or choose the voice model",
            "tui.status_line.command" => "A project may not choose a status-line command",
            "desktop.remote_hosts" => "A project may not choose which hosts the app connects to",
            "external_agents" => "A project may not choose which agent programs run",
            "external_agents.*.writable" => {
                "A project may not widen where an external agent can write"
            }
            _ => GUARD_REASON,
        }
    }
}

/// Reverts any guarded key `full` set relative to `without_project` back to
/// `without_project`'s value, returning what it reverted (plan.md §1.6:
/// "budget.* may not be raised above user/default, permissions.mode =
/// \"bypass\", sandbox.mode = \"danger-full-access\" or tiers.think.confirm
/// = false are ignored from the project file with a warning to stderr").
fn apply_project_guards(full: &mut Config, without_project: &Config) -> Vec<GuardViolation> {
    let mut violations = Vec::new();

    let mut check_budget_raise = |key, full_v: &mut f64, base_v: f64| {
        if *full_v > base_v {
            violations.push(GuardViolation {
                key,
                project_value: full_v.to_string(),
                reverted_to: base_v.to_string(),
            });
            *full_v = base_v;
        }
    };
    check_budget_raise(
        "budget.session_usd",
        &mut full.budget.session_usd,
        without_project.budget.session_usd,
    );
    check_budget_raise(
        "budget.monthly_usd",
        &mut full.budget.monthly_usd,
        without_project.budget.monthly_usd,
    );
    check_budget_raise(
        "budget.warn_at",
        &mut full.budget.warn_at,
        without_project.budget.warn_at,
    );

    // T34.2 review: a cost/concurrency guard like the budget keys above —
    // a project layer must not be able to widen how many `agent` tasks a
    // session lets run at once.
    if full.core.max_concurrent_subagents > without_project.core.max_concurrent_subagents {
        violations.push(GuardViolation {
            key: "core.max_concurrent_subagents",
            project_value: full.core.max_concurrent_subagents.to_string(),
            reverted_to: without_project.core.max_concurrent_subagents.to_string(),
        });
        full.core.max_concurrent_subagents = without_project.core.max_concurrent_subagents;
    }

    if full.permissions.mode == PermissionMode::Bypass
        && without_project.permissions.mode != PermissionMode::Bypass
    {
        violations.push(GuardViolation {
            key: "permissions.mode",
            project_value: "bypass".to_string(),
            reverted_to: format!("{:?}", without_project.permissions.mode).to_lowercase(),
        });
        full.permissions.mode = without_project.permissions.mode;
    }

    // T22.10 (A122): a project may only tighten the rule lists. figment
    // replaces an array wholesale, so without this a cloned repository's
    // `deny = []` drops the default `~/.ssh` deny and its `allow = ["Bash"]`
    // runs every command unasked. Its `allow` never counts and is reported;
    // its `deny` and `ask` lists only add rules, so there is nothing to
    // report for them.
    if full.permissions.allow != without_project.permissions.allow {
        violations.push(GuardViolation {
            key: "permissions.allow",
            project_value: rule_list(&full.permissions.allow),
            reverted_to: rule_list(&without_project.permissions.allow),
        });
        full.permissions.allow = without_project.permissions.allow.clone();
    }
    add_rules(
        &mut full.permissions.deny,
        &without_project.permissions.deny,
    );
    add_rules(&mut full.permissions.ask, &without_project.permissions.ask);

    if full.sandbox.mode == SandboxMode::DangerFullAccess
        && without_project.sandbox.mode != SandboxMode::DangerFullAccess
    {
        violations.push(GuardViolation {
            key: "sandbox.mode",
            project_value: "danger-full-access".to_string(),
            reverted_to: format!("{:?}", without_project.sandbox.mode).to_lowercase(),
        });
        full.sandbox.mode = without_project.sandbox.mode;
    }

    // T33.6 (PL§1, D14): a repository must not switch plugins back on
    // after the user turned them off; its own plugins still need a grant,
    // but the user's off switch has to hold regardless.
    if full.plugins.enabled && !without_project.plugins.enabled {
        violations.push(GuardViolation {
            key: "plugins.enabled",
            project_value: "true".to_string(),
            reverted_to: "false".to_string(),
        });
        full.plugins.enabled = false;
    }

    if !full.tiers.think.confirm && without_project.tiers.think.confirm {
        violations.push(GuardViolation {
            key: "tiers.think.confirm",
            project_value: "false".to_string(),
            reverted_to: "true".to_string(),
        });
        full.tiers.think.confirm = without_project.tiers.think.confirm;
    }

    // T33.42: the sandbox is exactly what contains a command a cloned
    // repository chose, so the project layer must never be the reason a
    // server's effective `sandbox` is `false` — an existing server or one
    // it adds outright, same difference: only user config (and env/flags,
    // `without_project`'s other layers) may opt a server out. A server is
    // reverted unless `without_project` alone already yields `sandbox =
    // false` for that name, i.e. a non-project layer opted it out on its
    // own.
    let weakened: Vec<String> = full
        .mcp
        .servers
        .iter()
        .filter(|(name, server)| {
            !server.sandbox
                && !without_project
                    .mcp
                    .servers
                    .get(name.as_str())
                    .is_some_and(|s| !s.sandbox)
        })
        .map(|(name, _)| name.clone())
        .collect();
    if !weakened.is_empty() {
        violations.push(GuardViolation {
            key: "mcp.servers.*.sandbox",
            project_value: format!("false ({})", weakened.join(", ")),
            reverted_to: "true".to_string(),
        });
        for name in &weakened {
            if let Some(server) = full.mcp.servers.get_mut(name) {
                server.sandbox = true;
            }
        }
    }

    // T41.1: an LSP server is a program cox spawns, so which ones exist
    // and what they run is the user's call alone; any project difference
    // (a new server, or a changed command, args or extensions) reverts the
    // whole map to the layers without the project.
    if full.lsp.servers != without_project.lsp.servers {
        let changed: Vec<&str> = full
            .lsp
            .servers
            .iter()
            .filter(|(name, s)| without_project.lsp.servers.get(name.as_str()) != Some(*s))
            .map(|(name, _)| name.as_str())
            .collect();
        let kept: Vec<&str> = without_project
            .lsp
            .servers
            .keys()
            .map(String::as_str)
            .collect();
        violations.push(GuardViolation {
            key: "lsp.servers",
            project_value: changed.join(", "),
            reverted_to: if kept.is_empty() {
                "none".to_string()
            } else {
                kept.join(", ")
            },
        });
        full.lsp.servers = without_project.lsp.servers.clone();
    }

    // T54.4 (A123): `[voice]` switches the microphone on and picks the
    // model file cox loads, so the whole table is the user's alone; any
    // project difference reverts all of it.
    if full.voice != without_project.voice {
        let (theirs, ours) = (&full.voice, &without_project.voice);
        let changed: Vec<&str> = [
            ("enabled", theirs.enabled != ours.enabled),
            ("model", theirs.model != ours.model),
            ("language", theirs.language != ours.language),
            ("key", theirs.key != ours.key),
            ("auto_submit", theirs.auto_submit != ours.auto_submit),
            ("max_seconds", theirs.max_seconds != ours.max_seconds),
        ]
        .into_iter()
        .filter_map(|(key, differs)| differs.then_some(key))
        .collect();
        violations.push(GuardViolation {
            key: "voice",
            project_value: changed.join(", "),
            reverted_to: "your own [voice] settings".to_string(),
        });
        full.voice = without_project.voice.clone();
    }

    // T46.1 (A77): the status-line command runs on every TUI start, before
    // any prompt, so a cloned repository must not choose it; Claude Code
    // gates the same key behind workspace trust.
    if full.tui.status_line.command != without_project.tui.status_line.command {
        violations.push(GuardViolation {
            key: "tui.status_line.command",
            project_value: full.tui.status_line.command.clone(),
            reverted_to: without_project.tui.status_line.command.clone(),
        });
        full.tui.status_line.command = without_project.tui.status_line.command.clone();
    }

    // T52.21: the app opens an ssh session to each saved host and runs cox
    // there, so a cloned repository must not add one; the whole list reverts.
    if full.desktop.remote_hosts != without_project.desktop.remote_hosts {
        violations.push(GuardViolation {
            key: "desktop.remote_hosts",
            project_value: full.desktop.remote_hosts.join(", "),
            reverted_to: rule_list(&without_project.desktop.remote_hosts),
        });
        full.desktop.remote_hosts = without_project.desktop.remote_hosts.clone();
    }

    // T52.2 (DT§3.3.1): an external agent's `writable` directories are
    // where a program may write outside the workspace, so only the user
    // lists them. Its own violation and reason, apart from the program
    // guard below, so the warning says which of the two a project tried.
    let base = &without_project.external_agents;
    let widened: Vec<String> = full
        .external_agents
        .iter()
        .filter(|(name, a)| {
            !a.writable.is_empty()
                && base.get(name.as_str()).map(|b| &b.writable) != Some(&a.writable)
        })
        .map(|(name, _)| name.clone())
        .collect();
    if !widened.is_empty() {
        let kept: Vec<String> = base
            .iter()
            .filter(|(_, b)| !b.writable.is_empty())
            .map(|(name, _)| name.clone())
            .collect();
        violations.push(GuardViolation {
            key: "external_agents.*.writable",
            project_value: widened.join(", "),
            reverted_to: rule_list(&kept),
        });
    }
    // T52.2: an entry is a program cox spawns, so a cloned repository must
    // neither add one nor change what one runs or which key it gets; any
    // project difference reverts the whole table to the user's.
    let changed: Vec<String> = full
        .external_agents
        .iter()
        .filter(|(name, a)| {
            base.get(name.as_str()).is_none_or(|b| {
                (&b.command, &b.args, &b.key_env) != (&a.command, &a.args, &a.key_env)
            })
        })
        .map(|(name, _)| name.clone())
        .collect();
    if !changed.is_empty() {
        let kept: Vec<String> = base.keys().cloned().collect();
        violations.push(GuardViolation {
            key: "external_agents",
            project_value: changed.join(", "),
            reverted_to: rule_list(&kept),
        });
    }
    if full.external_agents != *base {
        full.external_agents = base.clone();
    }

    violations
}

/// Makes `rules` (with the project layer) the union of `base` (without it)
/// and the project's extra rules. A list that already holds every `base`
/// rule stays as it is, in the project's own order.
fn add_rules(rules: &mut Vec<String>, base: &[String]) {
    if base.iter().all(|rule| rules.contains(rule)) {
        return;
    }
    let added: Vec<String> = rules
        .iter()
        .filter(|rule| !base.contains(rule))
        .cloned()
        .collect();
    *rules = base.iter().cloned().chain(added).collect();
}

/// A rule list as one line of a [`GuardViolation`], `none` when empty.
fn rule_list(rules: &[String]) -> String {
    if rules.is_empty() {
        "none".to_string()
    } else {
        rules.join(", ")
    }
}

/// Dotted keys the project-config guard list can revert (plan.md §1.6);
/// used only to pick which figment (with or without the project layer) a
/// reverted key's provenance is looked up in.
const GUARDED_KEYS: [&str; 16] = [
    "budget.session_usd",
    "budget.monthly_usd",
    "budget.warn_at",
    "core.max_concurrent_subagents",
    "desktop.remote_hosts",
    "external_agents",
    "external_agents.*.writable",
    "lsp.servers",
    "mcp.servers.*.sandbox",
    "permissions.allow",
    "permissions.mode",
    "plugins.enabled",
    "sandbox.mode",
    "tiers.think.confirm",
    "tui.status_line.command",
    "voice",
];

/// The result of [`load`]: the effective, guard-corrected `Config`, plus
/// enough of the layered figments to answer `source_of` for `cox config show
/// --sources`.
pub struct LoadedConfig {
    /// The effective configuration, after the project-config guard list.
    pub config: Config,
    /// Guard violations found in the project layer, if any (already applied
    /// to `config`; report these to stderr and/or a future `Notice`).
    pub violations: Vec<GuardViolation>,
    full_fig: Figment,
    pre_project_fig: Figment,
}

impl LoadedConfig {
    /// Which layer last set `key` (`default|user|project|env|flag`), for
    /// `cox config show --sources`. A key the project guard list reverted
    /// reports the layer its *effective* (post-revert) value came from.
    pub fn source_of(&self, key: &str) -> &'static str {
        // A guarded table (`lsp.servers`) is reverted whole, so each of its
        // leaf keys takes its provenance from the pre-project figment too.
        let under = |guarded: &str| {
            key == guarded
                || key
                    .strip_prefix(guarded)
                    .is_some_and(|rest| rest.starts_with('.'))
        };
        let reverted =
            GUARDED_KEYS.iter().any(|g| under(g)) && self.violations.iter().any(|v| under(v.key));
        let fig = if reverted {
            &self.pre_project_fig
        } else {
            &self.full_fig
        };
        match fig.find_metadata(key).map(|m| m.name.as_ref()) {
            Some("default") => "default",
            Some("user") => "user",
            Some("project") => "project",
            Some("env") => "env",
            Some("flag") => "flag",
            Some("claude-settings") => "claude-settings",
            _ => "default",
        }
    }
}

/// The imported `.claude/settings.json` layers for one `cwd`, split by owner
/// (T22.11, A122): a repository's files are left out of the figment without
/// the project, so the guard list treats their rules like a project
/// config's (`allow` reverted, `deny`/`ask` added), while the user's own
/// file counts like user config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeLayers {
    /// `~/.claude/settings.json`, as `cox_ext::claude_settings` lifts it.
    pub user: Option<JsonValue>,
    /// A repository's `.claude/settings.json` and `.claude/settings.local.json`.
    pub project: Option<JsonValue>,
}

fn build_figment(
    user_path: &Path,
    project_path: Option<&Path>,
    claude: &[&JsonValue],
    flags: &JsonValue,
) -> Figment {
    // `Toml::file` (not `file_exact`): both paths here are always absolute
    // (`cox_home()`/git-root-derived), and for an absolute path `Data::file`
    // checks existence directly rather than searching parent directories —
    // it just also treats "missing" as "empty" instead of an IO error,
    // which `file_exact` does not (it always attempts to read the path).
    let mut fig = Figment::new()
        .merge(named("default", Toml::string(DEFAULT_CONFIG_TOML)))
        .merge(named("user", Toml::file(user_path)));
    if let Some(project_path) = project_path {
        fig = fig.merge(named("project", Toml::file(project_path)));
    }
    for claude in claude {
        // `adjoin`, not `merge`: imported rules and hooks add to the `.cox`
        // lists rather than replace them (D13: imported, read-only).
        fig = fig.adjoin(named(
            "claude-settings",
            Serialized::defaults((*claude).clone()),
        ));
    }
    let key_tree = default_key_tree();
    fig = fig.merge(named(
        "env",
        // `COX_PROVIDER` / `COX_SCENARIO` / `COX_CASSETTES` select a test-double
        // provider (cox-provider::from_env), not config keys. `COX_HOME`
        // overrides `core.home` directly below. `COX_EXPECT_SANDBOX` pins the
        // backend a sandbox test asserts (CI sets it globally), so it must
        // not leak into the config tree as `expect.sandbox` either.
        // `COX_PLAIN` and `COX_AX_STARTUP_QUIET_MS` are read by the plain
        // surface (T29.1) itself. `COX_KEYRING` switches the OS keyring
        // off (A49). The ignore list matches pre-split keys
        // (`EXPECT_SANDBOX`, not dotted): it runs before the `map` below.
        Env::prefixed("COX_")
            .ignore(&[
                "home",
                "provider",
                "scenario",
                "cassettes",
                "expect_sandbox",
                "plain",
                "ax_startup_quiet_ms",
                "keyring",
            ])
            .map(move |name| env_key(&key_tree, name.as_str()).into()),
    ));
    if let Ok(home) = env::var("COX_HOME") {
        fig = fig.merge(named("env", Serialized::default("core.home", home)));
    }
    fig.merge(named("flag", Serialized::defaults(flags)))
}

/// The table/key tree of the embedded defaults, which [`env_key`] resolves
/// `COX_*` names against. Empty only if `default.toml` failed to parse, which
/// its own tests rule out; `env_key` then degrades to plain `_` splitting.
fn default_key_tree() -> Dict {
    Figment::from(Toml::string(DEFAULT_CONFIG_TOML))
        .extract()
        .unwrap_or_default()
}

/// Maps a `COX_`-stripped env name to a dotted key. Splitting on every `_`
/// would turn `TUI_SHOW_THINKING` into `tui.show.thinking`, so each level
/// takes the longest known name that is the whole rest or a prefix of it
/// followed by `_`. Whatever no known name covers (a user-defined tier, a
/// typo) is split on `_` as before, so it still lands where it did.
fn env_key(tree: &Dict, name: &str) -> String {
    let name = name.to_ascii_lowercase();
    let mut rest = name.as_str();
    let mut table = Some(tree);
    let mut parts: Vec<&str> = Vec::new();
    while let Some(dict) = table {
        let hit = dict
            .iter()
            .filter(|(key, _)| {
                rest.strip_prefix(key.as_str())
                    .is_some_and(|tail| tail.is_empty() || tail.starts_with('_'))
            })
            .max_by_key(|(key, _)| key.len());
        let Some((key, value)) = hit else { break };
        parts.push(key);
        rest = rest[key.len()..].strip_prefix('_').unwrap_or("");
        table = value.as_dict();
    }
    if !rest.is_empty() {
        parts.extend(rest.split('_'));
    }
    parts.join(".")
}

fn to_core_error(err: figment::Error) -> CoreError {
    CoreError::Config {
        key: err.path.join("."),
        message: err.to_string(),
    }
}

/// Loads and layers config (plan.md §1.6/D13), applies the project guard
/// list, and returns the effective config plus provenance.
///
/// `flags` is the sparse CLI-flag override tree; `claude_layer` reads the
/// imported `.claude/settings.json` layers for `cwd`, and is called only when
/// the `.cox` layers leave `permissions.import_claude_settings` on. The
/// caller reports `violations` (T32.16: no terminal output in this crate).
pub fn load(
    cwd: &Path,
    flags: &JsonValue,
    claude_layer: impl FnOnce(&Path) -> Option<ClaudeLayers>,
) -> Result<LoadedConfig, CoreError> {
    load_in(&user_config_path(), cwd, flags, claude_layer)
}

/// [`load`] with the user layer read from `user_path` rather than
/// `COX_HOME`'s: the desktop app's `App` owns a home of its own (T37.30).
pub fn load_in(
    user_path: &Path,
    cwd: &Path,
    flags: &JsonValue,
    claude_layer: impl FnOnce(&Path) -> Option<ClaudeLayers>,
) -> Result<LoadedConfig, CoreError> {
    let user_path = user_path.to_path_buf();
    let project_path = project_config_path(cwd);

    // Whether to import is itself a config key, so the `.cox` layers decide
    // before the Claude layer exists.
    let native: Config = build_figment(&user_path, project_path.as_deref(), &[], flags)
        .extract()
        .map_err(to_core_error)?;
    let claude = native
        .permissions
        .import_claude_settings
        .then(|| claude_layer(cwd))
        .flatten()
        .unwrap_or_default();
    // T22.11: a repository's `.claude` files join only the figment with the
    // project, so the guard list sees their rules as the project's.
    let user_claude: Vec<&JsonValue> = claude.user.iter().collect();
    let all_claude: Vec<&JsonValue> = claude.user.iter().chain(&claude.project).collect();
    let full_fig = build_figment(&user_path, project_path.as_deref(), &all_claude, flags);
    let pre_project_fig = build_figment(&user_path, None, &user_claude, flags);

    let full_cfg: Config = full_fig.extract().map_err(to_core_error)?;
    let without_project_cfg: Config = pre_project_fig.extract().map_err(to_core_error)?;

    let mut config = full_cfg;
    let violations = apply_project_guards(&mut config, &without_project_cfg);

    Ok(LoadedConfig {
        config,
        violations,
        full_fig,
        pre_project_fig,
    })
}

/// Serializes every test in this crate that mutates process-wide env vars
/// (`COX_HOME`, `COX_*`) — `cargo test` runs a binary's tests concurrently
/// by default, and env vars are global process state, so without this lock
/// `config_env_overrides_project`, `config_project_cannot_raise_budget` and
/// `cmd::tests::config_set_*` would race each other. The
/// `test-util` feature (T32.16) exposes it to `crates/cox`'s own
/// env-mutating tests, so there is one lock, not a copy per crate.
#[cfg(any(test, feature = "test-util"))]
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(any(test, feature = "test-util"))]
/// Sets env vars for the duration of `f`, restoring the previous value
/// (or absence) afterwards, holding [`ENV_LOCK`] throughout so this
/// doesn't race other env-mutating tests in the crate.
// why: env::set_var/remove_var are unsafe in edition 2024; ENV_LOCK is held.
#[allow(unsafe_code)]
pub fn temp_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(k, _)| (k.to_string(), env::var(k).ok()))
        .collect();
    for (k, v) in vars {
        match v {
            Some(v) => unsafe { env::set_var(k, v) },
            None => unsafe { env::remove_var(k) },
        }
    }
    f();
    for (k, v) in previous {
        match v {
            Some(v) => unsafe { env::set_var(&k, v) },
            None => unsafe { env::remove_var(&k) },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    /// `load` with no CLI flags and no `.claude/settings.json` layer: the
    /// flag layer and the Claude import are the caller's (T32.16).
    fn load_plain(cwd: &Path) -> Result<LoadedConfig, CoreError> {
        load(cwd, &JsonValue::Object(Default::default()), |_| None)
    }

    #[test]
    fn config_defaults_parse() {
        // The embedded default.toml alone, through the same struct tree the
        // full loader uses, with no unknown fields. `Config::default()` is
        // the neutral fallback *beneath* this layer (empty model lists, no
        // custom providers), so this spot-checks the registry it cannot
        // carry instead of asserting full equality with it.
        let fig = Figment::new().merge(Toml::string(DEFAULT_CONFIG_TOML));
        let cfg: Config = fig.extract().expect("default.toml deserializes cleanly");
        assert_eq!(cfg.tiers.code.model, "claude-sonnet-5");
        assert!(cfg.providers.custom.contains_key("deepseek"));
        assert!(!cfg.providers.anthropic.models.is_empty());
    }

    #[test]
    fn config_project_cannot_raise_budget() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[budget]\nsession_usd = 999.0\n",
        )
        .expect("write project config");

        // SAFETY-of-intent: tests run single-threaded within this process
        // for env-var mutation (see `config_env_overrides_project`, which
        // documents why this crate accepts that constraint for T0.3).
        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            assert_eq!(
                loaded.config.budget.session_usd, 5.0,
                "raise must be ignored"
            );
            assert!(
                loaded
                    .violations
                    .iter()
                    .any(|v| v.key == "budget.session_usd")
            );
            assert_eq!(loaded.source_of("budget.session_usd"), "default");
        });
    }

    /// T34.2 review: `core.max_concurrent_subagents` is a cost/concurrency
    /// guard like the budget keys — a project's `.cox/config.toml` must not
    /// be able to raise it, the same treatment `config_project_cannot_raise_budget`
    /// proves for `budget.session_usd`.
    #[test]
    fn config_project_cannot_raise_max_concurrent_subagents() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[core]\nmax_concurrent_subagents = 999\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            assert_eq!(
                loaded.config.core.max_concurrent_subagents, 8,
                "raise must be ignored"
            );
            assert!(
                loaded
                    .violations
                    .iter()
                    .any(|v| v.key == "core.max_concurrent_subagents")
            );
            assert_eq!(loaded.source_of("core.max_concurrent_subagents"), "default");
        });
    }

    /// T33.6: the user's `plugins.enabled = false` holds against a
    /// repository's own `.cox/config.toml` turning plugins back on.
    #[test]
    fn config_project_cannot_turn_plugins_on() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[plugins]\nenabled = false\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[plugins]\nenabled = true\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            assert!(!loaded.config.plugins.enabled, "turn-on must be ignored");
            assert!(loaded.violations.iter().any(|v| v.key == "plugins.enabled"));
            assert_eq!(loaded.source_of("plugins.enabled"), "user");
        });
    }

    /// T41.1: a repository must not choose a program cox runs, so a
    /// project `.cox/config.toml` can neither add an LSP server nor change
    /// a default one's command; the rest of `[lsp]` stays project-settable.
    #[test]
    fn project_config_cannot_set_lsp_servers() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[lsp.servers.zig]\ncommand = \"zls\"\nextensions = [\"zig\"]\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[lsp]\ntimeout_s = 10\n\n[lsp.servers.rust]\ncommand = \"./evil\"\n\n\
             [lsp.servers.new]\ncommand = \"./also-evil\"\nextensions = [\"x\"]\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            let servers = &loaded.config.lsp.servers;
            assert_eq!(servers["rust"].command, "rust-analyzer");
            assert!(!servers.contains_key("new"), "{servers:?}");
            assert_eq!(
                servers["zig"].command, "zls",
                "the user's own server must survive the revert"
            );
            assert_eq!(loaded.config.lsp.timeout_s, 10, "timeout_s is not guarded");
            let violation = loaded
                .violations
                .iter()
                .find(|v| v.key == "lsp.servers")
                .expect("an lsp.servers violation");
            assert!(violation.project_value.contains("rust"), "{violation:?}");
            assert!(violation.project_value.contains("new"), "{violation:?}");
            assert!(!violation.project_value.contains("zig"), "{violation:?}");
            // `cox config show --sources` asks per leaf key.
            assert_eq!(loaded.source_of("lsp.servers.rust.command"), "default");
            assert_eq!(loaded.source_of("lsp.servers.zig.command"), "user");
            assert_eq!(loaded.source_of("lsp.timeout_s"), "project");
        });
    }

    /// T52.2: an external agent is a program cox spawns and `writable`
    /// widens where it may write, so a project `.cox/config.toml` can
    /// neither add an entry, nor change the user's, nor give one
    /// `writable` directories; each attempt is its own violation with its
    /// own reason, and the user's entries survive untouched.
    #[test]
    fn external_agents_in_project_config_is_refused() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[external_agents.claude]\ncommand = \"claude-agent-acp\"\n\
             args = [\"--hide-claude-auth\"]\nkey_env = \"ANTHROPIC_API_KEY\"\n\
             writable = [\"~/.claude\"]\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[external_agents.claude]\ncommand = \"./evil\"\n\n\
             [external_agents.new]\ncommand = \"./also-evil\"\nkey_env = \"OPENAI_API_KEY\"\n\
             writable = [\"~/.ssh\"]\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            let agents = &loaded.config.external_agents;
            assert_eq!(agents.len(), 1, "{agents:?}");
            let claude = &agents["claude"];
            assert_eq!(claude.command, "claude-agent-acp");
            assert_eq!(claude.args, ["--hide-claude-auth"]);
            assert_eq!(claude.key_env, "ANTHROPIC_API_KEY");
            assert_eq!(claude.writable, [std::path::PathBuf::from("~/.claude")]);
            let find = |key: &str| {
                loaded
                    .violations
                    .iter()
                    .find(|v| v.key == key)
                    .unwrap_or_else(|| panic!("a {key} violation: {:?}", loaded.violations))
            };
            let program = find("external_agents");
            assert_eq!(program.project_value, "claude, new");
            assert_eq!(program.reverted_to, "claude");
            let writable = find("external_agents.*.writable");
            assert_eq!(writable.project_value, "new");
            assert_ne!(program.reason(), writable.reason());
            assert_eq!(loaded.source_of("external_agents.claude.command"), "user");
        });
    }

    /// T52.21: a repository must not choose where the app opens an ssh
    /// session, so a project `.cox/config.toml` cannot add a remote host;
    /// the user's own list survives and the rest of `[desktop]` stays
    /// project-settable.
    #[test]
    fn project_config_cannot_set_remote_hosts() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[desktop]\nremote_hosts = [\"devbox\"]\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[desktop]\nmenu_bar = false\nremote_hosts = [\"devbox\", \"attacker\"]\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            assert_eq!(
                loaded.config.desktop.remote_hosts,
                vec!["devbox".to_string()]
            );
            assert!(!loaded.config.desktop.menu_bar, "menu_bar is not guarded");
            let violation = loaded
                .violations
                .iter()
                .find(|v| v.key == "desktop.remote_hosts")
                .expect("a desktop.remote_hosts violation");
            assert!(
                violation.project_value.contains("attacker"),
                "{violation:?}"
            );
            assert_eq!(violation.reverted_to, "devbox");
            assert_eq!(loaded.source_of("desktop.remote_hosts"), "user");
        });
    }

    /// T54.4: voice is off until the user turns it on, and a transcript
    /// submits itself by default (only into an empty draft, T54.6).
    #[test]
    fn voice_defaults_are_off_with_auto_submit() {
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let voice = load_plain(cwd.path()).expect("load succeeds").config.voice;
            assert!(!voice.enabled);
            assert!(voice.auto_submit);
            assert_eq!(voice.model, "base.en");
            assert_eq!(voice.language, "en");
            assert_eq!(voice.key, "alt+v");
            assert_eq!(voice.max_seconds, 120);
            assert_eq!(voice, cox_protocol::config::VoiceConfig::default());
        });
    }

    /// T54.4 (A123): a cloned repository must not switch the microphone on
    /// or choose the model file cox loads, so no `voice.*` key survives from
    /// a project `.cox/config.toml`; the user's own `[voice]` does.
    #[test]
    fn project_config_cannot_set_voice_keys() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[voice]\nmodel = \"tiny.en\"\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[voice]\nenabled = true\nmodel = \"../../evil.bin\"\nmax_seconds = 9999\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            let voice = &loaded.config.voice;
            assert!(!voice.enabled);
            assert_eq!(voice.model, "tiny.en", "the user's own model survives");
            assert_eq!(voice.max_seconds, 120);
            let violation = loaded
                .violations
                .iter()
                .find(|v| v.key == "voice")
                .expect("a voice violation");
            assert_eq!(violation.project_value, "enabled, model, max_seconds");
            assert_ne!(violation.reason(), GUARD_REASON);
            assert_eq!(loaded.source_of("voice.enabled"), "default");
            assert_eq!(loaded.source_of("voice.model"), "user");
        });
    }

    /// T33.9: a `[plugins.<id>]` table flattens into `plugins.entries`
    /// beside the fixed `enabled` key, the `HooksConfig` pattern.
    #[test]
    fn plugin_table_flattens_into_entries() {
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[plugins]\nenabled = true\n\n[plugins.jev]\nroute = \"cheap\"\nlimit = 3\n",
        )
        .expect("write user config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let plugins = load_plain(cwd.path())
                .expect("load succeeds")
                .config
                .plugins;
            assert!(plugins.enabled);
            assert_eq!(
                plugins.entries.get("jev"),
                Some(&serde_json::json!({ "route": "cheap", "limit": 3 }))
            );
            assert!(!plugins.entries.contains_key("enabled"));
        });
    }

    /// T33.42: a project layer must not be able to flip an existing,
    /// user-configured server's `sandbox` off — that would silently drop
    /// the wrap on a server the user already trusted as sandboxed, without
    /// touching its command, so the change is easy to miss in review.
    #[test]
    fn config_project_cannot_disable_an_mcp_server_sandbox() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[mcp.servers.gh]\ncommand = \"gh-mcp\"\n\n[mcp.servers.opt-out]\ncommand = \"y\"\nsandbox = false\n",
        )
        .expect("write user config");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        // The project layer both disables an existing, user-trusted server
        // and adds a brand-new one already unsandboxed — the sandbox is
        // exactly what contains a command a cloned repository chose, so
        // neither may take effect.
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[mcp.servers.gh]\nsandbox = false\n\n[mcp.servers.new]\ncommand = \"x\"\nsandbox = false\n",
        )
        .expect("write project config");

        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(git_root.path()).expect("load succeeds");
            let gh = &loaded.config.mcp.servers["gh"];
            assert!(
                gh.sandbox,
                "the opt-out on an existing server must be ignored"
            );
            assert_eq!(
                gh.command.as_deref(),
                Some("gh-mcp"),
                "the guard must revert only sandbox, not the whole server"
            );
            assert!(
                loaded.config.mcp.servers["new"].sandbox,
                "a project layer must not be able to add an unsandboxed server either"
            );
            // Only user config (and env/flags) may opt a server out: its
            // own choice for a server it named must hold.
            assert!(!loaded.config.mcp.servers["opt-out"].sandbox);
            let violation = loaded
                .violations
                .iter()
                .find(|v| v.key == "mcp.servers.*.sandbox")
                .expect("both project-caused opt-outs are one violation");
            assert!(violation.project_value.contains("gh"), "{violation:?}");
            assert!(violation.project_value.contains("new"), "{violation:?}");
            assert!(
                !violation.project_value.contains("opt-out"),
                "{violation:?}"
            );
        });
    }

    #[test]
    fn config_ignores_test_only_cox_env_vars() {
        // `COX_EXPECT_SANDBOX` (set globally in CI) and the provider
        // selectors are not config keys; they must not fail the load as
        // `expect.sandbox` / unknown fields.
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().unwrap())),
                // Isolate from the real ~/.claude/settings.json import.
                ("HOME", Some(home.path().to_str().unwrap())),
                ("COX_EXPECT_SANDBOX", Some("bwrap")),
                ("COX_PROVIDER", Some("scripted")),
                ("COX_SCENARIO", Some("/tmp/scenario.toml")),
                ("COX_PLAIN", Some("1")),
                ("COX_AX_STARTUP_QUIET_MS", Some("300")),
                ("COX_KEYRING", Some("off")),
            ],
            || {
                let loaded = load_plain(cwd.path()).expect("load succeeds");
                // `COX_HOME` still overrides `core.home`; everything else
                // must be the embedded default layer (no `expect` layer
                // leaked in).
                assert_eq!(loaded.config.core.home, home.path().to_str().unwrap());
                let mut expected: Config = Figment::new()
                    .merge(Toml::string(DEFAULT_CONFIG_TOML))
                    .extract()
                    .expect("defaults parse");
                expected.core.home = loaded.config.core.home.clone();
                assert_eq!(loaded.config, expected);
            },
        );
    }

    #[test]
    fn config_env_overrides_keys_with_underscores() {
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().unwrap())),
                ("HOME", Some(home.path().to_str().unwrap())),
                ("COX_TUI_SHOW_THINKING", Some("full")),
                ("COX_TIERS_CODE_MAX_TOKENS", Some("1234")),
            ],
            || {
                let loaded = load_plain(cwd.path()).expect("load succeeds");
                assert_eq!(loaded.config.tui.show_thinking, "full");
                assert_eq!(loaded.config.tiers.code.max_tokens, 1234);
                assert_eq!(loaded.source_of("tui.show_thinking"), "env");
            },
        );
    }

    #[test]
    fn env_key_resolves_known_keys_and_splits_the_rest() {
        let tree = default_key_tree();
        assert_eq!(env_key(&tree, "HOOKS_TIMEOUT_S"), "hooks.timeout_s");
        assert_eq!(env_key(&tree, "TIERS_CODE_MODEL"), "tiers.code.model");
        assert_eq!(env_key(&tree, "TUI_ICONS_TOOL_ICON"), "tui.icons.tool.icon");
        assert_eq!(env_key(&tree, "TIERS_FAST_MODEL"), "tiers.fast.model");
    }

    #[test]
    fn config_env_overrides_project() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            "[tiers.code]\nmodel = \"project-model\"\n",
        )
        .expect("write project config");

        temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().unwrap())),
                ("COX_TIERS_CODE_MODEL", Some("env-model")),
            ],
            || {
                let loaded = load_plain(git_root.path()).expect("load succeeds");
                assert_eq!(loaded.config.tiers.code.model, "env-model");
                assert_eq!(loaded.source_of("tiers.code.model"), "env");
            },
        );
    }

    /// Loads with `user` as `~/.cox/config.toml` (if any) and `project` as a
    /// git root's `.cox/config.toml`, then hands the result to `check`.
    fn load_with_project(user: Option<&str>, project: &str, check: impl FnOnce(LoadedConfig)) {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        if let Some(user) = user {
            fs::write(home.path().join("config.toml"), user).expect("write user config");
        }
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::write(git_root.path().join(".cox/config.toml"), project).expect("write project config");
        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            check(load_plain(git_root.path()).expect("load succeeds"));
        });
    }

    fn violation<'a>(loaded: &'a LoadedConfig, key: &str) -> Option<&'a GuardViolation> {
        loaded.violations.iter().find(|v| v.key == key)
    }

    /// T22.10 (A122): a repository must not pre-approve a tool call.
    #[test]
    fn project_config_allow_is_reverted_with_a_violation() {
        load_with_project(None, "[permissions]\nallow = [\"Bash\"]\n", |loaded| {
            assert!(loaded.config.permissions.allow.is_empty());
            let v = violation(&loaded, "permissions.allow").expect("an allow violation");
            assert_eq!(v.project_value, "Bash");
            assert_eq!(v.reverted_to, "none");
            assert_eq!(v.reason(), "A project may not allow a tool call");
            assert_eq!(loaded.source_of("permissions.allow"), "default");
        });
    }

    /// T22.10 (A122): an empty project `deny` must not drop the default
    /// `~/.ssh` deny; a project list only adds rules, so nothing is reported.
    #[test]
    fn project_config_empty_deny_keeps_the_default_deny() {
        load_with_project(None, "[permissions]\ndeny = []\n", |loaded| {
            assert_eq!(
                loaded.config.permissions.deny,
                ["Read(~/.ssh/**)", "Read(~/.aws/**)", "Bash(rm -rf /*)"]
            );
            assert!(loaded.violations.is_empty(), "{:?}", loaded.violations);
        });
    }

    /// T22.10 (A122): a project deny rule tightens: it is added after the
    /// user's own rules rather than replacing them, with no notice.
    #[test]
    fn project_config_deny_rule_is_appended_to_the_user_deny() {
        load_with_project(
            Some("[permissions]\ndeny = [\"Read(~/.ssh/**)\"]\n"),
            "[permissions]\ndeny = [\"Bash(rm:*)\"]\n",
            |loaded| {
                assert_eq!(
                    loaded.config.permissions.deny,
                    ["Read(~/.ssh/**)", "Bash(rm:*)"]
                );
                assert!(loaded.violations.is_empty(), "{:?}", loaded.violations);
                assert_eq!(loaded.source_of("permissions.deny"), "project");
            },
        );
    }

    /// T22.10 (A122): `ask` follows `deny`: the project cannot remove the
    /// user's ask rule, and its own ask rule is appended.
    #[test]
    fn project_config_ask_keeps_the_user_ask_and_appends_its_own() {
        load_with_project(
            Some("[permissions]\nask = [\"Bash(git push:*)\"]\n"),
            "[permissions]\nask = [\"Bash(curl:*)\"]\n",
            |loaded| {
                assert_eq!(
                    loaded.config.permissions.ask,
                    ["Bash(git push:*)", "Bash(curl:*)"]
                );
                assert!(loaded.violations.is_empty(), "{:?}", loaded.violations);
            },
        );
    }

    /// T46.1 (A77): a repository must not choose a program cox runs on
    /// every TUI start; the user's own command survives, and the key's
    /// provenance is the layer the reverted value came from.
    #[test]
    fn project_layer_cannot_set_status_line_command() {
        load_with_project(
            Some("[tui.status_line]\ncommand = \"echo mine\"\n"),
            "[tui.status_line]\ncommand = \"./evil\"\nrefresh_s = 5\n",
            |loaded| {
                let status_line = &loaded.config.tui.status_line;
                assert_eq!(status_line.command, "echo mine");
                assert_eq!(status_line.refresh_s, 5, "refresh_s is not guarded");
                let v = violation(&loaded, "tui.status_line.command").expect("a command violation");
                assert_eq!(v.project_value, "./evil");
                assert_eq!(v.reverted_to, "echo mine");
                assert_eq!(loaded.source_of("tui.status_line.command"), "user");
                assert_eq!(loaded.source_of("tui.status_line.refresh_s"), "project");
            },
        );
    }

    /// T46.1: an out-of-range `timeout_ms` or `refresh_s` fails the load
    /// with an error that names the key, like any other bad value.
    #[test]
    fn status_line_timeout_out_of_range_is_rejected() {
        for (text, key) in [
            ("timeout_ms = 99", "tui.status_line.timeout_ms"),
            ("timeout_ms = 10001", "tui.status_line.timeout_ms"),
            ("refresh_s = 3601", "tui.status_line.refresh_s"),
        ] {
            let home = tempdir().expect("tempdir");
            let cwd = tempdir().expect("tempdir");
            fs::write(
                home.path().join("config.toml"),
                format!("[tui.status_line]\n{text}\n"),
            )
            .expect("write user config");
            temp_env(
                &[("COX_HOME", Some(home.path().to_str().unwrap()))],
                || match load_plain(cwd.path()) {
                    Err(CoreError::Config { key: at, message }) => {
                        assert_eq!(at, key, "{text}");
                        assert!(message.contains("out of range"), "{message}");
                    }
                    Err(other) => panic!("{text}: wrong error {other:?}"),
                    Ok(_) => panic!("{text} must be rejected"),
                },
            );
        }
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        fs::write(
            home.path().join("config.toml"),
            "[tui.status_line]\ntimeout_ms = 10000\nrefresh_s = 3600\n",
        )
        .expect("write user config");
        temp_env(&[("COX_HOME", Some(home.path().to_str().unwrap()))], || {
            let loaded = load_plain(cwd.path()).expect("the bounds load");
            assert_eq!(loaded.config.tui.status_line.timeout_ms, 10_000);
            assert_eq!(loaded.config.tui.status_line.refresh_s, 3_600);
        });
    }

    #[test]
    fn every_guarded_key_has_its_own_reason() {
        for key in GUARDED_KEYS {
            let v = GuardViolation {
                key,
                project_value: String::new(),
                reverted_to: String::new(),
            };
            assert_ne!(v.reason(), GUARD_REASON, "{key}");
        }
    }
}
