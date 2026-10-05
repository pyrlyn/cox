// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Session spend cap (plan.md T2.7).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{
    Caps, Event, ProviderEvent, ProviderId, Request, StopReason, Submission, Usage,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Pricey;

fn pricey_usage() -> Usage {
    Usage {
        input_tokens: 1,
        output_tokens: 1,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        estimated: true,
        cost_usd: 10.0,
        latency_ms: 0,
    }
}

#[async_trait]
impl Provider for Pricey {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }
    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: true,
            max_context: u32::MAX,
        }
    }
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let model = req.model.clone();
        let usage = pricey_usage();
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let _ = sink.send(ProviderEvent::MessageStart { model }).await;
        let _ = sink
            .send(ProviderEvent::TextDelta { text: "ok".into() })
            .await;
        let _ = sink
            .send(ProviderEvent::Stop {
                stop: StopReason::EndTurn,
            })
            .await;
        let _ = sink.send(ProviderEvent::Usage { usage }).await;
        Ok(usage)
    }
    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Ok(1)
    }
}

async fn drain(rx: &mut mpsc::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout")
            .expect("closed");
        let done = matches!(ev, Event::TurnDone { .. });
        out.push(ev);
        if done {
            break;
        }
    }
    out
}

fn kinds(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|e| match e {
            Event::SessionStarted { .. } => "session_started".into(),
            Event::TurnStarted { .. } => "turn_started".into(),
            Event::ItemStarted { .. } => "item_started".into(),
            Event::ItemDone { .. } => "item_done".into(),
            Event::TextDelta { .. } => "text_delta".into(),
            Event::Usage { .. } => "usage".into(),
            Event::Notice { .. } => "notice".into(),
            Event::TurnDone { stop, .. } => format!("turn_done:{stop:?}"),
            Event::Error { .. } => "error".into(),
            _ => "other".into(),
        })
        .collect()
}

#[tokio::test]
async fn budget_hit() {
    let mut config = cox_protocol::Config::default();
    config.budget.session_usd = 5.0;
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        Arc::new(Pricey),
        vec![],
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-budget"),
    )
    .expect("session");
    let mut rx = session.events().expect("rx");
    session
        .submit(Submission::UserTurn {
            text: "one".into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("first");
    let first = drain(&mut rx).await;
    assert!(matches!(
        first.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    session
        .submit(Submission::UserTurn {
            text: "two".into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("second");
    let second = drain(&mut rx).await;
    assert!(matches!(
        second.last(),
        Some(Event::TurnDone {
            stop: StopReason::Budget,
            ..
        })
    ));
    insta::with_settings!({
        snapshot_path => "scenarios",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_json_snapshot!("budget_hit.events", kinds(&second));
    });
}
