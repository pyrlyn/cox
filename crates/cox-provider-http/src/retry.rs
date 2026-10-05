// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Retry and backoff around one provider stream. Separate from each backend
//! so the policy (which errors retry, how long to wait, when to stop) is
//! written once; a backend wraps its single-attempt `stream` in
//! [`stream_with_retry`] and stays a plain HTTP call.
//!
//! The rule that matters for correctness: a retry happens only before any
//! event reached the caller. Once a byte was delivered the caller's state
//! (a half-built message) cannot be rewound, so the error surfaces instead.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cox_protocol::errors::ProviderError;
use cox_protocol::types::{ProviderEvent, Usage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Backoff parameters (`providers.<name>.max_retries`; plan.md §1.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Attempts after the first; `0` disables retries.
    pub max_retries: u32,
    /// The first wait; doubles per attempt. Tests shrink it.
    pub base: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base: Duration::from_secs(1),
        }
    }
}

impl Policy {
    /// The wait before retry number `attempt` (0-based): `base × 2ⁿ ± 25 %`,
    /// or the server's `retry-after` when it sent one (capped at a minute
    /// so a misconfigured proxy cannot park a session).
    pub fn delay(&self, attempt: u32, retry_after_s: Option<u64>) -> Duration {
        if let Some(s) = retry_after_s {
            return Duration::from_secs(s.min(60));
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        // Uniform in [-0.25, 0.25] from the clock's sub-second noise: enough
        // to de-synchronise concurrent sessions without a random crate.
        let jitter = (nanos % 501) as f64 / 1000.0 - 0.25;
        self.base
            .saturating_mul(1u32 << attempt.min(16))
            .mul_f64(1.0 + jitter)
    }
}

/// Which failures are worth another attempt (plan.md §1.14).
pub fn retryable(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::RateLimited { .. }
            | ProviderError::Overloaded
            | ProviderError::Network
            | ProviderError::Timeout
    )
}

/// Runs `attempt` until it succeeds, fails permanently, or has already
/// delivered an event to `sink`. Emits [`ProviderEvent::Retrying`] before
/// each wait; cancellation during a wait returns [`ProviderError::Cancelled`].
pub async fn stream_with_retry<F, Fut>(
    policy: Policy,
    sink: mpsc::Sender<ProviderEvent>,
    cancel: CancellationToken,
    mut attempt: F,
) -> Result<Usage, ProviderError>
where
    F: FnMut(mpsc::Sender<ProviderEvent>, CancellationToken) -> Fut,
    Fut: Future<Output = Result<Usage, ProviderError>>,
{
    let mut n = 0u32;
    loop {
        // A private channel per attempt: forwarding lets us count what the
        // caller has seen, which decides whether a retry is still safe.
        let (tx, mut rx) = mpsc::channel(64);
        let forward = async {
            let mut delivered = 0u32;
            while let Some(event) = rx.recv().await {
                delivered += 1;
                if sink.send(event).await.is_err() {
                    break;
                }
            }
            delivered
        };
        let (result, delivered) = tokio::join!(attempt(tx, cancel.clone()), forward);
        let error = match result {
            Ok(usage) => return Ok(usage),
            Err(e) if delivered == 0 && n < policy.max_retries && retryable(&e) => e,
            Err(e) => return Err(e),
        };
        let retry_after = match &error {
            ProviderError::RateLimited { retry_after } => *retry_after,
            _ => None,
        };
        let wait = policy.delay(n, retry_after);
        n += 1;
        let _ = sink
            .send(ProviderEvent::Retrying {
                attempt: n,
                after_ms: wait.as_millis() as u64,
            })
            .await;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn usage() -> Usage {
        Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
            estimated: true,
            latency_ms: 0,
        }
    }

    fn fast() -> Policy {
        Policy {
            max_retries: 4,
            base: Duration::from_millis(1),
        }
    }

    #[test]
    fn retry_delay_doubles_and_honours_retry_after() {
        let p = Policy::default();
        let d0 = p.delay(0, None).as_secs_f64();
        let d2 = p.delay(2, None).as_secs_f64();
        assert!((0.75..=1.25).contains(&d0), "{d0}");
        assert!((3.0..=5.0).contains(&d2), "{d2}");
        assert_eq!(p.delay(0, Some(7)), Duration::from_secs(7));
        assert_eq!(p.delay(0, Some(3600)), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn retry_retries_transient_then_succeeds_and_reports_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let (tx, mut rx) = mpsc::channel(16);
        let c = calls.clone();
        let out = stream_with_retry(fast(), tx, CancellationToken::new(), move |sink, _| {
            let c = c.clone();
            async move {
                if c.fetch_add(1, Ordering::SeqCst) < 2 {
                    return Err(ProviderError::Overloaded);
                }
                let _ = sink.send(ProviderEvent::ToolUseEnd).await;
                Ok(usage())
            }
        })
        .await;
        assert!(out.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let mut retrying = 0;
        while let Ok(ev) = rx.try_recv() {
            if let ProviderEvent::Retrying { attempt, .. } = ev {
                retrying += 1;
                assert_eq!(attempt, retrying);
            }
        }
        assert_eq!(retrying, 2);
    }

    #[tokio::test]
    async fn retry_gives_up_after_max_and_never_on_permanent_errors() {
        let (tx, _rx) = mpsc::channel(16);
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out = stream_with_retry(fast(), tx.clone(), CancellationToken::new(), move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            async { Err::<Usage, _>(ProviderError::Network) }
        })
        .await;
        assert!(matches!(out, Err(ProviderError::Network)));
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out = stream_with_retry(fast(), tx, CancellationToken::new(), move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            async { Err::<Usage, _>(ProviderError::Auth) }
        })
        .await;
        assert!(matches!(out, Err(ProviderError::Auth)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_does_not_retry_after_first_byte() {
        let (tx, mut rx) = mpsc::channel(16);
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out = stream_with_retry(fast(), tx, CancellationToken::new(), move |sink, _| {
            c.fetch_add(1, Ordering::SeqCst);
            async move {
                let _ = sink
                    .send(ProviderEvent::TextDelta { text: "hi".into() })
                    .await;
                Err::<Usage, _>(ProviderError::Network)
            }
        })
        .await;
        assert!(matches!(out, Err(ProviderError::Network)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(rx.try_recv(), Ok(ProviderEvent::TextDelta { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn retry_cancel_during_backoff_returns_cancelled() {
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let slow = Policy {
            max_retries: 1,
            base: Duration::from_secs(30),
        };
        let c = cancel.clone();
        let out = stream_with_retry(slow, tx, cancel, move |_, _| {
            c.cancel();
            async { Err::<Usage, _>(ProviderError::Overloaded) }
        })
        .await;
        assert!(matches!(out, Err(ProviderError::Cancelled)));
    }
}
