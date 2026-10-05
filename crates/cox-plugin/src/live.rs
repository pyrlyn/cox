// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A session's live plugins (PL§3–§6, T33.44): each granted plugin compiled
//! once under its real grant, initialised once, and shared as one
//! `Arc<PluginHost>` by its hooks, the event tap and (T33.12) its tools.
//! Its own module because `host`, `hooks` and `events` each take a
//! caller-supplied host; this is the one place that builds the per-plugin
//! bundle, so every surface wires the same thing and `crates/cox` stays thin.

use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use cox_plugin_api::{CommandDecl, InitOut, KeyDecl, PluginManifest, SessionInfo, Slot};
use cox_protocol::PluginStore;
use cox_protocol::config::PluginsConfig;
use cox_protocol::errors::CoreError;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{Advisor, EventTap, Hook, ModelCaller, Tool, ToolInvoker};
use cox_protocol::types::{Event, Level, ProviderEvent, Request, Tier, ToolResult};
use serde_json::Value;

use crate::PluginError;
use crate::advisor::PluginAdvisor;
use crate::context::Context;
use crate::events::{PluginTap, Redraw, subscriptions};
use crate::grant;
use crate::hooks::PluginHooks;
use crate::host::PluginHost;
use crate::hostfn::{HostEnv, init_input};

/// Where drained plugin notices go; the session emits each as
/// `Event::Notice`. Called from inside `EventTap::offer`, so it must not
/// block or emit synchronously (the caller forwards to a task).
pub type Notices = Arc<dyn Fn(Vec<(Level, String)>) + Send + Sync>;

/// One granted plugin: its single instance, the environment its host
/// functions act on, its granted-capability lines and, once `cox_init`
/// has run, what it declared.
pub struct Live {
    manifest: PluginManifest,
    host: Arc<PluginHost>,
    env: Arc<HostEnv>,
    granted: Vec<String>,
    init: Option<InitOut>,
}

impl Live {
    /// The plugin id.
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// The one instance; hooks, the tap and tools each hold a clone.
    pub fn host(&self) -> &Arc<PluginHost> {
        &self.host
    }

    /// `grant::capability_list` of the granted manifest.
    pub fn granted(&self) -> &[String] {
        &self.granted
    }

    /// What `cox_init` returned; `None` before [`LivePlugins::start`].
    pub fn init_out(&self) -> Option<&InitOut> {
        self.init.as_ref()
    }

    /// `InitOut.status` without the slots whose `ui.*` capability was not
    /// granted (PL§4: anything not granted is dropped).
    pub fn granted_status(&self) -> Vec<Slot> {
        let status = self.init.as_ref().map_or(&[][..], |i| &i.status[..]);
        status
            .iter()
            .copied()
            .filter(|slot| self.granted.iter().any(|g| g == slot_capability(*slot)))
            .collect()
    }

    /// `InitOut.commands`, or empty when `ui.commands` was not granted
    /// (PL§2, PL§4: anything not granted is dropped).
    pub fn granted_commands(&self) -> Vec<CommandDecl> {
        if !self.granted.iter().any(|g| g == "ui.commands") {
            return Vec::new();
        }
        self.init
            .as_ref()
            .map_or(Vec::new(), |i| i.commands.clone())
    }

    /// `InitOut.keys`, or empty when `ui.keys` was not granted (PL§2, PL§4:
    /// anything not granted is dropped).
    pub fn granted_keys(&self) -> Vec<KeyDecl> {
        if !self.granted.iter().any(|g| g == "ui.keys") {
            return Vec::new();
        }
        self.init.as_ref().map_or(Vec::new(), |i| i.keys.clone())
    }

    /// `InitOut.renderers` it may serve (T33.26, PL§2, PL§8): a target
    /// needs its own `ui.render:<target>` line, except `tool:` one of the
    /// plugin's own granted tools, which only it can ever produce.
    pub fn granted_renderers(&self) -> Vec<String> {
        let Some(init) = &self.init else {
            return Vec::new();
        };
        let granted = |kind: &str, item: &str| {
            self.granted
                .iter()
                .any(|g| g.strip_prefix(kind) == Some(item))
        };
        let own = |target: &str| {
            let Some(name) = target.strip_prefix("tool:") else {
                return false;
            };
            init.tools
                .iter()
                .filter_map(|t| t.get("name")?.as_str())
                .any(|t| {
                    granted(crate::tool::GRANT_PREFIX, t)
                        && name == crate::tool::qualified(self.id(), t)
                })
        };
        init.renderers
            .iter()
            .filter(|t| granted("ui.render:", t) || own(t))
            .cloned()
            .collect()
    }

