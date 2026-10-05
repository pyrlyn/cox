// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Subagent definitions from `.claude/agents/*.md` and `.cox/agents/*.md`
//! (T7.3): `name`, `description`, `tools`, `model`. A definition narrows
//! what the `agent` tool may hand a child.
//!
//! T34.1: `AgentDef` and `tier_for` moved to `cox_protocol::agent` (they
//! cross into `cox-core`, which may not depend on `cox-ext`'s filesystem
//! I/O) and are re-exported here at their old path, so this module keeps
//! owning only `discover` and its parsing — the actual filesystem read.

use std::fs;
use std::path::{Path, PathBuf};

pub use cox_protocol::agent::{AgentDef, tier_for};
use cox_protocol::types::PermissionMode;
use serde::Deserialize;

use crate::frontmatter;

/// The `explore` and `shell` presets shipped in the binary (T9.3): the same
/// names, tools and models as the core's `agent` presets, so `cox ext list`
/// shows them with no config files present.
const EXPLORE_MD: &str = include_str!("../agents/explore.md");
const SHELL_MD: &str = include_str!("../agents/shell.md");

#[derive(Debug, Default, PartialEq)]
pub struct Discovered {
    pub agents: Vec<AgentDef>,
    pub notices: Vec<String>,
}

#[derive(Deserialize)]
struct Header {
    name: Option<String>,
    description: Option<String>,
    tools: Option<serde_yaml::Value>,
    model: Option<String>,
    /// T34.10: `disabled: true` hides the def from the model without
    /// removing it from disk or from `cox ext list`.
    disabled: Option<bool>,
    /// T45.2: Claude Code's subagent field; it can only narrow the parent.
    #[serde(rename = "permissionMode")]
    permission_mode: Option<String>,
}

/// `~/.cox/agents`, `~/.claude/agents`, `.cox/agents`, `.claude/agents`.
pub fn agent_dirs(
    cox_home: Option<&Path>,
    claude_home: Option<&Path>,
    project: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = cox_home {
        dirs.push(h.join("agents"));
    }
    if let Some(h) = claude_home {
        dirs.push(h.join("agents"));
    }
    if let Some(p) = project {
        dirs.push(p.join(".cox").join("agents"));
        dirs.push(p.join(".claude").join("agents"));
    }
    dirs
}

pub fn discover(dirs: &[PathBuf]) -> Discovered {
    // Embedded presets first (T9.3): a same-named file in any dir overrides
    // them through the retain+push below, so users can replace either.
    let mut found = Discovered::default();
    for (name, text) in [("explore", EXPLORE_MD), ("shell", SHELL_MD)] {
        let path = PathBuf::from(format!("<embedded>/{name}.md"));
        match parse_agent_text(&path, text, &mut found.notices) {
            Ok(def) => found.agents.push(def),
            Err(reason) => found
                .notices
                .push(format!("embedded agent {name} skipped: {reason}")),
        }
    }
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            match parse_file(&path, &mut found.notices) {
                Ok(def) => {
                    found.agents.retain(|a| a.name != def.name);
                    found.agents.push(def);
                }
                Err(reason) => found
                    .notices
                    .push(format!("agent {} skipped: {reason}", path.display())),
            }
        }
    }
    found
}

/// One definition file, parsed by the same code `discover` uses; `pub`
/// for a granted plugin's `[[agents]]` files (T45.4), which live outside
/// the agent dirs. `notices` gets what loads with a caveat (T45.2).
pub fn parse_file(path: &Path, notices: &mut Vec<String>) -> Result<AgentDef, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_agent_text(path, &text, notices)
}

/// Adds one plugin's definitions to `local` (T45.4, PL§14 decision 15): a
/// name already present — a user or project file, an embedded preset or
/// an earlier plugin's — is kept, and the plugin's is skipped with a
/// notice, so installing a plugin never silently replaces a definition
/// the user wrote.
pub fn merge(local: &mut Discovered, plugin: Vec<AgentDef>, plugin_id: &str) {
    for def in plugin {
        if local.agents.iter().any(|a| a.name == def.name) {
            local.notices.push(format!(
                "plugin {plugin_id}: agent {} skipped; a definition with that name is already loaded",
                def.name
            ));
        } else {
            local.agents.push(def);
        }
    }
}

/// A def that parses but carries something cox cannot honour (an unknown
/// `permissionMode`) still loads, with a line in `notices` (fail open).
fn parse_agent_text(
    path: &Path,
    text: &str,
    notices: &mut Vec<String>,
) -> Result<AgentDef, String> {
    let (header, body): (Header, &str) = frontmatter::parse(text).map_err(|e| e.to_string())?;
    let name = header.name.ok_or("missing `name`")?;
    let description = header.description.ok_or("missing `description`")?;
    let permission_mode = header.permission_mode.as_deref().and_then(|raw| {
        let mode = mode_of(raw);
        if mode.is_none() {
            notices.push(format!(
                "agent {}: unknown permissionMode {raw:?} ignored; the parent's mode applies",
                path.display()
            ));
        }
        mode
    });
    Ok(AgentDef {
        name,
        description: description.trim().to_string(),
        tools: frontmatter::names(header.tools.as_ref()),
        model: header.model,
        path: path.to_path_buf(),
        body: body.trim().to_string(),
        disabled: header.disabled.unwrap_or(false),
        permission_mode,
    })
}

