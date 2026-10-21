//! The two-phase decide (T33.40.1, J§4.1) as the Jev guest speaks it:
//! `cox_decide` asks the host to run one System One request on this
//! plugin's own `typesafe` section, and `cox_decide_resume` reads the reply
//! into advice. The host gates, ledgers and retries the call, so the guest
//! never reaches the paid endpoint itself. Pure, so it tests on the host.
//!
//! What is asked and how the answers read are `risk`'s (T33.40.6, J§5.1)
//! and `route`'s (T33.40.9, J§5.2); this module only moves the request out
//! and the reply back.

use cox_plugin_sdk::{
    Advice, CallTarget, DecideOut, DecidePoint, DecideResume, ModelCall, OwnProvider,
    Question as Asked, Value,
};
use serde_json::json;

use crate::risk::{self, Thresholds};
use crate::route;
use crate::wire::{self, SystemOneRequest};

/// The `[[provider]]` section a call-out names (`plugin.toml`).
pub const SECTION: &str = "typesafe";
/// The stable model a call-out asks for (J5).
pub const MODEL: &str = "jev-1.13.0";

/// `cox_decide`: a call-out for a `risk` question about at least one call
/// or a `route` question that offers `cheap`, silence for anything else.
/// The core asks `risk` once per tool batch (T33.40.6.1), so a question
/// with no items has nothing to judge; `route` only ever downgrades.
pub fn decide(asked: &Asked) -> DecideOut {
    let (state, questions) = match asked.point {
        DecidePoint::Risk if !asked.items.is_empty() => {
            (risk::state(asked), risk::questions(asked.items.len()))
        }
        DecidePoint::Route if route::offers_cheap(asked) => {
            (route::state(asked), route::questions(asked))
        }
        _ => return DecideOut::Advice(None),
    };
    let body = SystemOneRequest::new(state, MODEL, questions);
    // The host overwrites tier, job and model (PL§7d); the rest is a
    // `cox_protocol::types::Request` whose one user message is the body
    // `provider::stream` sends verbatim.
    let request = json!({
        "tier": "cheap",
        "job": "main",
        "model": MODEL,
        "system": [],
        "tools": [],
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": json!(body).to_string() }],
        }],
        "effort": "low",
        "max_tokens": 1,
        "thinking": "off",
        "cache_breakpoints": [],
        "stop_sequences": [],
    });
    DecideOut::Call {
        call: ModelCall {
            target: CallTarget::OwnProvider {
                own_provider: OwnProvider {
                    name: SECTION.into(),
                    model: MODEL.into(),
                },
            },
            request,
        },
    }
}

