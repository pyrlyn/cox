// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The token meter and its popover as text (T37.25, DS§7–8, mockup 30):
//! every figure the desktop views show, formatted here so the UI does no
//! arithmetic on them. Separate from `usage.rs`, which folds events into
//! numbers; this only turns a folded `UsageView`, plus the few rates the fold
//! keeps aside, into strings.

use cox_protocol::types::ContextBreakdown;
use serde::{Deserialize, Serialize};

use crate::summary::{plural, seconds};
use crate::usage::{Tally, UsageView};

/// What the meter and the popover show, figure by figure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterText {
    /// Session ↑ sent, `218.5k`.
    pub sent: String,
    /// Session ↓ received, `9.8k`.
    pub received: String,
    /// The turn's tok/s, `71`; empty before the first rate.
    pub rate: String,
    /// The meter read aloud (DS§8): `218 thousand tokens sent, 9.8 thousand
    /// received, 71 tokens per second`.
    pub spoken: String,
    /// `This turn · 4 requests`, `Last turn · 1 request`; empty before a turn.
    pub heading: String,
    /// `streaming` while the turn runs, then `done`.
    pub phase: String,
    /// Beside the big rate: `tok/s now`, or `tok/s last call` once a call's
    /// exact figure replaced the estimate.
    pub rate_unit: String,
    /// `avg 64 tok/s · first token 800 ms · peak 77 tok/s`, what is known.
    pub rate_detail: String,
    /// The turn and session columns (`–` for the turn before one runs).
    pub rows: Vec<MeterRow>,
    /// `Context · 76.4k`, the last call's context.
    pub context: String,
    /// `7.6% of 1M`, that context's share of the model's window (A98);
    /// empty until a request was sent to a model with a known window.
    pub context_share: String,
    /// System, tools, instructions and history, in that order; empty
    /// until the first request.
    pub context_parts: Vec<ContextPart>,
    /// `923.6k`, what the window has left beside the parts (the context
    /// tab's `Free` row); empty while the window is unknown.
    pub context_free: String,
    /// `94% this turn`, the turn's cache reads over what it sent; empty
    /// before a turn sent anything.
    pub cache_hit: String,
    /// `88% this session`, every call's cache reads over what they sent
    /// (A104); empty before the session sent anything. `[desktop.context]
    /// cache_hit` picks which of the two the Context tab shows.
    pub cache_hit_session: String,
    /// `Cache hit 94% this turn · counts from the provider's usage, …`.
    pub footnote: String,
    /// `42%`, the share's percent without the window it is of; empty
    /// until a request was sent to a model with a known window. The
    /// toolbar copies this instead of splitting `context_share`.
    pub context_percent: String,
    /// The ring's fill: the parts' shares summed, capped at 1 so a
    /// used-over-window split cannot overflow.
    pub context_fill: f64,
    /// `$0.42`, the session's cost as the Cost row already writes it.
    pub cost: String,
}

/// One line of the popover's grid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterRow {
    pub label: String,
    pub turn: String,
    pub session: String,
    /// A breakdown of the row above it, drawn indented and quieter.
    pub detail: bool,
}

/// One part of the context bar and its legend row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextPart {
    /// `system`, `tools`, `instructions` or `history`: the part's colour
    /// role (`context.<kind>`).
    pub kind: String,
    /// `System`.
    pub label: String,
    /// `7.6k`.
    pub tokens: String,
    /// The part's width in the bar, a fraction of the window (of the whole
    /// context when the window is unknown).
    pub share: f64,
}

/// Figures the fold keeps beside `UsageView` for the popover alone.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Rates {
    /// The turn's received tokens over its calls' streaming time.
    pub avg: Option<f64>,
    pub peak: Option<f64>,
    pub session_thinking: u32,
    pub context: Option<ContextBreakdown>,
}

