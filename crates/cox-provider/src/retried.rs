// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Retried`: the host's backoff around a provider that has none of its own
//! (T33.40.4). Wire clients retry inside `stream`; a plugin's ABI provider
//! (`cox-plugin`) cannot, since that crate may not depend on
//! `cox-provider-http`, so the session wraps it here with the same
//! `retry::stream_with_retry` and the section's `max_retries`.

use std::sync::Arc;

use async_trait::async_trait;
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, Usage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::retry::{Policy, stream_with_retry};

/// Retries `inner`'s transient failures under `policy`.
pub struct Retried {
    inner: Arc<dyn Provider>,
    policy: Policy,
}

impl Retried {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn Provider>, policy: Policy) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl Provider for Retried {
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
        stream_with_retry(self.policy, sink, cancel, |sink, cancel| {
            let inner = Arc::clone(&self.inner);
            let req = req.clone();
            async move { inner.stream(req, sink, cancel).await }
        })
        .await
    }

    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        self.inner.count_tokens(req).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use cox_protocol::types::{Effort, Job, ModelId, Thinking, Tier};

    use super::*;

    /// Fails `Overloaded` every time and counts the attempts.
    #[derive(Default)]
    struct Down(AtomicU32);

    #[async_trait]
    impl Provider for Down {
        fn id(&self) -> ProviderId {
            ProviderId::Plugin("down".into())
        }

        fn capabilities(&self) -> Caps {
            Caps {
                cache: false,
                thinking: false,
                server_tools: false,
                count_tokens: false,
                max_context: 1,
            }
        }

        async fn stream(
            &self,
            _: Request,
            _: mpsc::Sender<ProviderEvent>,
            _: CancellationToken,
        ) -> Result<Usage, ProviderError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(ProviderError::Overloaded)
        }

        async fn count_tokens(&self, _: &Request) -> Result<u32, ProviderError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn retried_provider_tries_max_retries_more_times() {
        let down = Arc::new(Down::default());
        let retried = Retried::new(
            down.clone(),
            Policy {
                max_retries: 2,
                base: Duration::from_millis(1),
            },
        );
        let req = Request {
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
        let (tx, _rx) = mpsc::channel(16);
        let got = retried.stream(req, tx, CancellationToken::new()).await;
        assert_eq!(got, Err(ProviderError::Overloaded));
        assert_eq!(down.0.load(Ordering::Relaxed), 3);
    }
}
