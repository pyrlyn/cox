// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `apply_patch`'s `Tool` impl: resolves every patch path through
//! `crate::path::confine` before touching disk, archives what a hunk is
//! about to overwrite, then writes through `crate::write::atomic_write`.
//! Parsing, hunk matching and staging are `cox-patch` (T32.6) — this module
//! is the one place that talks to the filesystem and the trust guards it
//! needs stay in `cox-tools` (docs/design/crates.md, AGENTS.md trust
//! boundaries).

use std::collections::BTreeMap;

use async_trait::async_trait;
use cox_patch::{Op, parse, stage};
use cox_protocol::{
    ArchivePut, Concurrency, Diff, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec,
};
use serde_json::{Value, json};
use similar::TextDiff;

use crate::path::confine;
use crate::write::{atomic_write, str_field};

/// Past this many deletions one patch stops being an edit and starts being
/// a way to lose work, so it is escalated to `Risk::Destructive` and the
/// permission engine asks (plan.md §4 tool table, T3.5 step 4).
const DESTRUCTIVE_DELETES: usize = 5;

/// `apply_patch`: Codex's V4A grammar as a tool.
pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".to_string(),
            description: "Apply a V4A patch. `patch` is a `*** Begin Patch` … `*** End \
                 Patch` document containing `*** Add File: p` (with `+` lines), `*** \
                 Delete File: p`, and `*** Update File: p` (optionally followed by `*** \
                 Move to: q`) with `@@ context` hunks whose lines start with a space, \
                 `-` or `+`, and may end with `*** End of File`. Hunks are matched \
                 exactly first, then ignoring trailing whitespace, then ignoring all \
                 whitespace; a hunk that matches nowhere or in more than one place \
                 fails the whole patch and nothing is written. Errors: `invalid patch \
                 at line N: <why>`, `denied: hunk N <why>`, `not found` (an updated or \
                 deleted file does not exist), `io error`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "The V4A patch document."
                    }
                },
                "required": ["patch"]
            }),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }

    /// A patch that removes more than [`DESTRUCTIVE_DELETES`] files is
    /// escalated past the tool's default `Write`. An unparseable patch
    /// keeps the default: it will be rejected by `call` before it can do
    /// anything, and guessing a risk from text that has no grammar would
    /// be worse than declining to guess.
    fn risk(&self, input: &Value) -> Risk {
        match input.get("patch").and_then(Value::as_str).map(parse) {
            Some(Ok(p)) if p.deletes() > DESTRUCTIVE_DELETES => Risk::Destructive,
            _ => self.spec().risk,
        }
    }

    /// Every path the patch writes: each op's path plus a move's target.
    /// An unparseable patch touches nothing — `call` refuses it first.
    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        let patch = input
            .get("patch")
            .and_then(Value::as_str)
            .map(parse)?
            .ok()?;
        let mut paths = Vec::new();
        for op in &patch.ops {
            paths.push(op.path().to_string());
            if let Op::Update {
                move_to: Some(to), ..
            } = op
            {
                paths.push(to.clone());
            }
        }
        Some(paths)
    }

    fn subject(&self, input: &Value) -> String {
        let Some(Ok(patch)) = input.get("patch").and_then(Value::as_str).map(parse) else {
            return String::new();
        };
        patch
            .ops
            .iter()
            .map(|op| op.path().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let patch = parse(&str_field(&input, "patch")?)?;

        // Confine every path up front: a patch that escapes the workspace
        // is refused before a single hunk is resolved.
        let mut resolved = BTreeMap::new();
        for op in &patch.ops {
            let path = op.path().to_string();
            resolved.insert(path.clone(), confine(&cx.writable_roots, &cx.cwd, &path)?);
            if let Op::Update {
                move_to: Some(to), ..
            } = op
            {
                resolved.insert(to.clone(), confine(&cx.writable_roots, &cx.cwd, to)?);
            }
        }

        let read = |p: &str| {
            resolved
                .get(p)
                .and_then(|full| std::fs::read_to_string(full).ok())
        };
        let changes = stage(&patch, &read)?;

        // Archive what is about to be lost before anything on disk moves,
        // so `cox expand` can restore it (AGENTS.md: lossless by default).
        for c in changes.iter().filter(|c| c.before.is_some()) {
            let Some(before) = &c.before else { continue };
            cx.archive
                .put(ArchivePut {
                    session: cx.session,
                    call: cx.call,
                    tool: "apply_patch".to_string(),
                    subject: Some(c.from.clone().unwrap_or_else(|| c.path.clone())),
                    bytes: before.clone().into_bytes(),
                })
                .await
                .map_err(|_| ToolError::Io)?;
        }

        let mut summary = Vec::new();
        let mut unified = String::new();
        for c in &changes {
            let target = resolved.get(&c.path).ok_or(ToolError::Io)?;
            match &c.after {
                Some(after) => atomic_write(target, after.as_bytes())?,
                None => std::fs::remove_file(target).map_err(|_| ToolError::Io)?,
            }
            // A move is a write to the new path plus a removal of the old.
            if let Some(from) = &c.from {
                let old = resolved.get(from).ok_or(ToolError::Io)?;
                std::fs::remove_file(old).map_err(|_| ToolError::Io)?;
            }
            summary.push(match &c.from {
                Some(from) => format!("{} {from} -> {}", c.status(), c.path),
                None => format!("{} {}", c.status(), c.path),
            });
            unified.push_str(
                &TextDiff::from_lines(
                    c.before.as_deref().unwrap_or_default(),
                    c.after.as_deref().unwrap_or_default(),
                )
                .unified_diff()
                .context_radius(3)
                .header(&c.from.clone().unwrap_or_else(|| c.path.clone()), &c.path)
                .to_string(),
            );
        }

        Ok(ToolOutput {
            text: summary.join("\n"),
            is_error: false,
            diff: changes.first().map(|c| Diff {
                path: std::path::PathBuf::from(&c.path),
                unified,
            }),
            structured: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4a_risk_escalates_past_five_deletes() {
        let deletes = |n: usize| {
            let body: String = (0..n).map(|i| format!("*** Delete File: f{i}\n")).collect();
            json!({ "patch": format!("*** Begin Patch\n{body}*** End Patch") })
        };
        assert_eq!(ApplyPatchTool.risk(&deletes(5)), Risk::Write);
        assert_eq!(ApplyPatchTool.risk(&deletes(6)), Risk::Destructive);
        assert_eq!(
            ApplyPatchTool.risk(&json!({ "patch": "not a patch" })),
            Risk::Write,
            "an unparseable patch keeps the default risk"
        );
    }
}
