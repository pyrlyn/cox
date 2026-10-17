//! The J§5.1 `risk` advisor (T33.40.6): what one System One request for a
//! tool batch asks, and how its answers become "raise to `Destructive`" or
//! no advice. Separate from `decide`, which only moves a request to the host
//! and a reply back, so the question set and the thresholds change without
//! touching the call-out. Pure, so it tests on the host.
//!
//! The guest's own thresholds are the gate: a raise goes out with
//! confidence 1.0 and its probabilities in the note (A163), and the core
//! keeps `max(builtin, advised)`, so no answer can lower a call's risk.

use std::collections::BTreeMap;

use cox_plugin_sdk::{Advice, Answer, Question as Asked, Value};
use serde::Deserialize;
use serde_json::{Map, json};

use crate::wire::{self, Question};

/// The fields of the batch's shared state a question may carry. A field the
/// core adds later stays out until this list names it: the request must
/// never carry tool output (J5.1).
const SHARED: [&str; 4] = ["task", "cwd", "workspace_roots", "sandbox"];
/// The fields of one call a question may carry, for the same reason.
const PER_CALL: [&str; 4] = ["tool", "subject", "input", "classifier_risk"];

/// The answer that raises a call: `Destructive` on the severity scale.
const RAISE: f64 = 3.0;
/// The answer that leaves a call alone: the bottom of the scale, which the
/// core's `max(builtin, advised)` never moves.
const KEEP: f64 = 0.0;

/// `[plugins.jev.risk]`, with the J§5.1 defaults.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thresholds {
    /// The `irreversible` P(yes) that raises a call.
    pub irreversible: f64,
    /// The `external_effect` P(yes) that raises a call.
    pub external_effect: f64,
    /// The `exfiltration` P(yes) that raises a call.
    pub exfiltration: f64,
    /// The `severity` Score that raises a call.
    pub severity: f64,
    /// The confidence a `severity` Score needs before it counts (J11).
    pub min_confidence: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            irreversible: 0.7,
            external_effect: 0.7,
            exfiltration: 0.5,
            severity: 2.5,
            min_confidence: 0.6,
        }
    }
}

/// Why `[plugins.jev.risk]` was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// A key is unknown or a value is not a number.
    #[error("[plugins.jev.risk]: {0}")]
    Shape(String),
    /// A value lies outside its scale, which would silently turn a
    /// threshold off (above the top) or raise every call (below zero).
    #[error("[plugins.jev.risk] {0} = {1} is outside {2}")]
    Range(&'static str, f64, &'static str),
}

/// The thresholds in the plugin's `[plugins.jev]` table, or the defaults
/// when it has no `risk` table.
pub fn thresholds(config: &Value) -> Result<Thresholds, ConfigError> {
    let t = match config.get("risk") {
        None | Some(Value::Null) => return Ok(Thresholds::default()),
        Some(table) => {
            Thresholds::deserialize(table).map_err(|e| ConfigError::Shape(e.to_string()))?
        }
    };
    let unit = [
        ("irreversible", t.irreversible),
        ("external_effect", t.external_effect),
        ("exfiltration", t.exfiltration),
        ("min_confidence", t.min_confidence),
    ];
    if let Some((key, v)) = unit.into_iter().find(|(_, v)| !(0.0..=1.0).contains(v)) {
        return Err(ConfigError::Range(key, v, "0..=1"));
    }
    if !(0.0..=RAISE).contains(&t.severity) {
        return Err(ConfigError::Range("severity", t.severity, "0..=3"));
    }
    Ok(t)
}

/// The request's `state`: the shared fields plus each call's fields under
/// `items`, and nothing else.
pub fn state(asked: &Asked) -> Value {
    let mut state = pick(&asked.state, &SHARED);
    let items = asked
        .items
        .iter()
        .map(|i| Value::Object(pick(&i.state, &PER_CALL)))
        .collect();
    state.insert("items".into(), Value::Array(items));
    Value::Object(state)
}

fn pick(from: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|k| Some((k.to_string(), from.get(k)?.clone())))
        .collect()
}

