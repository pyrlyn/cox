// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox plugin` (plan.md T33.4, T33.7, T33.31, T33.32, T33.41): `list`, `install`,
//! `enable`, `disable`, `update`, `remove`, `link`. `install` and `update`
//! also take an https archive or a git repository (T53.2–T53.4), fetched
//! into staging by `plugin_fetch` and then installed like a local folder. `list` reports discovery results only — id, source, version,
//! digest and the manifest's *declared* capabilities, plus the real grant
//! state from `grant::check` (T33.7; T33.6 landed the check itself in
//! `cox-session`'s session-open path) — and never compiles or runs a
//! plugin's module (PL§1 line 47/445: a project plugin is untrusted
//! repository content and must not load before the user grants it).
//! `install`/`enable`/`disable`/`update`/`remove` are the only writers of
//! `plugin_grants` outside a session open. The files on disk change only
//! through `cox_plugin::install`; this module keeps the prompts and output. Every manifest string this module prints
//! (`name`, `description`, a capability line) goes through
//! `cox_sanitize::sanitize`, since a plugin's `plugin.toml` is untrusted
//! input the same way a tool result is (AGENTS "Trust boundaries").
//! `remove` (PL§1c) never needs the manifest to parse: a plugin whose
//! `plugin.toml` is corrupt must still be removable, so it is found by
//! whether its directory exists, not through `discover`.
//! `update`/`remove`'s printed lines are built by a shared, stdin-free core
//! (`update_core`/`remove_with`) that a `println!`-ing CLI wrapper and a
//! `_for_tui` wrapper both call (T33.33): the TUI has no terminal to
//! prompt on, so its wrapper always answers `yes` and never reaches
//! `confirm`'s stdin read. `list` already returned a `String`; nothing to
//! change there.

use std::fmt::Write as _;
use std::fs;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use cox_plugin::discover::{self, Source, State};
use cox_plugin::grant::{self, Verdict};
use cox_plugin::install;
use cox_plugin_api::{Capabilities, PluginManifest};
use cox_protocol::{Config, GrantScope, PluginGrant, PluginStore as _, Store as _};
use cox_sanitize::sanitize;
use cox_store::Store;

use crate::cli::Cli;
use crate::config_load::{self, cox_home, find_git_root};
use crate::confirm;
use crate::plugin_fetch;

/// `cox plugin list [--json]`.
pub fn list(cli: &Cli, cwd: &Path, json: bool) -> String {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let git_root = find_git_root(cwd);
    let found = discover::discover(&home, git_root.as_deref());
    // A store that fails to open counts as no grant for every plugin
    // (`grant::check`'s own rule for a read error), never a hard failure:
    // `list` must still show what discovery found.
    let store = Store::open(&home).ok();

    if json {
        let plugins: Vec<serde_json::Value> = found
            .plugins
            .iter()
            .map(|p| row_json(p, store.as_ref(), git_root.as_deref()))
            .collect();
        return serde_json::json!({ "plugins": plugins, "notices": found.notices }).to_string();
    }

    let mut out = String::new();
    if found.plugins.is_empty() {
        let _ = writeln!(out, "plugins: none");
    }
    for p in &found.plugins {
        let _ = writeln!(out, "{}", row_line(p, store.as_ref(), git_root.as_deref()));
    }
    for notice in &found.notices {
        let _ = writeln!(out, "notice: {notice}");
    }
    out
}

/// `cox plugin install <dir | https-url | git+url> [--sha256 <hex>]
/// [--rev <tag|commit>] [--path <subdir>] [--yes]` (PL§1): a URL (anything
/// with `://`) is checked for `https://` and a `--sha256` before a byte is
/// fetched, then downloaded and unpacked into staging (T53.2); a
/// `git+<url>` needs `--rev` and is cloned into staging (T53.3). Every
/// source ends in `install_tree`, the one install path.
pub fn install(
    cli: &Cli,
    source: &str,
    sha256: Option<&str>,
    rev: Option<&str>,
    path: Option<&str>,
    yes: bool,
) -> anyhow::Result<()> {
    if let Some(url) = source.strip_prefix("git+") {
        if sha256.is_some() {
            anyhow::bail!("--sha256 applies only to an https:// archive URL");
        }
        let rev =
            rev.ok_or_else(|| anyhow::anyhow!("--rev <tag|commit> is required with git+<url>"))?;
        return install_git(cli, url, rev, path.unwrap_or("."), yes);
    }
    if rev.is_some() || path.is_some() {
        anyhow::bail!("--rev and --path apply only to a git+<url> source");
    }
    if source.contains("://") {
        plugin_fetch::https_url(source)?;
        let sha256 = plugin_fetch::sha256_arg(sha256)?;
        return install_url(cli, source, &sha256, yes);
    }
    if sha256.is_some() {
        anyhow::bail!("--sha256 applies only to an https:// URL");
    }
    let dir = Path::new(source);
    // Absolute, so `update` can re-read the recorded source from any cwd.
    let dir = &fs::canonicalize(dir)
        .map_err(|e| anyhow::anyhow!("cannot install {}: {e}", dir.display()))?;
    install_tree(cli, dir, yes, |digest| {
        serde_json::json!({
            "kind": "path",
            "path": dir.display().to_string(),
            "digest": digest,
        })
    })
}

/// The URL half of `install`, after its scheme and hash checks — the seam
/// the tests reach a plain-http mock server through. Staging is dropped,
/// and so removed, on every return.
fn install_url(cli: &Cli, url: &str, sha256: &str, yes: bool) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let staging = plugin_fetch::Staging::new(&home)?;
    let root = plugin_fetch::fetch_archive(&staging, url, sha256)?;
    install_tree(
        cli,
        &root,
        yes,
        |_| serde_json::json!({ "kind": "url", "url": url, "sha256": sha256 }),
    )
}

/// The git half of `install` (T53.3): clones `rev` into staging,
/// confines `path` inside the clone and records the commit it resolved.
fn install_git(cli: &Cli, url: &str, rev: &str, path: &str, yes: bool) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let staging = plugin_fetch::Staging::new(&home)?;
    let (root, commit) = plugin_fetch::clone_git(&staging, url, rev, path)?;
    install_tree(cli, &root, yes, |_| {
        serde_json::json!({
            "kind": "git",
            "url": url,
            "rev": rev,
            "commit": commit,
            "path": path,
        })
    })
}

/// The one install path every source ends in (PL§1): validates and
/// digests `dir` through the same `discover::load_manifest` a discovered
/// plugin goes through, copies it into
/// `<home>/plugins/<id>/versions/<digest12>/`, writes `current` atomically
/// (temp file, then rename), then runs the same approval flow as `enable`,
/// recording `source(digest)` in the grant. Nothing in `dir` runs.
fn install_tree(
    cli: &Cli,
    dir: &Path,
    yes: bool,
    source: impl FnOnce(&str) -> serde_json::Value,
) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let manifest_path = dir.join("plugin.toml");
    let (manifest, digest) = discover::load_manifest(dir, &manifest_path, None)
        .map_err(|e| anyhow::anyhow!("cannot install {}: {e}", dir.display()))?;
    let digest12 = install::short(&digest);
    let plugin_dir = install::plugin_dir(&home, &manifest.id);
    let dest = install::stage(dir, &plugin_dir, &digest)?;
    install::activate(&plugin_dir, digest12)?;
    println!(
        "installed {} v{} (digest {digest12}) into {}",
        manifest.id,
        manifest.version,
        dest.display()
    );

    let store = Store::open(&home)?;
    let source = source(&digest);
    let mut out = Vec::new();
    let mut confirm_fn = |q: &str| confirm(q);
    decide(
        &mut out,
        &mut confirm_fn,
        &store,
        &manifest.id,
        &GrantScope::User,
        &digest,
        &manifest,
        source,
        yes,
    )?;
    for line in &out {
        println!("{line}");
    }
    Ok(())
}