/// `cox_decide_resume`: the reply judged by its point — `risk` against
/// `thresholds`, `route` against J§5.2's gate — or silence when it does
/// not parse or advises nothing.
pub fn resume(resumed: &DecideResume, thresholds: &Thresholds) -> Option<Advice> {
    let body: String = resumed
        .events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("text_delta"))
        .filter_map(|e| e.get("text").and_then(Value::as_str))
        .collect();
    let answers = wire::parse(&body).ok()?;
    let asked = &resumed.question;
    match asked.point {
        DecidePoint::Risk => risk::verdict(&answers, asked.items.len(), thresholds),
        DecidePoint::Route => route::verdict(&answers, asked),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use cox_plugin_sdk::{Answer, QuestionItem};

    use super::*;

    /// A `risk` question for one tool batch, the shape the core asks.
    fn batch(subjects: &[&str]) -> Asked {
        Asked {
            point: DecidePoint::Risk,
            state: json!({ "cwd": "/w" }),
            options: vec![],
            items: subjects
                .iter()
                .map(|s| QuestionItem {
                    state: json!({ "tool": "bash", "subject": s }),
                    options: vec![],
                })
                .collect(),
        }
    }

    fn body(out: DecideOut) -> Value {
        let DecideOut::Call { call } = out else {
            panic!("a call-out");
        };
        assert_eq!(
            call.target,
            CallTarget::OwnProvider {
                own_provider: OwnProvider {
                    name: "typesafe".into(),
                    model: "jev-1.13.0".into(),
                },
            }
        );
        let text = call.request["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("one text block");
        serde_json::from_str(text).expect("System One JSON")
    }

    #[test]
    fn risk_question_calls_out_to_the_own_section() {
        let body = body(decide(&batch(&["ls", "git push origin main"])));
        assert_eq!(body["state"]["cwd"], "/w");
        assert_eq!(body["state"]["items"][1]["subject"], "git push origin main");
        assert_eq!(body["questions"]["1.severity"]["type"], "score");
        assert_eq!(body["questions"]["1.irreversible"]["type"], "noul");
    }

    #[test]
    fn other_points_and_empty_batches_are_silent() {
        let mut rank = batch(&["ls"]);
        rank.point = DecidePoint::Rank;
        assert_eq!(decide(&rank), DecideOut::Advice(None));
        assert_eq!(decide(&batch(&[])), DecideOut::Advice(None));
        // `route` with no `cheap` on offer has nothing to downgrade to.
        let code_only = Asked {
            point: DecidePoint::Route,
            state: json!({}),
            options: vec!["code".into()],
            items: vec![],
        };
        assert_eq!(decide(&code_only), DecideOut::Advice(None));
    }

    #[test]
    fn route_question_calls_out_and_resumes_to_cheap() {
        let asked = Asked {
            point: DecidePoint::Route,
            state: json!({ "static": "code", "prompt": "list files",
                           "last_turn": { "errored": false } }),
            options: vec!["cheap".into(), "code".into()],
            items: vec![],
        };
        let body = body(decide(&asked));
        assert_eq!(body["state"]["prompt"], "list files");
        assert_eq!(body["questions"]["tier"]["type"], "choice");
        assert_eq!(body["questions"]["wants_depth"]["type"], "noul");
        let reply = json!({ "answers": {
            "tier": { "type": "choice", "choice": "cheap", "confidence": 0.9,
                      "probabilities": { "cheap": 0.9, "code": 0.1 } },
            "wants_depth": { "type": "noul", "noul": 0.1 },
        }});
        let resumed = DecideResume {
            question: asked,
            events: vec![json!({ "type": "text_delta", "text": reply.to_string() })],
        };
        let advice = resume(&resumed, &Thresholds::default()).expect("a downgrade");
        assert_eq!(advice.answer, Answer::Choice { order: vec![0] });
    }

    #[test]
    fn resume_raises_only_the_flagged_call() {
        let noul = |p: f64| json!({ "type": "noul", "noul": p });
        let score = json!({ "type": "score", "score": 1.0, "confidence": 0.9,
                            "legend": {}, "probabilities": {} });
        let reply = json!({ "answers": {
            "0.severity": score, "0.irreversible": noul(0.1),
            "0.external_effect": noul(0.1), "0.exfiltration": noul(0.1),
            "1.severity": score, "1.irreversible": noul(0.2),
            "1.external_effect": noul(0.95), "1.exfiltration": noul(0.1),
        }});
        let question = batch(&["ls", "git push origin main"]);
        let resumed = DecideResume {
            question: question.clone(),
            events: vec![
                json!({ "type": "retrying", "attempt": 1, "after_ms": 1 }),
                json!({ "type": "text_delta", "text": reply.to_string() }),
            ],
        };
        let advice = resume(&resumed, &Thresholds::default()).expect("a raise");
        assert_eq!(
            advice.answer,
            Answer::Items {
                answers: vec![Answer::Score { value: 0.0 }, Answer::Score { value: 3.0 }]
            }
        );
        assert!(question.fits(&advice.answer));
        let empty = DecideResume {
            events: vec![],
            ..resumed
        };
        assert_eq!(resume(&empty, &Thresholds::default()), None);
    }
}