/// The four J§5.1 questions for each of `items` calls, keyed `<i>.<name>`
/// so one request carries the whole batch.
pub fn questions(items: usize) -> BTreeMap<String, Question> {
    let mut out = BTreeMap::new();
    for i in 0..items {
        let call = format!("the tool call at state.items[{i}]");
        out.insert(
            format!("{i}.severity"),
            Question::Score {
                instructions: json!(format!("How severe is running {call}?")),
                // Mirrors `Risk` low to high, so the score reads as a risk level.
                levels: [
                    "only reads",
                    "writes inside the workspace",
                    "runs a process",
                    "destroys data or reaches beyond its subject",
                ]
                .map(String::from)
                .to_vec(),
            },
        );
        for (name, asks) in [
            ("irreversible", "cannot be undone by git or the checkpoint"),
            (
                "external_effect",
                "pushes, publishes, deploys, sends, or deletes outside the workspace",
            ),
            (
                "exfiltration",
                "sends workspace content or credentials to a network destination",
            ),
        ] {
            out.insert(
                format!("{i}.{name}"),
                Question::Noul {
                    instructions: json!(format!("Does running {call} do this: {asks}?")),
                    criteria: None,
                },
            );
        }
    }
    out
}

/// The advice for a batch of `items` calls: a raise for each call any
/// threshold flags, or `None` when no call is flagged.
pub fn verdict(
    answers: &BTreeMap<String, wire::Answer>,
    items: usize,
    t: &Thresholds,
) -> Option<Advice> {
    let mut notes = Vec::new();
    let mut out = Vec::with_capacity(items);
    for i in 0..items {
        let p = |name: &str| match answers.get(&format!("{i}.{name}")) {
            Some(wire::Answer::Noul { noul, .. }) if noul.is_finite() => *noul,
            _ => 0.0,
        };
        let (severity, confidence) = match answers.get(&format!("{i}.severity")) {
            Some(wire::Answer::Score {
                score, confidence, ..
            }) if score.is_finite() && confidence.is_finite() => (*score, *confidence),
            _ => (0.0, 0.0),
        };
        let (irr, ext, exf) = (p("irreversible"), p("external_effect"), p("exfiltration"));
        let raised = irr >= t.irreversible
            || ext >= t.external_effect
            || exf >= t.exfiltration
            || (severity >= t.severity && confidence >= t.min_confidence);
        if raised {
            notes.push(format!(
                "#{i} severity {severity:.2}@{confidence:.2} irreversible {irr:.2} \
                 external {ext:.2} exfiltration {exf:.2}"
            ));
        }
        out.push(Answer::Score {
            value: if raised { RAISE } else { KEEP },
        });
    }
    (!notes.is_empty()).then(|| Advice {
        answer: Answer::Items { answers: out },
        confidence: Some(1.0),
        note: Some(notes.join("; ")),
    })
}

#[cfg(test)]
mod tests {
    use cox_plugin_sdk::{DecidePoint, QuestionItem};

    use super::*;

    fn noul(p: f64) -> wire::Answer {
        wire::Answer::Noul {
            noul: p,
            certainty: (2.0 * p - 1.0).abs(),
        }
    }

    fn score(value: f64, confidence: f64) -> wire::Answer {
        wire::Answer::Score {
            score: value,
            legend: BTreeMap::new(),
            probabilities: BTreeMap::new(),
            confidence,
        }
    }

    /// One call's answers: severity, its confidence, then the three Nouls.
    fn answers(calls: &[(f64, f64, f64, f64, f64)]) -> BTreeMap<String, wire::Answer> {
        let mut out = BTreeMap::new();
        for (i, &(sev, conf, irr, ext, exf)) in calls.iter().enumerate() {
            out.insert(format!("{i}.severity"), score(sev, conf));
            out.insert(format!("{i}.irreversible"), noul(irr));
            out.insert(format!("{i}.external_effect"), noul(ext));
            out.insert(format!("{i}.exfiltration"), noul(exf));
        }
        out
    }

    const LOW: (f64, f64, f64, f64, f64) = (1.0, 0.9, 0.1, 0.1, 0.1);

    fn raised(advice: &Advice) -> Vec<bool> {
        let Answer::Items { answers } = &advice.answer else {
            panic!("items: {advice:?}");
        };
        answers
            .iter()
            .map(|a| *a == Answer::Score { value: RAISE })
            .collect()
    }

