// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Shared loop-test harness: stub tools, Scripted scenarios and an in-memory
//! session. Separate so `turn.rs` and `dedup.rs` drive the same loop.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Event, Risk, Submission, ToolOutput, ToolSpec};
use cox_provider::scripted::Scripted;
use serde_json::Value;
use tokio::sync::mpsc;

/// Read-only, parallel; returns `input.text`.
pub struct Echo;
/// Write, exclusive; subject is `input.path`.
pub struct Touch;
/// Read-only; loops until cancelled.
pub struct Slow;
/// Exec, exclusive; names no path, so a checkpoint must snapshot (T26.1).
pub struct Shell;

fn text(text: &str, is_error: bool) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error,
        diff: None,
        structured: None,
    }
}

#[async_trait]
impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "echo input text".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(text(&self.subject(&input), false))
    }
}

#[async_trait]
impl Tool for Touch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "touch".into(),
            description: "exclusive write stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }
    /// Like `edit`/`write`: the pre-image is read from the named path.
    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        Some(vec![self.subject(input)])
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(text("touched", false))
    }
}

#[async_trait]
impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".into(),
            description: "exec stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(text("ran", false))
    }
}

#[async_trait]
impl Tool for Slow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow".into(),
            description: "polls cancel".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        "slow".into()
    }
    async fn call(&self, _input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        loop {
            if cx.cancel.is_cancelled() {
                return Ok(text("cancelled", true));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Reads `tests/scenarios/<name>.toml`.
pub fn scenario(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scenarios")
        .join(format!("{name}.toml"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(Echo),
        Arc::new(Touch),
        Arc::new(Slow),
        Arc::new(Shell),
    ]
}

/// A session over `toml` with an in-memory store; the receiver is taken.
pub fn open(
    toml: &str,
    mut config: cox_protocol::Config,
) -> (Session, Arc<MemoryStore>, mpsc::Receiver<Event>) {
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider = Arc::new(Scripted::from_toml(toml, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        tools(),
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, store, rx)
}

/// Collects events up to and including `TurnDone`.
pub async fn drain(rx: &mut mpsc::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream closed");
        let done = matches!(ev, Event::TurnDone { .. });
        out.push(ev);
        if done {
            break;
        }
    }
    out
}

/// Spawns a user turn so the test can drain events (or submit `Interrupt`
/// / `Approve`) while it runs; the event channel is bounded, so a turn that
/// emits more than its capacity would otherwise block in `submit`.
pub fn spawn_turn(
    session: &Session,
    text: &str,
) -> tokio::task::JoinHandle<Result<(), cox_protocol::errors::CoreError>> {
    let session = session.clone();
    let text = text.to_owned();
    tokio::spawn(async move {
        session
            .submit(Submission::UserTurn {
                text,
                attachments: vec![],
                confirm_think: false,
            })
            .await
    })
}

/// One user turn over scenario `name` with `config`.
pub async fn run_with(
    name: &str,
    config: cox_protocol::Config,
) -> (Vec<Event>, Arc<MemoryStore>, Session) {
    let (session, store, mut rx) = open(&scenario(name), config);
    let running = spawn_turn(&session, name);
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    (events, store, session)
}

/// `(ok, visible)` of every `ToolCallDone`, in order.
pub fn tool_results(events: &[Event]) -> Vec<(bool, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ToolCallDone { result, .. } => Some((result.ok, result.visible.clone())),
            _ => None,
        })
        .collect()
}
