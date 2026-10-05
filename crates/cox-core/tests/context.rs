// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cache-prefix contracts for `assemble` (plan.md T2.3 / §1.9).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cox_core::assemble;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Content, Message, Risk, Role, Tier, ToolOutput, ToolSpec};
use serde_json::Value;

struct Echo;

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        String::new()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: String::new(),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

fn user(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![Content::Text { text: text.into() }],
    }
}

fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(Echo)]
}

#[test]
fn context_prefix_bytes_identical_between_turns() {
    let config = cox_protocol::Config::default();
    let tools = tools();
    let a = assemble(
        &[user("one")],
        &config,
        &tools,
        Path::new("/tmp/a"),
        "2026-01-01",
    );
    let b = assemble(
        &[user("one"), user("two")],
        &config,
        &tools,
        Path::new("/tmp/b"),
        "2026-12-31",
    );
    let pre_a = serde_json::to_vec(&a.system[0..=2]).expect("a");
    let pre_b = serde_json::to_vec(&b.system[0..=2]).expect("b");
    assert_eq!(pre_a, pre_b);
}

#[test]
fn context_volatile_content_after_breakpoint() {
    let req = assemble(
        &[user("hi")],
        &cox_protocol::Config::default(),
        &tools(),
        Path::new("/tmp/cox-turn"),
        "today",
    );
    assert!(req.system[3].text.contains("today"));
    assert!(req.system[3].text.contains("/tmp/cox-turn"));
    assert!(!req.system[3].cache);
    assert_eq!(req.cache_breakpoints.first().copied(), Some(2));
    assert!(!req.system[0].text.contains("today"));
    assert!(!req.system[1].text.contains("today"));
    assert!(!req.system[2].text.contains("today"));
}

#[test]
fn context_three_breakpoints_max() {
    let history = vec![user("a"), user("b"), user("c")];
    let req = assemble(
        &history,
        &cox_protocol::Config::default(),
        &tools(),
        Path::new("/tmp"),
        "d",
    );
    assert_eq!(req.cache_breakpoints.len(), 3);
    assert!(req.cache_breakpoints.windows(2).all(|w| w[0] < w[1]));
}

/// A deferred tool: absent from the request until `tool_search` names it.
struct Deferred(&'static str);

#[async_trait]
impl Tool for Deferred {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.into(),
            description: "deferred".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        String::new()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: String::new(),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[test]
fn deferred_tools_absent_until_searched() {
    let config = cox_protocol::Config::default();
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(Deferred("mcp__gh__issue")),
        Arc::new(Echo),
        Arc::new(Deferred("web_fetch")),
    ];
    let names = |req: &cox_protocol::types::Request| {
        req.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    };
    let before = assemble(&[user("hi")], &config, &tools, Path::new("/w"), "d");
    assert_eq!(names(&before), ["echo"]);
    assert!(!before.system[0].text.contains("web_fetch"));

    let found = ["web_fetch".to_string()];
    let after = cox_core::assemble_with(
        &[user("hi")],
        &config,
        Tier::Code,
        &tools,
        &found,
        Path::new("/w"),
        "d",
    );
    assert_eq!(
        names(&after),
        ["echo", "web_fetch"],
        "core first, discovered after"
    );
    assert!(after.system[0].text.contains("web_fetch"));
    assert!(!after.system[0].text.contains("mcp__gh__issue"));
    let again = cox_core::assemble_with(
        &[user("hi"), user("more")],
        &config,
        Tier::Code,
        &tools,
        &found,
        Path::new("/x"),
        "e",
    );
    assert_eq!(
        after.system[0].text, again.system[0].text,
        "stable after discovery"
    );

    let mut everything = config.clone();
    everything.context.deferred_tools = false;
    let all = assemble(&[user("hi")], &everything, &tools, Path::new("/w"), "d");
    assert_eq!(names(&all), ["echo", "mcp__gh__issue", "web_fetch"]);
}
