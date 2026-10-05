// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The loop over the real Chat Completions state machine (T38.1): a
//! provider that runs SSE bodies through `OpenAiChatStream` exactly as
//! `OpenAiChatProvider::stream_once` does, so the test proves that what the
//! Chat wire emits is something `consume_provider` commits. Separate from
//! `turn.rs` because every other loop test drives the `Scripted` provider,
//! whose events never pass through a wire's state machine.

mod common;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{drain, spawn_turn, tool_results, tools};
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, Usage};
use cox_provider::openai::chat::OpenAiChatStream;
use cox_provider::sse::parse_sse_str;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Serves one SSE body per provider call, in order.
struct ChatWire(Mutex<VecDeque<&'static str>>);

#[async_trait]
impl Provider for ChatWire {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }
    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: false,
            max_context: 128_000,
        }
    }
    async fn stream(
        &self,
        _req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let body = self
            .0
            .lock()
            .expect("lock")
            .pop_front()
            .expect("the test supplies every call");
        let mut machine = OpenAiChatStream::new();
        let mut events = Vec::new();
        for (_event, data) in parse_sse_str(body) {
            events.extend(machine.feed(&data)?);
        }
        events.extend(machine.finish());
        for event in events {
            sink.send(event)
                .await
                .map_err(|_| ProviderError::Cancelled)?;
        }
        Ok(machine.usage())
    }
    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Ok(1)
    }
}

/// Two `echo` calls interleaved by index: call 0's arguments finish only
/// after call 1 has started.
const TWO_INTERLEAVED_CALLS: &str = r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"echo","arguments":"{\"text\":"}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"echo","arguments":"{\"text\":\"b\"}"}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

"#;

const DONE: &str = r#"data: {"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}]}

"#;

#[tokio::test]
async fn chat_wire_parallel_calls_both_commit_and_run() {
    let provider = Arc::new(ChatWire(Mutex::new(VecDeque::from([
        TWO_INTERLEAVED_CALLS,
        DONE,
    ]))));
    let mut config = cox_protocol::Config::default();
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        tools(),
        store.clone(),
        store,
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events once");

    let running = spawn_turn(&session, "echo a and b");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");

    let mut results = tool_results(&events);
    results.sort();
    assert_eq!(
        results,
        [(true, "a".to_string()), (true, "b".to_string())],
        "{events:?}"
    );
}
