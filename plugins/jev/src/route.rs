//! The J§5.2 `route` advisor (T33.40.9): what one System One request for a
//! main turn asks, and how its answers become "run this turn on `cheap`" or
//! no advice. Separate from `decide`, which only moves a request to the host
//! and a reply back, and from `risk`, whose question set and gate differ.
//! Pure, so it tests on the host.
//!
//! Downgrade only: the core offers tiers at or below its static pick and
//! never `think` (D5), and this guest answers only with `cheap`. Anything
//! short of a confident, shallow `cheap` after a turn that did not fail is
//! silence, so the static pick stands.

use std::collections::BTreeMap;

use cox_plugin_sdk::{Advice, Answer, Question as Asked, Value};
use serde_json::json;

use crate::wire::{self, Question};

/// The fields of the core's state a question may carry. A field the core
/// adds later stays out until this list names it: the request must never
/// carry tool output (J5.2).
const STATE: [&str; 4] = ["prompt", "todo", "last_turn", "files_touched_count"];

/// The tier this advisor may name.
const CHEAP: &str = "cheap";
/// The option the model picks when neither tier fits; it reads as `code`.
const OTHER: &str = "other";

/// The J§5.2 gate: a downgrade needs this Choice confidence (J11: a
/// high-stakes action) and a `wants_depth` below the ceiling.
const MIN_CONFIDENCE: f64 = 0.8;
const MAX_DEPTH: f64 = 0.3;

/// What each tier is for, as the plugin describes it to the model (J5.2).
fn describe(tier: &str) -> &str {
    match tier {
        CHEAP => "mechanical: read, print, list, rename, run a named command, answer from context",
        "code" => "reasoning over code: multi-file edits, debugging, design",
        OTHER => "anything else; treated as code",
        other => other,
    }
}

/// Whether a `route` question is worth a request: the core offered `cheap`.
pub fn offers_cheap(asked: &Asked) -> bool {
    asked.options.iter().any(|o| o == CHEAP)
}

/// The request's `state`: the allowed fields of the core's state, nothing
/// else.
pub fn state(asked: &Asked) -> Value {
    Value::Object(
        STATE
            .iter()
            .filter_map(|k| Some((k.to_string(), asked.state.get(k)?.clone())))
            .collect(),
    )
}

/// The `tier` Choice over the offered tiers plus `other`, and the
/// `wants_depth` Noul.
pub fn questions(asked: &Asked) -> BTreeMap<String, Question> {
    let criteria = asked
        .options
        .iter()
        .map(String::as_str)
        .chain([OTHER])
        .map(|o| (o.to_string(), json!(describe(o))))
        .collect();
    BTreeMap::from([
        (
            "tier".to_string(),
            Question::Choice {
                instructions: json!(
                    "Which model tier should handle the user's next request in state.prompt?"
                ),
                criteria,
            },
        ),
        (
            "wants_depth".to_string(),
            Question::Noul {
                instructions: json!("Does the user ask for careful or deep work?"),
                criteria: None,
            },
        ),
    ])
}