impl MeterText {
    pub(crate) fn of(view: &UsageView, rates: Rates) -> Self {
        let s = &view.session;
        let turn = view.turn.as_ref();
        let rate = turn.and_then(|t| t.tok_per_s).map(per_second);
        let mut spoken = format!(
            "{} tokens sent, {} received",
            spoken(s.sent),
            spoken(s.received)
        );
        if let Some(r) = &rate {
            spoken += &format!(", {r} tokens per second");
        }
        let detail = [
            rates.avg.map(|r| format!("avg {} tok/s", per_second(r))),
            turn.and_then(|t| t.ttft_ms)
                .map(|ms| format!("first token {}", seconds(ms))),
            rates.peak.map(|r| format!("peak {} tok/s", per_second(r))),
        ];
        let t = turn.map(|t| (t.tally, t.thinking_tokens));
        let col = |f: fn(&Tally) -> String| t.map_or_else(|| "–".into(), |(t, _)| f(&t));
        let row = |label: &str, f: fn(&Tally) -> String, detail| MeterRow {
            label: label.into(),
            turn: col(f),
            session: f(s),
            detail,
        };
        let rows = vec![
            row("↑ Sent", |t| tokens(t.sent), false),
            row("cache read", |t| tokens(t.cache_read), true),
            row("cache write", |t| tokens(t.cache_write), true),
            row("uncached", |t| tokens(t.uncached), true),
            row("↓ Received", |t| tokens(t.received), false),
            MeterRow {
                label: "of it thinking".into(),
                turn: t.map_or_else(|| "–".into(), |(_, n)| tokens(n)),
                session: tokens(rates.session_thinking),
                detail: true,
            },
            row("Cost", |t| cost(t.cost_usd), false),
        ];
        let source = if s.estimated {
            "some counts are cox's estimate"
        } else {
            "counts from the provider's usage, one ledger row per request"
        };
        let cache_hit = t.and_then(|(t, _)| hit(&t, "this turn"));
        let footnote = match &cache_hit {
            Some(hit) => format!("Cache hit {hit} · {source}"),
            None => format!("C{}", &source[1..]),
        };
        let context_parts = rates
            .context
            .map(|b| parts(&b, view.context_tokens))
            .unwrap_or_default();
        let context_fill = context_parts.iter().map(|p| p.share).sum::<f64>().min(1.0);
        Self {
            sent: tokens(s.sent),
            received: tokens(s.received),
            rate: rate.unwrap_or_default(),
            spoken,
            heading: turn.map_or_else(String::new, |t| {
                let when = if t.done { "Last" } else { "This" };
                let calls = plural(u64::from(t.tally.calls), "request", "requests");
                format!("{when} turn · {calls}")
            }),
            phase: turn
                .map_or("", |t| if t.done { "done" } else { "streaming" })
                .into(),
            rate_unit: if turn.is_some_and(|t| t.exact) {
                "tok/s last call"
            } else {
                "tok/s now"
            }
            .into(),
            rate_detail: detail.into_iter().flatten().collect::<Vec<_>>().join(" · "),
            rows,
            context: format!("Context · {}", tokens(view.context_tokens)),
            context_share: rates
                .context
                .and_then(|b| share(&b, view.context_tokens))
                .unwrap_or_default(),
            context_parts,
            context_free: rates
                .context
                .and_then(|b| free(&b, view.context_tokens))
                .unwrap_or_default(),
            cache_hit: cache_hit.unwrap_or_default(),
            cache_hit_session: hit(s, "this session").unwrap_or_default(),
            footnote,
            context_percent: rates
                .context
                .and_then(|b| percent(&b, view.context_tokens))
                .unwrap_or_default(),
            context_fill,
            cost: cost(s.cost_usd),
        }
    }
}

/// `94% this turn`: `t`'s cache reads over what it sent; `None` before it
/// sent anything.
fn hit(t: &Tally, span: &str) -> Option<String> {
    (t.sent > 0).then(|| {
        let pct = f64::from(t.cache_read) / f64::from(t.sent) * 100.0;
        format!("{pct:.0}% {span}")
    })
}

/// The context the split is scaled to: the last call's reported context
/// (the `Context` figure), or the core's estimate before any call reported.
fn used(b: &ContextBreakdown, last_context: u32) -> u32 {
    if last_context > 0 {
        last_context
    } else {
        b.total
    }
}

/// `7.6%` or `42%`; `None` when the window is unknown.
fn percent(b: &ContextBreakdown, last_context: u32) -> Option<String> {
    let window = b.window.filter(|w| *w > 0)?;
    let pct = f64::from(used(b, last_context)) / f64::from(window) * 100.0;
    Some(if pct < 10.0 {
        format!("{pct:.1}%")
    } else {
        format!("{pct:.0}%")
    })
}

/// `7.6% of 1M`; `None` when the window is unknown.
fn share(b: &ContextBreakdown, last_context: u32) -> Option<String> {
    let window = b.window.filter(|w| *w > 0)?;
    Some(format!(
        "{} of {}",
        percent(b, last_context)?,
        window_size(window)
    ))
}

/// `$0.42`, as the Cost row and the toolbar both write a session cost.
fn cost(usd: f64) -> String {
    format!("${usd:.2}")
}

