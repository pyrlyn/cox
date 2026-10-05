// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One loaded plugin: a worker thread that owns the extism `Plugin` and serves
//! two queues, control before events (PL§4 "Threading"), plus a watchdog
//! thread that cancels a call at its deadline through a `CancelHandle`
//! (R§4.3.5 P9). A thread per plugin because `Plugin::call` takes `&mut self`
//! (P8): calls into one plugin run one at a time, different plugins run in
//! parallel, and a slow plugin never holds a core thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cox_plugin_api::{InitIn, InitOut, Limits};
use extism::{CancelHandle, Manifest, Plugin, PluginBuilder, Wasm};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::PluginError;
use crate::hostfn::HostEnv;

/// Control lane depth: hooks, decide, tools, provider, render, commands.
pub const CONTROL_DEPTH: usize = 16;
/// Event lane depth (the drop-oldest ring on top of it is T33.10's).
pub const EVENT_DEPTH: usize = 256;
const INIT: &str = "cox_init";
// PL§2 `[limits]`: the manifest's values are clamped to the host maxima.
const DEFAULT_MEMORY_MIB: u32 = 16;
const MAX_MEMORY_MIB: u32 = 64;
const DEFAULT_CALL_MS: u32 = 200;
const PAGES_PER_MIB: u32 = 16;

/// The queue a call waits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Served first: something in the core is waiting on the answer.
    Control,
    /// Served only when the control queue is empty.
    Event,
}

type Reply = Result<Option<Vec<u8>>, PluginError>;

struct Job {
    export: String,
    input: Vec<u8>,
    deadline: Duration,
    reply: SyncSender<Reply>,
}

#[derive(Default)]
struct Queues {
    control: VecDeque<Job>,
    events: VecDeque<Job>,
    closed: bool,
}

impl Queues {
    fn pop(&mut self) -> Option<Job> {
        self.control.pop_front().or_else(|| self.events.pop_front())
    }
}

#[derive(Default)]
struct Watch {
    deadline: Option<Instant>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queues: Mutex<Queues>,
    work: Condvar,
    watch: Mutex<Watch>,
    tick: Condvar,
}

// A poisoned lock only means another thread panicked mid-update; both
// structures stay consistent after every statement, so keep serving.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A loaded plugin. Dropping it cancels the running call and stops both threads.
pub struct PluginHost {
    shared: Arc<Shared>,
    cancel: CancelHandle,
    call_cap: Duration,
    threads: Vec<JoinHandle<()>>,
    /// `/plugin remove`'s liveness flag (T33.33, PL§1c): set by [`Self::stop`]
    /// once its manifest is gone from disk. The worker threads and queues
    /// keep running underneath — dropping the shared `Arc<PluginHost>` still
    /// does the real teardown at session end — this only tells a frozen
    /// `WasmTool` (which holds its own clone) to stop calling in, so it can
    /// answer `Denied` instead of a call that would otherwise still work
    /// against deleted files.
    stopped: Arc<AtomicBool>,
}

impl PluginHost {
    /// Compiles `wasm` under an environment that grants nothing: every
    /// `cox:host/v1` import links, and only `cox_log`/`cox_notify` do
    /// anything. Enough to prove a package compiles (T33.6's session check).
    pub fn load(id: &str, wasm: &[u8], limits: &Limits) -> Result<Self, PluginError> {
        Self::load_with(wasm, limits, Arc::new(HostEnv::new(id)))
    }