/// `cox plugin link <dir> [--yes]` (T33.41, PL§13's dev loop): uses a
/// built plugin from `<dir>` in place, without staging or copying it.
/// Writes the `link` pointer through `cox_plugin::install::link`, then
/// runs the same approval flow as `install`, but keyed to the fixed
/// `discover::link_digest()` rather than `<dir>`'s package digest — a
/// rebuild changes that digest on every save, and the whole point of
/// linking is to never re-ask for that alone (`grant::check`'s
/// `is_linked`).
pub fn link(cli: &Cli, dir: &Path, yes: bool) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    // Absolute, so `discover` can read it from any cwd, the same reason
    // `install` canonicalizes its `dir`.
    let dir = &fs::canonicalize(dir)
        .map_err(|e| anyhow::anyhow!("cannot link {}: {e}", dir.display()))?;
    let manifest_path = dir.join("plugin.toml");
    let (manifest, _digest) = discover::load_manifest(dir, &manifest_path, None)
        .map_err(|e| anyhow::anyhow!("cannot link {}: {e}", dir.display()))?;
    let plugin_dir = install::plugin_dir(&home, &manifest.id);
    install::link(&plugin_dir, dir)?;
    println!(
        "linked {} v{} (dev) -> {}",
        manifest.id,
        manifest.version,
        dir.display()
    );

    let store = Store::open(&home)?;
    let source = serde_json::json!({
        "kind": "link",
        "path": dir.display().to_string(),
    });
    let mut out = Vec::new();
    let mut confirm_fn = |q: &str| confirm(q);
    decide(
        &mut out,
        &mut confirm_fn,
        &store,
        &manifest.id,
        &GrantScope::User,
        &discover::link_digest(),
        &manifest,
        source,
        yes,
    )?;
    for line in &out {
        println!("{line}");
    }
    Ok(())
}

/// `cox plugin enable <id> [--project] [--yes]` (PL§3). `--project` is
/// what makes a project plugin reachable at all: without it, discovery
/// never looks under `.cox/plugins/`, so a project-only id comes back
/// "not found" and no grant is written — a repository must not gain a
/// grant just by being visited (`project_plugin_needs_project_grant`).
pub fn enable(cli: &Cli, cwd: &Path, id: &str, project: bool, yes: bool) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let root = if project { find_git_root(cwd) } else { None };
    if project && root.is_none() {
        println!(
            "plugin {id}: --project needs a git repository at {}",
            cwd.display()
        );
        return Ok(());
    }
    let found = discover::discover(&home, root.as_deref());
    let Some(p) = found.plugins.iter().find(|p| p.id == id) else {
        println!(
            "plugin {id} not found{}",
            if project {
                ""
            } else {
                " (pass --project to grant a project plugin)"
            }
        );
        return Ok(());
    };
    let (manifest, digest) = match &p.state {
        State::Loaded { manifest, digest } => (manifest.as_ref(), digest.as_str()),
        State::Skipped { reason } => {
            println!("plugin {id} skipped: {reason}");
            return Ok(());
        }
    };
    // `discover` only returns a project plugin when `root` is `Some`, so
    // `grant::scope` always resolves here.
    let Some(scope) = grant::scope(p.source, root.as_deref()) else {
        println!("plugin {id}: no repository root to scope the grant to");
        return Ok(());
    };
    let store = Store::open(&home)?;
    // A linked plugin's grant is keyed to `link_digest()`, not its
    // (rebuild-volatile) package digest (T33.41) — `p.grant_digest()`
    // picks the right key; `grant::check` still gets the real `digest` so
    // it can tell a genuine mismatch from a linked one.
    let grant_digest = p.grant_digest().unwrap_or_else(|| digest.to_string());
    let stored = store.grant_get(id, &scope, &grant_digest).ok().flatten();
    if grant::check(manifest, digest, stored.as_ref()) == Verdict::Granted {
        println!("plugin {id} is already granted");
        return Ok(());
    }
    let source = stored
        .map(|g| g.source)
        .unwrap_or_else(|| default_source(p.source, p.dev, &p.dir));
    let mut out = Vec::new();
    let mut confirm_fn = |q: &str| confirm(q);
    decide(
        &mut out,
        &mut confirm_fn,
        &store,
        id,
        &scope,
        &grant_digest,
        manifest,
        source,
        yes,
    )?;
    for line in &out {
        println!("{line}");
    }
    Ok(())
}