/// The window less `used`; `None` when the window is unknown.
fn free(b: &ContextBreakdown, last_context: u32) -> Option<String> {
    let window = b.window.filter(|w| *w > 0)?;
    Some(tokens(window.saturating_sub(used(b, last_context))))
}

/// The core's split scaled to `used`, so the legend sums to the `Context`
/// figure beside it and the bar's filled length is the share.
fn parts(b: &ContextBreakdown, last_context: u32) -> Vec<ContextPart> {
    let used = f64::from(used(b, last_context));
    let scale = used / f64::from(b.total.max(1));
    let whole = b.window.filter(|w| *w > 0).map_or(used, f64::from).max(1.0);
    [
        ("system", "System", b.system),
        ("tools", "Tools", b.tools),
        ("instructions", "Instructions", b.instructions),
        ("history", "History", b.history),
    ]
    .into_iter()
    .map(|(kind, label, n)| {
        let n = f64::from(n) * scale;
        ContextPart {
            kind: kind.into(),
            label: label.into(),
            tokens: tokens(n.round() as u32),
            share: n / whole,
        }
    })
    .collect()
}

/// A window as it is marketed: `1M`, `200k`, else as `tokens` shows it.
fn window_size(n: u32) -> String {
    match n {
        _ if n >= 1_000_000 && n.is_multiple_of(1_000_000) => format!("{}M", n / 1_000_000),
        _ if n < 1_000_000 && n.is_multiple_of(1000) => format!("{}k", n / 1000),
        _ => tokens(n),
    }
}

/// `950`, `9.8k`, `218.5k`, `1.2M`; the cost history's figures too.
pub(crate) fn tokens(n: u32) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", f64::from(n) / 1e3),
        _ => format!("{:.1}M", f64::from(n) / 1e6),
    }
}

/// `tokens` as VoiceOver should say it: `950`, `9.8 thousand`,
/// `218 thousand`, `1.2 million`.
fn spoken(n: u32) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..10_000 => format!("{:.1} thousand", f64::from(n) / 1e3),
        10_000..1_000_000 => format!("{} thousand", n / 1000),
        _ => format!("{:.1} million", f64::from(n) / 1e6),
    }
}