    /// Compiles `wasm` (binary or WAT text, P15) with no WASI and no
    /// compilation cache yet, under the clamped `limits`, with the
    /// `cox:host/v1` functions bound to `env` (T33.9).
    ///
    /// WASI stays off, and filesystem preopens (T33.14) stay blocked, until
    /// extism ships wasmtime >= 48: wasmtime 43's WASI filesystem has a
    /// sandbox escape (RUSTSEC-2026-0269, R§4.3.5 P39). Each plugin gets its
    /// own wasmtime `Engine` because extism 1.30 builds one inside
    /// `CompiledPlugin::new` and offers no way to pass a shared one; that is
    /// safe from RUSTSEC-2026-0222 only while nothing here moves a wasmtime
    /// object from one plugin to another.
    pub fn load_with(wasm: &[u8], limits: &Limits, env: Arc<HostEnv>) -> Result<Self, PluginError> {
        let mib = limits
            .memory_mib
            .unwrap_or(DEFAULT_MEMORY_MIB)
            .min(MAX_MEMORY_MIB);
        let call_cap = Duration::from_millis(limits.call_ms.unwrap_or(DEFAULT_CALL_MS).into());
        let manifest = Manifest::new([Wasm::data(wasm.to_vec())])
            .with_memory_max(mib * PAGES_PER_MIB)
            .with_timeout(call_cap);
        // Cache off: the default writes wasmtime's cache under the user's
        // system cache dir; `~/.cox/cache/wasmtime` is wired by a later card.
        let plugin = PluginBuilder::new(manifest)
            .with_wasi(false)
            .with_cache_disabled()
            .with_functions(env.functions())
            .build()
            .map_err(|e| PluginError::Load(format!("{e:#}")))?;
        if !plugin.function_exists(INIT) {
            return Err(PluginError::MissingExport(INIT));
        }
        let cancel = plugin.cancel_handle();
        let shared = Arc::new(Shared::default());
        let mut host = Self {
            shared: shared.clone(),
            cancel: cancel.clone(),
            call_cap,
            threads: Vec::new(),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        let dog = shared.clone();
        let id = env.id().to_string();
        host.spawn(format!("cox-plugin-{id}-deadline"), move || {
            watch(&dog, &cancel)
        })?;
        host.spawn(format!("cox-plugin-{id}"), move || {
            serve(plugin, &shared, &env)
        })?;
        Ok(host)
    }

    fn spawn(
        &mut self,
        name: String,
        f: impl FnOnce() + Send + 'static,
    ) -> Result<(), PluginError> {
        let handle = thread::Builder::new()
            .name(name)
            .spawn(f)
            .map_err(|e| PluginError::Load(e.to_string()))?;
        self.threads.push(handle);
        Ok(())
    }

    /// Calls the required `cox_init` under the plugin's own call budget.
    pub fn init(&self, input: &InitIn) -> Result<InitOut, PluginError> {
        self.call(Lane::Control, INIT, input, self.call_cap)?
            .ok_or(PluginError::MissingExport(INIT))
    }

    /// Calls `export` with JSON in and out, waiting at most `deadline`
    /// (never more than the plugin's `call_ms`) once the worker starts it.
    /// An optional export the guest does not have answers `Ok(None)`.
    pub fn call<I: Serialize, O: DeserializeOwned>(
        &self,
        lane: Lane,
        export: &str,
        input: &I,
        deadline: Duration,
    ) -> Result<Option<O>, PluginError> {
        let (reply, answer) = sync_channel(1);
        let job = Job {
            export: export.to_string(),
            input: serde_json::to_vec(input)?,
            deadline: deadline.min(self.call_cap),
            reply,
        };
        {
            let mut q = lock(&self.shared.queues);
            let (queue, depth) = match lane {
                Lane::Control => (&mut q.control, CONTROL_DEPTH),
                Lane::Event => (&mut q.events, EVENT_DEPTH),
            };
            if queue.len() >= depth {
                return Err(PluginError::Busy);
            }
            queue.push_back(job);
        }
        self.shared.work.notify_one();
        match answer.recv().map_err(|_| PluginError::Stopped)?? {
            Some(out) => Ok(Some(serde_json::from_slice(&out)?)),
            None => Ok(None),
        }
    }

    /// `/plugin remove <id>` (T33.33, PL§1c): flags this instance stopped so
    /// [`Self::is_stopped`] — the check a frozen `WasmTool` makes before
    /// every call — answers `true` from here on. Idempotent; does not touch
    /// the queues or threads, since the `Arc<PluginHost>` this session's
    /// tools/hooks/tap still hold keeps them alive until the session ends.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    /// Whether [`Self::stop`] has been called.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        lock(&self.shared.queues).closed = true;
        lock(&self.shared.watch).closed = true;
        self.shared.work.notify_all();
        self.shared.tick.notify_all();
        // A call still running would otherwise hold the join for up to
        // `call_ms`; the timer ignores a cancel when nothing runs.
        let _ = self.cancel.cancel();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

fn serve(mut plugin: Plugin, shared: &Shared, env: &HostEnv) {
    loop {
        let job = {
            let mut q = lock(&shared.queues);
            loop {
                if q.closed {
                    return;
                }
                if let Some(job) = q.pop() {
                    break job;
                }
                q = shared.work.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let reply = if plugin.function_exists(&job.export) {
            // Host functions read it to refuse what this export may not do.
            env.enter(&job.export);
            set_deadline(shared, Some(Instant::now() + job.deadline));
            let out = plugin.call::<&[u8], Vec<u8>>(&job.export, &job.input);
            set_deadline(shared, None);
            env.enter("");
            out.map(Some)
                .map_err(|e| PluginError::from_call(&job.export, &e))
        } else {
            Ok(None)
        };
        // The caller may have gone; its answer is simply dropped.
        let _ = job.reply.send(reply);
    }
}

fn set_deadline(shared: &Shared, deadline: Option<Instant>) {
    lock(&shared.watch).deadline = deadline;
    shared.tick.notify_one();
}

fn watch(shared: &Shared, cancel: &CancelHandle) {
    let mut w = lock(&shared.watch);
    while !w.closed {
        let Some(at) = w.deadline else {
            w = shared.tick.wait(w).unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let now = Instant::now();
        if now >= at {
            // Sent under the lock: the worker cannot clear the deadline and
            // start the next call before this cancel is queued, so it can
            // only reach the call it was armed for, or none.
            let _ = cancel.cancel();
            w.deadline = None;
        } else {
            w = shared
                .tick
                .wait_timeout(w, at - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cox_plugin_api::SessionInfo;
    use serde_json::{Value, json};

    // The extism kernel imports every test module uses (P15: the loader takes
    // WAT text as well as binaries). WAT wants imports before functions.
    const IMPORTS: &str = r#"
      (import "extism:host/env" "http_request" (func $http (param i64 i64) (result i64)))
      (import "extism:host/env" "input_length" (func $input_length (result i64)))
      (import "extism:host/env" "input_load_u8" (func $load (param i64) (result i32)))
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))"#;

    // `cox_init` copies its input to its output.
    const ECHO: &str = r#"
      (func $echo (result i32) (local $n i64) (local $off i64) (local $i i64)
        (local.set $n (call $input_length))
        (local.set $off (call $alloc (local.get $n)))
        (block $done (loop $copy
          (br_if $done (i64.ge_u (local.get $i) (local.get $n)))
          (call $store (i64.add (local.get $off) (local.get $i)) (call $load (local.get $i)))
          (local.set $i (i64.add (local.get $i) (i64.const 1)))
          (br $copy)))
        (call $output_set (local.get $off) (local.get $n))
        (i32.const 0))
      (export "cox_init" (func $echo))"#;

    /// A module with the kernel imports, `body`, and an echoing `cox_init`.
    pub(crate) fn module(body: &str) -> Vec<u8> {
        format!("(module {IMPORTS} {body} {ECHO})").into_bytes()
    }

    /// An `export` that answers the `len` bytes of JSON at offset 0; shared
    /// by the hook (`cox_hook`) and advisor (`cox_decide`) tests.
    pub(crate) fn answering(export: &str, json: &str) -> String {
        let escaped = json.replace('"', "\\\"");
        format!(
            r#"(memory 1)
               (data (i32.const 0) "{escaped}")
               (func (export "{export}") (result i32) (local $off i64) (local $i i64)
                 (local.set $off (call $alloc (i64.const {len})))
                 (block $done (loop $copy
                   (br_if $done (i64.ge_u (local.get $i) (i64.const {len})))
                   (call $store (i64.add (local.get $off) (local.get $i))
                     (i32.load8_u (i32.wrap_i64 (local.get $i))))
                   (local.set $i (i64.add (local.get $i) (i64.const 1)))
                   (br $copy)))
                 (call $output_set (local.get $off) (i64.const {len}))
                 (i32.const 0))"#,
            len = json.len()
        )
    }

    /// An `export` that never returns, to exercise a deadline.
    pub(crate) fn spinning(export: &str) -> String {
        format!(r#"(func (export "{export}") (result i32) (loop $l (br $l)) (i32.const 0))"#)
    }

    fn load(body: &str, limits: &Limits) -> PluginHost {
        PluginHost::load("t", &module(body), limits).expect("module loads")
    }

    fn init_in() -> InitIn {
        InitIn {
            api: 1,
            plugin_id: "t".into(),
            config: json!({ "greeting": "hi" }),
            session: SessionInfo {
                id: "s1".into(),
                cwd: "/w".into(),
            },
            granted: json!({ "events": ["turn_done"] }),
        }
    }

    #[test]
    fn wat_plugin_init_round_trips_json() {
        let host = load("", &Limits::default());
        let echoed: Option<Value> = host
            .call(Lane::Control, INIT, &init_in(), Duration::from_secs(5))
            .expect("echo call");
        assert_eq!(echoed, Some(serde_json::to_value(init_in()).expect("json")));
        // The echoed `InitIn` has no `InitOut` field, so it reads as the default.
        assert_eq!(host.init(&init_in()).expect("init"), InitOut::default());
    }

    #[test]
    fn runaway_call_is_cancelled_at_deadline() {
        let limits = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        let host = load(
            r#"(func (export "cox_on_event") (result i32) (loop $l (br $l)) (i32.const 0))"#,
            &limits,
        );
        let started = Instant::now();
        let err = host
            .call::<_, Value>(
                Lane::Event,
                "cox_on_event",
                &json!({}),
                Duration::from_millis(100),
            )
            .expect_err("a spin never returns");
        assert!(matches!(err, PluginError::Timeout { .. }), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        // The worker survives the cancel and serves the next call.
        assert_eq!(
            host.init(&init_in()).expect("init after cancel"),
            InitOut::default()
        );
    }

    #[test]
    fn memory_cap_traps_not_panics() {
        let limits = Limits {
            memory_mib: Some(2),
            ..Limits::default()
        };
        let host = load(
            r#"(memory 1)
               (func (export "cox_hook") (result i32)
                 (drop (memory.grow (i32.const 1000))) (i32.const 0))"#,
            &limits,
        );
        let err = host
            .call::<_, Value>(
                Lane::Control,
                "cox_hook",
                &json!({}),
                Duration::from_secs(5),
            )
            .expect_err("growing past the cap traps");
        assert!(matches!(err, PluginError::OutOfMemory { .. }), "{err:?}");
        assert_eq!(
            host.init(&init_in()).expect("init after trap"),
            InitOut::default()
        );
    }

    #[test]
    fn missing_optional_export_is_absent_not_error() {
        let host = load("", &Limits::default());
        let out: Option<Value> = host
            .call(
                Lane::Control,
                "cox_render",
                &json!({}),
                Duration::from_secs(1),
            )
            .expect("absent export is not an error");
        assert_eq!(out, None);
        // `cox_init` is the one required export.
        let no_init = PluginHost::load("t", b"(module)", &Limits::default());
        assert!(matches!(no_init, Err(PluginError::MissingExport(INIT))));
    }

    /// T33.33, PL§1c: fresh, `is_stopped` is `false`; `stop` flips it, and
    /// the worker keeps serving underneath — `stop` only marks liveness, it
    /// does not close the queues (that stays `Drop`'s job).
    #[test]
    fn stop_flags_is_stopped_without_closing_the_worker() {
        let host = load("", &Limits::default());
        assert!(!host.is_stopped());
        host.stop();
        assert!(host.is_stopped());
        assert_eq!(
            host.init(&init_in())
                .expect("worker still serves after stop"),
            InitOut::default()
        );
    }

    #[test]
    fn http_request_is_compiled_out() {
        // Builds `{"url":"http://example.com"}` in kernel memory and calls
        // extism's built-in `http_request` with it.
        let host = load(
            r#"(memory 1)
               (data (i32.const 0) "{\"url\":\"http://example.com\"}")
               (func (export "cox_tool_call") (result i32) (local $off i64) (local $i i64)
                 (local.set $off (call $alloc (i64.const 28)))
                 (block $done (loop $copy
                   (br_if $done (i64.ge_u (local.get $i) (i64.const 28)))
                   (call $store (i64.add (local.get $off) (local.get $i))
                     (i32.load8_u (i32.wrap_i64 (local.get $i))))
                   (local.set $i (i64.add (local.get $i) (i64.const 1)))
                   (br $copy)))
                 (drop (call $http (local.get $off) (i64.const 0)))
                 (i32.const 0))"#,
            &Limits::default(),
        );
        let err = host
            .call::<_, Value>(
                Lane::Control,
                "cox_tool_call",
                &json!({}),
                Duration::from_secs(5),
            )
            .expect_err("no http without the `http` feature");
        match err {
            PluginError::Trap { message, .. } => {
                assert!(message.contains("not enabled"), "{message}")
            }
            other => panic!("expected a trap, got {other:?}"),
        }
    }

    #[test]
    fn exception_handling_module_loads() {
        // A61: Kotlin/Wasm emits `try_table`/`throw`; without extism's
        // `wasmtime-exceptions` the module fails to parse (R§4.3.5 P42).
        let body = r#"(tag $e)
            (func (export "throws") (result i32)
              (block $caught (try_table (catch_all $caught) (throw $e)))
              (i32.const 0))"#;
        let host = PluginHost::load("t", &module(body), &Limits::default());
        assert!(host.is_ok(), "exceptions proposal is on: {:?}", host.err());
    }

    #[test]
    fn control_queue_is_served_before_events() {
        let job = |export: &str| Job {
            export: export.into(),
            input: Vec::new(),
            deadline: Duration::ZERO,
            reply: sync_channel(1).0,
        };
        let mut q = Queues::default();
        q.events.push_back(job("event"));
        q.control.push_back(job("control"));
        let order: Vec<String> = std::iter::from_fn(|| q.pop()).map(|j| j.export).collect();
        assert_eq!(order, ["control", "event"]);
    }
}
