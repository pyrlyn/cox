// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A custom subagent definition (`.cox/agents/*.md`, `.claude/agents/*.md`;
//! plan.md T7.3/T34.1): what `cox-ext`'s `agents::discover` reads off disk,
//! and what `cox-core`'s `agent` tool matches a `preset` name against.
//!
//! Lives here, not in [`crate::types`], because it never crosses the wire —
//! it is not reachable from `Submission`/`Event`, so it carries no
//! `JsonSchema` and no `docs/protocol.jsonschema` entry — but it does cross
//! the `cox-ext` → `cox-core` crate boundary, and AGENTS.md's crate rule
//! puts every such type in `cox-protocol`: `cox-core` may depend on
//! `cox-protocol`/`cox-permission`/`cox-models` only, never on `cox-ext`,
//! which reads the filesystem. `cox-ext::agents` re-exports this module's
//! items at their old path so existing callers keep working.

use std::path::PathBuf;

use crate::types::{PermissionMode, Tier};

/// One discovered subagent definition.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDef {
    /// The `preset` name the `agent` tool call matches against.
    pub name: String,
    /// Shown to the user/model to explain what the subagent is for.
    pub description: String,
    /// Tool names the child may use; empty means every tool the parent has.
    pub tools: Vec<String>,
    /// `model:` frontmatter value: a tier name, a Claude alias, a model id,
    /// or `inherit`/absent (see [`tier_for`]).
    pub model: Option<String>,
    /// Where the definition file lives, for diagnostics.
    pub path: PathBuf,
    /// The system prompt for the child.
    pub body: String,
    /// `disabled: true` in the frontmatter (T34.10): the definition still
    /// discovers and shows up in `cox ext list`, but the `agent` tool's own
    /// description omits it and `resolve` treats its name as unknown — the
    /// same "known to disk, invisible to the model" split OpenCode's
    /// `permission: deny` gives a subagent (research.md §4.3.7). Claude
    /// Code's documented subagent frontmatter (research.md M1) has no field
    /// for this, so there is no existing name to match.
    pub disabled: bool,
    /// `permissionMode:` frontmatter (T45.2, Claude Code's field name): the
    /// mode the child asks for. It can only narrow — the child runs in the
    /// narrower of this and the parent's live mode — and `None` means the
    /// parent's live mode.
    pub permission_mode: Option<PermissionMode>,
}

impl AgentDef {
    /// The parent's tools the child may keep, in the parent's order. A
    /// listed tool the parent lacks is silently absent — a child can never
    /// gain a tool by naming it.
    pub fn restrict<T: Clone>(&self, tools: &[(String, T)]) -> Vec<T> {
        tools
            .iter()
            .filter(|(name, _)| self.tools.is_empty() || self.tools.contains(name))
            .map(|(_, t)| t.clone())
            .collect()
    }
}

/// How a `model:` value picks a tier: tier names, Claude's aliases
/// (`haiku` → cheap, `sonnet` → code, `opus` → think), a model id by its
/// family, `inherit`/absent → the parent's tier (`None`).
pub fn tier_for(model: Option<&str>) -> Option<Tier> {
    let m = model?.trim().to_ascii_lowercase();
    if m == "inherit" || m.is_empty() {
        return None;
    }
    let pick = |t| Some(t);
    match m.as_str() {
        "cheap" | "haiku" => pick(Tier::Cheap),
        "code" | "sonnet" => pick(Tier::Code),
        "think" | "opus" | "fable" => pick(Tier::Think),
        id if id.contains("haiku") => pick(Tier::Cheap),
        id if id.contains("opus") || id.contains("fable") => pick(Tier::Think),
        _ => pick(Tier::Code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_model_aliases_map_to_tiers() {
        assert_eq!(tier_for(Some("haiku")), Some(Tier::Cheap));
        assert_eq!(tier_for(Some("sonnet")), Some(Tier::Code));
        assert_eq!(tier_for(Some("opus")), Some(Tier::Think));
        assert_eq!(tier_for(Some("think")), Some(Tier::Think));
        assert_eq!(tier_for(Some("claude-haiku-4-5")), Some(Tier::Cheap));
        assert_eq!(tier_for(Some("gpt-5")), Some(Tier::Code));
        assert_eq!(tier_for(Some("inherit")), None);
        assert_eq!(tier_for(None), None);
    }

    #[test]
    fn agent_def_restrict_keeps_only_named_tools_in_parent_order() {
        let def = AgentDef {
            name: "reviewer".into(),
            description: "d".into(),
            tools: vec!["read".into(), "grep".into()],
            model: None,
            path: PathBuf::from("<test>"),
            body: String::new(),
            disabled: false,
            permission_mode: None,
        };
        let parent = [
            ("read".to_string(), 0),
            ("bash".to_string(), 1),
            ("grep".to_string(), 2),
        ];
        assert_eq!(def.restrict(&parent), [0, 2]);
    }
}