    /// Queued `cox_notify`/`Effects` notices, attributed to the plugin.
    fn take_notices(&self) -> impl Iterator<Item = (Level, String)> + '_ {
        let id = self.id();
        self.env
            .take_notices()
            .into_iter()
            .map(move |(level, text)| (level, format!("plugin {id}: {text}")))
    }
}

/// The grant line a slot needs (`grant::capability_list`'s `ui.*` flags).
fn slot_capability(slot: Slot) -> &'static str {
    match slot {
        Slot::StatusLeft | Slot::StatusRight => "ui.status",
        Slot::Panel => "ui.panel",
        Slot::Overlay => "ui.overlay",
    }
}

/// The session as `cox_model_call`'s `ModelCaller` (T33.15) or
/// `cox_invoke_tool`'s `ToolInvoker` (T33.13), bound after load: a
/// plugin's `HostEnv` is built when it is compiled, before the session
/// exists. Only a `Weak` is kept, so no strong edge runs from a plugin back
/// to the session that owns it (session → tap → host → env).
struct Late<T: ?Sized>(OnceLock<Weak<T>>);

type LateCaller = Late<dyn ModelCaller>;

impl<T: ?Sized> Default for Late<T> {
    fn default() -> Self {
        Self(OnceLock::new())
    }
}

impl<T: ?Sized> Late<T> {
    fn session(&self, what: &str) -> Result<Arc<T>, CoreError> {
        self.0
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| CoreError::Denied {
                why: format!("no session is serving {what}"),
            })
    }
}

#[async_trait]
impl ModelCaller for LateCaller {
    async fn call(
        &self,
        id: &str,
        tier: Tier,
        request: Request,
    ) -> Result<Vec<ProviderEvent>, CoreError> {
        self.session("model calls")?.call(id, tier, request).await
    }
}

#[async_trait]
impl ToolInvoker for Late<dyn ToolInvoker> {
    async fn invoke(&self, id: &str, name: &str, input: Value) -> Result<ToolResult, CoreError> {
        self.session("tool calls")?.invoke(id, name, input).await
    }
}

/// Every live plugin of one session and the context they share.
#[derive(Default)]
pub struct LivePlugins {
    context: Arc<Context>,
    caller: Arc<LateCaller>,
    invoker: Arc<Late<dyn ToolInvoker>>,
    plugins: Vec<Live>,
}

impl LivePlugins {
    /// The session's folded context, read by every plugin's `cox_context`.
    pub fn context(&self) -> &Arc<Context> {
        &self.context
    }

    /// The plugins loaded so far (after `start`, only the started ones).
    pub fn plugins(&self) -> &[Live] {
        &self.plugins
    }

    /// Makes `caller` (the session) the one `cox_model_call` reaches, for
    /// every plugin loaded inside a tokio runtime. Weakly held: whoever
    /// binds it keeps it alive. A second bind is ignored.
    pub fn bind_model_caller(&self, caller: &Arc<dyn ModelCaller>) {
        let _ = self.caller.0.set(Arc::downgrade(caller));
    }

    /// Makes `invoker` (the session) the one `cox_invoke_tool` reaches,
    /// held weakly like the model caller. A second bind is ignored.
    pub fn bind_tool_invoker(&self, invoker: &Arc<dyn ToolInvoker>) {
        let _ = self.invoker.0.set(Arc::downgrade(invoker));
    }

