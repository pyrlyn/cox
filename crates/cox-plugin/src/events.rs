// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The session's event tap (PL§5, T33.10): `Session::emit` offers every
//! scrubbed rollout event here; each plugin gets the kinds it subscribed to
//! and was granted, through a ring of 256 that drops the oldest when full,
//! and a pump thread hands the ring to `cox_on_event` one batch at a time.
//! Its own module because `host` owns the worker and its queues and knows
//! nothing about `Event`; this one owns what a plugin sees and when.
//!
//! Emit never waits on a plugin: `offer` folds the shared context, then per
//! plugin takes the ring lock for one push. The pump holds that lock only to
//! swap the ring out, never while the plugin runs, so a slow plugin loses
//! its oldest events and the turn goes on.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cox_plugin_api::{Effects, EventBatch};
use cox_protocol::traits::EventTap;
use cox_protocol::types::Event;
use serde_json::Value;

use crate::PluginError;
use crate::context::Context;
use crate::host::{EVENT_DEPTH, Lane, PluginHost};
use crate::hostfn::HostEnv;

/// Events one plugin can have waiting (PL§5).
pub const RING: usize = EVENT_DEPTH;
/// A `cox_on_event` batch's budget (PL§12 "event batch 50 ms").
pub const BATCH_DEADLINE: Duration = Duration::from_millis(50);
const ON_EVENT: &str = "cox_on_event";

/// Called with a plugin's id when its `Effects.redraw` asks for its slots to
/// be rendered again; the TUI turns it into a render request (T33.23).
pub type Redraw = Arc<dyn Fn(&str) + Send + Sync>;

/// Where a batch goes: the plugin's `cox_on_event`. A trait so the rings
/// can be tested against a consumer that is slow on purpose.
pub(crate) trait Consumer: Send + Sync {
    /// Delivers one batch; `Ok(None)` when the plugin has no `cox_on_event`.
    fn on_event(&self, batch: &EventBatch) -> Result<Option<Effects>, PluginError>;
}

impl Consumer for PluginHost {
    fn on_event(&self, batch: &EventBatch) -> Result<Option<Effects>, PluginError> {
        self.call(Lane::Event, ON_EVENT, batch, BATCH_DEADLINE)
    }
}

