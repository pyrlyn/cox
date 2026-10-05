// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The token meter's view state (DS§7, T37.12): sent and received per turn
//! and per session, live tok/s with its sparkline, time to first token, and
//! the last call's context size. Separate from the timeline fold because
//! throughput needs a clock: the caller passes each event's arrival time, so
//! the fold stays pure and a test scripts the timings. Totals only add up
//! `Event::Usage`, which the core emits right after writing the same `Usage`
//! as a ledger row, so the meter keeps no second accounting.

use std::collections::VecDeque;
use std::time::Duration;

use cox_protocol::ids::TurnId;
use cox_protocol::types::{ContextBreakdown, Event, Usage};
use serde::{Deserialize, Serialize};

use crate::meter_text::{MeterText, Rates};

/// The rolling window live tok/s is estimated over.
pub const WINDOW: Duration = Duration::from_secs(2);
/// The shortest span a rate is taken over: below it the clock's jitter, not
/// the stream, sets the figure (a scripted or replayed call streams in
/// microseconds, which read as 180 000 tok/s), so no rate shows.
pub const MIN_SPAN: Duration = Duration::from_millis(100);
/// Sparkline points kept, one per output delta.
pub const SPARK_POINTS: usize = 32;
/// Bytes per token of the live estimate. Not `cox_tokens::estimate`: that
/// sizes a whole `Request` and would pull tiktoken-rs and reqwest into the
/// app core for one constant, and the call's exact count replaces the
/// estimate as soon as its usage arrives.
const BYTES_PER_TOKEN: f64 = 4.0;

/// Ledger totals over some calls, in the meter's terms (DS§7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Tally {
    /// ↑ input + cache read + cache write (`Usage::context_tokens`).
    pub sent: u32,
    /// ↓ output, thinking included.
    pub received: u32,
    pub cache_read: u32,
    pub cache_write: u32,
    /// Input billed at the full rate.
    pub uncached: u32,
    pub cost_usd: f64,
    /// Provider calls, i.e. ledger rows.
    pub calls: u32,
    /// Some call's usage was cox's estimate, not the provider's report.
    pub estimated: bool,
}

impl Tally {
    fn of(sum: Option<Usage>, calls: u32) -> Self {
        sum.map_or_else(Self::default, |u| Self {
            sent: u.context_tokens(),
            received: u.output_tokens,
            cache_read: u.cache_read_tokens,
            cache_write: u.cache_write_tokens,
            uncached: u.input_tokens,
            cost_usd: u.cost_usd,
            calls,
            estimated: u.estimated,
        })
    }
}

/// The running turn, or the last one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnUsage {
    pub turn: TurnId,
    pub tally: Tally,
    /// Estimated from thinking deltas: the ledger does not split thinking
    /// out of `received`.
    pub thinking_tokens: u32,
    /// From `TurnStarted` to the turn's first text or thinking delta.
    pub ttft_ms: Option<u64>,
    pub tok_per_s: Option<f64>,
    /// `tok_per_s` is the last call's exact figure, not the live estimate.
    pub exact: bool,
    /// Live tok/s after each output delta, oldest first.
    pub sparkline: Vec<f64>,
    pub done: bool,
}

/// What the token meter and its popover show.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageView {
    pub session: Tally,
    pub turn: Option<TurnUsage>,
    /// The last call's `Usage::context_tokens`, the TUI's `ctx` figure.
    pub context_tokens: u32,
    /// The figures above as the desktop shows them (T37.25).
    pub text: MeterText,
}

/// Folds events and their arrival times into a `UsageView`.
#[derive(Debug, Clone, Default)]
pub struct Meter {
    view: UsageView,
    session: (Option<Usage>, u32),
    turn: (Option<Usage>, u32),
    started: Duration,
    thinking_bytes: usize,
    /// `(arrival, estimated tokens)` of the output deltas within `WINDOW`.
    window: VecDeque<(Duration, f64)>,
    /// The current call's first output delta.
    call_start: Option<Duration>,
    /// The turn's exactly rated output and the seconds it streamed over.
    rated: (f64, f64),
    /// The turn's highest tok/s, live or exact.
    peak: Option<f64>,
    session_thinking_bytes: usize,
    /// The latest request's window and split (A98); only its formatted
    /// text crosses to the views.
    context: Option<ContextBreakdown>,
}

impl Meter {
    pub fn view(&self) -> &UsageView {
        &self.view
    }

    /// Folds `event`, which arrived at `now` (any monotonic origin; a
    /// replayed rollout passes one instant for all, so only the exact rates
    /// survive); true when the view changed.
    pub fn apply(&mut self, event: &Event, now: Duration) -> bool {
        let changed = self.fold(event, now);
        if changed {
            let rates = Rates {
                avg: (self.rated.1 > 0.0).then(|| self.rated.0 / self.rated.1),
                peak: self.peak,
                session_thinking: estimate(self.session_thinking_bytes),
                context: self.context,
            };
            self.view.text = MeterText::of(&self.view, rates);
        }
        changed
    }

