// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `Replay` provider: serves recorded HTTP cassettes so contract tests
//! and evals run with no network and no key (D12). Cassette hashing,
//! secret redaction and the write/near-miss-hint helpers live in
//! `cox-provider-testkit` (T32.11) — reused by `cox record` too —
//! re-exported here; this file is the streaming glue that reads a
//! cassette's SSE body and feeds it through `AnthropicStream`, which is
//! why it still needs `cox-provider`'s own `anthropic`/`sse` modules and
//! can't move: a cassette is a hashed wire capture, not a TOML scenario.
//!
//! Layout: `<dir>/<sha256>.request.json` + `<dir>/<sha256>.sse`.

use std::path::PathBuf;

use async_trait::async_trait;
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, Usage};
use cox_provider_testkit::replay::nearest_hint;
pub use cox_provider_testkit::replay::{cassette_hash, redact_secrets, write_cassette};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::anthropic::stream::AnthropicStream;
use crate::sse::parse_sse_str;

/// Replays cassettes from one directory.
pub struct Replay {
    dir: PathBuf,
}

impl Replay {
    /// `dir` is the cassette folder (`cassettes/<name>/` or `COX_CASSETTES`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `COX_PROVIDER=replay` plus `COX_CASSETTES=<dir>`.
    pub fn from_env() -> Result<Self, ProviderError> {
        let dir = std::env::var("COX_CASSETTES").map_err(|_| ProviderError::BadRequest {
            message: "COX_CASSETTES is required when COX_PROVIDER=replay".into(),
        })?;
        Ok(Self::new(dir))
    }
}

#[async_trait]
impl Provider for Replay {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: true,
            server_tools: false,
            count_tokens: false,
            max_context: u32::MAX,
        }
    }

    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let hash = cassette_hash(&req)?;
        let sse_path = self.dir.join(format!("{hash}.sse"));
        let sse = std::fs::read_to_string(&sse_path).map_err(|_| ProviderError::Unsupported {
            feature: format!("cassette miss: {hash}{}", nearest_hint(&self.dir, &hash)),
        })?;
        let mut machine = AnthropicStream::new();
        for (event, data) in parse_sse_str(&sse) {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            for ev in machine.feed(event.as_deref(), &data)? {
                if sink.send(ev).await.is_err() {
                    return Err(ProviderError::Cancelled);
                }
            }
        }
        Ok(machine.usage())
    }

    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Err(ProviderError::Unsupported {
            feature: "replay count_tokens".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::types::{Effort, Job, ModelId, Thinking, Tier};

    fn req() -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId("claude-sonnet-5".into()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: Effort::High,
            max_tokens: 1024,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    #[tokio::test]
    async fn cassette_miss_names_hash() {
        let dir = std::env::temp_dir().join(format!("cox-replay-miss-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let provider = Replay::new(&dir);
        let (tx, _rx) = mpsc::channel(8);
        let err = provider
            .stream(req(), tx, CancellationToken::new())
            .await
            .expect_err("miss");
        let ProviderError::Unsupported { feature } = err else {
            panic!("expected miss, got {err:?}");
        };
        assert!(feature.contains("cassette miss:"), "{feature}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replay_streams_anthropic_fixture() {
        let dir = std::env::temp_dir().join(format!("cox-replay-hit-{}", std::process::id()));
        let sse = include_str!("../../../fixtures/anthropic/text_only.sse");
        let hash = write_cassette(&dir, &req(), sse, false).expect("write");
        let provider = Replay::new(&dir);
        let (tx, mut rx) = mpsc::channel(64);
        let usage = provider
            .stream(req(), tx, CancellationToken::new())
            .await
            .expect("hit");
        assert!(
            std::fs::read_to_string(dir.join(format!("{hash}.sse")))
                .expect("sse")
                .contains("Hello")
        );
        let mut saw_hello = false;
        while let Ok(ev) = rx.try_recv() {
            if let ProviderEvent::TextDelta { text } = ev
                && text.contains("Hello")
            {
                saw_hello = true;
            }
        }
        assert!(saw_hello, "expected Hello delta");
        assert!(usage.output_tokens > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