/// `cox plugin disable <id> [--project]` (PL§1): clears `enabled` on the
/// grant row; the plugin's files and kv data stay untouched.
pub fn disable(cli: &Cli, cwd: &Path, id: &str, project: bool) -> anyhow::Result<()> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let root = if project { find_git_root(cwd) } else { None };
    let found = discover::discover(&home, root.as_deref());
    let Some(p) = found.plugins.iter().find(|p| p.id == id) else {
        println!("plugin {id} not found");
        return Ok(());
    };
    let State::Loaded { digest, .. } = &p.state else {
        println!("plugin {id} is skipped; nothing to disable");
        return Ok(());
    };
    let Some(scope) = grant::scope(p.source, root.as_deref()) else {
        println!("plugin {id}: no repository root to scope the grant to");
        return Ok(());
    };
    let store = Store::open(&home)?;
    // Same key `enable`/`list` use: a linked plugin's grant lives under
    // `link_digest()`, not the package digest (T33.41).
    let grant_digest = p.grant_digest().unwrap_or_else(|| digest.to_string());
    match store.grant_set_enabled(id, &scope, &grant_digest, false) {
        Ok(()) => println!("plugin {id} disabled"),
        Err(cox_protocol::StoreError::NotFound) => {
            println!("plugin {id} has no grant to disable");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// `cox plugin update [<id>… | --all] [--check] [--rollback] [--yes]`
/// (PL§1b, T33.31). User plugins only: a project plugin is read in place,
/// so there is nothing to update. Each id is handled on its own, so one
/// broken source does not stop the rest; any failure makes the exit
/// non-zero.
pub fn update(
    cli: &Cli,
    ids: &[String],
    all: bool,
    check: bool,
    rollback: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let mut out = Vec::new();
    let mut confirm_fn = |q: &str| confirm(q);
    let failed = update_core(
        cli,
        ids,
        all,
        check,
        rollback,
        yes,
        &mut confirm_fn,
        &mut out,
    )?;
    for line in &out {
        println!("{line}");
    }
    if failed {
        anyhow::bail!("not every plugin was updated");
    }
    Ok(())
}

/// `/plugin update` from the TUI (T33.33): always pre-approved, since the
/// TUI's raw mode has no stdin to prompt on — a widened capability list is
/// granted the same way `--yes` would grant it on the CLI. Returns the
/// lines `update` would have printed, joined for one notice; only a hard
/// failure (`Store::open`) is `Err` — a per-plugin failure still shows in
/// the text, so the caller does not need `update`'s "not every plugin was
/// updated" bail on top of it.
pub fn update_for_tui(
    cli: &Cli,
    ids: &[String],
    all: bool,
    check: bool,
    rollback: bool,
) -> anyhow::Result<String> {
    let mut out = Vec::new();
    update_core(
        cli,
        ids,
        all,
        check,
        rollback,
        true,
        &mut |_| true,
        &mut out,
    )?;
    Ok(out.join("\n"))
}

/// `update`'s and `update_for_tui`'s shared body (PL§1b, T33.31, T33.33):
/// builds the target list and runs each one through
/// `update_one`/`rollback_one`, collecting every line into `out` instead of
/// printing so both callers can present it their own way. Returns whether
/// any target failed.
#[allow(clippy::too_many_arguments)]
fn update_core(
    cli: &Cli,
    ids: &[String],
    all: bool,
    check: bool,
    rollback: bool,
    yes: bool,
    confirm: &mut dyn FnMut(&str) -> bool,
    out: &mut Vec<String>,
) -> anyhow::Result<bool> {
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let found = discover::discover(&home, None);
    let store = Store::open(&home)?;
    let targets: Vec<&str> = if all {
        found.plugins.iter().map(|p| p.id.as_str()).collect()
    } else {
        ids.iter().map(String::as_str).collect()
    };
    let mut failed = false;
    for id in targets {
        let Some(p) = found.plugins.iter().find(|p| p.id == id) else {
            out.push(format!(
                "plugin {id} not found (update covers installed user plugins)"
            ));
            failed = true;
            continue;
        };
        // A `cox plugin link`ed plugin (T33.41) has no staged source or
        // `previous` to switch between — it is read in place — so both
        // `update` and `--rollback` fail here with a specific message
        // instead of `update_one`'s generic "no recorded source path" or
        // `rollback_one`'s "no previous version".
        let done = if p.dev {
            Err(anyhow::anyhow!("linked plugin: rebuild in place"))
        } else if rollback {
            rollback_one(&store, &home, p, yes, confirm, out)
        } else {
            update_one(&store, &home, p, check, yes, confirm, out)
        };
        if let Err(e) = done {
            out.push(format!("plugin {id}: {e:#}"));
            failed = true;
        }
    }
    Ok(failed)
}

/// `cox plugin remove <id> [--keep-data] [--yes]` (PL§1c, T33.32). Found by
/// whether `<home>/plugins/<id>` or `<git root>/.cox/plugins/<id>` exists,
/// not through `discover`: a plugin whose `plugin.toml` no longer parses
/// must still be removable. Steps 2 and 4 of PL§1c collapse into one:
/// `grants_delete` removes every grant row for the id up front, which
/// already makes `grant::check` answer `NeedsApproval` for any digest, so
/// a concurrent session's next open cannot load it — the same effect
/// `disable` would have had, without needing to know which exact digest is
/// current. A project plugin's files are repository content (PL§1c): only
/// its grant and kv go, and the path is printed for the user to delete
/// with git.
pub fn remove(cli: &Cli, cwd: &Path, id: &str, keep_data: bool, yes: bool) -> anyhow::Result<()> {
    let mut confirm_fn = |q: &str| confirm(q);
    let out = remove_with(cli, cwd, id, keep_data, yes, &mut confirm_fn)?;
    for line in &out {
        println!("{line}");
    }
    Ok(())
}

/// `/plugin remove` from the TUI (T33.33, PL§1c): the confirm modal
/// already decided it, so this never prompts on stdin — `confirm` answers
/// yes unconditionally, the same effect a `--yes` CLI run has once the
/// modal said `y`. Returns the lines `remove` would have printed, joined
/// for one notice.
pub fn remove_for_tui(cli: &Cli, cwd: &Path, id: &str, keep_data: bool) -> anyhow::Result<String> {
    let out = remove_with(cli, cwd, id, keep_data, true, &mut |_| true)?;
    Ok(out.join("\n"))
}

/// `remove`'s and `remove_for_tui`'s shared body (PL§1c, T33.32, T33.33).
/// Collects its lines instead of printing so both callers can show them
/// their own way; `confirm` is only reached when `!yes`.
fn remove_with(
    cli: &Cli,
    cwd: &Path,
    id: &str,
    keep_data: bool,
    yes: bool,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    let home = cli.home.clone().unwrap_or_else(cox_home);
    let user_dir = install::plugin_dir(&home, id);
    let git_root = find_git_root(cwd);
    let project_dir = git_root
        .as_deref()
        .map(|root| root.join(".cox/plugins").join(id));
    let user_exists = user_dir.exists();
    let project_exists = project_dir.as_deref().is_some_and(Path::exists);
    if !user_exists && !project_exists {
        out.push(format!("plugin {id} not found"));
        return Ok(out);
    }
    if !yes && !confirm(&format!("remove plugin {id}?")) {
        out.push(format!("plugin {id} not removed"));
        return Ok(out);
    }

    let store = Store::open(&home)?;
    store.grants_delete(id)?;

    if user_exists {
        install::remove(&home, id)?;
    }
    if let Some(project_dir) = &project_dir
        && project_exists
    {
        out.push(format!(
            "plugin {id} is a project plugin; its files at {} stay — remove them with git if you want them gone",
            project_dir.display()
        ));
    }

    if keep_data {
        out.push(format!("plugin {id}: kept its stored data (--keep-data)"));
    } else {
        store.kv_delete_all(id)?;
    }

    out.push(format!("plugin {id} removed"));

    if let Ok(loaded) = config_load::load(cwd, cli) {
        for r in config_refs(&loaded.config, id) {
            out.push(format!("note: config still references {id}: {r}"));
        }
        for r in keybinding_refs(&home, id) {
            out.push(format!("note: keybindings.toml still references {id}: {r}"));
        }
    }
    Ok(out)
}

/// PL§1c step 6: what in the effective config still names `id` after
/// removal — never edited, only reported. Checked against the merged
/// `Config` rather than grepping the raw TOML text, so a reference through
/// a layered project or env override is still caught.
fn config_refs(config: &Config, id: &str) -> Vec<String> {
    let mut refs = Vec::new();
    if config.plugins.entries.contains_key(id) {
        refs.push(format!("[plugins.{id}]"));
    }
    if let Some(decide) = config
        .plugins
        .entries
        .get("decide")
        .and_then(|v| v.as_object())
    {
        for (point, plugin) in decide {
            if plugin.as_str() == Some(id) {
                refs.push(format!("[plugins.decide] {point} = \"{id}\""));
            }
        }
    }
    let prefix = format!("{id}-");
    let mut provider_names: Vec<&str> = Vec::new();
    for name in config.providers.custom.keys() {
        if name == id || name.starts_with(prefix.as_str()) {
            refs.push(format!("[providers.{name}]"));
            provider_names.push(name.as_str());
        }
    }
    for (tier_name, tier) in [
        ("cheap", &config.tiers.cheap),
        ("code", &config.tiers.code),
        ("think", &config.tiers.think),
    ] {
        if provider_names.contains(&tier.provider.as_str()) {
            refs.push(format!(
                "tiers.{tier_name}.provider = \"{}\"",
                tier.provider
            ));
        }
    }
    for name in config.mcp.servers.keys() {
        if name == id || name.starts_with(prefix.as_str()) {
            refs.push(format!("[mcp.servers.{name}]"));
        }
    }
    refs
}

/// PL§1c step 6's `keybindings.toml` rows for `plugin.<id>.*`: a plain
/// text scan, since `keybindings.toml` is a separate file `cox-config`
/// does not merge into `Config` (T25.5, `crate::config_load::keymap`).
fn keybinding_refs(home: &Path, id: &str) -> Vec<String> {
    let text = fs::read_to_string(home.join("keybindings.toml")).unwrap_or_default();
    let prefix = format!("plugin.{id}.");
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with(prefix.as_str()))
        .map(str::to_string)
        .collect()
}

/// PL§1b steps 1–8 for one plugin: re-read the source the current grant
/// recorded, validate and digest it through `discover::load_manifest`,
/// print the capability diff against that grant, then stage and switch.
/// Never called for a `cox plugin link`ed plugin (T33.41): `update`'s
/// caller intercepts those first.
#[allow(clippy::too_many_arguments)]
fn update_one(
    store: &Store,
    home: &Path,
    p: &discover::Plugin,
    check: bool,
    yes: bool,
    confirm: &mut dyn FnMut(&str) -> bool,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    let id = p.id.as_str();
    let current = match &p.state {
        State::Loaded { digest, .. } => digest.as_str(),
        State::Skipped { reason } => anyhow::bail!("current version is skipped: {reason}"),
    };
    let stored = store
        .grant_get(id, &GrantScope::User, current)
        .ok()
        .flatten();
    let recorded = stored.as_ref().map(|g| &g.source).ok_or_else(|| {
        anyhow::anyhow!("no recorded source path; reinstall with `cox plugin install <dir>`")
    })?;
    // `_staging` holds a fetched tree until this update returns.
    let (src, mut source, _staging) = reread(home, recorded)?;
    let src = src.as_path();
    let (manifest, digest) = discover::load_manifest(src, &src.join("plugin.toml"), Some(id))
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", src.display()))?;
    if digest == current {
        out.push(format!(
            "plugin {id} is up to date ({})",
            install::short(current)
        ));
        return Ok(());
    }
    out.push(format!(
        "plugin {id}: {} -> {} (v{})",
        install::short(current),
        install::short(&digest),
        sanitize(&manifest.version)
    ));
    diff_lines(&manifest, &digest, stored.as_ref(), out);
    if check {
        return Ok(());
    }
    let plugin_dir = install::plugin_dir(home, id);
    install::stage(src, &plugin_dir, &digest)?;
    if source["kind"] == "path" {
        source["digest"] = serde_json::json!(digest);
    }
    switch_to(
        store,
        &plugin_dir,
        id,
        (&manifest, &digest),
        source,
        yes,
        "",
        confirm,
        out,
    )
}

/// PL§1b step 1 per source kind (PL§1, T53.4): the directory to digest,
/// the source to record for the version it holds, and the staging a
/// fetched tree lives in. A URL is fetched at the same URL and hash, so
/// changed bytes there are a mismatch, never a silent update; a git source
/// is cloned at the same `rev`, so a tag that moved yields a new digest
/// and asks for the grant again. The URL's scheme is not checked again:
/// `install` checked it before recording it.
fn reread(
    home: &Path,
    recorded: &serde_json::Value,
) -> anyhow::Result<(PathBuf, serde_json::Value, Option<plugin_fetch::Staging>)> {
    let field = |key: &str| {
        recorded[key]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("the recorded source has no {key}; reinstall it"))
    };
    match recorded["kind"].as_str() {
        Some("path") => Ok((PathBuf::from(field("path")?), recorded.clone(), None)),
        Some("url") => {
            let url = field("url")?;
            let sha256 = plugin_fetch::sha256_arg(Some(field("sha256")?))?;
            let staging = plugin_fetch::Staging::new(home)?;
            let root = plugin_fetch::fetch_archive(&staging, url, &sha256).map_err(|e| {
                anyhow::anyhow!(
                    "{e:#}; a new version installs with `cox plugin install <url> --sha256 <hex>`"
                )
            })?;
            Ok((root, recorded.clone(), Some(staging)))
        }
        Some("git") => {
            let staging = plugin_fetch::Staging::new(home)?;
            let (root, commit) =
                plugin_fetch::clone_git(&staging, field("url")?, field("rev")?, field("path")?)?;
            let mut source = recorded.clone();
            source["commit"] = serde_json::json!(commit);
            Ok((root, source, Some(staging)))
        }
        _ => anyhow::bail!("no recorded source path; reinstall with `cox plugin install <dir>`"),
    }
}