    /// Compiles a plugin `grant::check` found `Granted`, with its host
    /// functions bound to that grant, the kv store, the shared context and,
    /// inside a tokio runtime, the late-bound model caller and tool invoker.
    pub fn load(
        &mut self,
        manifest: &PluginManifest,
        wasm: &[u8],
        store: Arc<dyn PluginStore>,
    ) -> Result<(), PluginError> {
        let granted = grant::capability_list(manifest);
        let mut env = HostEnv::new(manifest.id.as_str())
            .with_grant(granted.clone(), store)
            .with_context(self.context.clone());
        // The plugin's worker blocks on this handle to run the call, since
        // it is not a runtime thread itself (`hostfn`).
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            env = env
                .with_model_caller(self.caller.clone(), runtime.clone())
                .with_tool_invoker(self.invoker.clone(), runtime);
        }
        let env = Arc::new(env);
        let host = PluginHost::load_with(wasm, &manifest.limits, env.clone())?;
        self.plugins.push(Live {
            manifest: manifest.clone(),
            host: Arc::new(host),
            env,
            granted,
            init: None,
        });
        Ok(())
    }

    /// Folds the session's `SessionStarted` (the tap never sees it: the
    /// session appends it while it is built, before a tap can be set) and
    /// runs each plugin's `cox_init` once. A plugin whose init fails is
    /// dropped with a warning, never fatal (AGENTS.md "fail open").
    pub fn start(&mut self, config: &PluginsConfig, session: SessionId, cwd: &Path) -> Vec<String> {
        self.context.fold(&Event::SessionStarted {
            session,
            config_digest: String::new(),
            cwd: cwd.to_path_buf(),
        });
        let info = SessionInfo {
            id: session.to_string(),
            cwd: cwd.display().to_string(),
        };
        let mut warnings = Vec::new();
        self.plugins.retain_mut(|p| {
            if p.init.is_some() {
                return true;
            }
            match p.host.init(&init_input(&p.manifest, config, info.clone())) {
                Ok(out) => {
                    p.init = Some(out);
                    true
                }
                Err(e) => {
                    warnings.push(format!("plugin {} failed to start: {e}", p.id()));
                    false
                }
            }
        });
        warnings
    }

    /// Each started plugin as a hook source for `HookChain::new`.
    pub fn hooks(&self) -> Vec<(String, Arc<dyn Hook>)> {
        self.plugins
            .iter()
            .filter(|p| p.init.is_some())
            .map(|p| {
                let hooks = PluginHooks::new(p.id(), p.host.clone(), &p.granted);
                (p.id().to_string(), Arc::new(hooks) as Arc<dyn Hook>)
            })
            .collect()
    }

    /// Each started plugin as a decision-point source (T33.20); the session
    /// asks only the one `[plugins.decide]` names for a point, and a plugin
    /// without that point's grant answers nothing.
    pub fn advisors(&self) -> Vec<Arc<dyn Advisor>> {
        self.plugins
            .iter()
            .filter(|p| p.init.is_some())
            .map(|p| {
                Arc::new(PluginAdvisor::new(p.id(), p.host.clone(), &p.granted)) as Arc<dyn Advisor>
            })
            .collect()
    }

    /// Each started plugin's granted tools (PL§7, T33.12), built from the
    /// `cox_init` that already ran and sorted by (plugin id, tool), plus a
    /// warning per declared tool that was dropped.
    pub fn tools(&self) -> (Vec<Arc<dyn Tool>>, Vec<String>) {
        let mut started: Vec<&Live> = self.plugins.iter().filter(|p| p.init.is_some()).collect();
        started.sort_by(|a, b| a.id().cmp(b.id()));
        let mut tools = Vec::new();
        let mut warnings = Vec::new();
        for p in started {
            let specs = p.init.as_ref().map_or(&[][..], |i| &i.tools[..]);
            let (mine, dropped) = crate::tool::declared(p.id(), &p.host, &p.env, &p.granted, specs);
            tools.extend(mine.into_iter().map(|t| Arc::new(t) as Arc<dyn Tool>));
            warnings.extend(dropped);
        }
        (tools, warnings)
    }

    /// Each started plugin's instance by id: what the TUI's render server
    /// (T33.23) and plugin tools (T33.12) call.
    pub fn hosts(&self) -> Vec<(String, Arc<PluginHost>)> {
        self.plugins
            .iter()
            .filter(|p| p.init.is_some())
            .map(|p| (p.id().to_string(), p.host.clone()))
            .collect()
    }

    /// Notices queued since the last take, in plugin order.
    pub fn take_notices(&self) -> Vec<(Level, String)> {
        self.plugins.iter().flat_map(Live::take_notices).collect()
    }

    /// The session's event tap, owning the plugins for the rest of the
    /// session: attaches each started plugin to the kinds it subscribed to
    /// and was granted, and after each event hands the queued notices to
    /// `notices`. A plugin whose pump cannot start is warned about.
    pub fn into_tap(self, redraw: Redraw, notices: Notices) -> (SessionTap, Vec<String>) {
        let mut tap = PluginTap::new(self.context.clone(), redraw);
        let mut warnings = Vec::new();
        for p in self.plugins.iter().filter(|p| p.init.is_some()) {
            let subscribe = p.init.as_ref().map_or(&[][..], |i| &i.subscribe[..]);
            let kinds = subscriptions(subscribe, &p.granted);
            if let Err(e) = tap.attach(p.host.clone(), p.env.clone(), kinds) {
                warnings.push(format!("plugin {} gets no events: {e}", p.id()));
            }
        }
        let tap = SessionTap {
            tap,
            live: self,
            notices,
        };
        (tap, warnings)
    }
}

