// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `cox:host/v1` host functions (PL§4, T33.9): what a guest may ask the
//! host for, each call checked against the plugin's grant and the export it
//! is called from. Separate from `host` because that module owns threads and
//! queues, this one owns what a call is allowed to do.
//!
//! The wire matches the guest SDK (`plugins/sdk/src/lib.rs`, `docs/plugins.md`):
//! every import is `(u64) -> u64`; the argument block holds one JSON value
//! (an object keyed by argument name when there are several, `null` when
//! there are none) and the reply block holds `{"Ok": T}` or
//! `{"Err": AbiError}`, so a refusal is a value the plugin can handle, never
//! a trap that ends its call.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cox_plugin_api::{
    AbiError, HttpReq, InitIn, ModelCall, ModelTier, NoticeLevel, PluginManifest, SessionInfo,
    ToolCallIn,
};
use cox_protocol::config::PluginsConfig;
use cox_protocol::errors::CoreError;
use cox_protocol::traits::{KV_PLUGIN_LIMIT, KV_VALUE_LIMIT, ModelCaller, ToolInvoker};
use cox_protocol::types::{Level, Request, Tier, ToolOutput};
use cox_protocol::{PluginStore, StoreError};
use cox_sanitize::redact::scrub;
use cox_sanitize::sanitize;
use extism::{CurrentPlugin, Function, PTR, UserData, Val};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::context::Context;
use crate::net::Net;

/// The import module every host function lives in.
pub const NAMESPACE: &str = "cox:host/v1";

/// Every import ABI v1 names. All are registered so a guest built with the
/// SDK links whatever it imports; one this host does not implement yet
/// answers `Err(Failed)` instead of failing the load.
const IMPORTS: [&str; 12] = [
    "cox_log",
    "cox_notify",
    "cox_kv_get",
    "cox_kv_put",
    "cox_kv_delete",
    "cox_context",
    "cox_invoke_tool",
    "cox_model_call",
    "cox_http",
    "cox_output",
    "cox_cancelled",
    "cox_redraw",
];

/// Log lines a plugin may write per second (PL§4 "rate-limited").
const LOG_LINES_PER_SEC: u32 = 20;
/// Notices waiting for the session to drain them; more is a refusal, so a
/// loop inside one call cannot flood the transcript.
const PENDING_NOTICES: usize = 16;
/// Longest kv key. Keys are not counted in the value quota, so without a
/// cap one key could store what the quota exists to stop.
const MAX_KEY_BYTES: usize = 256;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct LogWindow {
    start: Option<Instant>,
    lines: u32,
    dropped: u32,
}

impl LogWindow {
    /// Whether one more line fits in the current second, and how many lines
    /// the second that just closed dropped.
    fn admit(&mut self, now: Instant) -> (bool, u32) {
        let mut closed = 0;
        if self
            .start
            .is_none_or(|s| now.duration_since(s) >= Duration::from_secs(1))
        {
            closed = std::mem::take(&mut self.dropped);
            self.start = Some(now);
            self.lines = 0;
        }
        if self.lines < LOG_LINES_PER_SEC {
            self.lines += 1;
            (true, closed)
        } else {
            self.dropped += 1;
            (false, closed)
        }
    }
}

/// What one plugin's host functions act on: its id, its granted
/// capabilities, the stores behind them, and the export now running.
pub struct HostEnv {
    id: String,
    granted: BTreeSet<String>,
    store: Option<Arc<dyn PluginStore>>,
    context: Arc<Context>,
    export: Mutex<String>,
    notices: Mutex<Vec<(Level, String)>>,
    log: Mutex<LogWindow>,
    // T33.15: `cox_model_call`'s route to `cox-core`'s router/budget/ledger
    // over the `ModelCaller` trait (this crate may not depend on cox-core),
    // and the tokio handle to block this plugin's plain OS worker thread on
    // it (`PluginHost`'s call, not a tokio runtime worker: `crate::host`).
    model_caller: Option<Arc<dyn ModelCaller>>,
    runtime: Option<tokio::runtime::Handle>,
    tool: Mutex<Option<ToolSlot>>,
    // T33.13: `cox_invoke_tool`'s route to the session's tool path, run on
    // `runtime` like `model_caller`.
    tool_invoker: Option<Arc<dyn ToolInvoker>>,
    // T33.14.1: `cox_http`'s allow-list, from the same grant as `granted`.
    net: Net,
}

/// The running `cox_tool_call`'s end of its `ToolCx` (T33.12): where
/// `cox_output` lines go and what `cox_cancelled` reads. Closures, so this
/// crate needs neither the channel nor the token type by name.
pub(crate) struct ToolSlot {
    pub(crate) output: Box<dyn Fn(String) + Send + Sync>,
    pub(crate) cancelled: Box<dyn Fn() -> bool + Send + Sync>,
}