/// `--rollback`: make `previous` current again. Its grant is keyed on its
/// own digest (PL§3), so a grant still on file lets it switch without
/// asking; a revoked or missing one asks like any other new digest.
fn rollback_one(
    store: &Store,
    home: &Path,
    p: &discover::Plugin,
    yes: bool,
    confirm: &mut dyn FnMut(&str) -> bool,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    let id = p.id.as_str();
    let plugin_dir = install::plugin_dir(home, id);
    let Some(previous) = install::read_pointer(&plugin_dir, install::PREVIOUS)? else {
        out.push(format!(
            "plugin {id} has no previous version to roll back to"
        ));
        return Ok(());
    };
    let dir = plugin_dir.join("versions").join(&previous);
    // Digested again rather than trusted by name: a tampered directory
    // gets a different digest, so no stored grant matches it.
    let (manifest, digest) = discover::load_manifest(&dir, &dir.join("plugin.toml"), Some(id))
        .map_err(|e| anyhow::anyhow!("cannot read previous version: {e}"))?;
    let source = match &p.state {
        State::Loaded { digest, .. } => store
            .grant_get(id, &GrantScope::User, digest)
            .ok()
            .flatten()
            .map(|g| g.source),
        State::Skipped { .. } => None,
    }
    .unwrap_or_else(|| default_source(Source::User, false, &dir));
    switch_to(
        store,
        &plugin_dir,
        id,
        (&manifest, &digest),
        source,
        yes,
        " --rollback",
        confirm,
        out,
    )
}

