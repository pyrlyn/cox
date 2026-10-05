// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Plugin timings for PL§11 (T33.28), measured on the Rust reference plugin
//! that `cox-plugin-fixtures` builds, through the same host paths a session
//! uses: `LivePlugins::load` + `start` (compile, instantiate, `cox_init`),
//! `cox_on_event`, `cox_render` and `PluginHooks` as the hook chain calls
//! it, with the plugin's kv store in a real `cox.db` under a scratch home.
//! Prints a markdown table for research.md R§4.7; `just bench` runs it.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cox_plugin::{Lane, LivePlugins, PluginHooks};
use cox_plugin_api::{EventBatch, PluginManifest, RenderIn, Slot};
use cox_plugin_fixtures::EXAMPLE_DIR;
use cox_protocol::config::PluginsConfig;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{Hook, PluginStore, Store as _};
use cox_protocol::types::HookEvent;
use serde_json::{Value, json};

const STARTS: usize = 20;
const CALLS: usize = 1_000;
const DEADLINE: Duration = Duration::from_secs(5);

/// The `q` quantile (0..=1) of `samples`, nearest rank.
fn quantile(samples: &mut [Duration], q: f64) -> Duration {
    samples.sort();
    let rank = ((samples.len() - 1) as f64 * q).round() as usize;
    samples[rank]
}

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1e3)
}

fn time<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<(Duration, T)> {
    let at = Instant::now();
    let out = f()?;
    Ok((at.elapsed(), out))
}

fn start(
    manifest: &PluginManifest,
    wasm: &[u8],
    store: &Arc<dyn PluginStore>,
    cwd: &Path,
) -> anyhow::Result<LivePlugins> {
    let mut live = LivePlugins::default();
    live.load(manifest, wasm, store.clone())?;
    let warnings = live.start(&PluginsConfig::default(), SessionId::new(), cwd);
    anyhow::ensure!(warnings.is_empty(), "{warnings:?}");
    Ok(live)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let dir = Path::new(EXAMPLE_DIR);
    let (manifest, _) = cox_plugin::discover::load_manifest(dir, &dir.join("plugin.toml"), None)
        .map_err(anyhow::Error::msg)?;
    let wasm_path = manifest
        .wasm
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the bench plugin has no wasm to load"))?;
    let wasm = std::fs::read(dir.join(wasm_path))?;
    let store: Arc<dyn PluginStore> = Arc::new(cox_store::Store::open(home.path())?);

    // The host runs with wasmtime's compilation cache off until its card
    // lands (host.rs), so every start compiles: cold and warm are one number.
    let mut starts = Vec::with_capacity(STARTS);
    for _ in 0..STARTS {
        starts.push(time(|| start(&manifest, &wasm, &store, home.path()))?.0);
    }
    let live = start(&manifest, &wasm, &store, home.path())?;
    let plugin = &live.plugins()[0];
    let host = plugin.host().clone();

    let batch = EventBatch {
        first_seq: 1,
        dropped: 0,
        events: vec![json!({ "type": "turn_started", "seq": 1 }); 16],
    };
    let mut events = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let call = || Ok(host.call::<_, Value>(Lane::Event, "cox_on_event", &batch, DEADLINE)?);
        events.push(time(call)?.0);
    }

    let render = RenderIn {
        slot: Slot::StatusRight,
        width: 80,
        height: 1,
    };
    let mut renders = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let call = || Ok(host.call::<_, Value>(Lane::Control, "cox_render", &render, DEADLINE)?);
        renders.push(time(call)?.0);
    }

    let hooks = PluginHooks::new(plugin.id(), host.clone(), plugin.granted());
    let payload = json!({
        "tool_name": "read",
        "tool_response": { "text": "no such file", "is_error": true },
    });
    let mut rounds = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let at = Instant::now();
        hooks
            .run(HookEvent::PostToolUseFailure, payload.clone(), DEADLINE)
            .await;
        rounds.push(at.elapsed());
        // The session drains notices after every event; so does the bench,
        // or the queue cap would refuse `cox_notify` from the 17th call on.
        live.take_notices();
    }

    println!("| Metric (PL§11) | Budget | Measured |");
    println!("|---|---|---|");
    println!(
        "| session start per plugin: load + `cox_init`, median of {STARTS} ({} KiB module, cache off) | ≤ 50 ms warm, ≤ 500 ms cold (1 MiB) | {} (max {}) |",
        wasm.len() / 1024,
        ms(quantile(&mut starts.clone(), 0.5)),
        ms(quantile(&mut starts, 1.0)),
    );
    println!(
        "| `cox_on_event` batch of 16, p50 of {CALLS} | ≤ 1 ms | {} |",
        ms(quantile(&mut events, 0.5))
    );
    println!(
        "| `cox_render` status segment, p95 of {CALLS} | ≤ 5 ms | {} |",
        ms(quantile(&mut renders, 0.95))
    );
    println!(
        "| hook round trip (`PluginHooks::run`), p95 of {CALLS} | ≤ 5 ms | {} |",
        ms(quantile(&mut rounds, 0.95))
    );
    Ok(())
}