/// Claude Code's `permissionMode` values (plus cox's own `auto`) as cox
/// modes; `acceptEdits` is cox's `auto` (writes run, `Exec` still asks).
fn mode_of(raw: &str) -> Option<PermissionMode> {
    match raw.trim() {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "acceptEdits" | "auto" => Some(PermissionMode::Auto),
        "bypassPermissions" => Some(PermissionMode::Bypass),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_embedded_defaults_include_explore_and_shell() {
        // No dirs at all: only the two shipped presets come back, and a
        // same-named file would override them (retain+push order).
        let found = discover(&[]);
        assert!(found.notices.is_empty(), "{:?}", found.notices);
        let names: Vec<&str> = found.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["explore", "shell"]);
        let explore = &found.agents[0];
        assert_eq!(explore.tools, ["read", "grep", "glob", "outline", "expand"]);
        assert_eq!(explore.model.as_deref(), Some("haiku"));
        assert!(!explore.body.is_empty());
        let shell = &found.agents[1];
        assert_eq!(shell.tools, ["bash", "web_fetch"]);
        assert_eq!(shell.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn agents_disabled_frontmatter_field_is_parsed() {
        let mut notices = Vec::new();
        let def = parse_agent_text(
            &PathBuf::from("<test>/blocked.md"),
            "---\nname: blocked\ndescription: not for the model\ndisabled: true\n---\nbody",
            &mut notices,
        )
        .unwrap();
        assert!(def.disabled);
        let enabled = parse_agent_text(
            &PathBuf::from("<test>/scout.md"),
            "---\nname: scout\ndescription: looks around\n---\nbody",
            &mut notices,
        )
        .unwrap();
        assert!(!enabled.disabled);
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn merge_keeps_local_definition_and_names_skipped_plugin_agent() {
        let def = |name: &str, body: &str| AgentDef {
            name: name.into(),
            description: "d".into(),
            tools: Vec::new(),
            model: None,
            path: PathBuf::from(format!("<test>/{name}.md")),
            body: body.into(),
            disabled: false,
            permission_mode: None,
        };
        let mut found = Discovered {
            agents: vec![def("reviewer", "local")],
            notices: Vec::new(),
        };
        merge(
            &mut found,
            vec![def("reviewer", "plugin"), def("tester", "plugin")],
            "review-kit",
        );
        let bodies: Vec<(&str, &str)> = found
            .agents
            .iter()
            .map(|a| (a.name.as_str(), a.body.as_str()))
            .collect();
        assert_eq!(bodies, [("reviewer", "local"), ("tester", "plugin")]);
        assert_eq!(found.notices.len(), 1, "{:?}", found.notices);
        assert!(
            found.notices[0].contains("review-kit") && found.notices[0].contains("reviewer"),
            "{:?}",
            found.notices
        );
    }

    #[test]
    fn agents_permission_mode_frontmatter_is_parsed() {
        let parse = |mode: &str| {
            let mut notices = Vec::new();
            let text = format!("---\nname: a\ndescription: d\npermissionMode: {mode}\n---\nbody");
            let def = parse_agent_text(&PathBuf::from("<test>/a.md"), &text, &mut notices)
                .unwrap()
                .permission_mode;
            assert!(notices.is_empty(), "{notices:?}");
            def
        };
        assert_eq!(parse("default"), Some(PermissionMode::Default));
        assert_eq!(parse("plan"), Some(PermissionMode::Plan));
        assert_eq!(parse("acceptEdits"), Some(PermissionMode::Auto));
        assert_eq!(parse("auto"), Some(PermissionMode::Auto));
        assert_eq!(parse("bypassPermissions"), Some(PermissionMode::Bypass));
        let mut notices = Vec::new();
        let none = parse_agent_text(
            &PathBuf::from("<test>/b.md"),
            "---\nname: b\ndescription: d\n---\nbody",
            &mut notices,
        )
        .unwrap();
        assert_eq!(none.permission_mode, None);
    }

    #[test]
    fn agents_unknown_permission_mode_is_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("odd.md"),
            "---\nname: odd\ndescription: d\npermissionMode: yolo\n---\nbody",
        )
        .unwrap();
        let found = discover(&[dir.path().to_path_buf()]);
        let odd = found.agents.iter().find(|a| a.name == "odd");
        assert_eq!(
            odd.map(|a| a.permission_mode),
            Some(None),
            "the def still loads, in the parent's mode"
        );
        assert_eq!(found.notices.len(), 1, "{:?}", found.notices);
        assert!(found.notices[0].contains("\"yolo\""), "{:?}", found.notices);
    }
}
