// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! End-of-session extraction (T10.2): `Shutdown` with `memory.extract`
//! runs the memory job, dedups against FTS and saves survivors.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::open;
use cox_protocol::traits::{Hook, Store as _};
use cox_protocol::types::{Event, HookEvent, HookOutcome, Job};
use serde_json::Value;

/// Records every hook event it saw.
struct Recorder {
    seen: Mutex<Vec<HookEvent>>,
}

#[async_trait]
impl Hook for Recorder {
    async fn run(&self, event: HookEvent, _payload: Value, _timeout: Duration) -> HookOutcome {
        self.seen.lock().unwrap().push(event);
        HookOutcome::Continue
    }
}

fn collect(rx: &mut tokio::sync::mpsc::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn notices(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Notice { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn memory_extract_disabled_by_default() {
    let (session, store, mut rx) = open("", cox_protocol::Config::default());
    session
        .submit(cox_protocol::types::Submission::Shutdown)
        .await
        .expect("shutdown");
    let events = collect(&mut rx);
    assert!(store.usage_rows().is_empty(), "no provider call");
    assert!(
        notices(&events).iter().all(|t| !t.contains("memory")),
        "{events:?}"
    );
}

#[tokio::test]
async fn memory_extract_saves_new_fact_and_skips_duplicate() {
    let dup_body = "Login goes through the auth module with session cookies attached.";
    // Single-quoted TOML literal: the JSON keeps its double quotes untouched.
    let toml = concat!(
        "[[turn]]\ntext = '[{\"name\": \"auth-flow\", \"type\": \"decision\", \"body\": \"",
        "Login goes through the auth module with session cookies attached.",
        "\"}, {\"name\": \"widget-api\", \"type\": \"fact\", \"body\": \"",
        "Canvas holds every widget by id in a hash map.\"}]'\n",
    );
    let mut config = cox_protocol::Config::default();
    config.memory.extract = true;
    let (session, store, mut rx) = open(toml, config);
    // Seed the duplicate under the slug extraction derives from /tmp/cox-turn.
    store
        .memory_upsert(
            "cox-turn",
            "auth-flow",
            "auth-flow.md",
            "decision",
            dup_body,
        )
        .expect("seed");
    session
        .submit(cox_protocol::types::Submission::Shutdown)
        .await
        .expect("shutdown");
    let events = collect(&mut rx);
    let notes = notices(&events);
    assert!(
        notes.iter().any(|t| t.contains("memory saved: widget-api")),
        "{notes:?}"
    );
    assert!(
        notes.iter().all(|t| !t.contains("memory saved: auth-flow")),
        "duplicate skipped: {notes:?}"
    );
    // Exactly one newcomer in the store, no duplicate row for the old fact.
    let found = store
        .memory_search("widget canvas hash", 5)
        .expect("search");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "widget-api");
    let dups = store.memory_search("auth flow", 5).expect("search");
    assert_eq!(dups.len(), 1, "{dups:?}");
    // The extraction call itself is a ledger row on the memory job.
    assert!(
        store.usage_rows().iter().any(|r| r.job == Job::Memory),
        "memory job row"
    );
    // Surfaces can drain what was saved to materialise the .md files.
    let drained = session.drain_extracted().await;
    assert_eq!(
        drained.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["widget-api"]
    );
    assert!(session.drain_extracted().await.is_empty(), "drain once");
}

#[tokio::test]
async fn memory_extract_fires_session_end_hook() {
    let recorder = Arc::new(Recorder {
        seen: Mutex::new(vec![]),
    });
    let (session, _, mut rx) = open("", cox_protocol::Config::default());
    session.set_hook(recorder.clone());
    session
        .submit(cox_protocol::types::Submission::Shutdown)
        .await
        .expect("shutdown");
    let _ = collect(&mut rx);
    assert!(
        recorder
            .seen
            .lock()
            .unwrap()
            .contains(&HookEvent::SessionEnd),
        "SessionEnd fired"
    );
}