/// A rate in whole tokens per second, `71`.
fn per_second(rate: f64) -> String {
    format!("{rate:.0}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cox_protocol::ids::TurnId;
    use cox_protocol::types::Usage;

    use super::*;
    use crate::usage::TurnUsage;

    fn tally(sent: u32, received: u32) -> Tally {
        Tally {
            sent,
            received,
            cache_read: sent / 10 * 9,
            cache_write: 0,
            uncached: sent / 10,
            cost_usd: 0.4249,
            calls: 4,
            estimated: false,
        }
    }

    #[test]
    fn numbers_read_as_the_mockup_and_ds_8_say() {
        let cases = [(950, "950", "950"), (9_800, "9.8k", "9.8 thousand")];
        for (n, shown, said) in cases {
            assert_eq!((tokens(n), spoken(n)), (shown.into(), said.into()));
        }
        assert_eq!(
            (tokens(218_500), spoken(218_500)),
            ("218.5k".into(), "218 thousand".into())
        );
        assert_eq!(tokens(1_200_000), "1.2M");
    }

    #[test]
    fn a_streaming_turn_formats_every_figure_the_popover_shows() {
        let mut session = tally(218_500, 9_800);
        session.cache_read = 190_000;
        let view = UsageView {
            session,
            turn: Some(TurnUsage {
                turn: TurnId::new(),
                tally: tally(41_600, 1_900),
                thinking_tokens: 600,
                ttft_ms: Some(800),
                tok_per_s: Some(71.4),
                exact: false,
                sparkline: vec![],
                done: false,
            }),
            context_tokens: 76_400,
            text: MeterText::default(),
        };
        let rates = Rates {
            avg: Some(64.2),
            peak: Some(77.0),
            session_thinking: 3_100,
            context: None,
        };
        let text = MeterText::of(&view, rates);
        assert_eq!(
            text.spoken,
            "218 thousand tokens sent, 9.8 thousand received, 71 tokens per second"
        );
        assert_eq!(
            (text.sent, text.received, text.rate),
            ("218.5k".into(), "9.8k".into(), "71".into())
        );
        assert_eq!(
            (text.heading.as_str(), text.phase.as_str()),
            ("This turn · 4 requests", "streaming")
        );
        assert_eq!(
            text.rate_detail,
            "avg 64 tok/s · first token 800 ms · peak 77 tok/s"
        );
        let thinking = &text.rows[5];
        assert_eq!(
            (thinking.turn.as_str(), thinking.session.as_str()),
            ("600", "3.1k")
        );
        assert_eq!(
            (text.rows[6].turn.as_str(), text.rows[6].session.as_str()),
            ("$0.42", "$0.42")
        );
        assert_eq!(text.context, "Context · 76.4k");
        assert_eq!(text.cache_hit, "90% this turn");
        assert_eq!(text.cache_hit_session, "87% this session");
        assert!(
            text.footnote
                .starts_with("Cache hit 90% this turn · counts"),
            "{}",
            text.footnote
        );
    }

    /// A98: the Meter keeps the latest split, and the text scales it to
    /// the last call's context so the legend sums to the `Context` figure.
    #[test]
    fn the_context_split_is_scaled_to_the_last_call_and_shared_of_the_window() {
        let turn = TurnId::new();
        let mut breakdown = ContextBreakdown {
            window: Some(1_000_000),
            total: 100_000,
            system: 10_000,
            tools: 30_000,
            instructions: 10_000,
            history: 50_000,
            cached: 0,
        };
        let mut meter = crate::usage::Meter::default();
        let split = |b| cox_protocol::types::Event::ContextBreakdown { turn, breakdown: b };
        assert!(meter.apply(&split(breakdown), Duration::ZERO));
        let text = &meter.view().text;
        assert_eq!(
            text.context_share, "10% of 1M",
            "the estimate before a reply"
        );
        assert_eq!(text.context_parts[3].tokens, "50.0k");
        let usage = Usage {
            input_tokens: 400,
            output_tokens: 10,
            cache_read_tokens: 76_000,
            cache_write_tokens: 0,
            estimated: false,
            cost_usd: 0.0,
            latency_ms: 1,
        };
        meter.apply(
            &cox_protocol::types::Event::Usage { turn, usage },
            Duration::ZERO,
        );
        let text = &meter.view().text;
        assert_eq!(text.context, "Context · 76.4k");
        assert_eq!(text.context_share, "7.6% of 1M");
        assert_eq!(text.context_free, "923.6k", "the window less the context");
        let shown: Vec<_> = text
            .context_parts
            .iter()
            .map(|p| (p.kind.as_str(), p.label.as_str(), p.tokens.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("system", "System", "7.6k"),
                ("tools", "Tools", "22.9k"),
                ("instructions", "Instructions", "7.6k"),
                ("history", "History", "38.2k"),
            ]
        );
        let filled: f64 = text.context_parts.iter().map(|p| p.share).sum();
        assert!((filled - 0.0764).abs() < 1e-9, "{filled}");

        breakdown.window = None;
        meter.apply(&split(breakdown), Duration::ZERO);
        let text = &meter.view().text;
        assert_eq!(text.context_share, "", "no window, no share");
        assert_eq!(text.context_free, "", "no window, nothing free");
        let filled: f64 = text.context_parts.iter().map(|p| p.share).sum();
        assert!((filled - 1.0).abs() < 1e-9, "the bar is the whole context");
        assert_eq!(window_size(200_000), "200k");
        assert_eq!(window_size(262_144), "262.1k");
    }

    #[test]
    fn before_any_turn_the_turn_column_is_a_dash_and_no_rate_is_spoken() {
        let text = MeterText::of(&UsageView::default(), Rates::default());
        assert_eq!(text.spoken, "0 tokens sent, 0 received");
        assert_eq!((text.rate.as_str(), text.heading.as_str()), ("", ""));
        assert_eq!(text.cache_hit, "", "no turn, no cache hit");
        assert_eq!(text.cache_hit_session, "", "nothing sent, no cache hit");
        assert!(text.rows.iter().all(|r| r.turn == "–"));
        assert!(text.footnote.starts_with("Counts from"));
    }

    #[test]
    fn context_fill_is_capped_at_one() {
        let view = UsageView {
            context_tokens: 2_000,
            ..UsageView::default()
        };
        let text = MeterText::of(
            &view,
            Rates {
                context: Some(ContextBreakdown {
                    window: Some(1_000),
                    total: 2_000,
                    system: 500,
                    tools: 500,
                    instructions: 500,
                    history: 500,
                    cached: 0,
                }),
                ..Rates::default()
            },
        );
        let parts: f64 = text.context_parts.iter().map(|p| p.share).sum();
        assert!(parts > 1.0, "{parts}");
        assert_eq!(text.context_fill, 1.0);
        assert_eq!(text.context_percent, "200%");
        assert_eq!(text.cost, "$0.00");
    }
}
