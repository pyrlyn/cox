// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The binary's side of config loading (T32.16): `cox-config` owns the
//! layering, guards, provenance and editing, and this module keeps only what
//! needs `crates/cox`'s own types or crates `cox-config` must not depend on:
//! the CLI-flag layer built from clap's `Cli`, the `.claude/settings.json`
//! reader (`cox-ext`), the keymap for `cox-tui`, and the stderr warnings.
//! Everything else is re-exported from `cox_config::load` at this old path,
//! so callers keep writing `config_load::...`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cox_config::load::ClaudeLayers;
use cox_ext::claude_settings;
use cox_protocol::CoreError;
use serde_json::Value as JsonValue;

#[cfg(test)]
pub(crate) use cox_config::load::temp_env;
pub use cox_config::load::{LoadedConfig, cox_home, find_git_root, home_dir};

use crate::cli::Cli;

/// Loads and layers config for `cwd` with `cli`'s flags and the Claude
/// settings import, and warns on stderr about each project-config guard
/// violation (`cox_config::load::load` itself prints nothing).
pub fn load(cwd: &Path, cli: &Cli) -> Result<LoadedConfig, CoreError> {
    let loaded = cox_config::load::load(cwd, &flag_overrides(cli), claude_layer)?;
    for v in &loaded.violations {
        eprintln!(
            "cox: warning: project config ignores {} = {} (guard); using {}",
            v.key, v.project_value, v.reverted_to
        );
    }
    Ok(loaded)
}

/// T25.5: `<cox_home>/keybindings.toml` over whatever `<claude_home>/
/// keybindings.json` binds that cox also has. Shared by the TUI session and
/// `doctor` so both see the same map; a missing file is just the defaults.
pub fn keymap(cox_home: &Path, claude_home: &Path) -> cox_tui::keymap::Loaded {
    let toml = std::fs::read_to_string(cox_home.join("keybindings.toml")).ok();
    let claude = claude_settings::keybindings(claude_home);
    let mut loaded = cox_tui::keymap::load(toml.as_deref(), &claude.bindings);
    loaded.warnings.extend(claude.notices);
    loaded
}

/// Maps a clap long-flag name (without the leading `--`) to the dotted
/// config key it conceptually overrides (plan.md §1.12: "every flag maps to
/// a config key"). A key under `runtime.` is not a real `Config` field — it
/// documents that the flag is a per-invocation parameter, not persisted
/// config (the prompt text, `--continue`, ...); `apply_flags` below only
/// writes the ones that *are* real `Config` fields into the flag layer.
pub fn flag_key_map() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        // Global (plan.md §1.12 "Global:" row).
        ("provider", "tiers.code.provider"),
        ("model", "tiers.code.model"),
        ("tier", "tiers.<tier>.model"),
        ("sandbox", "sandbox.mode"),
        ("permission-mode", "permissions.mode"),
        ("mode", "core.mode"),
        ("approve", "permissions.approval"),
        ("budget", "budget.session_usd"),
        ("profile", "core.profile"),
        ("cwd", "core.workspace_roots"),
        ("add-dir", "core.workspace_roots"),
        // T27.3: resolved once in `main` into `--cwd`/`--add-dir`.
        ("worktree", "runtime.worktree"),
        ("home", "core.home"),
        ("verbose", "core.log_level"),
        ("no-hooks", "hooks.enabled"),
        ("no-mcp", "mcp.enabled"),
        ("no-plugins", "plugins.enabled"),
        ("plain", "tui.screen_reader"),
        // `cox init` (plan.md T25.6) headlessly.
        ("force", "runtime.force"),
        ("prompt", "runtime.prompt"),
        ("output-format", "runtime.output_format"),
        ("max-turns", "core.max_turns"),
        ("allowed-tools", "permissions.allow"),
        ("answer", "runtime.answer"),
        ("continue", "runtime.continue"),
        ("resume", "runtime.resume"),
        ("deep", "runtime.deep"),
        // T40.7: `cox run --image` attaches files to this one prompt.
        ("image", "runtime.image"),
        // T27.6: `cox run --loop`/`--max-iterations` are invocation
        // parameters (the loop's own state), not persisted config.
        ("loop", "runtime.loop"),
        ("max-iterations", "runtime.max_iterations"),
        // `cox stats --project` (T28.2): a read-only scope flag, not config.
        ("project", "runtime.project"),
    ])
}

/// Sets `root[dotted.path] = value`, creating intermediate objects as needed.
fn set_dotted(root: &mut JsonValue, dotted: &str, value: JsonValue) {
    let parts: Vec<&str> = dotted.split('.').collect();
    set_path(root, &parts, value);
}

