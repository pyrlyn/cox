// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Usage and cost tracking (plan.md §1.7/D5/D6g). `Price`/`PriceError`/
//! `PriceTable` moved into the pure `cox-models` crate by T30.24 (the model
//! catalog owns them now, alongside context window, efforts and
//! capabilities); this module keeps the parts that touch a live call:
//! [`load_price_table`] (the one place that still does file I/O for a
//! price table), [`ledger_row`], and [`Priced`].
//!
//! [`Priced`] is where the table meets a live call: it wraps the session's
//! provider so every caller (turns, compaction, memory, init, subagents)
//! gets a costed `Usage` without each one looking prices up itself.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use cox_protocol::errors::ProviderError;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::{Provider, UsageRow};
use cox_protocol::types::{Caps, Job, ModelId, ProviderEvent, ProviderId, Request, Tier, Usage};

pub use cox_models::{Price, PriceError, PriceTable};

/// Reads `path` and parses it as a price table, falling back to the
/// embedded defaults when the file does not exist — the unchanged
/// behaviour of the pre-T30.24 `PriceTable::load`. The parsing itself now
/// lives in `cox_models::PriceTable`, which does no I/O of its own, so this
/// free function is the one place in the workspace that still reads a
/// price file from disk.
pub fn load_price_table<P: AsRef<Path>>(path: P) -> Result<PriceTable, PriceError> {
    match std::fs::read_to_string(&path) {
        Ok(content) => PriceTable::parse(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PriceTable::embedded(),
        Err(e) => Err(PriceError::Io(e)),
    }
}

/// Fills in the cost of a call ([`PriceTable::apply`]) and returns the row
/// `Store::usage_insert` takes.
#[allow(clippy::too_many_arguments)]
pub fn ledger_row(
    session: SessionId,
    turn: u32,
    job: Job,
    tier: Tier,
    provider: ProviderId,
    model: ModelId,
    effort: Option<cox_protocol::types::Effort>,
    usage: Usage,
    prices: &PriceTable,
) -> UsageRow {
    let mut usage = usage;
    prices.apply(&model, &mut usage);
    UsageRow {
        session_id: session,
        turn,
        job,
        tier,
        provider,
        model,
        effort,
        usage,
    }
}

/// A provider that costs every call it makes. Backends report tokens only;
/// this prices both the `Usage` event callers fold into the ledger and the
/// `Usage` the call returns, by `req.model`, so the two always agree.
pub struct Priced {
    inner: Arc<dyn Provider>,
    prices: Arc<PriceTable>,
}

impl Priced {
    /// Wraps `inner`; `prices` is shared by every call it makes.
    pub fn new(inner: Arc<dyn Provider>, prices: Arc<PriceTable>) -> Self {
        Self { inner, prices }
    }
}

#[async_trait]
impl Provider for Priced {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn capabilities(&self) -> Caps {
        self.inner.capabilities()
    }

    fn accepts_images(&self, model: &str) -> bool {
        self.inner.accepts_images(model)
    }

    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let model = req.model.clone();
        let (tx, mut rx) = mpsc::channel(64);
        let forward = async {
            while let Some(mut event) = rx.recv().await {
                if let ProviderEvent::Usage { usage } = &mut event {
                    self.prices.apply(&model, usage);
                }
                // A closed sink means the caller stopped listening; dropping
                // `rx` then lets the backend see its own send fail.
                if sink.send(event).await.is_err() {
                    break;
                }
            }
        };
        let (result, ()) = tokio::join!(self.inner.stream(req, tx, cancel), forward);
        let mut usage = result?;
        self.prices.apply(&model, &mut usage);
        Ok(usage)
    }

    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        self.inner.count_tokens(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_usage() -> Usage {
        Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: false,
            cost_usd: 0.0,
            latency_ms: 50,
        }
    }

    #[test]
    fn usage_ledger_row_carries_cost_and_identity() {
        let table = PriceTable::embedded().expect("default prices parse");
        let session = SessionId::new();
        let row = ledger_row(
            session,
            3,
            Job::Main,
            Tier::Code,
            ProviderId::Anthropic,
            ModelId("claude-haiku-4-5".into()),
            Some(cox_protocol::types::Effort::Low),
            sample_usage(),
            &table,
        );
        assert_eq!(row.session_id, session);
        assert_eq!(row.turn, 3);
        assert_eq!(row.job, Job::Main);
        assert_eq!(row.tier, Tier::Code);
        assert_eq!(row.provider, ProviderId::Anthropic);
        assert_eq!(row.model.0, "claude-haiku-4-5");
        // Haiku: 1M input @ $1/M + 100k output @ $5/M = $1.50.
        assert!((row.usage.cost_usd - 1.5).abs() < 0.0001);
        assert!(!row.usage.estimated);
    }

    #[test]
    fn usage_ledger_row_for_unknown_model_is_zero_cost_and_estimated() {
        let table = PriceTable::embedded().expect("default prices parse");
        let row = ledger_row(
            SessionId::new(),
            1,
            Job::Main,
            Tier::Code,
            ProviderId::Local,
            ModelId("some-local-model".into()),
            Some(cox_protocol::types::Effort::High),
            sample_usage(),
            &table,
        );
        // An unpriced model still produces a row — costed 0, flagged estimated.
        assert_eq!(row.usage.cost_usd, 0.0);
        assert!(row.usage.estimated);
        assert_eq!(row.usage.input_tokens, 1_000_000);
    }

    /// Reports fixed tokens and no cost, the way every real backend does.
    struct TokensOnly;

    #[async_trait]
    impl Provider for TokensOnly {
        fn id(&self) -> ProviderId {
            ProviderId::Anthropic
        }

        fn capabilities(&self) -> Caps {
            Caps {
                cache: true,
                thinking: false,
                server_tools: false,
                count_tokens: false,
                max_context: 200_000,
            }
        }

        async fn stream(
            &self,
            _req: Request,
            sink: mpsc::Sender<ProviderEvent>,
            _cancel: CancellationToken,
        ) -> Result<Usage, ProviderError> {
            let usage = sample_usage();
            let _ = sink.send(ProviderEvent::Usage { usage }).await;
            Ok(usage)
        }

        async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
            Ok(0)
        }
    }

    fn request(model: &str) -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId(model.into()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: cox_protocol::types::Effort::High,
            max_tokens: 1024,
            thinking: cox_protocol::types::Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    /// Runs one call through `Priced`; returns the forwarded `Usage` event
    /// and the call's own return value.
    async fn priced_call(model: &str) -> (Usage, Usage) {
        let table = PriceTable::embedded().expect("default prices parse");
        let priced = Priced::new(Arc::new(TokensOnly), Arc::new(table));
        let (tx, mut rx) = mpsc::channel(8);
        let returned = priced
            .stream(request(model), tx, CancellationToken::new())
            .await
            .expect("call succeeds");
        let mut event = None;
        while let Some(ev) = rx.recv().await {
            if let ProviderEvent::Usage { usage } = ev {
                event = Some(usage);
            }
        }
        (event.expect("usage event forwarded"), returned)
    }

    #[tokio::test]
    async fn priced_provider_costs_both_the_event_and_the_return() {
        let (event, returned) = priced_call("claude-haiku-4-5").await;
        // Haiku: 1M input @ $1/M + 100k output @ $5/M = $1.50.
        assert!((event.cost_usd - 1.5).abs() < 0.0001, "{}", event.cost_usd);
        assert_eq!(event, returned);
        assert!(!returned.estimated);
    }

    #[tokio::test]
    async fn priced_provider_flags_an_unknown_model_as_estimated() {
        let (event, returned) = priced_call("no-such-model").await;
        assert_eq!(event.cost_usd, 0.0);
        assert!(event.estimated && returned.estimated);
    }
}