/// `PluginTap` plus notice draining. Fields drop in order: the pumps stop
/// before the instances they call.
pub struct SessionTap {
    tap: PluginTap,
    live: LivePlugins,
    notices: Notices,
}

impl EventTap for SessionTap {
    fn offer(&self, seq: u64, ev: &Event) {
        self.tap.offer(seq, ev);
        // Not after a `Notice`: a plugin answering every notice with one
        // would otherwise feed itself without end.
        if matches!(ev, Event::Notice { .. }) {
            return;
        }
        let drained = self.live.take_notices();
        if !drained.is_empty() {
            (self.notices)(drained);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cox_protocol::types::{HookEvent, HookOutcome};
    use serde_json::json;

    use super::*;
    use crate::host::tests::module;

    fn manifest(id: &str, caps: serde_json::Value) -> PluginManifest {
        serde_json::from_value(json!({
            "api": 1, "id": id, "version": "0.1.0", "name": id, "wasm": "plugin.wasm",
            "capabilities": caps,
        }))
        .expect("manifest")
    }

    /// A store nothing reads: these plugins ask for no kv.
    fn store() -> Arc<dyn PluginStore> {
        Arc::new(crate::hostfn::tests::MemKv::default())
    }

    // `cox_hook` counts its calls in a global and answers `continue`;
    // `cox_on_event` answers one notice whose last digit is that count.
    const COUNTER: &str = r#"(memory 1)
      (global $n (mut i32) (i32.const 0))
      (data (i32.const 0) "{\"type\":\"continue\"}")
      (data (i32.const 64) "{\"redraw\":false,\"notices\":[{\"level\":\"info\",\"text\":\"hooks 0\"}]}")
      (func $out (param $p i32) (param $len i32) (local $off i64) (local $i i32)
        (local.set $off (call $alloc (i64.extend_i32_u (local.get $len))))
        (block $done (loop $copy
          (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
          (call $store (i64.add (local.get $off) (i64.extend_i32_u (local.get $i)))
            (i32.load8_u (i32.add (local.get $p) (local.get $i))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $copy)))
        (call $output_set (local.get $off) (i64.extend_i32_u (local.get $len))))
      (func (export "cox_hook") (result i32)
        (global.set $n (i32.add (global.get $n) (i32.const 1)))
        (call $out (i32.const 0) (i32.const 19))
        (i32.const 0))
      (func (export "cox_on_event") (result i32)
        (i32.store8 (i32.const 121) (i32.add (i32.const 48) (global.get $n)))
        (call $out (i32.const 64) (i32.const 62))
        (i32.const 0))"#;

    #[tokio::test]
    async fn hooks_and_event_tap_share_one_plugin_instance() {
        let caps = json!({ "hooks": ["PreToolUse"], "events": ["notice"] });
        let mut live = LivePlugins::default();
        live.load(&manifest("ctr", caps), &module(COUNTER), store())
            .expect("loads");
        // The echoing `cox_init` hands back `InitIn`, so subscribe is empty;
        // attach by hand what an `InitOut.subscribe` of `notice` would.
        assert!(
            live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"))
                .is_empty()
        );
        live.plugins[0].init = Some(InitOut {
            subscribe: vec!["notice".into()],
            ..InitOut::default()
        });
        let hooks = live.hooks();
        let out = hooks[0]
            .1
            .run(HookEvent::PreToolUse, json!({}), Duration::from_secs(5))
            .await;
        assert_eq!(out, HookOutcome::Continue);
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let notices: Notices = Arc::new(move |n| {
            let _ = tx.lock().map(|tx| tx.send(n));
        });
        let (tap, warnings) = live.into_tap(Arc::new(|_: &str| {}), notices);
        assert!(warnings.is_empty(), "{warnings:?}");
        let notice = Event::Notice {
            level: Level::Info,
            text: "x".into(),
        };
        tap.offer(1, &notice);
        // The pump answers asynchronously; the next non-notice event drains.
        let got = (0..100)
            .find_map(|seq| {
                std::thread::sleep(Duration::from_millis(20));
                let delta = Event::TextDelta {
                    item: cox_protocol::ids::ItemId::new(),
                    text: String::new(),
                };
                tap.offer(seq + 2, &delta);
                rx.try_recv().ok()
            })
            .expect("a notice drained");
        // One instance: the event side saw the count the hook side left.
        assert_eq!(got, [(Level::Info, "plugin ctr: hooks 1".to_string())]);
    }

    #[test]
    fn only_granted_status_slots_are_declared() {
        let caps = json!({ "ui": { "status": true } });
        let mut live = LivePlugins::default();
        live.load(&manifest("ui", caps), &module(""), store())
            .expect("loads");
        live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        live.plugins[0].init = Some(InitOut {
            status: vec![Slot::StatusRight, Slot::Panel],
            ..InitOut::default()
        });
        assert_eq!(live.plugins()[0].granted_status(), [Slot::StatusRight]);
        assert_eq!(live.hosts().len(), 1);
    }

    #[test]
    fn commands_and_keys_are_dropped_without_their_capability() {
        let caps = json!({ "ui": { "commands": true } });
        let mut live = LivePlugins::default();
        live.load(&manifest("cmd", caps), &module(""), store())
            .expect("loads");
        live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        live.plugins[0].init = Some(InitOut {
            commands: vec![CommandDecl {
                name: "go".into(),
                description: String::new(),
            }],
            keys: vec![KeyDecl {
                key: "g".into(),
                name: "go".into(),
                description: String::new(),
            }],
            ..InitOut::default()
        });
        // `ui.commands` was granted, `ui.keys` was not: keys drop, commands
        // don't (PL§2, PL§4).
        assert_eq!(live.plugins()[0].granted_commands().len(), 1);
        assert!(live.plugins()[0].granted_keys().is_empty());
    }

    #[test]
    fn renderers_outside_own_tools_need_their_grant() {
        let caps = json!({ "tools": ["sum"], "ui": { "render": ["item:assistant_message"] } });
        let mut live = LivePlugins::default();
        live.load(&manifest("look", caps), &module(""), store())
            .expect("loads");
        live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        let targets = [
            "tool:wasm__look__sum",
            "tool:wasm__look__other",
            "tool:read",
            "item:assistant_message",
        ];
        live.plugins[0].init = Some(InitOut {
            tools: vec![json!({ "name": "sum" }), json!({ "name": "other" })],
            renderers: targets.iter().map(|t| t.to_string()).collect(),
            ..InitOut::default()
        });
        // Its own granted tool needs no line; `other` was never granted,
        // `read` is not its own, and the assistant item has its grant.
        assert_eq!(
            live.plugins()[0].granted_renderers(),
            ["tool:wasm__look__sum", "item:assistant_message"]
        );
    }

    #[tokio::test]
    async fn model_caller_is_held_weakly() {
        let live = LivePlugins::default();
        use cox_protocol::types::{Effort, Job, ModelId, Thinking};
        let request = Request {
            tier: Tier::Cheap,
            job: Job::Main,
            model: ModelId("m".into()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: Effort::Low,
            max_tokens: 1,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        };
        let unbound = live.caller.call("p", Tier::Cheap, request.clone()).await;
        assert!(
            matches!(unbound, Err(CoreError::Denied { .. })),
            "{unbound:?}"
        );
        let session: Arc<dyn ModelCaller> = Arc::new(LateCaller::default());
        live.bind_model_caller(&session);
        drop(session);
        // Nothing but the binder kept it alive, so the plugin side cannot.
        let gone = live.caller.call("p", Tier::Cheap, request).await;
        assert!(matches!(gone, Err(CoreError::Denied { .. })), "{gone:?}");
        assert!(live.caller.0.get().is_some_and(|w| w.upgrade().is_none()));
    }

    #[test]
    fn failed_init_drops_the_plugin_with_a_warning() {
        let mut live = LivePlugins::default();
        let trap = br#"(module (func (export "cox_init") (result i32) unreachable))"#;
        live.load(&manifest("boom", json!({})), trap, store())
            .expect("compiles");
        let warnings = live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("plugin boom failed to start"),
            "{warnings:?}"
        );
        assert!(live.plugins().is_empty() && live.hooks().is_empty());
    }
}
