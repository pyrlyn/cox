//! The J§5.2 `route` advisor (T33.40.9): a main turn may move from `code`
//! down to `cheap`, and never the other way. Separate from `decide`, which
//! only moves a request to the host and a reply back. Pure, so it tests on
//! the host.
//!
//! Silence is the static pick. A downgrade goes out only when the model
//! chose `cheap` with confidence at least 0.8, `wants_depth` is below 0.3,
//! and the previous turn did not end in an error. `other`, and any name
//! that was not offered, means `code`.

use std::collections::BTreeMap;

use cox_plugin_sdk::{Advice, Answer, Question as Asked, Value};
use serde_json::json;

use crate::wire::{self, Question};

/// J5.2: a high-stakes downgrade needs this confidence (J11).
const MIN_CONFIDENCE: f64 = 0.8;
/// J5.2: at or above this P(yes), the user asked for careful work.
const MAX_DEPTH: f64 = 0.3;

/// Fields a `route` question may carry. Anything else (tool output, a
/// model-written note) stays out of the request (J5.2, J12).
const STATE: [&str; 5] = [
    "prompt",
    "todo",
    "last_turn",
    "files_touched_count",
    "static",
];

/// The request's `state`: only [`STATE`].
pub fn state(asked: &Asked) -> Value {
    Value::Object(
        STATE
            .iter()
            .filter_map(|k| Some((k.to_string(), asked.state.get(*k)?.clone())))
            .collect(),
    )
}

/// `tier` over the offered names plus `other`, and `wants_depth`. `None`
/// when `cheap` is not offered: there is nothing this point is allowed to
/// choose, so the host must not be asked.
pub fn questions(options: &[String]) -> Option<BTreeMap<String, Question>> {
    if !options.iter().any(|o| o == "cheap") {
        return None;
    }
    let mut criteria = BTreeMap::new();
    for option in options {
        criteria.insert(option.clone(), json!(describe(option)));
    }
    criteria.insert(
        "other".into(),
        json!("none of the offered tiers; treat as code"),
    );
    let mut out = BTreeMap::new();
    out.insert(
        "tier".into(),
        Question::Choice {
            instructions: json!("Which offered tier should run this turn?"),
            criteria,
        },
    );
    out.insert(
        "wants_depth".into(),
        Question::Noul {
            instructions: json!("the user asks for careful or deep work"),
            criteria: None,
        },
    );
    Some(out)
}

fn describe(option: &str) -> &'static str {
    match option {
        "cheap" => {
            "mechanical: read, print, list, rename, run a named command, answer from context"
        }
        "code" => "reasoning over code: multi-file edits, debugging, design",
        _ => "not a downgrade this point may take",
    }
}

/// Advice that selects `cheap` in `asked.options`, or `None` so the core
/// keeps the static pick.
pub fn verdict(answers: &BTreeMap<String, wire::Answer>, asked: &Asked) -> Option<Advice> {
    if last_turn_errored(&asked.state) {
        return None;
    }
    let (choice, confidence, probabilities) = match answers.get("tier") {
        Some(wire::Answer::Choice {
            choice,
            confidence,
            probabilities,
        }) if confidence.is_finite() => (choice.as_str(), *confidence, probabilities),
        _ => return None,
    };
    let depth = match answers.get("wants_depth") {
        Some(wire::Answer::Noul { noul, .. }) if noul.is_finite() => *noul,
        _ => return None,
    };
    // `other` and any name that is not the offered `cheap` stay on `code`.
    if choice != "cheap" || confidence < MIN_CONFIDENCE || depth >= MAX_DEPTH {
        return None;
    }
    let index = asked.options.iter().position(|o| o == "cheap")?;
    let note = probabilities
        .iter()
        .map(|(name, p)| format!("{name} {p:.2}"))
        .collect::<Vec<_>>()
        .join(" ");
    Some(Advice {
        answer: Answer::Choice {
            order: vec![u32::try_from(index).ok()?],
        },
        confidence: Some(confidence),
        note: (!note.is_empty()).then_some(note),
    })
}

/// `last_turn.error` or `last_turn.ended_in_error`. A missing turn is the
/// first one, which did not fail.
fn last_turn_errored(state: &Value) -> bool {
    let Some(turn) = state.get("last_turn").and_then(Value::as_object) else {
        return false;
    };
    flag(turn, "error") || flag(turn, "ended_in_error")
}

fn flag(map: &serde_json::Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(last_error: bool) -> Asked {
        let mut state = serde_json::Map::new();
        state.insert("prompt".into(), json!("rename foo to bar"));
        state.insert("static".into(), json!("code"));
        state.insert("files_touched_count".into(), json!(1));
        if last_error {
            state.insert(
                "last_turn".into(),
                json!({ "tools": ["bash"], "error": true }),
            );
        }
        Asked {
            point: cox_plugin_sdk::DecidePoint::Route,
            state: Value::Object(state),
            options: vec!["cheap".into(), "code".into()],
            items: vec![],
        }
    }

    fn answers(choice: &str, confidence: f64, depth: f64) -> BTreeMap<String, wire::Answer> {
        let mut out = BTreeMap::new();
        out.insert(
            "tier".into(),
            wire::Answer::Choice {
                choice: choice.into(),
                confidence,
                probabilities: BTreeMap::from([
                    ("cheap".into(), confidence),
                    ("code".into(), 1.0 - confidence),
                ]),
            },
        );
        out.insert(
            "wants_depth".into(),
            wire::Answer::Noul {
                noul: depth,
                certainty: (2.0 * depth - 1.0).abs(),
            },
        );
        out
    }

    #[test]
    fn route_needs_high_confidence() {
        let question = asked(false);
        assert_eq!(verdict(&answers("cheap", 0.79, 0.1), &question), None);
        assert_eq!(verdict(&answers("cheap", 0.9, 0.3), &question), None);
        let advice = verdict(&answers("cheap", 0.8, 0.29), &question).expect("a downgrade");
        assert_eq!(advice.answer, Answer::Choice { order: vec![0] });
        assert_eq!(advice.confidence, Some(0.8));
    }

    #[test]
    fn route_keeps_code_after_error_turn() {
        let question = asked(true);
        assert_eq!(verdict(&answers("cheap", 0.95, 0.0), &question), None);
    }

    #[test]
    fn route_other_means_code() {
        let question = asked(false);
        assert_eq!(verdict(&answers("other", 0.99, 0.0), &question), None);
        assert_eq!(verdict(&answers("think", 0.99, 0.0), &question), None);
        assert_eq!(verdict(&answers("code", 0.99, 0.0), &question), None);
    }

    #[test]
    fn route_questions_describe_the_offered_tiers() {
        let asked = questions(&["cheap".into(), "code".into()]).expect("cheap is offered");
        let Question::Choice { criteria, .. } = &asked["tier"] else {
            panic!("tier is a choice");
        };
        assert!(
            criteria["cheap"]
                .as_str()
                .unwrap()
                .starts_with("mechanical:"),
            "{}",
            criteria["cheap"]
        );
        assert!(criteria["code"].as_str().unwrap().contains("multi-file"));
        assert!(criteria.contains_key("other"));
        assert!(matches!(asked["wants_depth"], Question::Noul { .. }));
        assert!(questions(&["code".into()]).is_none());
    }
}