/// The advice for `asked`: `cheap` when the reply chose it with confidence
/// of at least [`MIN_CONFIDENCE`], `wants_depth` stays below [`MAX_DEPTH`]
/// and the last turn did not end in an error; `None` otherwise.
pub fn verdict(answers: &BTreeMap<String, wire::Answer>, asked: &Asked) -> Option<Advice> {
    let errored = asked.state["last_turn"]["errored"].as_bool() != Some(false);
    let Some(wire::Answer::Choice {
        choice, confidence, ..
    }) = answers.get("tier")
    else {
        return None;
    };
    // A missing or broken `wants_depth` is no evidence the work is shallow.
    let depth = match answers.get("wants_depth") {
        Some(wire::Answer::Noul { noul, .. }) if noul.is_finite() => *noul,
        _ => 1.0,
    };
    let index = asked.options.iter().position(|o| o == CHEAP)?;
    let index = u32::try_from(index).ok()?;
    let cheap = choice == CHEAP && *confidence >= MIN_CONFIDENCE && depth < MAX_DEPTH;
    // No note: the core keeps a `route` note out of the rollout.
    (cheap && !errored).then(|| Advice {
        answer: Answer::Choice { order: vec![index] },
        confidence: Some(*confidence),
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use cox_plugin_sdk::DecidePoint;

    use super::*;

    fn asked(errored: bool) -> Asked {
        Asked {
            point: DecidePoint::Route,
            state: json!({
                "static": "code",
                "prompt": "list the files",
                "todo": [{ "text": "list", "state": "in_progress" }],
                "last_turn": { "tools": ["bash"], "errors": 0, "errored": errored },
                "files_touched_count": 2,
                "output": "SECRET-OUTPUT",
            }),
            options: vec!["cheap".into(), "code".into()],
            items: vec![],
        }
    }

    fn reply(choice: &str, confidence: f64, depth: f64) -> BTreeMap<String, wire::Answer> {
        BTreeMap::from([
            (
                "tier".to_string(),
                wire::Answer::Choice {
                    choice: choice.into(),
                    probabilities: BTreeMap::new(),
                    confidence,
                },
            ),
            (
                "wants_depth".to_string(),
                wire::Answer::Noul {
                    noul: depth,
                    certainty: (2.0 * depth - 1.0).abs(),
                },
            ),
        ])
    }

    #[test]
    fn route_needs_high_confidence() {
        let q = asked(false);
        let advice = verdict(&reply("cheap", 0.8, 0.1), &q).expect("a downgrade");
        assert_eq!(advice.answer, Answer::Choice { order: vec![0] });
        assert_eq!(advice.confidence, Some(0.8));
        assert!(q.fits(&advice.answer));
        assert_eq!(verdict(&reply("cheap", 0.79, 0.1), &q), None);
        // Confident, but the user asks for depth.
        assert_eq!(verdict(&reply("cheap", 0.95, 0.3), &q), None);
        assert_eq!(verdict(&reply("cheap", 0.95, f64::NAN), &q), None);
        let mut no_depth = reply("cheap", 0.95, 0.0);
        no_depth.remove("wants_depth");
        assert_eq!(verdict(&no_depth, &q), None);
    }

    #[test]
    fn route_keeps_code_after_error_turn() {
        assert_eq!(verdict(&reply("cheap", 0.99, 0.0), &asked(true)), None);
        // A state with no last-turn report is no proof the turn went well.
        let mut q = asked(false);
        q.state = json!({ "prompt": "list the files" });
        assert_eq!(verdict(&reply("cheap", 0.99, 0.0), &q), None);
    }

    #[test]
    fn route_other_means_code() {
        let q = asked(false);
        for choice in ["other", "code", "think"] {
            assert_eq!(verdict(&reply(choice, 0.99, 0.0), &q), None, "{choice}");
        }
        let Question::Choice { criteria, .. } = &questions(&q)["tier"] else {
            panic!("a choice");
        };
        let names: Vec<_> = criteria.keys().map(String::as_str).collect();
        assert_eq!(names, ["cheap", "code", "other"]);
        assert_eq!(criteria["other"], "anything else; treated as code");
    }

    #[test]
    fn route_state_carries_only_the_turn_summary() {
        let state = state(&asked(false));
        assert!(!state.to_string().contains("SECRET"), "{state}");
        let keys: Vec<_> = state.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 4, "{keys:?}");
        assert_eq!(state["files_touched_count"], 2);
        assert_eq!(state["last_turn"]["tools"][0], "bash");
    }

    #[test]
    fn route_without_cheap_on_offer_is_not_asked() {
        let mut q = asked(false);
        assert!(offers_cheap(&q));
        q.options = vec!["code".into()];
        assert!(!offers_cheap(&q));
        assert_eq!(verdict(&reply("cheap", 0.99, 0.0), &q), None);
    }
}
