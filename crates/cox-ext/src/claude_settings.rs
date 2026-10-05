// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `.claude/settings.json` import (plan.md T7.5, D4/D13): permission rules
//! and hooks from a Claude Code setup, read-only, as one config layer the
//! binary merges above `.cox` project config. Only the keys cox understands
//! are lifted; everything else in the file is ignored, never an error.
//! `~/.claude/keybindings.json` (T25.5) is read here too, for the same
//! reason: one module owns what cox lifts from a Claude Code setup.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cox_protocol::config::HookConfig;
use serde::Deserialize;
use serde_json::{Value, json};

/// The lifted subset, merged across files: rules and hooks accumulate in
/// file order (a later file cannot cancel an earlier deny), `env` overrides.
#[derive(Debug, Default, PartialEq)]
pub struct ClaudeSettings {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
    pub hooks: HashMap<String, Vec<HookConfig>>,
    pub env: HashMap<String, String>,
    pub files: Vec<PathBuf>,
    pub notices: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    permissions: Permissions,
    hooks: HashMap<String, Vec<Matcher>>,
    env: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Permissions {
    allow: Vec<String>,
    ask: Vec<String>,
    deny: Vec<String>,
}

/// One `{ "matcher": "Bash", "hooks": [ { "type": "command", ... } ] }`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Matcher {
    matcher: Option<String>,
    hooks: Vec<Entry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    command: String,
    timeout: Option<u32>,
}

/// `~/.claude/settings.json`, `.claude/settings.json`,
/// `.claude/settings.local.json` — Claude's precedence order.
pub fn paths(claude_home: Option<&Path>, project: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(h) = claude_home {
        out.push(h.join("settings.json"));
    }
    if let Some(p) = project {
        out.push(p.join(".claude").join("settings.json"));
        out.push(p.join(".claude").join("settings.local.json"));
    }
    out
}

/// Reads every existing file; a file that is not JSON is skipped with a notice.
pub fn load(paths: &[PathBuf]) -> ClaudeSettings {
    let mut s = ClaudeSettings::default();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let file: File = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                s.notices
                    .push(format!("claude settings {} skipped: {e}", path.display()));
                continue;
            }
        };
        s.allow.extend(file.permissions.allow);
        s.ask.extend(file.permissions.ask);
        s.deny.extend(file.permissions.deny);
        for (event, matchers) in file.hooks {
            let list = s.hooks.entry(event).or_default();
            for m in matchers {
                // Claude also has `prompt`/`agent` hook types that call a
                // model; only `command` maps onto cox's runner.
                for e in m.hooks.into_iter().filter(|e| e.kind == "command") {
                    list.push(HookConfig {
                        matcher: m.matcher.clone(),
                        command: e.command,
                        timeout_s: e.timeout,
                    });
                }
            }
        }
        s.env.extend(file.env);
        s.files.push(path.clone());
    }
    s
}

/// Claude Code's keybindings as `(key, action)` pairs, for the TUI to map
/// onto its own actions (it knows which exist in both).
#[derive(Debug, Default, PartialEq)]
pub struct ClaudeKeybindings {
    pub bindings: Vec<(String, String)>,
    pub notices: Vec<String>,
}

/// `{ "bindings": [ { "context": "Chat", "bindings": { "ctrl+s": "chat:submit" } } ] }`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct KeybindingsFile {
    bindings: Vec<KeyContext>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct KeyContext {
    bindings: serde_json::Map<String, Value>,
}

/// `<claude_home>/keybindings.json`, every context block flattened. A
/// `null` action unbinds one of Claude's own defaults, which means nothing
/// to cox, so it is dropped; a file that is not that shape is a notice.
pub fn keybindings(claude_home: &Path) -> ClaudeKeybindings {
    let path = claude_home.join("keybindings.json");
    let mut out = ClaudeKeybindings::default();
    let Ok(text) = fs::read_to_string(&path) else {
        return out;
    };
    match serde_json::from_str::<KeybindingsFile>(&text) {
        Ok(file) => {
            out.bindings = file
                .bindings
                .into_iter()
                .flat_map(|c| c.bindings)
                .filter_map(|(key, action)| Some((key, action.as_str()?.to_string())))
                .collect();
        }
        Err(e) => out.notices.push(format!(
            "claude keybindings {} skipped: {e}",
            path.display()
        )),
    }
    out
}

impl ClaudeSettings {
    /// The layer as config data (`permissions.*` rule lists and
    /// `hooks.<Event>` tables), for the binary to adjoin below env/flags.
    /// `env` has no config key yet, so it is not part of the layer.
    pub fn to_layer(&self) -> Value {
        let hooks: serde_json::Map<String, Value> = self
            .hooks
            .iter()
            .map(|(event, list)| (event.clone(), json!(list)))
            .collect();
        json!({
            "permissions": { "allow": self.allow, "ask": self.ask, "deny": self.deny },
            "hooks": hooks,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}