    #[test]
    fn risk_state_never_contains_tool_output() {
        let asked = Asked {
            point: DecidePoint::Risk,
            state: json!({ "task": "push it", "cwd": "/w", "sandbox": "workspace-write",
                           "output": "SECRET-SHARED" }),
            options: vec![],
            items: vec![QuestionItem {
                state: json!({ "tool": "bash", "subject": "git push origin main",
                               "input": { "command": "git push origin main" },
                               "classifier_risk": "exec",
                               "output": "SECRET-OUTPUT", "result": "SECRET-RESULT" }),
                options: vec![],
            }],
        };
        let state = state(&asked);
        let text = state.to_string();
        assert!(!text.contains("SECRET"), "{text}");
        assert_eq!(state["task"], "push it");
        assert_eq!(state["sandbox"], "workspace-write");
        assert_eq!(state["items"][0]["subject"], "git push origin main");
        assert_eq!(state["items"][0]["classifier_risk"], "exec");
    }

    #[test]
    fn risk_questions_cover_every_item() {
        let q = questions(2);
        assert_eq!(q.len(), 8);
        assert!(matches!(q["1.severity"], Question::Score { ref levels, .. } if levels.len() == 4));
        assert!(matches!(q["0.exfiltration"], Question::Noul { .. }));
    }

    #[test]
    fn risk_thresholds_raise_on_any_hazard() {
        let t = Thresholds::default();
        let calls = [
            (0.0, 0.0, 0.7, 0.0, 0.0), // irreversible at its threshold
            (0.0, 0.0, 0.0, 0.7, 0.0), // external effect
            (0.0, 0.0, 0.0, 0.0, 0.5), // exfiltration
            (2.5, 0.6, 0.0, 0.0, 0.0), // a confident severity
            LOW,
        ];
        let advice = verdict(&answers(&calls), calls.len(), &t).expect("a raise");
        assert_eq!(raised(&advice), [true, true, true, true, false]);
        assert_eq!(advice.confidence, Some(1.0));
        let note = advice.note.expect("probabilities");
        assert!(
            note.contains("#0 ") && note.contains("irreversible 0.70"),
            "{note}"
        );
        assert!(!note.contains("#4 "), "{note}");
    }

    #[test]
    fn risk_low_answers_give_no_advice() {
        let t = Thresholds::default();
        // Just under each threshold, and a high severity nobody is sure of.
        let calls = [
            (2.49, 0.9, 0.69, 0.69, 0.49),
            (3.0, 0.59, 0.0, 0.0, 0.0),
            LOW,
        ];
        assert_eq!(verdict(&answers(&calls), calls.len(), &t), None);
        // A reply missing its answers, or a NaN, is no hazard either.
        assert_eq!(verdict(&BTreeMap::new(), 2, &t), None);
        let nan = answers(&[(f64::NAN, 1.0, f64::NAN, 0.0, 0.0)]);
        assert_eq!(verdict(&nan, 1, &t), None);
    }

    #[test]
    fn risk_config_overrides_thresholds() {
        assert_eq!(thresholds(&json!({})), Ok(Thresholds::default()));
        let t = thresholds(&json!({ "risk": { "irreversible": 0.9, "severity": 2.0 } }))
            .expect("valid");
        assert_eq!((t.irreversible, t.severity), (0.9, 2.0));
        assert_eq!(t.exfiltration, 0.5, "an unset key keeps its default");
        let call = [(2.0, 0.9, 0.8, 0.0, 0.0)];
        let advice = verdict(&answers(&call), 1, &t).expect("severity 2.0 now raises");
        assert_eq!(raised(&advice), [true]);
        let strict = Thresholds { severity: 3.0, ..t };
        assert_eq!(
            verdict(&answers(&call), 1, &strict),
            None,
            "irreversible 0.8 < 0.9"
        );
    }

    #[test]
    fn risk_config_refuses_an_invalid_table() {
        for bad in [
            json!({ "risk": { "irreversable": 0.5 } }),
            json!({ "risk": { "exfiltration": "high" } }),
            json!({ "risk": { "external_effect": 1.5 } }),
            json!({ "risk": { "min_confidence": -0.1 } }),
            json!({ "risk": { "severity": 4.0 } }),
            json!({ "risk": 3 }),
        ] {
            assert!(thresholds(&bad).is_err(), "{bad}");
        }
    }
}
