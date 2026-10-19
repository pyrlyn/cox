// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/goal` stop condition (T67.4). Pure: the goal, how a judge answer
//! becomes a verdict, and the side request the cheap tier sees. The
//! session owns the loop. An answer that is not one of the three tags
//! is `NotMet`, never `Met`.

use cox_protocol::types::Job;

use crate::side::Side;

/// The instruction the judge sees. No tools are attached to the request.
const SYSTEM: &str = "You judge whether a goal is met. You see only the condition and a \
    transcript summary. Reply with exactly one line and nothing else: \
    `MET: <reason>`, `NOTMET: <reason>`, or `IMPOSSIBLE: <reason>`.";

/// One active goal. `max_turns` is the autonomous run's turn cap (T66.9).
pub(crate) struct Goal {
    pub condition: String,
    pub turns_used: u32,
    pub max_turns: u32,
}

/// The judge's answer, or a loop stop the session adds itself.
#[derive(Debug)]
pub(crate) enum Verdict {
    Met(String),
    NotMet(String),
    Impossible(String),
}

/// `MET:` / `NOTMET:` / `NOT_MET:` / `IMPOSSIBLE:`, case-insensitive.
/// Anything else, including an empty `MET:`, is [`Verdict::NotMet`].
pub(crate) fn parse_verdict(text: &str) -> Verdict {
    if let Some(reason) = tagged(text, "met:") {
        if reason.is_empty() {
            return Verdict::NotMet("unparseable".into());
        }
        return Verdict::Met(reason.to_string());
    }
    if let Some(reason) = tagged(text, "impossible:") {
        let reason = if reason.is_empty() {
            "impossible"
        } else {
            reason
        };
        return Verdict::Impossible(reason.to_string());
    }
    if let Some(reason) = tagged(text, "notmet:").or_else(|| tagged(text, "not_met:")) {
        let reason = if reason.is_empty() { "not met" } else { reason };
        return Verdict::NotMet(reason.to_string());
    }
    let reason = text.trim();
    Verdict::NotMet(if reason.is_empty() {
        "unparseable".into()
    } else {
        reason.to_string()
    })
}

/// The cheap side request. `summary` is the condition plus the last turn;
/// the caller owns that string for the duration of the call.
pub(crate) fn evaluator_request<'a>(goal: &'a Goal, summary: &'a str) -> Side<'a> {
    Side {
        job: Job::Summarize,
        turn: goal.turns_used,
        system: SYSTEM,
        text: summary,
        max_tokens: 128,
        max_chars: 400,
    }
}

fn tagged<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let text = text.trim();
    if text.len() >= tag.len() && text[..tag.len()].eq_ignore_ascii_case(tag) {
        Some(text[tag.len()..].trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unparseable_verdict_is_not_met() {
        assert!(matches!(parse_verdict("maybe later"), Verdict::NotMet(_)));
        assert!(matches!(parse_verdict("MET:"), Verdict::NotMet(_)));
        assert!(matches!(parse_verdict(""), Verdict::NotMet(_)));
        assert!(matches!(
            parse_verdict("MET without a colon"),
            Verdict::NotMet(_)
        ));
    }

    #[test]
    fn tagged_lines_keep_the_reason() {
        match parse_verdict("MET: tests pass") {
            Verdict::Met(reason) => assert_eq!(reason, "tests pass"),
            other => panic!("met, got {other:?}"),
        }
        match parse_verdict("not_met: still open") {
            Verdict::NotMet(reason) => assert_eq!(reason, "still open"),
            other => panic!("not met, got {other:?}"),
        }
        match parse_verdict("Impossible: no such file") {
            Verdict::Impossible(reason) => assert_eq!(reason, "no such file"),
            other => panic!("impossible, got {other:?}"),
        }
    }
}