impl HostEnv {
    /// An environment that grants nothing: enough to link a module and run
    /// `cox_log`/`cox_notify`, nothing more.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            granted: BTreeSet::new(),
            store: None,
            context: Arc::new(Context::new()),
            export: Mutex::new(String::new()),
            notices: Mutex::new(Vec::new()),
            log: Mutex::new(LogWindow::default()),
            model_caller: None,
            runtime: None,
            tool: Mutex::new(None),
            tool_invoker: None,
            net: Net::new(&BTreeSet::new()),
        }
    }

    /// The granted capability lines (`grant::capability_list` of a manifest
    /// that `grant::check` found `Granted`) and the kv store.
    pub fn with_grant(mut self, granted: Vec<String>, store: Arc<dyn PluginStore>) -> Self {
        self.granted = granted.into_iter().collect();
        self.net = Net::new(&self.granted);
        self.store = Some(store);
        self
    }

    /// The session's folded context, shared by all of its plugins.
    pub fn with_context(mut self, context: Arc<Context>) -> Self {
        self.context = context;
        self
    }

    /// The seam `cox_model_call` routes through (T33.44 wires the real
    /// `Session` and its runtime `Handle` in at session open; without this,
    /// `cox_model_call` answers `Failed`).
    pub fn with_model_caller(
        mut self,
        caller: Arc<dyn ModelCaller>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        self.model_caller = Some(caller);
        self.runtime = Some(runtime);
        self
    }

    /// The seam `cox_invoke_tool` routes through (T33.13), on the same
    /// runtime `cox_model_call` blocks on; without it, `Failed`.
    pub fn with_tool_invoker(
        mut self,
        invoker: Arc<dyn ToolInvoker>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        self.tool_invoker = Some(invoker);
        self.runtime = Some(runtime);
        self
    }

    /// The plugin id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Takes the notices `cox_notify` queued, already capped at `Warn` and
    /// sanitized, for the session to emit as `Event::Notice`.
    pub fn take_notices(&self) -> Vec<(Level, String)> {
        std::mem::take(&mut *lock(&self.notices))
    }

    /// Queues one notice for `take_notices`, sanitized and redacted: from
    /// `cox_notify` and from `Effects.notices` (T33.10) alike.
    pub(crate) fn notify(&self, level: NoticeLevel, text: &str) -> Result<(), AbiError> {
        let mut queue = lock(&self.notices);
        if queue.len() >= PENDING_NOTICES {
            return Err(failed("too many notices"));
        }
        let level = match level {
            NoticeLevel::Info => Level::Info,
            NoticeLevel::Warn => Level::Warn,
        };
        queue.push((level, clean(text)));
        Ok(())
    }

    /// Records the export the worker is about to call (`""` when idle).
    pub(crate) fn enter(&self, export: &str) {
        export.clone_into(&mut lock(&self.export));
    }

    /// Binds (or, with `None`, releases) the tool call `cox_output` and
    /// `cox_cancelled` act on; `WasmTool` holds it for one call.
    pub(crate) fn bind_tool(&self, slot: Option<ToolSlot>) {
        *lock(&self.tool) = slot;
    }

    /// Every `cox:host/v1` import, bound to this environment.
    pub(crate) fn functions(self: &Arc<Self>) -> Vec<Function> {
        IMPORTS
            .iter()
            .map(|&name| {
                Function::new(
                    name,
                    [PTR],
                    [PTR],
                    UserData::new(self.clone()),
                    move |plugin: &mut CurrentPlugin,
                          inputs: &[Val],
                          outputs: &mut [Val],
                          env: UserData<Arc<HostEnv>>| {
                        let env = lock(&*env.get()?).clone();
                        let reply = match inputs.first() {
                            Some(offset) => plugin
                                .memory_get_val::<&[u8]>(offset)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice(bytes).ok())
                                .map_or_else(
                                    || Err(failed("the argument is not JSON")),
                                    |arg| env.dispatch(name, arg),
                                ),
                            None => Err(failed("no argument")),
                        };
                        let bytes = serde_json::to_vec(&reply)?;
                        match outputs.first_mut() {
                            Some(out) => plugin.memory_set_val(out, bytes.as_slice()),
                            None => Ok(()),
                        }
                    },
                )
                .with_namespace(NAMESPACE)
            })
            .collect()
    }

    fn dispatch(&self, import: &str, arg: Value) -> Result<Value, AbiError> {
        match import {
            "cox_log" => {
                let line: Line = parse(arg)?;
                self.log(line);
                Ok(Value::Null)
            }
            "cox_notify" => {
                self.outside_render()?;
                let line: Line = parse(arg)?;
                self.notify(cap(line.level), &line.text)?;
                Ok(Value::Null)
            }
            "cox_kv_get" => {
                let store = self.kv()?;
                let key = key(parse(arg)?)?;
                match store.kv_get(&self.id, &key).map_err(store_error)? {
                    Some(bytes) => serde_json::from_slice(&bytes)
                        .map_err(|_| failed("the stored value is not JSON")),
                    None => Ok(Value::Null),
                }
            }
            "cox_kv_put" => {
                let store = self.kv()?;
                let KvPut { key: k, value } = parse(arg)?;
                let bytes = serde_json::to_vec(&value).map_err(|e| failed(&e.to_string()))?;
                let limit = if bytes.len() > KV_VALUE_LIMIT {
                    KV_VALUE_LIMIT
                } else {
                    KV_PLUGIN_LIMIT
                };
                match store.kv_put(&self.id, &key(k)?, &bytes) {
                    Err(StoreError::QuotaExceeded) => Err(AbiError::TooLarge {
                        limit: limit as u64,
                    }),
                    other => other.map(|()| Value::Null).map_err(store_error),
                }
            }
            "cox_kv_delete" => {
                let store = self.kv()?;
                let key = key(parse(arg)?)?;
                store.kv_delete(&self.id, &key).map_err(store_error)?;
                Ok(Value::Null)
            }
            "cox_context" => {
                self.require("context")?;
                Ok(self.context.snapshot())
            }
            "cox_model_call" => self.model_call(arg),
            "cox_invoke_tool" => self.invoke_tool(arg),
            "cox_http" => self.http(arg),
            "cox_output" => {
                self.in_tool_call()?;
                let line: String = parse(arg)?;
                if let Some(slot) = lock(&self.tool).as_ref() {
                    (slot.output)(line);
                }
                Ok(Value::Null)
            }
            "cox_cancelled" => {
                self.in_tool_call()?;
                // No bound call means its caller already gave up on it.
                let slot = lock(&self.tool);
                Ok(Value::Bool(slot.as_ref().is_none_or(|s| (s.cancelled)())))
            }
            other => Err(failed(&format!("`{other}` is not available in this cox"))),
        }
    }

    fn require(&self, capability: &str) -> Result<(), AbiError> {
        if self.granted.contains(capability) {
            Ok(())
        } else {
            Err(AbiError::NotGranted {
                capability: capability.into(),
            })
        }
    }

    /// `cox_render` has a strict time cap, so it may only read (PL§4).
    fn outside_render(&self) -> Result<(), AbiError> {
        match lock(&self.export).as_str() {
            "cox_render" | "cox_render_item" => Err(AbiError::NotInThisContext),
            _ => Ok(()),
        }
    }

    /// `cox_output`/`cox_cancelled` exist only inside `cox_tool_call` (PL§4).
    fn in_tool_call(&self) -> Result<(), AbiError> {
        match lock(&self.export).as_str() {
            crate::tool::EXPORT => Ok(()),
            _ => Err(AbiError::NotInThisContext),
        }
    }

    fn kv(&self) -> Result<&dyn PluginStore, AbiError> {
        self.require("kv")?;
        self.outside_render()?;
        self.store.as_deref().ok_or_else(|| AbiError::NotGranted {
            capability: "kv".into(),
        })
    }

    fn log(&self, line: Line) {
        let (admit, dropped) = lock(&self.log).admit(Instant::now());
        let id = &self.id;
        if dropped > 0 {
            tracing::warn!(plugin = %id, dropped, "plugin log lines dropped by the rate limit");
        }
        if !admit {
            return;
        }
        let text = clean(&line.text);
        match cap(line.level) {
            NoticeLevel::Info => tracing::info!(plugin = %id, "{text}"),
            NoticeLevel::Warn => tracing::warn!(plugin = %id, "{text}"),
        }
    }

    /// `cox_model_call` (PL§7d, T33.15): the tier is clamped to the grant
    /// (`model:code` covers `model:cheap`, mirroring `grant::covered`) and
    /// never reaches `think` — `ModelTier` cannot even express it — then the
    /// call blocks this plugin's worker thread on `ModelCaller::call`, which
    /// runs it through the router and the budget gate and writes the one
    /// `usage` row (`crates/cox-core/src/plugin_model.rs`).
    fn model_call(&self, arg: Value) -> Result<Value, AbiError> {
        self.outside_render()?;
        let granted = if self.granted.contains(MODEL_CODE) {
            Tier::Code
        } else if self.granted.contains(MODEL_CHEAP) {
            Tier::Cheap
        } else {
            return Err(AbiError::NotGranted {
                capability: MODEL_CHEAP.into(),
            });
        };
        let call: ModelCall = parse(arg)?;
        let requested = match call.tier {
            ModelTier::Cheap => Tier::Cheap,
            ModelTier::Code => Tier::Code,
        };
        // `Tier`'s order is cost order, so the cheaper of the two is the clamp.
        let tier = requested.min(granted);
        let request: Request = serde_json::from_value(call.request)
            .map_err(|e| failed(&format!("bad request: {e}")))?;
        let caller = self
            .model_caller
            .as_ref()
            .ok_or_else(|| failed("model calls are not available in this cox"))?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| failed("model calls are not available in this cox"))?;
        let events = runtime
            .block_on(caller.call(&self.id, tier, request))
            .map_err(model_call_error)?;
        serde_json::to_value(events).map_err(|e| failed(&e.to_string()))
    }

    /// `cox_http` (PL§4, T33.14.1): to a granted `net` host from any export
    /// but render, blocking this plugin's worker on the session runtime as
    /// `cox_model_call` does.
    fn http(&self, arg: Value) -> Result<Value, AbiError> {
        self.outside_render()?;
        let request: HttpReq = parse(arg)?;
        let url = self.net.target(&request.url)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| failed("http is not available in this cox"))?;
        let response = runtime.block_on(self.net.send(request, url))?;
        serde_json::to_value(response).map_err(|e| failed(&e.to_string()))
    }

    /// `cox_invoke_tool` (PL§4, T33.13): a granted tool, run by the session
    /// on the model's own path (`PreToolUse`, `Engine::decide`, sandbox,
    /// archive) while this plugin's one worker blocks on it. So nothing on
    /// that path may need this plugin again: the loop waits on `cox_hook`,
    /// `cox_decide` and `cox_provider_stream`, `cox_render` may only read,
    /// and the plugin's own tools and hooks would queue behind this call.
    fn invoke_tool(&self, arg: Value) -> Result<Value, AbiError> {
        match lock(&self.export).as_str() {
            "cox_on_event" | "cox_command" | "cox_key" | crate::tool::EXPORT => {}
            _ => return Err(AbiError::NotInThisContext),
        }
        let ToolCallIn { name, input } = parse(arg)?;
        self.require(&format!("invoke:{name}"))?;
        if name.starts_with(&crate::tool::qualified(&self.id, "")) {
            return Err(failed("a plugin cannot invoke its own tool"));
        }
        // Which hooks a call fires depends on the tool (`agent` runs a
        // whole turn), so any hook grant could wait on this worker.
        if self.granted.iter().any(|g| g.starts_with("hooks:")) {
            return Err(failed("a plugin that holds hooks cannot invoke tools"));
        }
        let (Some(invoker), Some(runtime)) = (&self.tool_invoker, &self.runtime) else {
            return Err(failed("tool calls are not available in this cox"));
        };
        let result = runtime
            .block_on(invoker.invoke(&self.id, &name, input))
            .map_err(|e| failed(&e.to_string()))?;
        let output = ToolOutput {
            text: result.visible,
            is_error: !result.ok,
            diff: result.diff,
            structured: None,
        };
        serde_json::to_value(output).map_err(|e| failed(&e.to_string()))
    }
}

