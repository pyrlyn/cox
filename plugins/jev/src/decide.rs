//! The two-phase decide (T33.40.1, J§4.1) as the Jev guest speaks it:
//! `cox_decide` asks the host to run one System One request on this
//! plugin's own `typesafe` section, and `cox_decide_resume` reads the reply
//! into advice. The host gates, ledgers and retries the call, so the guest
//! never reaches the paid endpoint itself. Pure, so it tests on the host.
//!
//! The one question is a minimal `risk` severity Score (T33.40.4): enough to
//! drive the provider path end to end. The J5.1 question set, thresholds
//! and batching are T33.40.6, which also adds `decide = ["risk"]` to the
//! manifest; until then no session grants this plugin a decision point.

use std::collections::BTreeMap;

use cox_plugin_sdk::{
    Advice, Answer, CallTarget, DecideOut, DecidePoint, DecideResume, ModelCall, OwnProvider,
    Question as Asked, Value,
};
use serde_json::json;

use crate::wire::{self, Question, SystemOneRequest};

/// The `[[provider]]` section a call-out names (`plugin.toml`).
pub const SECTION: &str = "typesafe";
/// The stable model a call-out asks for (J5).
pub const MODEL: &str = "jev-1.13.0";
/// The key the severity question and its answer travel under.
const SEVERITY: &str = "severity";

/// `cox_decide`: a call-out for a `risk` question, silence for any other.
pub fn decide(asked: &Asked) -> DecideOut {
    if asked.point != DecidePoint::Risk {
        return DecideOut::Advice(None);
    }
    let severity = Question::Score {
        instructions: json!("How severe is running this tool call?"),
        // Mirrors `Risk` low to high, so the score reads as a risk level.
        levels: [
            "only reads",
            "writes inside the workspace",
            "runs a process",
            "destroys data or reaches beyond its subject",
        ]
        .map(String::from)
        .to_vec(),
    };
    let body = SystemOneRequest::new(
        asked.state.clone(),
        MODEL,
        BTreeMap::from([(SEVERITY.to_string(), severity)]),
    );
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

/// `cox_decide_resume`: the severity Score as advice, or silence when the
/// reply carries none.
pub fn resume(resumed: &DecideResume) -> Option<Advice> {
    let body: String = resumed
        .events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("text_delta"))
        .filter_map(|e| e.get("text").and_then(Value::as_str))
        .collect();
    match wire::parse(&body).ok()?.remove(SEVERITY)? {
        wire::Answer::Score {
            score, confidence, ..
        } => Some(Advice {
            answer: Answer::Score { value: score },
            confidence: Some(confidence),
            note: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(point: DecidePoint) -> Asked {
        Asked {
            point,
            state: json!({ "tool": "write", "subject": "notes.txt" }),
            options: vec![],
            items: vec![],
        }
    }

    #[test]
    fn risk_question_calls_out_to_the_own_section() {
        let DecideOut::Call { call } = decide(&asked(DecidePoint::Risk)) else {
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
        let body: Value = serde_json::from_str(text).expect("System One JSON");
        assert_eq!(body["state"]["subject"], "notes.txt");
        assert_eq!(body["questions"]["severity"]["type"], "score");
    }

    #[test]
    fn other_points_are_silent() {
        assert_eq!(decide(&asked(DecidePoint::Route)), DecideOut::Advice(None));
    }

    #[test]
    fn resume_reads_the_severity_score() {
        let reply = json!({
            "answers": { "severity": {
                "type": "score", "score": 1.2, "confidence": 0.9,
                "legend": {}, "probabilities": {}
            }}
        });
        let resumed = DecideResume {
            question: asked(DecidePoint::Risk),
            events: vec![
                json!({ "type": "retrying", "attempt": 1, "after_ms": 1 }),
                json!({ "type": "text_delta", "text": reply.to_string() }),
            ],
        };
        let advice = resume(&resumed).expect("advice");
        assert_eq!(advice.answer, Answer::Score { value: 1.2 });
        assert_eq!(advice.confidence, Some(0.9));
        let empty = DecideResume {
            events: vec![],
            ..resumed
        };
        assert_eq!(resume(&empty), None);
    }
}