    fn fold(&mut self, event: &Event, now: Duration) -> bool {
        match event {
            Event::TurnStarted { turn, .. } => {
                (self.started, self.turn, self.thinking_bytes) = (now, (None, 0), 0);
                self.window.clear();
                (self.call_start, self.rated, self.peak) = (None, (0.0, 0.0), None);
                self.view.turn = Some(TurnUsage {
                    turn: *turn,
                    tally: Tally::default(),
                    thinking_tokens: 0,
                    ttft_ms: None,
                    tok_per_s: None,
                    exact: false,
                    sparkline: vec![],
                    done: false,
                });
                true
            }
            Event::TextDelta { text, .. } => self.output(text.len(), false, now),
            Event::ThinkingDelta { text, .. } => self.output(text.len(), true, now),
            Event::Usage { turn, usage } => {
                self.session = (Some(add_to(self.session.0, usage)), self.session.1 + 1);
                self.view.session = Tally::of(self.session.0, self.session.1);
                self.view.context_tokens = usage.context_tokens();
                self.window.clear();
                let streamed = self.call_start.take().map(|at| now.saturating_sub(at));
                if let Some(t) = self.view.turn.as_mut().filter(|t| t.turn == *turn) {
                    self.turn = (Some(add_to(self.turn.0, usage)), self.turn.1 + 1);
                    t.tally = Tally::of(self.turn.0, self.turn.1);
                    if let Some((rate, secs)) = exact_rate(usage, streamed) {
                        (t.tok_per_s, t.exact) = (Some(rate), true);
                        self.rated.0 += f64::from(usage.output_tokens);
                        self.rated.1 += secs;
                        self.peak = Some(self.peak.map_or(rate, |p| p.max(rate)));
                    }
                }
                true
            }
            Event::ContextBreakdown { breakdown, .. } => {
                self.context = Some(*breakdown);
                true
            }
            Event::TurnDone { turn, .. } => match self.view.turn.as_mut() {
                Some(t) if t.turn == *turn => {
                    t.done = true;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn output(&mut self, bytes: usize, thinking: bool, now: Duration) -> bool {
        let Some(t) = self.view.turn.as_mut().filter(|t| !t.done) else {
            return false;
        };
        if thinking {
            self.thinking_bytes += bytes;
            self.session_thinking_bytes += bytes;
            t.thinking_tokens = estimate(self.thinking_bytes);
        }
        t.ttft_ms
            .get_or_insert(now.saturating_sub(self.started).as_millis() as u64);
        self.call_start.get_or_insert(now);
        self.window.push_back((now, bytes as f64 / BYTES_PER_TOKEN));
        while self
            .window
            .front()
            .is_some_and(|(at, _)| now.saturating_sub(*at) > WINDOW)
        {
            self.window.pop_front();
        }
        // The oldest delta opens the span, so its own tokens stay out.
        let span = self
            .window
            .front()
            .map_or(0.0, |(at, _)| now.saturating_sub(*at).as_secs_f64());
        if span >= MIN_SPAN.as_secs_f64() {
            let rate = self.window.iter().skip(1).map(|(_, n)| n).sum::<f64>() / span;
            (t.tok_per_s, t.exact) = (Some(rate), false);
            t.sparkline.push(rate);
            self.peak = Some(self.peak.map_or(rate, |p| p.max(rate)));
            let over = t.sparkline.len().saturating_sub(SPARK_POINTS);
            t.sparkline.drain(..over);
        }
        true
    }
}

/// Tokens in `bytes` of streamed text, by the live estimate's constant.
fn estimate(bytes: usize) -> u32 {
    (bytes as f64 / BYTES_PER_TOKEN).round() as u32
}

/// A call's exact tok/s and the seconds it is over: its reported output
/// over the time from its first output delta to its usage, or over the ledger's latency (which includes
/// the wait for the first token) when that time is unknown or under
/// `MIN_SPAN` — a call that streamed no text, streamed it in one burst, or a
/// replay. `None` for a call that produced nothing or took under `MIN_SPAN`.
fn exact_rate(usage: &Usage, streamed: Option<Duration>) -> Option<(f64, f64)> {
    let span = streamed
        .filter(|d| *d >= MIN_SPAN)
        .unwrap_or(Duration::from_millis(usage.latency_ms));
    let secs = span.as_secs_f64();
    (span >= MIN_SPAN && usage.output_tokens > 0)
        .then(|| (f64::from(usage.output_tokens) / secs, secs))
}

/// `sum` plus one more call; the one sum the meter and `TurnMeta` share.
pub(crate) fn add_to(sum: Option<Usage>, b: &Usage) -> Usage {
    let Some(a) = sum else { return *b };
    Usage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_read_tokens: a.cache_read_tokens.saturating_add(b.cache_read_tokens),
        cache_write_tokens: a.cache_write_tokens.saturating_add(b.cache_write_tokens),
        estimated: a.estimated || b.estimated,
        cost_usd: a.cost_usd + b.cost_usd,
        latency_ms: a.latency_ms.saturating_add(b.latency_ms),
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::ItemId;
    use cox_protocol::types::{Job, ModelId, StopReason, Tier};

    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn usage(output: u32, latency_ms: u64) -> Usage {
        Usage {
            input_tokens: 100,
            output_tokens: output,
            cache_read_tokens: 900,
            cache_write_tokens: 50,
            estimated: false,
            cost_usd: 0.02,
            latency_ms,
        }
    }

    /// A turn started at 0 whose reply streams 40 bytes (10 tokens) every
    /// 100 ms from 300 ms to 3 s: 100 tok/s.
    fn steady(meter: &mut Meter) -> TurnId {
        let turn = TurnId::new();
        let started = Event::TurnStarted {
            turn,
            seq: 1,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        };
        meter.apply(&started, ms(0));
        let item = ItemId::new();
        for at in (300..=3000).step_by(100) {
            let text = "x".repeat(40);
            meter.apply(&Event::TextDelta { item, text }, ms(at));
        }
        turn
    }

    #[test]
    fn live_rate_of_a_steady_stream_is_within_five_percent() {
        let mut meter = Meter::default();
        steady(&mut meter);
        let turn = meter.view().turn.clone().expect("turn");
        let rate = turn.tok_per_s.expect("rate");
        assert!((rate - 100.0).abs() <= 5.0, "rate {rate}");
        assert_eq!((turn.ttft_ms, turn.exact), (Some(300), false));
        // Every delta after the first adds a point: 27, under the cap.
        assert_eq!(turn.sparkline.len(), 27);
        assert!(turn.sparkline.iter().all(|r| (r - 100.0).abs() <= 5.0));
    }

    #[test]
    fn usage_replaces_the_estimate_with_the_exact_rate_and_sums_the_ledger() {
        let mut meter = Meter::default();
        let turn = steady(&mut meter);
        // 540 tokens over the 2.7 s from the first delta to the usage…
        meter.apply(
            &Event::Usage {
                turn,
                usage: usage(540, 3100),
            },
            ms(3000),
        );
        let first = meter.view().turn.as_ref().and_then(|t| t.tok_per_s);
        assert_eq!(first, Some(200.0));
        // …then a call that streamed no text: its ledger latency.
        meter.apply(
            &Event::Usage {
                turn,
                usage: usage(60, 600),
            },
            ms(3900),
        );
        let done = Event::TurnDone {
            turn,
            stop: StopReason::EndTurn,
        };
        assert!(meter.apply(&done, ms(4000)));
        let view = meter.view();
        let t = view.turn.clone().expect("turn");
        assert_eq!((t.tok_per_s, t.exact, t.done), (Some(100.0), true, true));
        let tally = Tally {
            sent: 2100,
            received: 600,
            cache_read: 1800,
            cache_write: 100,
            uncached: 200,
            cost_usd: 0.04,
            calls: 2,
            estimated: false,
        };
        assert_eq!(
            (t.tally, view.session, view.context_tokens),
            (tally, tally, 1050)
        );
        // 600 tokens over 2.7 s + 0.6 s; the peak is the first call's exact rate.
        let text = &view.text;
        assert_eq!(
            (text.rate.as_str(), text.heading.as_str()),
            ("100", "Last turn · 2 requests")
        );
        assert_eq!(
            text.rate_detail,
            "avg 182 tok/s · first token 300 ms · peak 200 tok/s"
        );
    }

    #[test]
    fn a_replayed_rate_comes_from_the_ledger_latency() {
        let mut meter = Meter::default();
        let turn = TurnId::new();
        let started = Event::TurnStarted {
            turn,
            seq: 1,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        };
        for event in [
            started,
            Event::TextDelta {
                item: ItemId::new(),
                text: "hi".into(),
            },
            Event::Usage {
                turn,
                usage: usage(50, 500),
            },
        ] {
            meter.apply(&event, Duration::ZERO);
        }
        let t = meter.view().turn.clone().expect("turn");
        assert_eq!((t.tok_per_s, t.ttft_ms), (Some(100.0), Some(0)));
    }

    #[test]
    fn a_rate_over_a_near_zero_span_does_not_show() {
        let mut meter = Meter::default();
        let turn = TurnId::new();
        let started = Event::TurnStarted {
            turn,
            seq: 1,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        };
        meter.apply(&started, ms(0));
        // A scripted call: its deltas and usage microseconds apart, no latency.
        let item = ItemId::new();
        let us = Duration::from_micros;
        for at in 1..=5 {
            let text = "x".repeat(40);
            meter.apply(&Event::TextDelta { item, text }, us(at));
        }
        let call = Event::Usage {
            turn,
            usage: usage(21, 0),
        };
        meter.apply(&call, us(6));
        let view = meter.view();
        let t = view.turn.clone().expect("turn");
        assert_eq!((t.tok_per_s, t.sparkline.len()), (None, 0));
        assert_eq!(view.text.rate, "");
        // A burst under the floor falls back to the ledger latency.
        meter.apply(
            &Event::TextDelta {
                item,
                text: "y".into(),
            },
            ms(10),
        );
        meter.apply(
            &Event::Usage {
                turn,
                usage: usage(50, 500),
            },
            ms(20),
        );
        let t = meter.view().turn.clone().expect("turn");
        assert_eq!((t.tok_per_s, t.exact), (Some(100.0), true));
    }
}