/// The kinds a plugin receives: what `InitOut.subscribe` asked for and
/// the grant allows (`events:<tag>` lines of `grant::capability_list`).
pub fn subscriptions(subscribe: &[String], granted: &[String]) -> BTreeSet<String> {
    let allowed: BTreeSet<&str> = granted
        .iter()
        .filter_map(|line| line.strip_prefix("events:"))
        .collect();
    subscribe
        .iter()
        .filter(|kind| allowed.contains(kind.as_str()))
        .cloned()
        .collect()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waiting events with their rollout sequence numbers.
type Waiting = VecDeque<(u64, Arc<Value>)>;

/// One plugin's waiting events. Values are shared between plugins, so an
/// event is serialized once however many plugins want it.
#[derive(Default)]
struct Ring {
    events: Waiting,
    dropped: u64,
    closed: bool,
}

impl Ring {
    fn push(&mut self, seq: u64, ev: Arc<Value>) {
        if self.events.len() >= RING {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back((seq, ev));
    }

    /// Everything waiting and the drop count since the last take; O(1), so
    /// the lock is held no longer than a push.
    fn take(&mut self) -> Option<(Waiting, u64)> {
        if self.events.is_empty() {
            return None;
        }
        Some((
            std::mem::take(&mut self.events),
            std::mem::take(&mut self.dropped),
        ))
    }
}

fn batch(events: Waiting, dropped: u64) -> EventBatch {
    EventBatch {
        first_seq: events.front().map_or(0, |(seq, _)| *seq),
        dropped,
        events: events.into_iter().map(|(_, v)| Value::clone(&v)).collect(),
    }
}

/// One plugin's side of the tap.
struct Feed {
    id: String,
    kinds: BTreeSet<String>,
    ring: Mutex<Ring>,
    wake: Condvar,
}

impl Feed {
    fn push(&self, seq: u64, ev: &Arc<Value>) {
        {
            let mut ring = lock(&self.ring);
            if ring.closed {
                return;
            }
            ring.push(seq, ev.clone());
        }
        self.wake.notify_one();
    }

    fn close(&self) {
        lock(&self.ring).closed = true;
        self.wake.notify_all();
    }
}

/// The `EventTap` a session gets (`Session::set_event_tap`): folds the
/// shared context and fans each event out to its plugins' rings.
pub struct PluginTap {
    context: Arc<Context>,
    redraw: Redraw,
    feeds: Vec<(Arc<Feed>, Option<JoinHandle<()>>)>,
}

impl PluginTap {
    /// A tap with no plugins yet; `context` is the one every plugin's
    /// `HostEnv` reads (`HostEnv::with_context`).
    pub fn new(context: Arc<Context>, redraw: Redraw) -> Self {
        Self {
            context,
            redraw,
            feeds: Vec::new(),
        }
    }

    /// Feeds `host` the kinds in `kinds` (see [`subscriptions`]); `env` is
    /// the environment it was loaded with, where `Effects.notices` queue
    /// like `cox_notify`'s. A plugin with no kinds gets no pump thread.
    pub fn attach(
        &mut self,
        host: Arc<PluginHost>,
        env: Arc<HostEnv>,
        kinds: BTreeSet<String>,
    ) -> Result<(), PluginError> {
        self.attach_consumer(host, env, kinds)
    }

    pub(crate) fn attach_consumer(
        &mut self,
        consumer: Arc<dyn Consumer>,
        env: Arc<HostEnv>,
        kinds: BTreeSet<String>,
    ) -> Result<(), PluginError> {
        if kinds.is_empty() {
            return Ok(());
        }
        let feed = Arc::new(Feed {
            id: env.id().to_string(),
            kinds,
            ring: Mutex::new(Ring::default()),
            wake: Condvar::new(),
        });
        let (pumped, redraw) = (feed.clone(), self.redraw.clone());
        let handle = thread::Builder::new()
            .name(format!("cox-plugin-{}-events", feed.id))
            .spawn(move || pump(&pumped, consumer.as_ref(), &env, &redraw))
            .map_err(|e| PluginError::Load(e.to_string()))?;
        self.feeds.push((feed, Some(handle)));
        Ok(())
    }
}

impl EventTap for PluginTap {
    fn offer(&self, seq: u64, ev: &Event) {
        self.context.fold(ev);
        if self.feeds.is_empty() {
            return;
        }
        // Serialization cannot fail for `Event`; if it ever did, plugins
        // miss one event rather than the session failing.
        let Ok(json) = serde_json::to_value(ev) else {
            return;
        };
        let Some(kind) = json.get("type").and_then(Value::as_str).map(str::to_owned) else {
            return;
        };
        let json = Arc::new(json);
        for (feed, _) in &self.feeds {
            if feed.kinds.contains(&kind) {
                feed.push(seq, &json);
            }
        }
    }
}

impl Drop for PluginTap {
    fn drop(&mut self) {
        for (feed, _) in &self.feeds {
            feed.close();
        }
        // A pump inside a call returns within `BATCH_DEADLINE`.
        for (_, handle) in &mut self.feeds {
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// One plugin's delivery loop: waits for its ring, swaps it out, and calls
/// the plugin with the batch outside the lock.
fn pump(feed: &Feed, consumer: &dyn Consumer, env: &HostEnv, redraw: &Redraw) {
    loop {
        let (events, dropped) = {
            let mut ring = lock(&feed.ring);
            loop {
                if ring.closed {
                    return;
                }
                if let Some(taken) = ring.take() {
                    break taken;
                }
                ring = feed.wake.wait(ring).unwrap_or_else(PoisonError::into_inner);
            }
        };
        match consumer.on_event(&batch(events, dropped)) {
            Ok(Some(effects)) => {
                for notice in effects.notices {
                    // A full queue refuses the rest, as it does `cox_notify`.
                    if env.notify(notice.level, &notice.text).is_err() {
                        break;
                    }
                }
                // After the notices, so a redraw shows them.
                if effects.redraw {
                    redraw(&feed.id);
                }
            }
            // Subscribed without the export: stop queueing for it.
            Ok(None) => {
                feed.close();
                return;
            }
            // Fail open (AGENTS.md): the batch is lost, the plugin keeps
            // its subscription and sees the next one.
            Err(e) => tracing::warn!(plugin = %feed.id, error = %e, "cox_on_event failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Instant;

    use cox_core::{MemoryStore, Session};
    use cox_plugin_api::Limits;
    use cox_protocol::Store;
    use cox_protocol::types::{Level, Submission};
    use cox_provider::scripted::Scripted;

    /// Records every batch, sleeping first when told to: a plugin slow on
    /// purpose.
    struct Recorder {
        sleep: Duration,
        seen: Mutex<Sender<EventBatch>>,
    }

    impl Consumer for Recorder {
        fn on_event(&self, batch: &EventBatch) -> Result<Option<Effects>, PluginError> {
            thread::sleep(self.sleep);
            let _ = lock(&self.seen).send(batch.clone());
            Ok(Some(Effects::default()))
        }
    }

    fn kinds(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|k| (*k).to_string()).collect()
    }

    fn tap_with(
        sleep: Duration,
        subscribed: BTreeSet<String>,
    ) -> (PluginTap, Receiver<EventBatch>) {
        let (tx, rx) = channel();
        let mut tap = PluginTap::new(Arc::new(Context::new()), Arc::new(|_: &str| {}));
        let recorder = Arc::new(Recorder {
            sleep,
            seen: Mutex::new(tx),
        });
        tap.attach_consumer(recorder, Arc::new(HostEnv::new("rec")), subscribed)
            .expect("attach");
        (tap, rx)
    }

    fn session(
        turns: usize,
    ) -> (
        Session,
        Arc<MemoryStore>,
        tokio::sync::mpsc::Receiver<Event>,
    ) {
        let toml: String = (0..turns)
            .map(|i| format!("[[turn]]\ntext = \"reply {i}\"\n"))
            .collect();
        let store = Arc::new(MemoryStore::new());
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-events")];
        let session = Session::new(
            config,
            Arc::new(Scripted::from_toml(&toml, "").expect("scenario")),
            Vec::new(),
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-events"),
        )
        .expect("session");
        let rx = session.events().expect("events once");
        (session, store, rx)
    }

    /// Runs `turns` user turns, draining the surface's stream as it goes,
    /// and checks nothing follows a `TurnDone` (§1.15 rule 7).
    async fn run_turns(
        session: &Session,
        rx: &mut tokio::sync::mpsc::Receiver<Event>,
        turns: usize,
    ) -> Duration {
        let started = Instant::now();
        for i in 0..turns {
            let s = session.clone();
            let turn = tokio::spawn(async move {
                s.submit(Submission::UserTurn {
                    text: format!("turn {i}"),
                    attachments: vec![],
                    confirm_think: false,
                })
                .await
            });
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .expect("event in time")
                    .expect("stream open");
                if matches!(ev, Event::TurnDone { .. }) {
                    break;
                }
            }
            turn.await.expect("join").expect("turn");
            assert!(rx.try_recv().is_err(), "an event followed TurnDone");
        }
        started.elapsed()
    }

    #[test]
    fn ring_drops_oldest_and_counts() {
        let mut ring = Ring::default();
        for seq in 0..(RING as u64 + 44) {
            ring.push(seq, Arc::new(Value::from(seq)));
        }
        let (events, dropped) = ring.take().expect("full ring");
        let b = batch(events, dropped);
        assert_eq!((b.first_seq, b.dropped, b.events.len()), (44, 44, RING));
        assert_eq!(b.events.last(), Some(&Value::from(RING as u64 + 43)));
        // The count resets once delivered; an empty ring has no batch.
        assert!(ring.take().is_none());
        ring.push(500, Arc::new(Value::Null));
        let (events, dropped) = ring.take().expect("one event");
        assert_eq!((batch(events, dropped).first_seq, dropped), (500, 0));
    }

    #[test]
    fn subscriptions_are_what_was_asked_and_granted() {
        let asked = ["turn_done".to_string(), "text_delta".to_string()];
        let granted = [
            "events:turn_done".to_string(),
            "events:usage".to_string(),
            "kv".into(),
        ];
        assert_eq!(subscriptions(&asked, &granted), kinds(&["turn_done"]));
    }

    #[tokio::test]
    async fn plugin_sees_only_subscribed_kinds() {
        let (tap, seen) = tap_with(Duration::ZERO, kinds(&["turn_done"]));
        let (session, store, mut rx) = session(1);
        session.set_event_tap(Arc::new(tap));
        run_turns(&session, &mut rx, 1).await;
        let b = seen.recv_timeout(Duration::from_secs(5)).expect("a batch");
        let types: Vec<&str> = b.events.iter().filter_map(|e| e["type"].as_str()).collect();
        assert_eq!(types, ["turn_done"], "{b:?}");
        // `first_seq` is what `rollout_append` returned for that event
        // (the memory store numbers from 1).
        let rollout = store.rollout_read(&session.id()).expect("rollout");
        let at = rollout
            .iter()
            .position(|e| matches!(e, Event::TurnDone { .. }))
            .expect("TurnDone recorded");
        assert_eq!((b.first_seq, b.dropped), (at as u64 + 1, 0));
    }

    #[tokio::test]
    async fn tap_never_blocks_emit() {
        const TURNS: usize = 50;
        let (session_a, _, mut rx_a) = session(TURNS);
        let base = run_turns(&session_a, &mut rx_a, TURNS).await;
        // Every event kind the session emits, to a plugin that takes 100 ms
        // a batch: were emit to wait on it, 50 turns would take seconds.
        let all = kinds(&[
            "session_started",
            "turn_started",
            "item_started",
            "text_delta",
            "item_done",
            "usage",
            "notice",
            "turn_done",
        ]);
        let (tap, seen) = tap_with(Duration::from_millis(100), all);
        let (session_b, _, mut rx_b) = session(TURNS);
        session_b.set_event_tap(Arc::new(tap));
        let tapped = run_turns(&session_b, &mut rx_b, TURNS).await;
        assert!(
            tapped < base * 3 + Duration::from_millis(500),
            "base {base:?}, tapped {tapped:?}"
        );
        assert!(
            seen.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the plugin got events"
        );
    }

    // The extism kernel imports a module needs to set its output.
    const KERNEL: &str = r#"
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))"#;

    #[test]
    fn effects_redraw_and_notices_are_forwarded() {
        // `cox_on_event` answers a fixed 56-byte `Effects` for any batch.
        let wat = format!(
            r#"(module {KERNEL}
              (memory 1)
              (data (i32.const 0) "{{\"redraw\":true,\"notices\":[{{\"level\":\"info\",\"text\":\"hi\"}}]}}")
              (func (export "cox_init") (result i32) (i32.const 0))
              (func (export "cox_on_event") (result i32) (local $off i64) (local $i i64)
                (local.set $off (call $alloc (i64.const 56)))
                (block $done (loop $copy
                  (br_if $done (i64.ge_u (local.get $i) (i64.const 56)))
                  (call $store (i64.add (local.get $off) (local.get $i))
                    (i32.load8_u (i32.wrap_i64 (local.get $i))))
                  (local.set $i (i64.add (local.get $i) (i64.const 1)))
                  (br $copy)))
                (call $output_set (local.get $off) (i64.const 56))
                (i32.const 0)))"#
        );
        let env = Arc::new(HostEnv::new("fx"));
        let host = PluginHost::load_with(wat.as_bytes(), &Limits::default(), env.clone())
            .expect("module loads");
        let (tx, redrawn) = channel::<String>();
        let tx = Mutex::new(tx);
        let redraw: Redraw = Arc::new(move |id: &str| {
            let _ = lock(&tx).send(id.to_string());
        });
        let mut tap = PluginTap::new(Arc::new(Context::new()), redraw);
        tap.attach(Arc::new(host), env.clone(), kinds(&["notice"]))
            .expect("attach");
        tap.offer(
            7,
            &Event::Notice {
                level: Level::Info,
                text: "x".into(),
            },
        );
        assert_eq!(
            redrawn.recv_timeout(Duration::from_secs(5)).as_deref(),
            Ok("fx")
        );
        assert_eq!(env.take_notices(), [(Level::Info, "hi".to_string())]);
    }
}