/// Makes a staged version current once its grant allows it: a grant on
/// file for that exact digest switches at once; otherwise the user is
/// asked through `decide`. Headless never approves (PL§1b): stdin that is
/// not a terminal — `cox run -p`, ACP, a pipe, CI — gets no prompt at all,
/// even if it could supply a `y`, so `current` stays and a warning names
/// the command to run. Only `--yes`, a user's own provisioning script,
/// approves without a terminal.
#[allow(clippy::too_many_arguments)]
fn switch_to(
    store: &Store,
    plugin_dir: &Path,
    id: &str,
    (manifest, digest): (&PluginManifest, &str),
    source: serde_json::Value,
    yes: bool,
    flag: &str,
    confirm: &mut dyn FnMut(&str) -> bool,
    out: &mut Vec<String>,
) -> anyhow::Result<()> {
    let digest12 = install::short(digest);
    let own = store
        .grant_get(id, &GrantScope::User, digest)
        .ok()
        .flatten();
    let approved = if grant::check(manifest, digest, own.as_ref()) == Verdict::Granted {
        true
    } else if !yes && !std::io::stdin().is_terminal() {
        out.push(format!(
            "warning: update for {id} waits for approval: run `cox plugin update {id}{flag}`"
        ));
        return Ok(());
    } else {
        decide(
            out,
            confirm,
            store,
            id,
            &GrantScope::User,
            digest,
            manifest,
            source,
            yes,
        )?
    };
    if approved {
        install::activate(plugin_dir, digest12)?;
        out.push(format!("plugin {id} is now at {digest12}"));
    }
    Ok(())
}

/// PL§1b step 4: what the new manifest asks for beyond the stored grant
/// (`+`, first so it stands out) and what it no longer asks for (`-`).
/// Computed by `grant::check` itself so the diff and the load decision
/// never disagree; a disabled grant is diffed as if enabled, since
/// `Disabled` would otherwise hide the capabilities it granted.
fn diff_lines(
    manifest: &PluginManifest,
    digest: &str,
    stored: Option<&PluginGrant>,
    out: &mut Vec<String>,
) {
    let active = stored.map(|g| PluginGrant {
        enabled: true,
        ..g.clone()
    });
    let Verdict::NeedsApproval { added, removed } = grant::check(manifest, digest, active.as_ref())
    else {
        return;
    };
    if added.is_empty() && removed.is_empty() {
        out.push("  capabilities unchanged; the new bytes still need approval".to_string());
    }
    for cap in &added {
        out.push(format!("  + {} (new)", sanitize(cap)));
    }
    for cap in &removed {
        out.push(format!("  - {}", sanitize(cap)));
    }
}

/// Prints the capability list in words and asks on stdin unless `yes`;
/// on approval, upserts the grant (`grant_put` replaces any row at the
/// same `(plugin_id, scope, digest)`, PL§3) and returns whether it is now
/// enabled. A decline writes nothing, so a later `enable` sees a fresh
/// `NeedsApproval`, not a stale `Disabled`.
#[allow(clippy::too_many_arguments)]
fn decide(
    out: &mut Vec<String>,
    confirm: &mut dyn FnMut(&str) -> bool,
    store: &Store,
    id: &str,
    scope: &GrantScope,
    digest: &str,
    manifest: &PluginManifest,
    source: serde_json::Value,
    yes: bool,
) -> anyhow::Result<bool> {
    let caps = grant::capability_list(manifest);
    out.push(format!(
        "{} ({})",
        sanitize(&manifest.name),
        sanitize(&manifest.version)
    ));
    if !manifest.description.is_empty() {
        out.push(format!("  {}", sanitize(&manifest.description)));
    }
    if caps.is_empty() {
        out.push("  asks for no capabilities".to_string());
    } else {
        out.push("  asks to be able to:".to_string());
        for cap in &caps {
            out.push(format!("    - {}", sanitize(cap)));
        }
    }
    let approved = yes || confirm(&format!("grant {id} these capabilities?"));
    if !approved {
        out.push(format!("plugin {id} not enabled"));
        return Ok(false);
    }
    write_grant(store, id, scope, digest, caps, source)?;
    out.push(format!("plugin {id} enabled"));
    Ok(true)
}

/// The one place a `PluginGrant` row is assembled (PL§3, T33.8), shared
/// with the TUI's grant dialog (`session.rs`'s `write_plugin_grant`).
pub(crate) use cox_session::write_grant;

/// The `source` a grant records when nothing was stored yet: a project
/// plugin is repository content, so there is no external path to
/// remember; a user plugin's is its version directory, the best guess
/// available to a bare `enable` that did not go through `install`. `dev`
/// is `p.dev` (T33.41): a plugin found through a `link` pointer gets
/// `{"kind": "link", ...}`, not `{"kind": "path", ...}`, so a bare
/// `enable` on one — bypassing `cox plugin link` — still records the
/// shape `grant::check`'s `is_linked` looks for.
fn default_source(source: Source, dev: bool, dir: &Path) -> serde_json::Value {
    match source {
        Source::Project => serde_json::json!({ "kind": "project" }),
        Source::User if dev => {
            serde_json::json!({ "kind": "link", "path": dir.display().to_string() })
        }
        Source::User => serde_json::json!({ "kind": "path", "path": dir.display().to_string() }),
    }
}

/// The grant verdict for one discovered, loaded plugin: looks up the row
/// at its grant digest (`p.grant_digest()`: T33.6's own key for a staged
/// plugin, or the fixed `link_digest()` for one found through `cox plugin
/// link`, T33.41) and runs it through `grant::check`, the one pure answer
/// every surface shares, passing the real `digest` so it can still tell a
/// genuine mismatch from a linked one. A missing or unreadable store
/// counts as no grant, never a wider one. `pub(crate)` so
/// `doctor::check_plugins` and `ext_cmd::list` (T33.39) share this same
/// walk instead of a third verdict implementation.
pub(crate) fn verdict_for(
    p: &discover::Plugin,
    manifest: &PluginManifest,
    digest: &str,
    store: Option<&Store>,
    project_root: Option<&Path>,
) -> Verdict {
    let grant_digest = p.grant_digest().unwrap_or_else(|| digest.to_string());
    let stored = grant::scope(p.source, project_root).and_then(|scope| {
        store.and_then(|s| s.grant_get(&p.id, &scope, &grant_digest).ok().flatten())
    });
    grant::check(manifest, digest, stored.as_ref())
}

/// Counts of what a manifest's `[capabilities]` declares, never what a
/// plugin actually does at runtime: nothing here has executed the module.
fn declared_summary(caps: &Capabilities) -> String {
    let counted = [
        ("tools", caps.tools.len()),
        ("events", caps.events.len()),
        ("hooks", caps.hooks.len()),
        ("invoke", caps.invoke.len()),
        ("decide", caps.decide.len()),
        ("render", caps.ui.render.len()),
    ];
    let mut parts: Vec<String> = counted
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(name, n)| format!("{name}={n}"))
        .collect();
    for (flag, name) in [
        (caps.ui.status, "status"),
        (caps.ui.panel, "panel"),
        (caps.ui.overlay, "overlay"),
        (caps.ui.commands, "commands"),
        (caps.ui.keys, "keys"),
    ] {
        if flag {
            parts.push(name.to_string());
        }
    }
    if parts.is_empty() {
        "no declared capabilities".to_string()
    } else {
        parts.join(" ")
    }
}

/// `("granted"|"disabled"|"needs approval (…)", "loaded"|"not loaded")`.
fn grant_words(v: &Verdict) -> (String, &'static str) {
    match v {
        Verdict::Granted => ("granted".to_string(), "loaded"),
        Verdict::Disabled => ("disabled".to_string(), "not loaded"),
        Verdict::NeedsApproval { added, .. } if added.is_empty() => {
            ("needs approval (package changed)".to_string(), "not loaded")
        }
        Verdict::NeedsApproval { added, .. } => (
            format!("needs approval ({})", added.join(", ")),
            "not loaded",
        ),
    }
}