/// `model:cheap` and `model:code` (mirrors `grant.rs`'s private consts; not
/// imported because this crate's `dispatch` should not need `grant::covered`
/// for a single two-tier comparison).
const MODEL_CHEAP: &str = "model:cheap";
const MODEL_CODE: &str = "model:code";

fn model_call_error(error: CoreError) -> AbiError {
    match error {
        CoreError::Budget { .. } => AbiError::Budget,
        other => failed(&other.to_string()),
    }
}

/// `cox_log`/`cox_notify`'s arguments. The level stays raw JSON so a level
/// past `warn` is capped rather than refused.
#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    level: Value,
    text: String,
}

/// `cox_kv_put`'s arguments.
#[derive(Deserialize)]
struct KvPut {
    key: String,
    value: Value,
}

/// A plugin can never raise `Budget` or `Security` (PL§4): anything but
/// `info` becomes `warn`.
fn cap(level: Value) -> NoticeLevel {
    serde_json::from_value(level).unwrap_or(NoticeLevel::Warn)
}

/// Plugin text leaving the host: escapes stripped, secret shapes redacted.
fn clean(text: &str) -> String {
    let safe = sanitize(text);
    scrub(&safe).into_owned()
}

fn key(key: String) -> Result<String, AbiError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(AbiError::TooLarge {
            limit: MAX_KEY_BYTES as u64,
        });
    }
    Ok(key)
}