/// Walks `parts`, replacing any non-object on the way with an empty object.
fn set_path(node: &mut JsonValue, parts: &[&str], value: JsonValue) {
    if !node.is_object() {
        *node = JsonValue::Object(Default::default());
    }
    let JsonValue::Object(map) = node else {
        return;
    };
    match parts {
        [] => {}
        [leaf] => {
            map.insert((*leaf).to_string(), value);
        }
        [head, rest @ ..] => {
            let child = map
                .entry((*head).to_string())
                .or_insert_with(|| JsonValue::Object(Default::default()));
            set_path(child, rest, value);
        }
    }
}

/// Builds the sparse CLI-flag override tree (only fields the user actually
/// passed), applying only entries that name a real `Config` field — the
/// `runtime.*`-mapped flags in [`flag_key_map`] are invocation parameters,
/// not config, and are left for the caller (T2.x) to read off `Cli` directly.
///
/// Looks each key up in [`flag_key_map`] (rather than repeating the dotted
/// strings inline) so the map stays the single source of truth for "which
/// key does this flag override" — `every_flag_has_a_config_key` checks the
/// map is complete; this checks the map is actually load-bearing.
pub fn flag_overrides(cli: &Cli) -> JsonValue {
    let keys = flag_key_map();
    let mut root = JsonValue::Object(Default::default());
    if let Some(provider) = &cli.provider {
        set_dotted(
            &mut root,
            keys["provider"],
            JsonValue::from(provider.clone()),
        );
    }
    if let Some(model) = &cli.model {
        set_dotted(&mut root, keys["model"], JsonValue::from(model.clone()));
    }
    for pair in &cli.tier {
        if let Some((tier, model)) = pair.split_once('=') {
            set_dotted(
                &mut root,
                &format!("tiers.{tier}.model"),
                JsonValue::from(model.to_string()),
            );
        }
    }
    if let Some(sandbox) = &cli.sandbox {
        set_dotted(&mut root, keys["sandbox"], JsonValue::from(sandbox.clone()));
    }
    if let Some(mode) = &cli.permission_mode {
        set_dotted(
            &mut root,
            keys["permission-mode"],
            JsonValue::from(mode.clone()),
        );
    }
    if let Some(mode) = &cli.mode {
        set_dotted(&mut root, keys["mode"], JsonValue::from(mode.clone()));
    }
    if let Some(approve) = &cli.approve {
        set_dotted(&mut root, keys["approve"], JsonValue::from(approve.clone()));
    }
    if let Some(budget) = cli.budget {
        set_dotted(&mut root, keys["budget"], JsonValue::from(budget));
    }
    if let Some(profile) = &cli.profile {
        set_dotted(&mut root, keys["profile"], JsonValue::from(profile.clone()));
    }
    if !cli.add_dir.is_empty() || cli.cwd.is_some() {
        let mut roots: Vec<JsonValue> = cli
            .add_dir
            .iter()
            .map(|p| JsonValue::from(p.display().to_string()))
            .collect();
        if let Some(cwd) = &cli.cwd {
            roots.push(JsonValue::from(cwd.display().to_string()));
        }
        set_dotted(&mut root, keys["add-dir"], JsonValue::Array(roots));
    }
    if let Some(home) = &cli.home {
        set_dotted(
            &mut root,
            keys["home"],
            JsonValue::from(home.display().to_string()),
        );
    }
    if cli.verbose > 0 {
        let level = if cli.verbose >= 2 { "trace" } else { "debug" };
        set_dotted(&mut root, keys["verbose"], JsonValue::from(level));
    }
    if cli.no_hooks {
        set_dotted(&mut root, keys["no-hooks"], JsonValue::from(false));
    }
    if cli.no_mcp {
        set_dotted(&mut root, keys["no-mcp"], JsonValue::from(false));
    }
    if cli.no_plugins {
        set_dotted(&mut root, keys["no-plugins"], JsonValue::from(false));
    }
    if cli.plain {
        set_dotted(&mut root, keys["plain"], JsonValue::from(true));
    }
    root
}

