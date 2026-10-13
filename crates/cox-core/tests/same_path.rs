// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T64.8: parallel write-class calls that name the same path run one after
//! another in model order; calls on different paths still overlap.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::{drain, scenario, spawn_turn, tool_results};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use serde_json::Value;

/// Parallel like `write`/`edit`, and names its path through `touches`.
/// Logs `start:<label>` / `end:<label>` so a test sees the interleaving.
struct Stamp(Arc<Mutex<Vec<String>>>);

impl Stamp {
    fn log(&self, entry: String) {
        if let Ok(mut log) = self.0.lock() {
            log.push(entry);
        }
    }
}

#[async_trait]
impl Tool for Stamp {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stamp".into(),
            description: "parallel write stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::Write,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into()
    }
    fn touches(&self, input: &Value) -> Option<Vec<String>> {
        Some(vec![self.subject(input)])
    }
    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let label = input
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let ms = input.get("ms").and_then(Value::as_u64).unwrap_or(0);
        self.log(format!("start:{label}"));
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.log(format!("end:{label}"));
        Ok(ToolOutput {
            text: label,
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[tokio::test]
async fn same_path_calls_run_in_model_order_while_other_paths_overlap() {
    let mut config = cox_protocol::Config::default();
    config.permissions.allow = vec!["stamp".into()];
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let log = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(
        cox_provider::scripted::Scripted::from_toml(&scenario("same_path_batch"), "")
            .expect("scenario"),
    );
    let store = Arc::new(cox_core::MemoryStore::new());
    let session = cox_core::Session::new(
        config,
        provider,
        vec![Arc::new(Stamp(log.clone()))],
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");
    let running = spawn_turn(&session, "same_path_batch");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");

    let log = log.lock().expect("log").clone();
    let at = |entry: &str| {
        log.iter()
            .position(|e| e == entry)
            .unwrap_or_else(|| panic!("{entry} missing from {log:?}"))
    };
    // The slow first call must finish before the second on the same path
    // starts, or the last rename wins whichever call is faster.
    assert!(at("end:a") < at("start:b"), "{log:?}");
    // A different path is not held back by the slow call.
    assert!(at("start:c") < at("end:a"), "{log:?}");
    assert_eq!(
        tool_results(&events)
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}
