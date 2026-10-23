// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/goal` (T67.4): a condition the cheap tier judges after each turn.
//! Pure so the parser and the prompt can be tested without a provider.
//! The session owns the loop; this module only decides what the judge is
//! asked and what an answer means. An answer that does not parse is
//! `NotMet`, never `Met`, so a confused judge cannot stop the loop early.

use cox_protocol::types::{Content, Job, Message, Role};

use crate::side::Side;

/// How many characters of the last turn the judge is shown.
const SUMMARY_CHARS: usize = 4_000;

/// The judge's only allowed answers. The leading tag is matched
/// case-insensitively; anything else is `NotMet`.
const JUDGE_SYSTEM: &str = "\
You judge whether a goal already holds. Reply with one line and nothing else:\n\
MET: <reason>\n\
NOT_MET: <reason>\n\
IMPOSSIBLE: <reason>\n\
Reply MET only when the last turn shows the goal already holds. If you are unsure, reply NOT_MET.";

/// A goal the session is working toward. `max_turns` and `max_tokens`
/// come from the autonomous policy (T66.9) rather than a second cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Goal {
    pub condition: String,
    pub turns_used: u32,
    pub max_turns: u32,
    pub tokens_used: u64,
    pub max_tokens: u64,
}

/// What the judge said. The string is the reason, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Met(String),
    NotMet(String),
    Impossible(String),
}

impl Verdict {
    /// The `goal_verdict` tag on the wire.
    pub(crate) fn tag(&self) -> &'static str {
        match self {
            Self::Met(_) => "met",
            Self::NotMet(_) => "not_met",
            Self::Impossible(_) => "impossible",
        }
    }

    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Met(reason) | Self::NotMet(reason) | Self::Impossible(reason) => reason,
        }
    }
}

/// Why the loop must not start another turn. `None` means the judge
/// still gets to answer.
pub(crate) fn limit_hit(goal: &Goal) -> Option<&'static str> {
    if goal.turns_used >= goal.max_turns {
        Some("max_turns")
    } else if goal.max_tokens > 0 && goal.tokens_used >= goal.max_tokens {
        Some("max_tokens")
    } else {
        None
    }
}

/// The last assistant text, clipped. Tool noise is left out: the judge
/// is shown what the model said, not the raw tool payloads.
pub(crate) fn summary_of(history: &[Message]) -> String {
    let text = history.iter().rev().find_map(|message| {
        if message.role != Role::Assistant {
            return None;
        }
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (!text.is_empty()).then_some(text)
    });
    clip(
        &text.unwrap_or_else(|| "(no assistant text)".to_string()),
        SUMMARY_CHARS,
    )
}

/// The user message the judge sees: the condition and the summary, and
/// nothing that could be read as a tool instruction.
pub(crate) fn judge_prompt(goal: &Goal, summary: &str) -> String {
    format!("Goal:\n{}\n\nLast turn:\n{summary}", goal.condition)
}

/// The next user turn when the judge said the goal is not met yet.
pub(crate) fn continue_prompt(goal: &Goal, reason: &str) -> String {
    format!(
        "The goal is not met yet.\n\nGoal: {}\n\nJudge: {reason}\n\nContinue until the goal is met.",
        goal.condition
    )
}

/// One cheap side request. `side_call` sends it with an empty tool list,
/// so the judge cannot call tools even if the text asked it to.
pub(crate) fn evaluator_request<'a>(goal: &'a Goal, last_turn_summary: &'a str) -> Side<'a> {
    Side {
        job: Job::Summarize,
        turn: goal.turns_used,
        system: JUDGE_SYSTEM,
        text: last_turn_summary,
        max_tokens: 256,
        max_chars: 2_000,
    }
}

/// `MET` / `NOT_MET` / `IMPOSSIBLE`, or one JSON object with `verdict`
/// and `reason`. Anything else, including an empty answer, is `NotMet`.
pub(crate) fn parse_verdict(raw: &str) -> Verdict {
    let text = raw.trim();
    if text.is_empty() {
        return Verdict::NotMet("unparseable verdict".into());
    }
    if let Some(verdict) = parse_json(text) {
        return verdict;
    }
    let (tag, reason) = text
        .split_once(':')
        .map(|(tag, reason)| (tag.trim(), reason.trim()))
        .unwrap_or((text, ""));
    let reason = if reason.is_empty() {
        "unparseable verdict".into()
    } else {
        reason.to_string()
    };
    match tag.to_ascii_uppercase().as_str() {
        "MET" => Verdict::Met(reason),
        "NOT_MET" | "NOT MET" => Verdict::NotMet(reason),
        "IMPOSSIBLE" => Verdict::Impossible(reason),
        _ => Verdict::NotMet(clip(text, 500)),
    }
}

fn parse_json(text: &str) -> Option<Verdict> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let tag = value.get("verdict")?.as_str()?.trim();
    let reason = value
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let reason = if reason.is_empty() {
        "unparseable verdict".into()
    } else {
        reason
    };
    match tag.to_ascii_lowercase().as_str() {
        "met" => Some(Verdict::Met(reason)),
        "not_met" | "not met" => Some(Verdict::NotMet(reason)),
        "impossible" => Some(Verdict::Impossible(reason)),
        _ => Some(Verdict::NotMet(clip(text, 500))),
    }
}

fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.len() + ch.len_utf8() > max {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal() -> Goal {
        Goal {
            condition: "tests pass".into(),
            turns_used: 2,
            max_turns: 4,
            tokens_used: 10,
            max_tokens: 100,
        }
    }

    #[test]
    fn parse_verdict_accepts_the_three_lines_and_json() {
        assert_eq!(
            parse_verdict("MET: the tests passed"),
            Verdict::Met("the tests passed".into())
        );
        assert_eq!(
            parse_verdict("not_met: still failing"),
            Verdict::NotMet("still failing".into())
        );
        assert_eq!(
            parse_verdict("IMPOSSIBLE: no repository"),
            Verdict::Impossible("no repository".into())
        );
        assert_eq!(
            parse_verdict(r#"{"verdict":"met","reason":"done"}"#),
            Verdict::Met("done".into())
        );
    }

    #[test]
    fn parse_verdict_treats_anything_else_as_not_met() {
        assert_eq!(
            parse_verdict("I think it is done"),
            Verdict::NotMet("I think it is done".into())
        );
        assert!(matches!(parse_verdict("   "), Verdict::NotMet(_)));
        assert!(matches!(
            parse_verdict(r#"{"verdict":"allow","reason":"sure"}"#),
            Verdict::NotMet(_)
        ));
        assert!(!matches!(parse_verdict("maybe MET"), Verdict::Met(_)));
    }

    #[test]
    fn limit_hit_prefers_turns_then_tokens() {
        let mut goal = goal();
        assert_eq!(limit_hit(&goal), None);
        goal.turns_used = 4;
        assert_eq!(limit_hit(&goal), Some("max_turns"));
        goal.turns_used = 1;
        goal.tokens_used = 100;
        assert_eq!(limit_hit(&goal), Some("max_tokens"));
    }

    #[test]
    fn evaluator_request_is_a_cheap_summary_with_the_prompt_verbatim() {
        let goal = goal();
        let prompt = judge_prompt(&goal, "added the test");
        let side = evaluator_request(&goal, &prompt);
        assert_eq!(side.job, Job::Summarize);
        assert!(side.system.contains("NOT_MET"));
        assert!(side.text.contains("tests pass"));
        assert!(side.text.contains("added the test"));
        assert!(!side.system.contains("tests pass"));
    }
}