/// The imported `.claude/settings.json` files for `cwd`, if any exist, the
/// user's apart from the repository's (T22.11: the guard list treats the
/// repository's like project config). Broken files are warned about and
/// skipped (D14).
fn claude_layer(cwd: &Path) -> Option<ClaudeLayers> {
    let home = home_dir();
    let claude_home = home.join(".claude");
    // A home directory that is itself a git checkout (a dotfiles repo)
    // must not turn the user's own file into a repository's;
    // `find_git_root` returns a canonical path.
    let canonical_home = home.canonicalize().unwrap_or(home);
    let project = find_git_root(cwd).filter(|root| *root != canonical_home);
    let read = |paths: Vec<PathBuf>| {
        let settings = claude_settings::load(&paths);
        for notice in &settings.notices {
            eprintln!("cox: warning: {notice}");
        }
        (!settings.is_empty()).then(|| settings.to_layer())
    };
    let layers = ClaudeLayers {
        user: read(claude_settings::paths(Some(&claude_home), None)),
        project: read(claude_settings::paths(None, project.as_deref())),
    };
    (layers.user.is_some() || layers.project.is_some()).then_some(layers)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn config_every_flag_has_a_config_key() {
        let map = flag_key_map();
        let excluded = ["help", "version", "json", "verbose"];
        let mut missing = Vec::new();

        let cmd = Cli::command();
        for arg in cmd.get_arguments() {
            if arg.is_positional() {
                continue;
            }
            if let Some(long) = arg.get_long()
                && !excluded.contains(&long)
                && !map.contains_key(long)
            {
                missing.push(long.to_string());
            }
        }
        let run = cmd.find_subcommand("run").expect("run subcommand exists");
        for arg in run.get_arguments() {
            if arg.is_positional() {
                continue;
            }
            if let Some(long) = arg.get_long()
                && !excluded.contains(&long)
                && !map.contains_key(long)
            {
                missing.push(long.to_string());
            }
        }
        // T25.6: `cox init --force` registers its flag like `run`'s own.
        let init = cmd.find_subcommand("init").expect("init subcommand exists");
        for arg in init.get_arguments() {
            if arg.is_positional() {
                continue;
            }
            if let Some(long) = arg.get_long()
                && !excluded.contains(&long)
                && !map.contains_key(long)
            {
                missing.push(long.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "flags missing a config-key mapping: {missing:?}"
        );
    }

    #[test]
    fn mode_flag_maps_to_core_mode() {
        use clap::Parser;

        let cli = Cli::parse_from(["cox", "--mode", "architect"]);
        let layer = flag_overrides(&cli);
        assert_eq!(layer["core"]["mode"], JsonValue::from("architect"));
        assert_eq!(flag_key_map()["mode"], "core.mode");

        let parsed: cox_protocol::config::CoreConfig =
            serde_json::from_value(layer["core"].clone()).expect("core layer parses");
        assert_eq!(parsed.mode, cox_protocol::types::Mode::Architect);

        assert!(Cli::try_parse_from(["cox", "--mode", "chaos"]).is_err());
        assert!(
            flag_overrides(&Cli::parse_from(["cox"]))
                .get("core")
                .is_none()
        );
    }
}

#[cfg(test)]
mod claude_settings_tests {
    use clap::Parser;
    use cox_core::permission::{Engine, Outcome};
    use cox_protocol::Config;
    use cox_protocol::ids::CallId;
    use cox_protocol::types::{ApprovalPolicy, PermissionMode, Risk, SandboxMode, ToolCall};
    use std::fs;
    use tempfile::tempdir;

    use super::*;
    use crate::cli::Cli;

    fn rm_call() -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: serde_json::json!({ "command": "rm -rf build" }),
            risk: Risk::Exec,
            subject: "rm -rf build".into(),
            segments: None,
        }
    }

    fn deny_of(cfg: &Config, cwd: &Path) -> Option<String> {
        let engine = Engine::compile(&cfg.permissions, None, cwd).expect("engine");
        match engine.decide(
            &rm_call(),
            PermissionMode::Auto,
            ApprovalPolicy::Never,
            SandboxMode::WorkspaceWrite,
            &[],
        ) {
            Outcome::Deny { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// T7.5 step 4: the fixture yields the same decision as native rules,
    /// adds to (not replaces) the project's own list, and is labelled.
    #[test]
    fn config_claude_settings_import_matches_native_rules() {
        // T22.10: a project list that omits a default deny gets it back
        // ahead of its own rules, so each project list repeats the defaults
        // to keep the lists below in its own order.
        const DEFAULTS: &str = r#""Read(~/.ssh/**)", "Read(~/.aws/**)", "Bash(rm -rf /*)", "#;
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".cox")).expect("mkdir .cox");
        fs::create_dir_all(git_root.path().join(".claude")).expect("mkdir .claude");
        fs::write(
            git_root.path().join(".cox/config.toml"),
            format!("[permissions]\ndeny = [{DEFAULTS}\"Bash(curl *)\"]\n"),
        )
        .expect("write project config");
        fs::write(
            git_root.path().join(".claude/settings.json"),
            r#"{"permissions":{"deny":["Bash(rm -rf *)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#,
        )
        .expect("write settings");
        let native_dir = tempdir().expect("tempdir");
        fs::create_dir_all(native_dir.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(native_dir.path().join(".cox")).expect("mkdir .cox");
        fs::write(
            native_dir.path().join(".cox/config.toml"),
            format!("[permissions]\ndeny = [{DEFAULTS}\"Bash(curl *)\", \"Bash(rm -rf *)\"]\n"),
        )
        .expect("write native config");

        temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().unwrap())),
                ("HOME", Some(home.path().to_str().unwrap())),
            ],
            || {
                let cli = Cli::parse_from(["cox"]);
                let imported = load(git_root.path(), &cli).expect("load imported");
                let native = load(native_dir.path(), &cli).expect("load native");
                assert_eq!(
                    imported.config.permissions.deny,
                    native.config.permissions.deny
                );
                let denied = deny_of(&imported.config, git_root.path());
                assert!(denied.is_some(), "rm must be denied");
                assert_eq!(denied, deny_of(&native.config, native_dir.path()));
                // A list both layers feed keeps the first layer's label
                // (figment `adjoin`); a key only Claude sets is labelled.
                assert_eq!(imported.source_of("permissions.deny"), "project");
                assert_eq!(imported.source_of("hooks.Stop"), "claude-settings");
                assert_eq!(native.source_of("permissions.deny"), "project");
                assert_eq!(imported.config.hooks.events["Stop"][0].command, "say done");

                // The import is opt-out.
                fs::write(
                    git_root.path().join(".cox/config.toml"),
                    format!(
                        "[permissions]\ndeny = [{DEFAULTS}\"Bash(curl *)\"]\n\
                         import_claude_settings = false\n"
                    ),
                )
                .expect("rewrite project config");
                let off = load(git_root.path(), &cli).expect("load opt-out");
                assert_eq!(
                    off.config.permissions.deny,
                    [
                        "Read(~/.ssh/**)",
                        "Read(~/.aws/**)",
                        "Bash(rm -rf /*)",
                        "Bash(curl *)"
                    ]
                );
                assert!(off.config.hooks.events.is_empty());
            },
        );
    }

    /// Loads `git_root` with `home` as both `HOME` and `COX_HOME`, so the
    /// only `.claude` files are the ones a test wrote.
    fn load_isolated(home: &Path, git_root: &Path) -> LoadedConfig {
        let mut loaded = None;
        temp_env(
            &[
                ("COX_HOME", Some(home.to_str().unwrap())),
                ("HOME", Some(home.to_str().unwrap())),
            ],
            || loaded = Some(load(git_root, &Cli::parse_from(["cox"])).expect("load")),
        );
        loaded.expect("temp_env ran the closure")
    }

    /// T22.11 (A122): a repository's `.claude/settings.json` counts as
    /// project config: its `allow` is dropped and reported like a project
    /// `allow`, and its `deny` is added to the default deny.
    #[test]
    fn project_claude_settings_allow_is_dropped_and_its_deny_added() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".claude")).expect("mkdir .claude");
        fs::write(
            git_root.path().join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash"],"deny":["Bash(curl *)"]}}"#,
        )
        .expect("write settings");

        let loaded = load_isolated(home.path(), git_root.path());
        let permissions = &loaded.config.permissions;
        assert!(permissions.allow.is_empty(), "{:?}", permissions.allow);
        let v = loaded
            .violations
            .iter()
            .find(|v| v.key == "permissions.allow")
            .expect("an allow violation");
        assert_eq!(v.project_value, "Bash");
        assert_eq!(v.reason(), "A project may not allow a tool call");
        assert_eq!(
            permissions.deny,
            [
                "Read(~/.ssh/**)",
                "Read(~/.aws/**)",
                "Bash(rm -rf /*)",
                "Bash(curl *)"
            ]
        );
    }

    /// T22.11 (A122): the user's own `~/.claude/settings.json` allow rule
    /// still applies, while a repository's `.claude/settings.local.json`
    /// allow next to it is dropped.
    #[test]
    fn user_claude_settings_allow_still_applies() {
        let home = tempdir().expect("tempdir");
        let git_root = tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join(".claude")).expect("mkdir ~/.claude");
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(git status)"]}}"#,
        )
        .expect("write user settings");
        fs::create_dir_all(git_root.path().join(".git")).expect("mkdir .git");
        fs::create_dir_all(git_root.path().join(".claude")).expect("mkdir .claude");
        fs::write(
            git_root.path().join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":["Bash"]}}"#,
        )
        .expect("write local settings");

        let loaded = load_isolated(home.path(), git_root.path());
        assert_eq!(loaded.config.permissions.allow, ["Bash(git status)"]);
        let v = loaded
            .violations
            .iter()
            .find(|v| v.key == "permissions.allow")
            .expect("an allow violation");
        assert_eq!(v.project_value, "Bash(git status), Bash");
        assert_eq!(v.reverted_to, "Bash(git status)");
    }
}
