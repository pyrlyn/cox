// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Generic Server-Sent-Events framing, shared by every provider that
//! streams over SSE (today: Anthropic; OpenAI Responses in T1.3 reuses it).
//! Kept separate from `anthropic::stream` so the wire-framing bug class
//! (multi-line `data:`, split chunks, missing `event:`) is tested once
//! instead of once per provider.

use bytes::Bytes;
use eventsource_stream::{Event as SseEvent, EventStreamError, Eventsource};
use futures::{Stream, StreamExt};

/// One SSE frame, reduced to what a state machine needs: the event name
/// (`None` when the frame carried no explicit `event:` field — the SSE spec
/// calls that the default "message" type) and the `data:` payload, with
/// multi-line `data:` fields already joined by `\n` per spec.
pub type SseFrame = (Option<String>, String);

fn to_frame(e: SseEvent) -> SseFrame {
    // eventsource-stream fills in the spec default itself, so "message"
    // here means "no event: line was sent", same as an absent field.
    let event = if e.event.is_empty() || e.event == "message" {
        None
    } else {
        Some(e.event)
    };
    (event, e.data)
}

/// Wraps a byte stream — a `reqwest` response body via `.bytes_stream()` —
/// into a stream of SSE frames. Generic over the byte stream's error type
/// so a provider client's `stream()` can map failures into its own
/// `ProviderError` without this module knowing about `reqwest`.
pub fn sse_stream<S, E>(bytes: S) -> impl Stream<Item = Result<SseFrame, EventStreamError<E>>>
where
    S: Stream<Item = Result<Bytes, E>>,
{
    bytes.eventsource().map(|frame| frame.map(to_frame))
}

/// Parses a whole SSE body already in memory: fixtures and tests, no
/// network. Runs the same parser as [`sse_stream`] (one in-memory chunk fed
/// through the identical `eventsource-stream` state machine), so a fixture
/// that parses here behaves exactly like the live path.
pub fn parse_sse_str(body: &str) -> Vec<SseFrame> {
    let chunk: Result<Bytes, std::convert::Infallible> =
        Ok(Bytes::copy_from_slice(body.as_bytes()));
    let one_shot = futures::stream::iter(vec![chunk]);
    futures::executor::block_on(
        one_shot
            .eventsource()
            .filter_map(|frame| async move { frame.ok().map(to_frame) })
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_events_blank_line_separated() {
        let body = "event: message_start\ndata: {\"a\":1}\n\nevent: message_stop\ndata: {}\n\n";
        let frames = parse_sse_str(body);
        assert_eq!(
            frames,
            vec![
                (Some("message_start".into()), "{\"a\":1}".into()),
                (Some("message_stop".into()), "{}".into()),
            ]
        );
    }

    #[test]
    fn joins_multiline_data_fields() {
        let body = "event: x\ndata: line one\ndata: line two\n\n";
        let frames = parse_sse_str(body);
        assert_eq!(
            frames,
            vec![(Some("x".into()), "line one\nline two".into())]
        );
    }

    #[test]
    fn frame_with_no_event_field_is_none() {
        let body = "data: bare\n\n";
        let frames = parse_sse_str(body);
        assert_eq!(frames, vec![(None, "bare".into())]);
    }
}
