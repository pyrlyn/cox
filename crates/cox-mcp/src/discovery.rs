// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Where MCP servers are declared (plan.md T7.6 step 1): `[mcp.servers]`
//! in config, the project's `.mcp.json`, and Claude Code's `~/.claude.json`
//! (read-only, D4), plus granted plugins' `[[mcp]]` entries (PL§7c).
//! Config wins over `.mcp.json` over `~/.claude.json` over a plugin;
//! `${VAR}` / `${VAR:-default}` expand from the environment, except in a
//! plugin's entries (`add_plugin`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cox_protocol::config::McpServerConfig;
use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Discovered {
    pub servers: HashMap<String, McpServerConfig>,
    /// Where each server came from: `config`, `.mcp.json`, `~/.claude.json`
    /// or `plugin:<id>`.
    pub sources: HashMap<String, String>,
    pub notices: Vec<String>,
}

/// Merges the three sources, lowest precedence first.
pub fn discover(
    config: &HashMap<String, McpServerConfig>,
    project: Option<&Path>,
    home: Option<&Path>,
) -> Discovered {
    let mut found = Discovered::default();
    if let Some(home) = home {
        let path = home.join(".claude.json");
        let file = read_json(&path, &mut found.notices);
        // Claude keeps user-scope servers at the top and project-scope ones
        // under `projects.<abs path>`.
        let mut entries = servers_in(file.get("mcpServers"));
        if let Some(project) = project {
            let key = project.display().to_string();
            entries.extend(servers_in(
                file.pointer(&format!("/projects/{}/mcpServers", key.replace('/', "~1"))),
            ));
        }
        add(&mut found, entries, "~/.claude.json");
    }
    if let Some(project) = project {
        let file = read_json(&project.join(".mcp.json"), &mut found.notices);
        add(&mut found, servers_in(file.get("mcpServers")), ".mcp.json");
    }
    add(&mut found, config.clone(), "config");
    for cfg in found.servers.values_mut() {
        expand_config(cfg);
    }
    found
}

fn add(found: &mut Discovered, entries: HashMap<String, McpServerConfig>, source: &'static str) {
    for (name, cfg) in entries {
        found.sources.insert(name.clone(), source.to_string());
        found.servers.insert(name, cfg);
    }
}

/// PL§7c: a granted plugin's servers join as the lowest-precedence source,
/// named `<id>-<name>`: a server of that name from any other source wins,
/// and the plugin's is dropped with a notice. Call it after `discover`.
/// Nothing is `${VAR}`-expanded: the user approved the argv as shown, and
/// expansion would hand the user's environment (keys included) to the
/// plugin's process. The caller has already wrapped a stdio command in the
/// sandbox, so this crate never needs to know about one.
pub fn add_plugin(found: &mut Discovered, id: &str, servers: Vec<(String, McpServerConfig)>) {
    for (name, cfg) in servers {
        let name = format!("{id}-{name}");
        if let Some(source) = found.sources.get(&name) {
            found.notices.push(format!(
                "mcp: plugin {id}'s server {name} is shadowed by {source}"
            ));
            continue;
        }
        found.sources.insert(name.clone(), format!("plugin:{id}"));
        found.servers.insert(name, cfg);
    }
}

/// A missing file is nothing; a broken one is a notice (D14: fail open).
fn read_json(path: &Path, notices: &mut Vec<String>) -> Value {
    let Ok(text) = fs::read_to_string(path) else {
        return Value::Null;
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        notices.push(format!("mcp: {} skipped: {e}", path.display()));
        Value::Null
    })
}

/// `{ name: { command, args, env, url, ... } }`; keys cox does not model
/// (`type`, `headers`, `disabled`) are ignored rather than rejected.
fn servers_in(map: Option<&Value>) -> HashMap<String, McpServerConfig> {
    let Some(map) = map.and_then(Value::as_object) else {
        return HashMap::new();
    };
    map.iter()
        .map(|(name, v)| {
            let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
            let cfg = McpServerConfig {
                command: str_of("command"),
                args: v
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                url: str_of("url"),
                env: v
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
                // `.mcp.json`/`~/.claude.json` carry no opt-out (T33.42):
                // cox's own `[mcp.servers.<name>]` is where `sandbox = false`
                // is set, and it fully shadows a same-named entry from here.
                sandbox: true,
            };
            (name.clone(), cfg)
        })
        .collect()
}

fn expand_config(cfg: &mut McpServerConfig) {
    let lookup = |k: &str| std::env::var(k).ok();
    if let Some(c) = &cfg.command {
        cfg.command = Some(expand(c, &lookup));
    }
    if let Some(u) = &cfg.url {
        cfg.url = Some(expand(u, &lookup));
    }
    for a in &mut cfg.args {
        *a = expand(a, &lookup);
    }
    for v in cfg.env.values_mut() {
        *v = expand(v, &lookup);
    }
}

/// `${VAR}` and `${VAR:-default}`; an unset variable without a default
/// expands to nothing, as a shell would.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let (name, default) = match after[..end].split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (&after[..end], None),
        };
        match lookup(name) {
            Some(v) => out.push_str(&v),
            None => out.push_str(default.unwrap_or("")),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `.mcp.json` path cox reads for `project`, for `cox ext`.
pub fn project_file(project: &Path) -> PathBuf {
    project.join(".mcp.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_env_expansion_matches_shell_forms() {
        let env = |k: &str| (k == "TOKEN").then(|| "t0k".to_string());
        assert_eq!(expand("Bearer ${TOKEN}", &env), "Bearer t0k");
        assert_eq!(expand("${MISSING:-x}/${TOKEN}", &env), "x/t0k");
        assert_eq!(expand("${MISSING}", &env), "");
        assert_eq!(expand("${unterminated", &env), "${unterminated");
    }

    fn stdio(command: &str) -> McpServerConfig {
        McpServerConfig {
            command: Some(command.into()),
            ..McpServerConfig::default()
        }
    }

    #[test]
    fn project_mcp_json_shadows_plugin_server() {
        let project = tempfile::tempdir().expect("tempdir");
        fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"gh-tools":{"command":"project-server"}}}"#,
        )
        .expect("write .mcp.json");
        let mut found = discover(&HashMap::new(), Some(project.path()), None);
        let plugin = vec![
            ("tools".to_string(), stdio("plugin-server")),
            ("other".to_string(), stdio("${HOME}/x")),
        ];
        add_plugin(&mut found, "gh", plugin);

        assert_eq!(found.servers["gh-tools"], stdio("project-server"));
        assert_eq!(found.sources["gh-tools"], ".mcp.json");
        assert_eq!(found.sources["gh-other"], "plugin:gh");
        // A plugin's argv reaches the spawn exactly as approved.
        assert_eq!(found.servers["gh-other"], stdio("${HOME}/x"));
        assert!(
            found.notices.iter().any(|n| n.contains("gh-tools")),
            "{:?}",
            found.notices
        );
    }
}
