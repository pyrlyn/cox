// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox ext` (plan.md T7.3 step 3): what instruction files, skills, commands
//! and agent definitions are in effect for this cwd. Lives in the binary
//! because it needs both the config roots and every cox-ext discoverer;
//! hooks and MCP servers join the report in T7.4/T7.6. `plugins` (T33.39)
//! shows the same per-plugin state as `cox doctor`, through
//! `doctor::check_plugins` — one walk, one row shape, two surfaces.

use std::fmt::Write as _;
use std::path::Path;

use cox_ext::{agents, commands, instructions, skills};

use crate::cli::Cli;
use crate::config_load::{self, cox_home, find_git_root, home_dir};

/// `cox ext list [--json]` (T9.3 Check): agent (and sibling) definitions in
/// effect, including the embedded presets with no config files present.
pub fn list(cli: &Cli, cwd: &Path, json: bool) -> String {
    let cox_home = cli.home.clone().unwrap_or_else(cox_home);
    let claude_home = home_dir().join(".claude");
    let git_root = find_git_root(cwd);
    let project = git_root.clone().unwrap_or_else(|| cwd.to_path_buf());
    let (ch, cl, pr) = (
        Some(cox_home.as_path()),
        Some(claude_home.as_path()),
        Some(project.as_path()),
    );
    let defs = agents::discover(&agents::agent_dirs(ch, cl, pr));
    let cmds = commands::discover(&commands::command_dirs(ch, cl, pr));
    let found = skills::discover(&skills::skill_dirs(ch, cl, pr));
    // T33.39: loaded, skipped, not granted, or dev, plus catalog price
    // conflicts (T33.16) — the same row `cox doctor` shows. Config-loaded
    // best-effort, same as `report`'s `unwrap_or_default`, so a broken
    // config never hides the rest of `ext list`.
    let config = config_load::load(cwd, cli)
        .map(|l| l.config)
        .unwrap_or_default();
    let plugin_rows = crate::doctor::check_plugins(cwd, &cox_home, &config);
    if !json {
        let mut out = String::new();
        // T34.10: `disabled: true` still discovers here, marked, even
        // though the `agent` tool's own description omits it.
        let agent_lines: Vec<String> = defs
            .agents
            .iter()
            .map(|a| {
                if a.disabled {
                    format!("{} (disabled)", a.name)
                } else {
                    a.name.clone()
                }
            })
            .collect();
        section(&mut out, "agents", agent_lines.iter().map(String::as_str));
        section(
            &mut out,
            "commands",
            cmds.commands.iter().map(|c| c.name.as_str()),
        );
        section(
            &mut out,
            "skills",
            found.skills.iter().map(|s| s.name.as_str()),
        );
        if plugin_rows.is_empty() {
            let _ = writeln!(out, "plugins: none");
        } else {
            let _ = writeln!(out, "plugins:");
            for row in &plugin_rows {
                let _ = write!(out, " {}", crate::doctor::human(row));
            }
        }
        return out;
    }
    serde_json::json!({
        "agents": defs.agents.iter().map(|a| serde_json::json!({
            "name": a.name,
            "description": a.description,
            "tools": a.tools,
            "model": a.model,
            "disabled": a.disabled,
        })).collect::<Vec<_>>(),
        "commands": cmds.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        "skills": found.skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        "plugins": serde_json::to_value(&plugin_rows).unwrap_or_default(),
    })
    .to_string()
}

pub fn report(cli: &Cli, cwd: &Path) -> String {
    let config = config_load::load(cwd, cli)
        .map(|l| l.config)
        .unwrap_or_default();
    let cox_home = cli.home.clone().unwrap_or_else(cox_home);
    let claude_home = home_dir().join(".claude");
    let roots = cox_session::instruction_roots(&cox_home, &claude_home, cwd);
    let project = roots.git_root.clone().unwrap_or_else(|| cwd.to_path_buf());
    let mut out = String::new();
    let loaded = instructions::load(&roots, u32::MAX);
    section(
        &mut out,
        "instructions",
        loaded.files.iter().map(String::as_str),
    );
    let (ch, cl, pr) = (
        Some(cox_home.as_path()),
        Some(claude_home.as_path()),
        Some(project.as_path()),
    );
    let found = skills::discover(&skills::skill_dirs(ch, cl, pr));
    section(
        &mut out,
        "skills",
        found.skills.iter().map(|s| s.name.as_str()),
    );
    let cmds = commands::discover(&commands::command_dirs(ch, cl, pr));
    section(
        &mut out,
        "commands",
        cmds.commands.iter().map(|c| c.name.as_str()),
    );
    let defs = agents::discover(&agents::agent_dirs(ch, cl, pr));
    section(
        &mut out,
        "agents",
        defs.agents.iter().map(|a| a.name.as_str()),
    );
    let mcp = cox_mcp::discovery::discover(&config.mcp.servers, Some(&project), Some(&home_dir()));
    let mut servers: Vec<String> = mcp
        .servers
        .keys()
        .map(|n| format!("{n} ({})", mcp.sources[n]))
        .collect();
    servers.sort();
    section(&mut out, "mcp servers", servers.iter().map(String::as_str));
    let notices: Vec<String> = loaded
        .notices
        .into_iter()
        .chain(found.notices)
        .chain(cmds.notices)
        .chain(defs.notices)
        .chain(mcp.notices)
        .collect();
    section(&mut out, "notices", notices.iter().map(String::as_str));
    out
}

fn section<'a>(out: &mut String, title: &str, items: impl Iterator<Item = &'a str>) {
    let items: Vec<&str> = items.collect();
    if items.is_empty() {
        let _ = writeln!(out, "{title}: none");
        return;
    }
    let _ = writeln!(out, "{title}:");
    for item in items {
        let _ = writeln!(out, "  {item}");
    }
}
