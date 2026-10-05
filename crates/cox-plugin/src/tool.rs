// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Plugin tools (PL§7, T33.12): each granted tool a plugin declares in
//! `cox_init` becomes a `WasmTool` named `wasm__<id>__<tool>`, which runs
//! the guest's `cox_tool_call` on the plugin's one `PluginHost`. Its own
//! module because it is the one place a plugin's answer becomes a
//! `ToolOutput`. Nothing here decides or shortens a call: the permission
//! engine decides it before `call`, and the core archives the full text
//! before it truncates what the model sees (D6a), as for every tool.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_plugin_api::ToolCallIn;
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::PluginError;
use crate::host::{Lane, PluginHost};
use crate::hostfn::{HostEnv, ToolSlot};

/// The guest export a plugin tool runs; `cox_output`/`cox_cancelled` are
/// allowed only inside it.
pub(crate) const EXPORT: &str = "cox_tool_call";
/// The prefix of every plugin tool's model-facing name.
pub const PREFIX: &str = "wasm__";
/// The granted-capability line prefix `grant::capability_list` writes.
pub(crate) const GRANT_PREFIX: &str = "tools:";

/// The model-facing name of plugin `id`'s tool `tool`. Plugin ids have no
/// `_`, so the second `__` always ends the id.
pub fn qualified(id: &str, tool: &str) -> String {
    format!("{PREFIX}{id}__{tool}")
}

/// One entry of `InitOut.tools`. Only these fields are read: `deferred`
/// and `concurrency` are the host's to set, never the plugin's.
#[derive(Deserialize)]
struct Declared {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "object_schema")]
    input_schema: Value,
    /// Silence means `Write`, as for an MCP tool without `readOnlyHint`.
    #[serde(default)]
    risk: Option<Risk>,
}

fn object_schema() -> Value {
    json!({ "type": "object" })
}

/// What `cox_tool_call` answers. `diff` and `structured` are not read: the
/// core treats `structured.discovered` as a `tool_search` result, which a
/// plugin must not be able to forge.
#[derive(Deserialize)]
struct Answer {
    text: String,
    #[serde(default)]
    is_error: bool,
}

/// One granted plugin tool. Its spec is fixed when it is built, from the
/// session's one `cox_init`, so every request of the session carries the
/// same bytes for it (§1.15 invariant 15).
pub struct WasmTool {
    spec: ToolSpec,
    tool: String,
    host: Arc<PluginHost>,
    env: Arc<HostEnv>,
    /// Shared by all of one plugin's tools: a queued call must never bind
    /// its `ToolSlot` over one the guest is still running.
    serial: Arc<tokio::sync::Mutex<()>>,
}

impl WasmTool {
    fn failed(&self, why: impl std::fmt::Display) -> ToolOutput {
        ToolOutput {
            text: format!("{}: {why}", self.spec.name),
            is_error: true,
            diff: None,
            structured: None,
        }
    }
}

/// Plugin `id`'s tools from its `InitOut.tools`, sorted by tool name. Only
/// granted names count and the first declaration of a name wins; each
/// dropped entry is a warning.
pub(crate) fn declared(
    id: &str,
    host: &Arc<PluginHost>,
    env: &Arc<HostEnv>,
    granted: &[String],
    specs: &[Value],
) -> (Vec<WasmTool>, Vec<String>) {
    let serial = Arc::new(tokio::sync::Mutex::new(()));
    let mut tools: Vec<WasmTool> = Vec::new();
    let mut warnings = Vec::new();
    for raw in specs {
        let d = match Declared::deserialize(raw) {
            Ok(d) => d,
            Err(e) => {
                warnings.push(format!("plugin {id}: a tool spec is not valid: {e}"));
                continue;
            }
        };
        // A granted name passed manifest validation, so the qualified name
        // is a valid tool name for every provider.
        let granted = granted
            .iter()
            .any(|g| g.strip_prefix(GRANT_PREFIX) == Some(d.name.as_str()));
        if !granted || tools.iter().any(|t| t.tool == d.name) {
            let why = if granted {
                "declared twice"
            } else {
                "not granted"
            };
            warnings.push(format!("plugin {id}: tool `{}` dropped: {why}", d.name));
            continue;
        }
        tools.push(WasmTool {
            spec: ToolSpec {
                name: qualified(id, &d.name),
                description: d.description,
                input_schema: d.input_schema,
                deferred: true,
                risk: d.risk.unwrap_or(Risk::Write),
                // Calls into one plugin run one at a time anyway (PL§4).
                concurrency: Concurrency::Exclusive,
            },
            tool: d.name,
            host: host.clone(),
            env: env.clone(),
            serial: serial.clone(),
        });
    }
    tools.sort_by(|a, b| a.tool.cmp(&b.tool));
    (tools, warnings)
}

