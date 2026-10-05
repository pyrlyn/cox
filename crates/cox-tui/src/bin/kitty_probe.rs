// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T23.1: a tiny binary that exists only for `tests/shell.rs`'s
//! `pty_pops_keyboard_flags_on_exit`. `cox-tui` is a library with no binary
//! of its own (`crates/cox` owns the real one) and has no `[[bin]]`-worthy
//! reason to grow one — but a PTY e2e needs a real process whose controlling
//! terminal is the PTY slave, the way `crates/cox/tests/tui_e2e.rs` spawns
//! the actual `cox` binary; this is that process for `cox_tui::app::run`
//! alone. `NullProvider` stands in for `cox-provider`'s real backends: a
//! `[[bin]]` target only ever gets `[dependencies]` (never
//! `[dev-dependencies]`), and a regular `cox-provider` dependency would
//! break `crates/cox/tests/deps.rs`'s crate-direction rule (`cox-tui` may
//! only depend on `cox-core`/`cox-protocol` among workspace crates); this
//! probe never submits a turn, so nothing here is ever actually called. It
//! quits itself shortly after start by feeding two synthetic `Ctrl+C` on
//! the same channel the real binary uses for off-screen events — the same
//! path an idle-then-repeated `Ctrl+C` from a real keyboard takes.
//!
//! `COX_PROBE_SCENARIO=cells` (T23.2) first feeds 40 finished `Notice`
//! cells on that channel, so `shell.rs` can count what each `insert_before`
//! costs on screen; core events are the same `Msg::Event`s a session would
//! deliver, so no provider has to run. `COX_PROBE_SCENARIO=resize`
//! (T23.7) feeds 12 cells, starts a reply, waits for the terminal to be
//! resized, then finishes the reply and feeds 4 more cells.
//! `COX_PROBE_SCENARIO=progress` (T23.6) starts and ends one turn;
//! `COX_PROBE_OSC9_4=1` says the terminal draws OSC 9;4 progress.
//! `COX_PROBE_MOUSE=1` (T22.4) sets `tui.mouse`, so `app::run` asks for
//! mouse capture; needs no scenario of its own since capture is a startup
//! decision, not something a fed `Msg` drives.

use std::sync::Arc;
use std::time::Duration;

use cox_core::{MemoryStore, Session};
use cox_protocol::Config;
use cox_protocol::errors::ProviderError;
use cox_protocol::ids::{ItemId, TurnId};
use cox_protocol::traits::Provider;
use cox_protocol::types::{
    Caps, Event, ItemKind, Job, Level, ModelId, ProviderEvent, ProviderId, Request, StopReason,
    Tier, Usage,
};
use cox_protocol::types::{PermissionMode, SandboxMode};
use cox_tui::state::{Msg, State};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Never actually called: this probe never submits a turn.
struct NullProvider;