/// " dev" for a `cox plugin link`ed plugin (T33.41), shown in every
/// listing surface: `list`'s text and JSON here, `cox doctor` and the TUI
/// grant dialog (`PluginGrantDialog`) elsewhere.
fn dev_tag(dev: bool) -> &'static str {
    if dev { ", dev" } else { "" }
}

fn row_line(p: &discover::Plugin, store: Option<&Store>, project_root: Option<&Path>) -> String {
    match &p.state {
        State::Skipped { reason } => format!(
            "{} ({}{}): skipped — {reason}",
            p.id,
            p.source,
            dev_tag(p.dev)
        ),
        State::Loaded { manifest, digest } => {
            let digest12 = &digest[..12];
            let verdict = verdict_for(p, manifest, digest, store, project_root);
            let (grant_word, loaded_word) = grant_words(&verdict);
            format!(
                "{} ({}{}, v{}, digest {digest12}, grant {grant_word}, {loaded_word}): {}",
                p.id,
                p.source,
                dev_tag(p.dev),
                manifest.version,
                declared_summary(&manifest.capabilities)
            )
        }
    }
}

fn row_json(
    p: &discover::Plugin,
    store: Option<&Store>,
    project_root: Option<&Path>,
) -> serde_json::Value {
    match &p.state {
        State::Skipped { reason } => serde_json::json!({
            "id": p.id,
            "source": p.source.to_string(),
            "state": "skipped",
            "reason": reason,
            "dev": p.dev,
        }),
        State::Loaded { manifest, digest } => {
            let verdict = verdict_for(p, manifest, digest, store, project_root);
            let (grant_state, loaded, added, removed) = match &verdict {
                Verdict::Granted => ("granted", true, None, None),
                Verdict::Disabled => ("disabled", false, None, None),
                Verdict::NeedsApproval { added, removed } => {
                    ("needs_approval", false, Some(added), Some(removed))
                }
            };
            let mut v = serde_json::json!({
                "id": p.id,
                "source": p.source.to_string(),
                "version": manifest.version,
                "digest": digest,
                "digest12": &digest[..12],
                "grant": grant_state,
                "loaded": loaded,
                "state": "discovered",
                "dev": p.dev,
                "declared": serde_json::to_value(&manifest.capabilities).unwrap_or_default(),
            });
            if let (Some(added), Some(removed)) = (added, removed) {
                v["grant_added"] = serde_json::json!(added);
                v["grant_removed"] = serde_json::json!(removed);
            }
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_summary_lists_only_nonempty_kinds() {
        let caps = Capabilities {
            tools: vec!["summarise".to_string()],
            ..Capabilities::default()
        };
        assert_eq!(declared_summary(&caps), "tools=1");
        assert_eq!(
            declared_summary(&Capabilities::default()),
            "no declared capabilities"
        );
    }

    #[test]
    fn grant_words_name_loaded_only_when_granted() {
        assert_eq!(
            grant_words(&Verdict::Granted),
            ("granted".to_string(), "loaded")
        );
        assert_eq!(
            grant_words(&Verdict::Disabled),
            ("disabled".to_string(), "not loaded")
        );
        assert_eq!(
            grant_words(&Verdict::NeedsApproval {
                added: vec!["kv".to_string()],
                removed: Vec::new(),
            }),
            ("needs approval (kv)".to_string(), "not loaded")
        );
    }

    #[test]
    fn config_refs_reports_plugins_table_and_decide_entry_and_edits_nothing() {
        let mut config = Config::default();
        config
            .plugins
            .entries
            .insert("git-glance".to_string(), serde_json::json!({"foo": 1}));
        config.plugins.entries.insert(
            "decide".to_string(),
            serde_json::json!({"route": "git-glance"}),
        );

        let refs = config_refs(&config, "git-glance");

        assert!(
            refs.contains(&"[plugins.git-glance]".to_string()),
            "{refs:?}"
        );
        assert!(
            refs.iter()
                .any(|r| r.contains("decide") && r.contains("route")),
            "{refs:?}"
        );
        // Unrelated ids never show up.
        assert!(config_refs(&config, "other-id").is_empty());
    }

    #[test]
    fn config_refs_reports_a_provider_section_and_the_tier_naming_it() {
        let mut config = Config::default();
        config.providers.custom.insert(
            "git-glance-relay".to_string(),
            cox_protocol::config::CompatibleProviderConfig::default(),
        );
        config.tiers.code.provider = "git-glance-relay".to_string();

        let refs = config_refs(&config, "git-glance");

        assert!(
            refs.contains(&"[providers.git-glance-relay]".to_string()),
            "{refs:?}"
        );
        assert!(
            refs.contains(&"tiers.code.provider = \"git-glance-relay\"".to_string()),
            "{refs:?}"
        );
    }

    #[test]
    fn config_refs_reports_an_mcp_server_named_after_the_plugin() {
        let mut config = Config::default();
        config
            .mcp
            .servers
            .insert("git-glance-relay".to_string(), Default::default());

        let refs = config_refs(&config, "git-glance");

        assert!(
            refs.contains(&"[mcp.servers.git-glance-relay]".to_string()),
            "{refs:?}"
        );
    }

    #[test]
    fn keybinding_refs_reports_rows_for_the_plugin_only() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("keybindings.toml"),
            "plugin.git-glance.status = \"ctrl-g\"\nplugin.other.status = \"ctrl-o\"\n",
        )
        .unwrap();

        let refs = keybinding_refs(home.path(), "git-glance");

        assert_eq!(refs, vec!["plugin.git-glance.status = \"ctrl-g\""]);
    }

    const MANIFEST: &str =
        "api = 1\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nwasm = \"plugin.wasm\"\n";

    fn cli_at(home: &Path) -> Cli {
        use clap::Parser as _;
        Cli::parse_from(["cox", "--home", home.to_str().unwrap()])
    }

    /// A plain ustar archive (`tar -xf` reads it as readily as a `.tar.gz`)
    /// written by hand, so a test can hold the entries a well-behaved `tar`
    /// refuses to create: `(name, type flag, link target, bytes)`, with
    /// `b'0'` a file and `b'2'` a symlink.
    fn ustar(entries: &[(&str, u8, &str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, kind, link, data) in entries {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            h[108..115].copy_from_slice(b"0000000");
            h[116..123].copy_from_slice(b"0000000");
            h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            h[136..147].copy_from_slice(b"00000000000");
            h[156] = *kind;
            h[157..157 + link.len()].copy_from_slice(link.as_bytes());
            h[257..263].copy_from_slice(b"ustar\0");
            h[263..265].copy_from_slice(b"00");
            h[148..156].fill(b' ');
            let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(data);
            out.resize(out.len().next_multiple_of(512), 0);
        }
        out.resize(out.len() + 1024, 0);
        out
    }

    fn package(prefix: &str) -> Vec<u8> {
        ustar(&[
            (
                &format!("{prefix}plugin.toml"),
                b'0',
                "",
                MANIFEST.as_bytes(),
            ),
            (
                &format!("{prefix}plugin.wasm"),
                b'0',
                "",
                b"dummy wasm bytes",
            ),
        ])
    }

    /// A mock server that outlives the test body: `server` drops (and
    /// verifies) before the runtime serving it.
    struct Served {
        server: wiremock::MockServer,
        rt: tokio::runtime::Runtime,
    }

    impl Served {
        fn url(&self) -> String {
            format!("{}/demo.tar.gz", self.server.uri())
        }

        fn hits(&self) -> usize {
            self.rt
                .block_on(self.server.received_requests())
                .unwrap_or_default()
                .len()
        }
    }

    impl Served {
        /// The same URL now serves `bytes`.
        fn replace(&self, bytes: Vec<u8>) {
            self.rt.block_on(async {
                self.server.reset().await;
                mount(&self.server, bytes).await;
            });
        }
    }

    async fn mount(server: &wiremock::MockServer, bytes: Vec<u8>) {
        wiremock::Mock::given(wiremock::matchers::path("/demo.tar.gz"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(server)
            .await;
    }

    fn serve(bytes: Vec<u8>) -> Served {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(async {
            let server = wiremock::MockServer::start().await;
            mount(&server, bytes).await;
            server
        });
        Served { server, rt }
    }

    /// Nothing was installed and staging is gone.
    fn assert_nothing_left(home: &Path) {
        assert!(!home.join("plugins/demo").exists(), "nothing installed");
        assert!(
            !home.join("plugins/.staging").exists(),
            "staging is removed on every exit"
        );
    }

    #[test]
    fn plugin_install_url_rejects_a_hash_mismatch() {
        let home = tempfile::tempdir().unwrap();
        let served = serve(package(""));

        let err = install_url(&cli_at(home.path()), &served.url(), &"0".repeat(64), true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(err.contains("nothing was unpacked"), "{err}");
        assert_eq!(served.hits(), 1);
        assert_nothing_left(home.path());
    }

    #[test]
    fn plugin_install_url_rejects_http() {
        let home = tempfile::tempdir().unwrap();
        let bytes = package("");
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);
        let cli = cli_at(home.path());

        for url in [served.url(), "file:///etc/passwd".to_string()] {
            let err = install(&cli, &url, Some(&sha), None, None, true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("must be https://"), "{err}");
        }
        let https = served.url().replacen("http://", "https://", 1);
        let err = install(&cli, &https, None, None, None, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--sha256 <hex> is required"), "{err}");

        assert_eq!(served.hits(), 0, "refused before a byte is fetched");
        assert!(!home.path().join("plugins").exists());
    }

    #[test]
    fn plugin_install_url_rejects_a_symlink_entry() {
        let home = tempfile::tempdir().unwrap();
        let bytes = ustar(&[
            ("plugin.toml", b'0', "", MANIFEST.as_bytes()),
            ("plugin.wasm", b'2', "/etc/passwd", b""),
        ]);
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);

        let err = install_url(&cli_at(home.path()), &served.url(), &sha, true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("not a plain file or directory"), "{err}");
        assert_nothing_left(home.path());
    }

    #[test]
    fn plugin_install_url_rejects_dot_dot_entries() {
        let home = tempfile::tempdir().unwrap();
        let bytes = ustar(&[
            ("plugin.toml", b'0', "", MANIFEST.as_bytes()),
            ("../../../escaped", b'0', "", b"out"),
        ]);
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);

        let err = install_url(&cli_at(home.path()), &served.url(), &sha, true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("leaves the staging directory"), "{err}");
        assert!(!home.path().join("escaped").exists());
        assert!(!home.path().join("plugins/escaped").exists());
        assert_nothing_left(home.path());
    }

    #[test]
    fn plugin_install_url_records_the_source() {
        let home = tempfile::tempdir().unwrap();
        let bytes = package("demo-0.1.0/");
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);

        install_url(&cli_at(home.path()), &served.url(), &sha, true).unwrap();

        let plugin_dir = home.path().join("plugins/demo");
        let current = fs::read_to_string(plugin_dir.join("current")).unwrap();
        let version = plugin_dir.join("versions").join(current.trim());
        assert!(version.join("plugin.wasm").is_file());
        assert!(
            !version.join("package.tar").exists(),
            "only the tree, never the archive"
        );
        let digest = cox_plugin::package_digest(&version).unwrap();
        let grant = Store::open(home.path())
            .unwrap()
            .grant_get("demo", &GrantScope::User, &digest)
            .unwrap()
            .expect("granted with --yes");
        assert_eq!(
            grant.source,
            serde_json::json!({ "kind": "url", "url": served.url(), "sha256": sha })
        );
        assert!(!home.path().join("plugins/.staging").exists());
    }

    /// `git` for building a fixture repository, isolated from the
    /// developer's own git config and signing setup.
    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A `file://` bare repository holding `files`, tagged `v1` and with a
    /// `stable` branch, plus the commit both name.
    fn bare_repo(dir: &Path, files: &[(&str, &str)]) -> (String, String) {
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        git_in(&work, &["init", "--quiet"]);
        for (rel, body) in files {
            let path = work.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        git_in(&work, &["add", "-A"]);
        git_in(&work, &["commit", "--quiet", "-m", "plugin"]);
        git_in(&work, &["tag", "v1"]);
        git_in(&work, &["branch", "stable"]);
        let commit = git_in(&work, &["rev-parse", "HEAD"]);
        git_in(dir, &["clone", "--quiet", "--bare", "work", "repo.git"]);
        let url = format!("file://{}", dir.join("repo.git").display());
        (url, commit)
    }

    const PLUGIN_FILES: [(&str, &str); 2] = [
        ("plugin.toml", MANIFEST),
        ("plugin.wasm", "dummy wasm bytes"),
    ];

    fn grant_source(home: &Path) -> serde_json::Value {
        let plugin_dir = home.join("plugins/demo");
        let current = fs::read_to_string(plugin_dir.join("current")).unwrap();
        let digest =
            cox_plugin::package_digest(&plugin_dir.join("versions").join(current.trim())).unwrap();
        Store::open(home)
            .unwrap()
            .grant_get("demo", &GrantScope::User, &digest)
            .unwrap()
            .expect("granted with --yes")
            .source
    }

    #[test]
    fn plugin_install_git_from_a_local_bare_repo() {
        let repos = tempfile::tempdir().unwrap();
        let (url, commit) = bare_repo(repos.path(), &PLUGIN_FILES);

        let home = tempfile::tempdir().unwrap();
        let source = format!("git+{url}");
        install(&cli_at(home.path()), &source, None, Some("v1"), None, true).unwrap();
        assert_eq!(
            grant_source(home.path()),
            serde_json::json!({
                "kind": "git", "url": url, "rev": "v1", "commit": commit, "path": ".",
            })
        );
        assert!(!home.path().join("plugins/.staging").exists());

        // A full commit hash is fetched directly.
        let home = tempfile::tempdir().unwrap();
        install(
            &cli_at(home.path()),
            &source,
            None,
            Some(&commit),
            None,
            true,
        )
        .unwrap();
        assert_eq!(grant_source(home.path())["commit"], commit.as_str());
    }

    #[test]
    fn plugin_install_git_refuses_a_branch() {
        let repos = tempfile::tempdir().unwrap();
        let (url, _) = bare_repo(repos.path(), &PLUGIN_FILES);
        let home = tempfile::tempdir().unwrap();
        let cli = cli_at(home.path());
        let source = format!("git+{url}");

        let err = install(&cli, &source, None, Some("stable"), None, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("names a branch"), "{err}");
        let err = install(&cli, &source, None, None, None, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--rev <tag|commit> is required"), "{err}");
        let err = install(&cli, &source, None, Some("abc123"), None, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("neither a tag"), "{err}");
        assert_nothing_left(home.path());
    }

    #[cfg(unix)]
    #[test]
    fn plugin_install_git_path_cannot_escape_the_clone() {
        let repos = tempfile::tempdir().unwrap();
        let outside = repos.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("plugin.toml"), MANIFEST).unwrap();
        fs::write(outside.join("plugin.wasm"), "outside bytes").unwrap();
        let work = repos.path().join("work");
        fs::create_dir_all(&work).unwrap();
        std::os::unix::fs::symlink(&outside, work.join("link")).unwrap();
        let (url, _) = bare_repo(repos.path(), &[("pkg/plugin.toml", MANIFEST)]);
        let home = tempfile::tempdir().unwrap();
        let cli = cli_at(home.path());
        let source = format!("git+{url}");

        for path in ["../outside", "/etc", "pkg/../../outside"] {
            let err = install(&cli, &source, None, Some("v1"), Some(path), true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("must stay inside the clone"), "{path}: {err}");
        }
        let err = install(&cli, &source, None, Some("v1"), Some("link"), true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory inside the clone"), "{err}");
        assert_nothing_left(home.path());
    }

    #[test]
    fn plugin_install_git_digest_excludes_dot_git() {
        let repos = tempfile::tempdir().unwrap();
        let (url, _) = bare_repo(repos.path(), &PLUGIN_FILES);
        let home = tempfile::tempdir().unwrap();

        install(
            &cli_at(home.path()),
            &format!("git+{url}"),
            None,
            Some("v1"),
            None,
            true,
        )
        .unwrap();

        let plain = tempfile::tempdir().unwrap();
        for (rel, body) in PLUGIN_FILES {
            fs::write(plain.path().join(rel), body).unwrap();
        }
        let want = cox_plugin::package_digest(plain.path()).unwrap();
        let versions = home.path().join("plugins/demo/versions");
        let version = versions.join(install::short(&want));
        assert!(version.is_dir(), "the digest is the tree's alone");
        assert!(!version.join(".git").exists());
    }

    /// `update demo` as the CLI runs it, collecting its lines; `confirm`
    /// declines, so only `yes` or a stored grant can switch.
    fn update_demo(home: &Path, check: bool, yes: bool) -> (bool, String) {
        let mut out = Vec::new();
        let failed = update_core(
            &cli_at(home),
            &["demo".to_string()],
            false,
            check,
            false,
            yes,
            &mut |_| false,
            &mut out,
        )
        .unwrap();
        (failed, out.join("\n"))
    }

    fn current(home: &Path) -> String {
        fs::read_to_string(home.join("plugins/demo/current")).unwrap()
    }

    fn version_count(home: &Path) -> usize {
        fs::read_dir(home.join("plugins/demo/versions"))
            .unwrap()
            .count()
    }

    #[test]
    fn plugin_update_url_same_bytes_is_up_to_date() {
        let home = tempfile::tempdir().unwrap();
        let bytes = package("");
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);
        install_url(&cli_at(home.path()), &served.url(), &sha, true).unwrap();

        let (failed, out) = update_demo(home.path(), false, false);

        assert!(!failed, "{out}");
        assert!(out.contains("plugin demo is up to date"), "{out}");
        assert_eq!(served.hits(), 2, "the URL is fetched again");
        assert!(!home.path().join("plugins/.staging").exists());
    }

    #[test]
    fn plugin_update_url_changed_bytes_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let bytes = package("");
        let sha = plugin_fetch::sha256_hex(&bytes);
        let served = serve(bytes);
        install_url(&cli_at(home.path()), &served.url(), &sha, true).unwrap();
        let before = current(home.path());
        served.replace(ustar(&[
            ("plugin.toml", b'0', "", MANIFEST.as_bytes()),
            ("plugin.wasm", b'0', "", b"other bytes at the same URL"),
        ]));

        let (failed, out) = update_demo(home.path(), false, true);

        assert!(failed, "{out}");
        assert!(out.contains("sha256 mismatch"), "{out}");
        assert!(out.contains("cox plugin install <url> --sha256"), "{out}");
        assert_eq!(current(home.path()), before);
        assert_eq!(version_count(home.path()), 1);
        assert!(!home.path().join("plugins/.staging").exists());
    }

    /// Moves tag `v1` of `bare_repo`'s repository to a new commit whose
    /// manifest also asks for `kv`; returns that commit.
    fn move_tag_widening(repos: &Path) -> String {
        let work = repos.join("work");
        let widened = format!("{MANIFEST}\n[capabilities]\nkv = true\n");
        fs::write(work.join("plugin.toml"), widened).unwrap();
        git_in(&work, &["commit", "--quiet", "-am", "widen"]);
        git_in(&work, &["tag", "--force", "v1"]);
        let bare = repos.join("repo.git");
        git_in(
            &work,
            &[
                "push",
                "--quiet",
                "--force",
                bare.to_str().unwrap(),
                "refs/tags/v1",
            ],
        );
        git_in(&work, &["rev-parse", "HEAD"])
    }

    fn installed_from_git(home: &Path, repos: &Path) -> String {
        let (url, _) = bare_repo(repos, &PLUGIN_FILES);
        install(
            &cli_at(home),
            &format!("git+{url}"),
            None,
            Some("v1"),
            None,
            true,
        )
        .unwrap();
        current(home)
    }

    #[test]
    fn plugin_update_git_moved_tag_asks_again() {
        let repos = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let before = installed_from_git(home.path(), repos.path());
        let moved = move_tag_widening(repos.path());

        let (_, out) = update_demo(home.path(), false, false);
        assert!(out.contains(&format!("{} -> ", before.trim())), "{out}");
        assert!(out.contains("+ kv (new)"), "{out}");
        assert_eq!(current(home.path()), before, "no grant, no switch: {out}");

        let (failed, out) = update_demo(home.path(), false, true);
        assert!(!failed, "{out}");
        assert_ne!(current(home.path()), before, "{out}");
        assert_eq!(grant_source(home.path())["commit"], moved.as_str());
        assert!(!home.path().join("plugins/.staging").exists());
    }

    #[test]
    fn plugin_update_check_changes_nothing() {
        let repos = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let before = installed_from_git(home.path(), repos.path());
        move_tag_widening(repos.path());

        let (failed, out) = update_demo(home.path(), true, true);

        assert!(!failed, "{out}");
        assert!(out.contains(&format!("{} -> ", before.trim())), "{out}");
        assert!(out.contains("+ kv (new)"), "{out}");
        assert_eq!(current(home.path()), before);
        assert_eq!(version_count(home.path()), 1, "--check stages nothing");
        assert!(!home.path().join("plugins/.staging").exists());
    }
}