fn parse<T: DeserializeOwned>(arg: Value) -> Result<T, AbiError> {
    serde_json::from_value(arg).map_err(|e| failed(&format!("bad argument: {e}")))
}

fn failed(message: &str) -> AbiError {
    AbiError::Failed {
        message: message.into(),
    }
}

fn store_error(e: StoreError) -> AbiError {
    failed(&e.to_string())
}

/// `cox_init`'s input (PL§4): the plugin's own `[plugins.<id>]` table (an
/// empty table when there is none) and the capabilities it was granted,
/// which under `Verdict::Granted` are exactly the ones its manifest asks for.
pub fn init_input(
    manifest: &PluginManifest,
    plugins: &PluginsConfig,
    session: SessionInfo,
) -> InitIn {
    InitIn {
        api: cox_plugin_api::API_MAJOR,
        plugin_id: manifest.id.clone(),
        config: plugins
            .entries
            .get(&manifest.id)
            .cloned()
            .unwrap_or_else(|| json!({})),
        session,
        granted: serde_json::to_value(&manifest.capabilities).unwrap_or_else(|_| json!({})),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use cox_plugin_api::{Capabilities, Limits};
    use cox_protocol::{GrantScope, PluginGrant};

    use tokio::runtime::Runtime;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::host::{Lane, PluginHost};

    /// An in-memory `PluginStore`: only kv is exercised here.
    #[derive(Default)]
    pub(crate) struct MemKv(Mutex<HashMap<(String, String), Vec<u8>>>);

    impl PluginStore for MemKv {
        fn grant_get(
            &self,
            _: &str,
            _: &GrantScope,
            _: &str,
        ) -> Result<Option<PluginGrant>, StoreError> {
            Ok(None)
        }
        fn grant_put(&self, _: &PluginGrant) -> Result<(), StoreError> {
            Ok(())
        }
        fn grant_set_enabled(
            &self,
            _: &str,
            _: &GrantScope,
            _: &str,
            _: bool,
        ) -> Result<(), StoreError> {
            Ok(())
        }
        fn grants_delete(&self, _: &str) -> Result<(), StoreError> {
            Ok(())
        }
        fn kv_get(&self, id: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
            Ok(lock(&self.0).get(&(id.into(), key.into())).cloned())
        }
        fn kv_put(&self, id: &str, key: &str, value: &[u8]) -> Result<(), StoreError> {
            lock(&self.0).insert((id.into(), key.into()), value.to_vec());
            Ok(())
        }
        fn kv_delete(&self, id: &str, key: &str) -> Result<(), StoreError> {
            lock(&self.0).remove(&(id.into(), key.into()));
            Ok(())
        }
        fn kv_delete_all(&self, _: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    /// A guest whose every export but `cox_init` passes its input to the
    /// `cox:host/v1` import `import` and returns its reply block.
    fn relay(import: &str) -> Vec<u8> {
        format!(
            r#"(module
              (import "extism:host/env" "input_length" (func $input_length (result i64)))
              (import "extism:host/env" "input_load_u8" (func $load (param i64) (result i32)))
              (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
              (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
              (import "extism:host/env" "length" (func $length (param i64) (result i64)))
              (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
              (import "cox:host/v1" "{import}" (func $host (param i64) (result i64)))
              (func $relay (result i32) (local $n i64) (local $off i64) (local $i i64) (local $r i64)
                (local.set $n (call $input_length))
                (local.set $off (call $alloc (local.get $n)))
                (block $done (loop $copy
                  (br_if $done (i64.ge_u (local.get $i) (local.get $n)))
                  (call $store (i64.add (local.get $off) (local.get $i)) (call $load (local.get $i)))
                  (local.set $i (i64.add (local.get $i) (i64.const 1)))
                  (br $copy)))
                (local.set $r (call $host (local.get $off)))
                (call $output_set (local.get $r) (call $length (local.get $r)))
                (i32.const 0))
              (func (export "cox_init") (result i32) (i32.const 0))
              (export "cox_command" (func $relay))
              (export "cox_hook" (func $relay))
              (export "cox_key" (func $relay))
              (export "cox_on_event" (func $relay))
              (export "cox_tool_call" (func $relay))
              (export "cox_render" (func $relay)))"#
        )
        .into_bytes()
    }

    fn call(env: &Arc<HostEnv>, import: &str, export: &str, arg: Value) -> Value {
        let host = PluginHost::load_with(&relay(import), &Limits::default(), env.clone())
            .expect("relay loads");
        host.call::<_, Value>(Lane::Control, export, &arg, Duration::from_secs(5))
            .expect("relay call")
            .expect("relay export exists")
    }

    #[test]
    fn notify_cannot_raise_security_level() {
        let env = Arc::new(HostEnv::new("t"));
        for level in ["security", "budget", "warn"] {
            let reply = call(
                &env,
                "cox_notify",
                "cox_command",
                json!({ "level": level, "text": "\u{1b}[2Jpwned" }),
            );
            assert_eq!(reply, json!({ "Ok": null }), "{level}");
        }
        call(
            &env,
            "cox_notify",
            "cox_command",
            json!({ "level": "info", "text": "hi" }),
        );
        let notices = env.take_notices();
        assert_eq!(notices.len(), 4);
        assert!(
            notices[..3]
                .iter()
                .all(|n| *n == (Level::Warn, "pwned".into())),
            "{notices:?}"
        );
        assert_eq!(notices[3], (Level::Info, "hi".into()));
        // `cox_render` may only read.
        let reply = call(
            &env,
            "cox_notify",
            "cox_render",
            json!({ "level": "info", "text": "x" }),
        );
        assert_eq!(reply, json!({ "Err": { "kind": "not_in_this_context" } }));
        assert!(env.take_notices().is_empty());
    }

    #[test]
    fn kv_denied_without_capability() {
        let store = Arc::new(MemKv::default());
        let put = json!({ "key": "turns", "value": 3 });

        let ungranted =
            Arc::new(HostEnv::new("t").with_grant(vec!["context".into()], store.clone()));
        let reply = call(&ungranted, "cox_kv_put", "cox_command", put.clone());
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "not_granted", "capability": "kv" } })
        );
        assert!(lock(&store.0).is_empty(), "a refused put wrote");

        let granted = Arc::new(HostEnv::new("t").with_grant(vec!["kv".into()], store.clone()));
        assert_eq!(
            call(&granted, "cox_kv_put", "cox_command", put),
            json!({ "Ok": null })
        );
        assert_eq!(
            call(&granted, "cox_kv_get", "cox_command", json!("turns")),
            json!({ "Ok": 3 })
        );
        assert_eq!(
            call(&granted, "cox_kv_get", "cox_render", json!("turns")),
            json!({ "Err": { "kind": "not_in_this_context" } })
        );
        assert_eq!(
            call(&granted, "cox_kv_delete", "cox_command", json!("turns")),
            json!({ "Ok": null })
        );
        assert_eq!(
            call(&granted, "cox_kv_get", "cox_command", json!("turns")),
            json!({ "Ok": null })
        );
    }

    #[test]
    fn context_needs_its_capability_and_is_allowed_in_render() {
        let store = Arc::new(MemKv::default());
        let none = Arc::new(HostEnv::new("t"));
        assert_eq!(
            call(&none, "cox_context", "cox_command", Value::Null),
            json!({ "Err": { "kind": "not_granted", "capability": "context" } })
        );
        let env = Arc::new(HostEnv::new("t").with_grant(vec!["context".into()], store));
        let reply = call(&env, "cox_context", "cox_render", Value::Null);
        assert!(reply["Ok"]["items"].is_array(), "{reply}");
    }

    #[test]
    fn output_and_cancelled_work_only_inside_tool_call() {
        let env = Arc::new(HostEnv::new("t"));
        let refused = json!({ "Err": { "kind": "not_in_this_context" } });
        assert_eq!(call(&env, "cox_output", "cox_command", json!("x")), refused);
        assert_eq!(
            call(&env, "cox_cancelled", "cox_command", Value::Null),
            refused
        );
        // Unbound: the caller has gone, so the guest should stop.
        assert_eq!(
            call(&env, "cox_cancelled", "cox_tool_call", Value::Null),
            json!({ "Ok": true })
        );
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        env.bind_tool(Some(ToolSlot {
            output: Box::new(move |line| lock(&sink).push(line)),
            cancelled: Box::new(|| false),
        }));
        assert_eq!(
            call(&env, "cox_output", "cox_tool_call", json!("step 1")),
            json!({ "Ok": null })
        );
        assert_eq!(
            call(&env, "cox_cancelled", "cox_tool_call", Value::Null),
            json!({ "Ok": false })
        );
        assert_eq!(*lock(&lines), ["step 1"]);
    }

    #[test]
    fn unimplemented_import_links_and_answers_failed() {
        let env = Arc::new(HostEnv::new("t"));
        let reply = call(&env, "cox_redraw", "cox_command", json!({}));
        assert_eq!(reply["Err"]["kind"], "failed", "{reply}");
    }

    /// An environment granted `net:<net>`, blocking on `rt` like a session's.
    fn http_env(net: &str, rt: &Runtime) -> Arc<HostEnv> {
        let mut env =
            HostEnv::new("t").with_grant(vec![format!("net:{net}")], Arc::new(MemKv::default()));
        env.runtime = Some(rt.handle().clone());
        Arc::new(env)
    }

    fn mock(rt: &Runtime, route: &str, response: ResponseTemplate) -> MockServer {
        rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(path(route))
                .respond_with(response)
                .mount(&server)
                .await;
            server
        })
    }

    fn hits(rt: &Runtime, server: &MockServer) -> usize {
        rt.block_on(server.received_requests())
            .map_or(0, |requests| requests.len())
    }

    fn get(url: String) -> Value {
        json!({ "method": "GET", "url": url })
    }

    #[test]
    fn http_to_allowed_host_round_trips() {
        let rt = Runtime::new().expect("runtime");
        let response = ResponseTemplate::new(201)
            .set_body_string("hi")
            .insert_header("x-a", "1");
        let server = mock(&rt, "/ok", response);
        let request = json!({
            "method": "POST",
            "url": format!("{}/ok", server.uri()),
            "headers": { "x-q": "v" },
            "body": "sent",
        });
        let reply = call(
            &http_env("127.0.0.1", &rt),
            "cox_http",
            "cox_command",
            request,
        );
        assert_eq!(reply["Ok"]["status"], 201, "{reply}");
        assert_eq!(reply["Ok"]["body"], "hi");
        assert_eq!(reply["Ok"]["headers"]["x-a"], "1");
        let got = rt.block_on(server.received_requests()).expect("recorded");
        assert_eq!(got[0].body, b"sent");
        assert_eq!(got[0].headers["x-q"], "v");
    }

    #[test]
    fn http_outside_allow_list_is_refused() {
        let rt = Runtime::new().expect("runtime");
        let server = mock(&rt, "/x", ResponseTemplate::new(200));
        let env = http_env("api.github.com", &rt);
        let reply = call(
            &env,
            "cox_http",
            "cox_command",
            get(format!("{}/x", server.uri())),
        );
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "not_granted", "capability": "net:127.0.0.1" } })
        );
        let none = Arc::new(HostEnv::new("t"));
        let reply = call(
            &none,
            "cox_http",
            "cox_command",
            get(format!("{}/x", server.uri())),
        );
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "not_granted", "capability": "net" } })
        );
        assert_eq!(hits(&rt, &server), 0, "a refused request was sent");
    }

    #[test]
    fn http_in_render_is_not_in_this_context() {
        let rt = Runtime::new().expect("runtime");
        let server = mock(&rt, "/x", ResponseTemplate::new(200));
        let env = http_env("127.0.0.1", &rt);
        let reply = call(
            &env,
            "cox_http",
            "cox_render",
            get(format!("{}/x", server.uri())),
        );
        assert_eq!(reply, json!({ "Err": { "kind": "not_in_this_context" } }));
        assert_eq!(hits(&rt, &server), 0);
    }

    #[test]
    fn http_body_over_cap_is_too_large() {
        let rt = Runtime::new().expect("runtime");
        let big = vec![b'a'; crate::net::MAX_HTTP_RESPONSE_BYTES + 1];
        let server = mock(&rt, "/big", ResponseTemplate::new(200).set_body_bytes(big));
        let env = http_env("127.0.0.1", &rt);
        let reply = call(
            &env,
            "cox_http",
            "cox_command",
            get(format!("{}/big", server.uri())),
        );
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "too_large", "limit": crate::net::MAX_HTTP_RESPONSE_BYTES } })
        );
    }

    #[test]
    fn http_redirect_is_not_followed() {
        let rt = Runtime::new().expect("runtime");
        let server = rt.block_on(MockServer::start());
        let to = format!("{}/elsewhere", server.uri());
        rt.block_on(
            Mock::given(path("/r"))
                .respond_with(ResponseTemplate::new(302).insert_header("location", to.as_str()))
                .mount(&server),
        );
        let env = http_env("127.0.0.1", &rt);
        let reply = call(
            &env,
            "cox_http",
            "cox_command",
            get(format!("{}/r", server.uri())),
        );
        assert_eq!(reply["Ok"]["status"], 302, "{reply}");
        assert_eq!(reply["Ok"]["headers"]["location"], to);
        assert_eq!(hits(&rt, &server), 1, "the redirect was followed");
    }

    #[test]
    fn log_is_rate_limited_per_second() {
        let mut w = LogWindow::default();
        let t0 = Instant::now();
        let admitted = (0..LOG_LINES_PER_SEC + 5).filter(|_| w.admit(t0).0).count();
        assert_eq!(admitted, LOG_LINES_PER_SEC as usize);
        // The next second admits again and reports what the last one dropped.
        assert_eq!(w.admit(t0 + Duration::from_secs(1)), (true, 5));
    }

    #[test]
    fn init_config_is_the_plugins_own_table() {
        let manifest = PluginManifest {
            api: 1,
            id: "jev".into(),
            version: "0.1.0".into(),
            name: "Jev".into(),
            description: String::new(),
            wasm: Some("plugin.wasm".into()),
            wasi: false,
            limits: Limits::default(),
            capabilities: Capabilities {
                kv: true,
                ..Capabilities::default()
            },
            provider: Vec::new(),
            models: Vec::new(),
            mcp: Vec::new(),
            external_agents: Vec::new(),
            cloud_agents: Vec::new(),
            agents: Vec::new(),
        };
        let mut plugins = PluginsConfig::default();
        plugins
            .entries
            .insert("jev".into(), json!({ "route": "cheap" }));
        plugins
            .entries
            .insert("other".into(), json!({ "secret": 1 }));
        let session = SessionInfo {
            id: "s1".into(),
            cwd: "/w".into(),
        };
        let init = init_input(&manifest, &plugins, session.clone());
        assert_eq!(init.config, json!({ "route": "cheap" }));
        assert_eq!(init.granted["kv"], true);
        let bare = init_input(&manifest, &PluginsConfig::default(), session);
        assert_eq!(bare.config, json!({}));
    }

    /// A `ModelCaller` that records the tier it was called at and answers
    /// with one `TextDelta`, standing in for `cox-core`'s `Session` (T33.15
    /// wires the real one; T33.44 puts it in a live `HostEnv`).
    #[derive(Default)]
    struct FakeCaller {
        seen_tier: Mutex<Option<Tier>>,
    }

    #[async_trait::async_trait]
    impl ModelCaller for FakeCaller {
        async fn call(
            &self,
            _id: &str,
            tier: Tier,
            _request: Request,
        ) -> Result<Vec<cox_protocol::types::ProviderEvent>, CoreError> {
            *lock(&self.seen_tier) = Some(tier);
            Ok(vec![cox_protocol::types::ProviderEvent::TextDelta {
                text: "ok".into(),
            }])
        }
    }

    fn model_call_request(tier: ModelTier) -> Value {
        let request = Request {
            tier: Tier::Cheap,
            job: cox_protocol::types::Job::Main,
            model: cox_protocol::types::ModelId(String::new()),
            system: vec![cox_protocol::types::SystemBlock {
                text: "you are a plugin's own prompt".into(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![],
            effort: cox_protocol::types::Effort::Low,
            max_tokens: 64,
            thinking: cox_protocol::types::Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        };
        json!({
            "tier": tier,
            "request": serde_json::to_value(request).expect("request serializes"),
        })
    }

    fn env_with_caller(granted: Vec<String>) -> (Arc<HostEnv>, Arc<FakeCaller>) {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let caller = Arc::new(FakeCaller::default());
        let env = Arc::new(
            HostEnv::new("t")
                .with_grant(granted, Arc::new(MemKv::default()))
                .with_model_caller(caller.clone(), rt.handle().clone()),
        );
        // Leaking the runtime keeps its handle alive for the env's lifetime;
        // the test process exits right after, so nothing outlives it.
        std::mem::forget(rt);
        (env, caller)
    }

    #[test]
    fn model_call_denied_without_capability() {
        let env = Arc::new(HostEnv::new("t"));
        let reply = call(
            &env,
            "cox_model_call",
            "cox_command",
            model_call_request(ModelTier::Cheap),
        );
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "not_granted", "capability": "model:cheap" } })
        );
    }

    #[test]
    fn model_call_denied_in_render() {
        let (env, _caller) = env_with_caller(vec!["model:cheap".into()]);
        let reply = call(
            &env,
            "cox_model_call",
            "cox_render",
            model_call_request(ModelTier::Cheap),
        );
        assert_eq!(reply, json!({ "Err": { "kind": "not_in_this_context" } }));
    }

    #[test]
    fn model_call_clamps_tier_to_the_grant() {
        let (env, caller) = env_with_caller(vec!["model:cheap".into()]);
        let reply = call(
            &env,
            "cox_model_call",
            "cox_command",
            model_call_request(ModelTier::Code),
        );
        assert!(reply["Ok"].is_array(), "{reply}");
        assert_eq!(*lock(&caller.seen_tier), Some(Tier::Cheap));
    }

    #[test]
    fn model_call_round_trips_at_the_granted_tier() {
        let (env, caller) = env_with_caller(vec!["model:code".into()]);
        let reply = call(
            &env,
            "cox_model_call",
            "cox_command",
            model_call_request(ModelTier::Code),
        );
        assert_eq!(
            reply,
            json!({ "Ok": [{ "type": "text_delta", "text": "ok" }] })
        );
        assert_eq!(*lock(&caller.seen_tier), Some(Tier::Code));
    }

    /// Records every `invoke` so a refusal can prove nothing reached it.
    #[derive(Default)]
    struct FakeInvoker(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl ToolInvoker for FakeInvoker {
        async fn invoke(
            &self,
            _id: &str,
            name: &str,
            _input: Value,
        ) -> Result<cox_protocol::types::ToolResult, CoreError> {
            lock(&self.0).push(name.into());
            Err(CoreError::Interrupted)
        }
    }

    fn env_with_invoker(
        granted: &[&str],
    ) -> (Arc<HostEnv>, Arc<FakeInvoker>, tokio::runtime::Runtime) {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let invoker = Arc::new(FakeInvoker::default());
        let granted = granted.iter().map(|g| g.to_string()).collect();
        let env = HostEnv::new("t")
            .with_grant(granted, Arc::new(MemKv::default()))
            .with_tool_invoker(invoker.clone(), rt.handle().clone());
        (Arc::new(env), invoker, rt)
    }

    fn probe_call() -> Value {
        json!({ "name": "probe", "input": {} })
    }

    #[test]
    fn invoke_from_hook_context_is_refused() {
        let (env, invoker, _rt) = env_with_invoker(&["invoke:probe"]);
        let refused = json!({ "Err": { "kind": "not_in_this_context" } });
        // The loop waits on `cox_hook`; `cox_render` may only read.
        for export in ["cox_hook", "cox_render"] {
            let reply = call(&env, "cox_invoke_tool", export, probe_call());
            assert_eq!(reply, refused, "{export}");
        }
        assert!(lock(&invoker.0).is_empty());
    }

    #[test]
    fn plugin_invoke_outside_grant_is_refused() {
        let (env, invoker, _rt) = env_with_invoker(&["invoke:probe"]);
        let reply = call(
            &env,
            "cox_invoke_tool",
            "cox_command",
            json!({ "name": "bash", "input": { "command": "true" } }),
        );
        assert_eq!(
            reply,
            json!({ "Err": { "kind": "not_granted", "capability": "invoke:bash" } })
        );
        assert!(lock(&invoker.0).is_empty());
    }

    #[test]
    fn invoke_that_would_wait_on_its_own_worker_is_refused() {
        // Its own tool runs on the worker this call is blocking.
        let (env, invoker, _rt) = env_with_invoker(&["invoke:wasm__t__echo"]);
        let own = json!({ "name": "wasm__t__echo", "input": {} });
        let reply = call(&env, "cox_invoke_tool", "cox_tool_call", own);
        assert_eq!(reply["Err"]["kind"], "failed", "{reply}");
        // So does its own hook on the invoked call's path.
        let (hooked, hooked_invoker, _rt2) =
            env_with_invoker(&["hooks:PreToolUse", "invoke:probe"]);
        let reply = call(&hooked, "cox_invoke_tool", "cox_command", probe_call());
        assert_eq!(reply["Err"]["kind"], "failed", "{reply}");
        assert!(lock(&invoker.0).is_empty() && lock(&hooked_invoker.0).is_empty());
    }

    /// A read-only tool that counts its runs, named so a rule can match it.
    struct Probe(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl cox_protocol::traits::Tool for Probe {
        fn spec(&self) -> cox_protocol::types::ToolSpec {
            cox_protocol::types::ToolSpec {
                name: "probe".into(),
                description: String::new(),
                input_schema: json!({ "type": "object" }),
                deferred: false,
                risk: cox_protocol::types::Risk::ReadOnly,
                concurrency: cox_protocol::types::Concurrency::Parallel,
            }
        }
        fn subject(&self, _input: &Value) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: Value,
            _cx: &cox_protocol::traits::ToolCx,
        ) -> Result<ToolOutput, cox_protocol::errors::ToolError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput {
                text: "probed".into(),
                is_error: false,
                diff: None,
                structured: None,
            })
        }
    }

    /// A real session whose permissions `rules` shapes, a plugin `t`
    /// granted `invoke:probe` bound to it, and a surface that answers every
    /// approval the way headless mode does (`run.rs`: no approver, deny).
    struct Wired {
        env: Arc<HostEnv>,
        events: Arc<Mutex<Vec<cox_protocol::types::Event>>>,
        runs: Arc<std::sync::atomic::AtomicUsize>,
        _rt: tokio::runtime::Runtime,
    }

    fn wired(rules: impl FnOnce(&mut cox_protocol::config::PermissionsConfig)) -> Wired {
        use cox_protocol::types::{Decision, Event, Submission};
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let _in_rt = rt.enter();
        let mut config = cox_protocol::Config::default();
        rules(&mut config.permissions);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let store = Arc::new(cox_core::MemoryStore::new());
        let session = cox_core::Session::new(
            config,
            Arc::new(cox_provider::scripted::Scripted::from_toml("", "").expect("scenario")),
            vec![Arc::new(Probe(runs.clone()))],
            store.clone(),
            store,
            std::path::PathBuf::from("/tmp"),
        )
        .expect("session");
        let mut rx = session.events().expect("events once");
        let events = Arc::new(Mutex::new(Vec::new()));
        let (seen, surface) = (events.clone(), session.clone());
        rt.spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let Event::ApprovalRequired { call, .. } = &ev {
                    let reason = "no approver in headless mode".into();
                    let deny = Submission::Approve {
                        call_id: call.id,
                        decision: Decision::Deny { reason },
                    };
                    let _ = surface.submit(deny).await;
                }
                lock(&seen).push(ev);
            }
        });
        let env = HostEnv::new("t")
            .with_grant(vec!["invoke:probe".into()], Arc::new(MemKv::default()))
            .with_tool_invoker(Arc::new(session), rt.handle().clone());
        drop(_in_rt);
        Wired {
            env: Arc::new(env),
            events,
            runs,
            _rt: rt,
        }
    }

    impl Wired {
        /// The events up to the invoked call's `ToolCallDone`, once the
        /// surface has read that far.
        fn events(&self) -> Vec<cox_protocol::types::Event> {
            for _ in 0..500 {
                let events = lock(&self.events).clone();
                if events
                    .iter()
                    .any(|e| matches!(e, cox_protocol::types::Event::ToolCallDone { .. }))
                {
                    return events;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no ToolCallDone: {:?}", lock(&self.events));
        }

        fn runs(&self) -> usize {
            self.runs.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[test]
    fn plugin_invoke_denied_by_rule() {
        use cox_protocol::types::{DecidedBy, Decision, Event};
        let w = wired(|p| p.deny.push("probe".into()));
        let reply = call(&w.env, "cox_invoke_tool", "cox_command", probe_call());
        assert_eq!(reply["Ok"]["is_error"], true, "{reply}");
        let text = reply["Ok"]["text"].as_str().unwrap_or_default();
        assert!(text.contains("denied by rule probe"), "{reply}");
        assert_eq!(w.runs(), 0);
        let events = w.events();
        // Visible like a model call, and attributed to the plugin.
        assert!(events.iter().any(|e| matches!(e,
            Event::Notice { text, .. } if text == "plugin t runs probe")));
        assert!(events.iter().any(|e| matches!(e,
            Event::ToolCallRequested { call } if call.name == "probe")));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ApprovalDecided {
                decision: Decision::Deny { .. },
                by: DecidedBy::Rule,
                ..
            }
        )));
    }

    #[test]
    fn plugin_invoke_ask_is_denied_headless() {
        use cox_protocol::types::Event;
        let w = wired(|p| p.ask.push("probe".into()));
        let reply = call(&w.env, "cox_invoke_tool", "cox_on_event", probe_call());
        assert_eq!(
            reply["Ok"]["text"], "permission denied: no approver in headless mode",
            "{reply}"
        );
        assert_eq!(w.runs(), 0);
        let asked = w.events().into_iter().find_map(|e| match e {
            Event::ApprovalRequired { source, .. } => source.and_then(|s| s.agent),
            _ => None,
        });
        assert_eq!(asked.as_deref(), Some("plugin t"));
    }

    #[test]
    fn plugin_invoke_allowed_runs_and_is_archived() {
        use cox_protocol::types::Event;
        let w = wired(|p| p.allow.push("probe".into()));
        let reply = call(&w.env, "cox_invoke_tool", "cox_key", probe_call());
        assert_eq!(reply["Ok"]["text"], "probed", "{reply}");
        assert_eq!(w.runs(), 1);
        let archived = w
            .events()
            .into_iter()
            .any(|e| matches!(e, Event::ToolCallDone { result, .. } if result.archive.is_some()));
        assert!(archived);
    }
}