#[async_trait::async_trait]
impl Provider for NullProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: false,
            max_context: 0,
        }
    }

    async fn stream(
        &self,
        _req: Request,
        _sink: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        Err(ProviderError::Overloaded)
    }

    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Err(ProviderError::Overloaded)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let kitty = std::env::var("COX_KITTY_PROBE_KITTY").as_deref() == Ok("1");
    let cwd = std::env::temp_dir();

    let mut config = Config::default();
    config.core.workspace_roots = vec![cwd.clone()];
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        Arc::new(NullProvider),
        Vec::new(),
        store.clone(),
        store,
        cwd,
    )
    .expect("session opens");

    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    state.caps.kitty_keyboard = kitty;
    state.caps.osc9_4 = std::env::var("COX_PROBE_OSC9_4").as_deref() == Ok("1");
    // T22.4: `tui.mouse`, off by default here like every other probe switch.
    state.mouse = std::env::var("COX_PROBE_MOUSE").as_deref() == Ok("1");
    state.caps.osc52 = std::env::var("COX_PROBE_OSC52").as_deref() == Ok("1");
    // The notify scenario needs a terminal that shows OSC 9 and reports focus.
    if std::env::var("COX_PROBE_SCENARIO").as_deref() == Ok("notify") {
        state.caps.osc9 = true;
        state.caps.focus = true;
    }

    let (feed_tx, feed_rx) = mpsc::channel::<Msg>(4);
    let (ask_tx, _ask_rx) = mpsc::channel::<cox_tui::state::Ask>(4);
    let (persist_tx, _persist_rx) = mpsc::channel::<(String, String)>(4);
    let (grant_tx, _grant_rx) = mpsc::channel::<cox_tui::state::GrantDecision>(4);
    let (plugin_tx, _plugin_rx) = mpsc::channel::<cox_tui::state::PluginRequest>(4);
    let (plugin_mgmt_tx, _plugin_mgmt_rx) = mpsc::channel::<cox_tui::state::PluginMgmtRequest>(4);

    let scenario = std::env::var("COX_PROBE_SCENARIO").unwrap_or_default();
    tokio::spawn(async move {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        match scenario.as_str() {
            "cells" => {
                for n in 1..=40 {
                    let _ = feed_tx.send(notice(n)).await;
                }
            }
            "resize" => {
                for n in 1..=12 {
                    let _ = feed_tx.send(notice(n)).await;
                }
                let item = ItemId::new();
                let text = String::new();
                let kind = ItemKind::AssistantMessage { text };
                let _ = feed_tx
                    .send(Msg::Event(Event::ItemStarted { item, kind }))
                    .await;
                let _ = feed_tx.send(delta(item, "reply-start ")).await;
                // The test resizes once it sees the reply; the rest of it
                // streams into whatever state the resize left behind.
                let before = crossterm::terminal::size().ok();
                let start = std::time::Instant::now();
                while crossterm::terminal::size().ok() == before
                    && start.elapsed() < Duration::from_secs(30)
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let _ = feed_tx
                    .send(delta(item, "streamed after the resize "))
                    .await;
                let _ = feed_tx.send(delta(item, "reply-end")).await;
                let _ = feed_tx.send(Msg::Event(Event::ItemDone { item })).await;
                for n in 13..=16 {
                    let _ = feed_tx.send(notice(n)).await;
                }
            }
            // T23.5: one turn ends with focus lost, one with focus held;
            // only the first may ring.
            "notify" => {
                for focused in [false, true] {
                    let _ = feed_tx.send(Msg::Focus(focused)).await;
                    let turn = TurnId::new();
                    let stop = StopReason::EndTurn;
                    let _ = feed_tx
                        .send(Msg::Event(Event::TurnDone { turn, stop }))
                        .await;
                }
            }
            "progress" => {
                let turn = TurnId::new();
                let started = Event::TurnStarted {
                    seq: 1,
                    turn,
                    job: Job::Main,
                    tier: Tier::Code,
                    model: ModelId("m".into()),
                };
                let _ = feed_tx.send(Msg::Event(started)).await;
                let stop = StopReason::EndTurn;
                let _ = feed_tx
                    .send(Msg::Event(Event::TurnDone { turn, stop }))
                    .await;
            }
            // T23.4: a reply still streaming (no `ItemDone`) stays in
            // `state.transcript` rather than draining to scrollback, so `y`
            // has a cell to find deterministically — no PTY-timing wait.
            "copy" => {
                let item = ItemId::new();
                let kind = ItemKind::AssistantMessage {
                    text: String::new(),
                };
                let _ = feed_tx
                    .send(Msg::Event(Event::ItemStarted { item, kind }))
                    .await;
                let _ = feed_tx.send(delta(item, "cell text")).await;
                let y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
                let _ = feed_tx.send(Msg::Key(y)).await;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = feed_tx.send(Msg::Key(ctrl_c)).await;
        // Idle, so the first arms quit rather than interrupting a turn; the
        // second (still idle) turns into `Cmd::Quit`.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = feed_tx.send(Msg::Key(ctrl_c)).await;
    });

    let _ = cox_tui::app::run(
        session,
        state,
        feed_rx,
        ask_tx,
        persist_tx,
        grant_tx,
        plugin_tx,
        plugin_mgmt_tx,
        None,
    )
    .await;
}

/// One streamed chunk of the reply `item`.
fn delta(item: ItemId, text: &str) -> Msg {
    Msg::Event(Event::TextDelta {
        item,
        text: text.to_string(),
    })
}

/// A one-line finished cell whose text the PTY test can find again.
fn notice(n: u32) -> Msg {
    Msg::Event(Event::Notice {
        level: Level::Info,
        text: format!("cell-{n:02}"),
    })
}