#[async_trait]
impl Tool for WasmTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    /// The qualified name, as an MCP tool's: rules match on
    /// `wasm__<id>__<tool>`.
    fn subject(&self, _input: &Value) -> String {
        self.spec.name.clone()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        // T33.33, PL§1c: `/plugin remove` calls `PluginHost::stop` on this
        // instance but does not tear down its worker — this session's
        // frozen spec (`declared` above) still lists the tool, so a call
        // already queued by the model must answer instead of hanging or
        // running against files `plugin_cmd::remove_for_tui` just deleted.
        // Not a second permission guard (AGENTS.md "Trust boundaries"): the
        // Engine already decided this call is allowed; this only answers
        // whether the plugin behind it is still there to run it.
        if self.host.is_stopped() {
            return Err(ToolError::Denied {
                why: "plugin removed".into(),
            });
        }
        let guard = self.serial.clone().lock_owned().await;
        let (output, cancel) = (cx.output.clone(), cx.cancel.clone());
        let watched = cancel.clone();
        self.env.bind_tool(Some(ToolSlot {
            // A full channel drops the line rather than block the worker.
            output: Box::new(move |line| {
                let _ = output.try_send(line);
            }),
            cancelled: Box::new(move || watched.is_cancelled()),
        }));
        let (host, env) = (self.host.clone(), self.env.clone());
        let call = ToolCallIn {
            name: self.tool.clone(),
            input,
        };
        // Blocks until the worker answers, so off the runtime; the slot and
        // the lock are released only when the guest is done, even when the
        // caller stopped waiting.
        let job = tokio::task::spawn_blocking(move || {
            let answer = host.call::<_, Answer>(Lane::Control, EXPORT, &call, Duration::MAX);
            env.bind_tool(None);
            drop(guard);
            answer
        });
        // The guest learns of the cancel through `cox_cancelled`; the call's
        // deadline still bounds one that never asks.
        let answer = tokio::select! {
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            joined = job => joined,
        };
        match answer {
            Ok(Ok(Some(a))) => Ok(ToolOutput {
                text: a.text,
                is_error: a.is_error,
                diff: None,
                structured: None,
            }),
            Ok(Ok(None)) => Ok(self.failed(format!("the plugin has no {EXPORT} export"))),
            Ok(Err(PluginError::Timeout { .. })) => Err(ToolError::Timeout),
            Ok(Err(e)) => Ok(self.failed(e)),
            Err(e) => Ok(self.failed(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use cox_plugin_api::PluginManifest;
    use cox_protocol::config::PluginsConfig;
    use cox_protocol::ids::{CallId, SessionId};
    use cox_protocol::types::{SandboxMode, SandboxPolicy};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::LivePlugins;
    use crate::hostfn::init_input;

    const INIT: &str = r#"{"tools":[{"name":"echo","description":"echo v0","deferred":false,"risk":"read_only","concurrency":"parallel","input_schema":{"type":"object"}},{"name":"sneaky"}]}"#;

    /// `cox_init` answers `INIT` with the digit after `v` counting its
    /// calls; `cox_tool_call` sends one `cox_output` line, then polls
    /// `cox_cancelled` until it says true and answers `done`.
    fn plugin() -> String {
        let data = |at: usize, text: &str| {
            format!(r#"(data (i32.const {at}) "{}")"#, text.replace('"', "\\\""))
        };
        format!(
            r#"(module
              (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
              (import "extism:host/env" "free" (func $free (param i64)))
              (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
              (import "extism:host/env" "load_u8" (func $load (param i64) (result i32)))
              (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
              (import "cox:host/v1" "cox_output" (func $output (param i64) (result i64)))
              (import "cox:host/v1" "cox_cancelled" (func $cancelled (param i64) (result i64)))
              (memory 1)
              (global $n (mut i32) (i32.const 48))
              {init} {line} {null} {done}
              (func $copy (param $p i32) (param $len i32) (result i64) (local $off i64) (local $i i32)
                (local.set $off (call $alloc (i64.extend_i32_u (local.get $len))))
                (block $end (loop $next
                  (br_if $end (i32.ge_u (local.get $i) (local.get $len)))
                  (call $store (i64.add (local.get $off) (i64.extend_i32_u (local.get $i)))
                    (i32.load8_u (i32.add (local.get $p) (local.get $i))))
                  (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br $next)))
                (local.get $off))
              (func $out (param $p i32) (param $len i32)
                (call $output_set (call $copy (local.get $p) (local.get $len))
                  (i64.extend_i32_u (local.get $len))))
              (func (export "cox_init") (result i32)
                (global.set $n (i32.add (global.get $n) (i32.const 1)))
                (i32.store8 (i32.const {digit}) (global.get $n))
                (call $out (i32.const 0) (i32.const {init_len})) (i32.const 0))
              (func (export "cox_tool_call") (result i32) (local $arg i64) (local $r i64) (local $c i32)
                (call $free (call $output (call $copy (i32.const 512) (i32.const 9))))
                (local.set $arg (call $copy (i32.const 576) (i32.const 4)))
                (block $stop (loop $wait
                  (local.set $r (call $cancelled (local.get $arg)))
                  (local.set $c (call $load (i64.add (local.get $r) (i64.const 6))))
                  (call $free (local.get $r))
                  (br_if $stop (i32.eq (local.get $c) (i32.const 116)))
                  (br $wait)))
                (call $out (i32.const 640) (i32.const 15)) (i32.const 0)))"#,
            init = data(0, INIT),
            line = data(512, r#""working""#),
            null = data(576, "null"),
            done = data(640, r#"{"text":"done"}"#),
            digit = INIT.find("v0").expect("v0") + 1,
            init_len = INIT.len(),
        )
    }

    fn manifest() -> PluginManifest {
        serde_json::from_value(json!({
            "api": 1, "id": "frz", "version": "0.1.0", "name": "frz", "wasm": "plugin.wasm",
            "limits": { "call_ms": 30000 },
            "capabilities": { "tools": ["echo"] },
        }))
        .expect("manifest")
    }

    /// Runs the plugin's `cox_init` once more, as the next session would.
    fn init_again(live: &LivePlugins) -> cox_plugin_api::InitOut {
        let info = cox_plugin_api::SessionInfo {
            id: "next".into(),
            cwd: "/w".into(),
        };
        live.plugins()[0]
            .host()
            .init(&init_input(&manifest(), &PluginsConfig::default(), info))
            .expect("init again")
    }

    fn started() -> LivePlugins {
        let mut live = LivePlugins::default();
        let store = Arc::new(crate::hostfn::tests::MemKv::default());
        live.load(&manifest(), plugin().as_bytes(), store)
            .expect("loads");
        let warnings = live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        assert!(warnings.is_empty(), "{warnings:?}");
        live
    }

    #[test]
    fn plugin_tool_specs_frozen_within_session() {
        let live = started();
        let (tools, warnings) = live.tools();
        assert_eq!(warnings, ["plugin frz: tool `sneaky` dropped: not granted"]);
        let frozen = tools[0].spec();
        assert_eq!(frozen.name, "wasm__frz__echo");
        assert_eq!(frozen.description, "echo v1");
        // Always deferred and exclusive, whatever the plugin says.
        assert!(frozen.deferred);
        assert_eq!(frozen.concurrency, Concurrency::Exclusive);
        assert_eq!(frozen.risk, Risk::ReadOnly);
        // The next `cox_init` (the next session's) declares something else…
        let next = init_again(&live);
        assert_eq!(next.tools[0]["description"], "echo v2");
        // …which this session's tool never shows.
        assert_eq!(tools[0].spec(), frozen);
        assert_eq!(live.tools().0[0].spec(), frozen);
    }

    fn cx(output: mpsc::Sender<String>, cancel: CancellationToken) -> ToolCx {
        ToolCx {
            roots: vec![PathBuf::from("/w")],
            writable_roots: vec![PathBuf::from("/w")],
            cwd: PathBuf::from("/w"),
            sandbox: SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            archive: Arc::new(cox_core::MemoryStore::new()),
            cancel,
            output,
            session: SessionId::new(),
            call: CallId::new(),
            agent: None,
            preset: None,
            relay: None,
        }
    }

    #[tokio::test]
    async fn tool_call_streams_output_and_stops_on_cox_cancelled() {
        let live = started();
        let tool = live.tools().0.remove(0);
        let (tx, mut rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let cx = cx(tx, cancel.clone());
        let run = tokio::spawn(async move { tool.call(json!({}), &cx).await });
        let line = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await;
        assert_eq!(line.expect("a line in time"), Some("working".to_string()));
        cancel.cancel();
        let out = tokio::time::timeout(Duration::from_secs(10), run)
            .await
            .expect("returns on cancel")
            .expect("joins");
        assert!(matches!(out, Err(ToolError::Cancelled)), "{out:?}");
        // The guest read `cox_cancelled` and returned, so its worker is free
        // long before the 30 s `call_ms` would have stopped it.
        let started = Instant::now();
        init_again(&live);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// T33.33, PL§1c (Done-when): once `/plugin remove` calls
    /// `PluginHost::stop` on this session's instance, its `WasmTool`
    /// answers `Denied` for every further call — never reaching the guest,
    /// so it cannot hang on `cox_tool_call`'s `cox_cancelled` loop — until
    /// the next session builds a fresh `PluginHost`. `stopped` lives on
    /// this one instance, so a fresh `cox_init` elsewhere (`init_again`,
    /// standing in for the next session's `open`) is unaffected.
    #[tokio::test]
    async fn removed_plugin_tool_is_denied_until_next_session() {
        let live = started();
        let tool = live.tools().0.remove(0);
        // `tools()` erases `WasmTool` to `Arc<dyn Tool>`, so `stop` is
        // called through the same accessor `init_again` uses — the clone
        // `declared` gave the tool shares this one `PluginHost`.
        live.plugins()[0].host().stop();
        let (tx, _rx) = mpsc::channel(4);
        let cx = cx(tx, CancellationToken::new());
        let out = tool.call(json!({}), &cx).await;
        assert!(
            matches!(&out, Err(ToolError::Denied { why }) if why == "plugin removed"),
            "{out:?}"
        );
        init_again(&live);
    }
}
