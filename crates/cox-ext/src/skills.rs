// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Agent Skills (T7.2): `SKILL.md` discovery, the index line the model sees
//! up front, and the deferred `skill` tool that returns a body on demand.
//! The body stays out of the prompt until invoked — that is the whole point
//! of the format — so discovery and invocation are separate steps.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::frontmatter;

/// One parsed `SKILL.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    /// Tools the skill may use while active; empty means no restriction.
    pub allowed_tools: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub compatibility: Option<String>,
    /// The `SKILL.md` path, for `cox ext list` and error messages.
    pub path: PathBuf,
    /// Markdown after the frontmatter, returned on invoke.
    pub body: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Discovered {
    /// Search order, later directories replacing earlier same-name skills
    /// (project over home).
    pub skills: Vec<Skill>,
    pub notices: Vec<String>,
}

#[derive(Deserialize)]
struct Header {
    name: Option<String>,
    description: Option<String>,
    license: Option<String>,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<serde_yaml::Value>,
    metadata: Option<BTreeMap<String, serde_yaml::Value>>,
    compatibility: Option<String>,
}

/// The directories to scan, in precedence order (later wins).
pub fn skill_dirs(
    cox_home: Option<&Path>,
    claude_home: Option<&Path>,
    project: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = cox_home {
        dirs.push(h.join("skills"));
    }
    if let Some(h) = claude_home {
        dirs.push(h.join("skills"));
    }
    if let Some(p) = project {
        dirs.push(p.join(".cox").join("skills"));
        dirs.push(p.join(".claude").join("skills"));
    }
    dirs
}

/// Scans `<dir>/*/SKILL.md` for each directory. Malformed skills are
/// skipped with a notice; a missing directory is simply empty.
pub fn discover(dirs: &[PathBuf]) -> Discovered {
    let mut found = Discovered::default();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path().join("SKILL.md"))
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            match parse_skill(&path) {
                Ok(skill) => {
                    found.skills.retain(|s| s.name != skill.name);
                    found.skills.push(skill);
                }
                Err(reason) => found
                    .notices
                    .push(format!("skill {} skipped: {reason}", path.display())),
            }
        }
    }
    found
}

/// The spec's name rule: lowercase letters, digits and hyphens, ≤ 64 chars,
/// and equal to the directory name.
fn valid_name(name: &str, path: &Path) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("name must be 1–64 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "name `{name}` must be lowercase letters, digits and hyphens"
        ));
    }
    let dir = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if dir != name {
        return Err(format!(
            "name `{name}` does not match its directory `{dir}`"
        ));
    }
    Ok(())
}

fn parse_skill(path: &Path) -> Result<Skill, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (header, body): (Header, &str) = frontmatter::parse(&text).map_err(|e| e.to_string())?;
    let name = header.name.ok_or("missing `name`")?;
    valid_name(&name, path)?;
    let description = header
        .description
        .filter(|d| !d.trim().is_empty())
        .ok_or("missing `description`")?;
    let metadata = header
        .metadata
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| {
            let v = match v {
                serde_yaml::Value::String(s) => s,
                other => serde_yaml::to_string(&other)
                    .unwrap_or_default()
                    .trim_end()
                    .to_string(),
            };
            (k, v)
        })
        .collect();
    Ok(Skill {
        name,
        description: description.trim().to_string(),
        license: header.license,
        allowed_tools: frontmatter::names(header.allowed_tools.as_ref()),
        metadata,
        compatibility: header.compatibility,
        path: path.to_path_buf(),
        body: body.trim().to_string(),
    })
}

/// The `system[2]` index: one line per skill, no bodies. Empty when there
/// are no skills so the prefix does not change for users without any.
pub fn index(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("# Skills\nCall the `skill` tool with a name to load its instructions.\n");
    for s in skills {
        out.push_str(&format!("- {}: {}\n", s.name, s.description));
    }
    out
}

/// Deferred tool: `{"name": "<skill>"}` → the skill body as a visible item.
/// `structured.allowed_tools` carries the narrowing for the engine.
pub struct SkillTool {
    skills: Arc<Vec<Skill>>,
}

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self {
            skills: Arc::new(skills),
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "Load a skill's full instructions by name. The names and one-line descriptions are listed under `# Skills` in the system prompt.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string", "description": "Skill name from the index." } },
                "required": ["name"]
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input["name"].as_str().unwrap_or("").to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let name = input["name"].as_str().unwrap_or("");
        let skill = self
            .skills
            .iter()
            .find(|s| s.name == name)
            .ok_or(ToolError::NotFound)?;
        Ok(ToolOutput {
            text: format!("# Skill: {}\n\n{}", skill.name, skill.body),
            is_error: false,
            diff: None,
            structured: Some(json!({
                "name": skill.name,
                "allowed_tools": skill.allowed_tools,
            })),
        })
    }
}
