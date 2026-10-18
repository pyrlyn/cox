//! The System One wire (J1, J2): `POST /v1/systemone`'s `{state, model,
//! questions}` request and `{answers, usage}` response, typed and pure —
//! no network, no `extism-pdk`, so it builds and tests on the host target
//! the same as `wasm32-unknown-unknown` (T33.40.2). Ported from
//! the former `crates/cox-provider/src/jev.rs` (deleted by T33.40.12), with
//! two fixes the port made against the vendor docs rather than the old code:
//!
//! - a Noul's certainty is `|2p − 1|`, computed here, never read off a
//!   `confidence` field the API does not send for Noul (J11);
//! - `state` plus the longest question is capped at 32k estimated tokens
//!   (J6), truncating only `state` — a shortened question could silently
//!   change what is being asked, so a question is never touched.
//!
//! Multiple questions ride one request (`questions` is keyed, not singular):
//! a decision point batches every question it has at one moment into one
//! call (J10 — 12x cheaper, 10x faster than one call per question).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Model id a caller sends when it names no explicit Jev model (J5).
pub const DEFAULT_MODEL: &str = "jev-latest";

/// `state` plus the longest question's estimated tokens must not exceed
/// this (J6: "32k tokens for `state` plus the longest question").
const MAX_STATE_AND_QUESTION_TOKENS: usize = 32_000;

/// UTF-8 bytes per token, the same constant `crates/cox-tokens::estimate`
/// uses for its no-tokenizer fallback (`BYTES_PER_TOKEN`). This crate
/// cannot depend on `cox-tokens` (a host crate: it pulls in `tiktoken-rs`
/// and takes `cox_protocol::Request`, and the guest workspace never
/// depends on a host crate, PL§9); `state`/`Question` are plain JSON here,
/// not a `Request`, so the byte-per-token half of that heuristic is
/// reimplemented directly rather than reused.
const BYTES_PER_TOKEN: f64 = 3.8;

/// A `POST /v1/systemone` request body (J1). Field order matches the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneRequest {
    /// What the questions are about (J16): a string, a JSON object or an
    /// array. Capped to [`MAX_STATE_AND_QUESTION_TOKENS`] by [`Self::new`].
    pub state: Value,
    /// The Jev model id.
    pub model: String,
    /// One or more questions, keyed by the caller's own id (batched, J10).
    pub questions: BTreeMap<String, Question>,
}

impl SystemOneRequest {
    /// Builds a request, capping `state` per J6. `model` falls back to
    /// [`DEFAULT_MODEL`] when empty, the same default the former built-in client
    /// used for an unnamed tier.
    pub fn new(
        state: Value,
        model: impl Into<String>,
        questions: BTreeMap<String, Question>,
    ) -> Self {
        let model = model.into();
        let model = if model.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            model
        };
        let state = cap_state(state, &questions);
        Self {
            state,
            model,
            questions,
        }
    }
}

/// One of the three question kinds Jev answers (J2). Every kind carries
/// `type` (the serde tag) and `instructions`, which the docs say "may be a
/// string, an object or an array" — hence `Value`, not `String`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Picks one of at most 255 described options (J2, J3). `criteria` maps
    /// an option to its description — a string, or for options that are
    /// easy to confuse, an object with `what`/`not_for`/`examples` (J3) —
    /// hence `Value`, not `String`.
    Choice {
        /// What is being decided.
        instructions: Value,
        /// Option name to description.
        criteria: BTreeMap<String, Value>,
    },
    /// Rates on an ordered rubric of 2-10 levels (J2). `levels[i]` is the
    /// description of level `i`, in order — the shape a `score` answer's
    /// `legend` echoes back keyed by index (J2).
    Score {
        /// What is being rated.
        instructions: Value,
        /// The rubric, low to high.
        levels: Vec<String>,
    },
    /// A yes/no probability (J2). `criteria`, when given, defines what
    /// counts as true and false; the API's own shape for it is not
    /// published beyond "defines true and false" (J2), so it rides through
    /// as `Value` rather than a guessed struct.
    Noul {
        /// What is being asked.
        instructions: Value,
        /// Optional true/false definition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<Value>,
    },
}

/// One answer, normalised from a `POST /v1/systemone` response (J2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// A Choice answer: the picked option, the full distribution and a
    /// confidence in `[0, 1]` (J2).
    Choice {
        /// The picked option.
        choice: String,
        /// Option name to probability.
        probabilities: BTreeMap<String, f64>,
        /// Model-reported confidence.
        confidence: f64,
    },
    /// A Score answer: the rating, the rubric it was read against, the full
    /// distribution and a confidence in `[0, 1]` (J2).
    Score {
        /// The rating.
        score: f64,
        /// Level index (as a string key, matching the wire) to description.
        legend: BTreeMap<String, String>,
        /// Level index to probability.
        probabilities: BTreeMap<String, f64>,
        /// Model-reported confidence.
        confidence: f64,
    },
    /// A Noul answer: P(yes) and a certainty derived from it.
    Noul {
        /// P(yes), in `[0, 1]`.
        noul: f64,
        /// `|2 * noul - 1|` (J11): the API sends no Noul confidence, and a
        /// confident "no" (`noul` near 0) is high certainty, not low —
        /// reading `noul` itself as a confidence would get that backwards.
        certainty: f64,
    },
}

/// Why a `POST /v1/systemone` response did not parse. Never a guessed
/// default: a missing or malformed field is an error, not an answer nobody
/// gave (mirrors `jev.rs`'s `ProviderError::Parse`, typed per-cause here
/// because this parser now handles more than one question kind per call).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WireError {
    /// The body is not valid JSON.
    #[error("response body is not valid JSON")]
    Json,
    /// `answers` is missing or empty.
    #[error("response has no answers")]
    NoAnswers,
    /// An answer is not a JSON object.
    #[error("answer {0:?} is not a JSON object")]
    NotAnObject(String),
    /// An answer has no `type` field.
    #[error("answer {0:?} has no \"type\" field")]
    MissingType(String),
    /// An answer's `type` is not `choice`, `score` or `noul`.
    #[error("answer {0:?} has unknown type {1:?}")]
    UnknownKind(String, String),
    /// An answer is missing a field its `type` requires.
    #[error("answer {0:?} is missing field {1:?}")]
    MissingField(String, &'static str),
}

/// Parses a `POST /v1/systemone` response body into its answers, keyed the
/// same as the request's `questions` (J1). Every entry in `answers` must
/// parse; an empty or absent `answers` is [`WireError::NoAnswers`] rather
/// than an empty (and therefore silently ignored) map.
pub fn parse(body: &str) -> Result<BTreeMap<String, Answer>, WireError> {
    let value: Value = serde_json::from_str(body).map_err(|_| WireError::Json)?;
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(WireError::NoAnswers)?;
    if answers.is_empty() {
        return Err(WireError::NoAnswers);
    }
    let mut out = BTreeMap::new();
    for (id, raw) in answers {
        out.insert(id.clone(), parse_answer(id, raw)?);
    }
    Ok(out)
}

/// Parses one entry of the response's `answers` object.
fn parse_answer(id: &str, value: &Value) -> Result<Answer, WireError> {
    let obj = value
        .as_object()
        .ok_or_else(|| WireError::NotAnObject(id.to_string()))?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::MissingType(id.to_string()))?;
    let field = |name: &'static str| {
        obj.get(name)
            .ok_or_else(|| WireError::MissingField(id.to_string(), name))
    };
    match kind {
        "choice" => {
            let choice = field("choice")?
                .as_str()
                .ok_or(WireError::MissingField(id.to_string(), "choice"))?
                .to_string();
            let probabilities = parse_f64_map(field("probabilities")?, id, "probabilities")?;
            let confidence = field("confidence")?
                .as_f64()
                .ok_or(WireError::MissingField(id.to_string(), "confidence"))?;
            Ok(Answer::Choice {
                choice,
                probabilities,
                confidence,
            })
        }
        "score" => {
            let score = field("score")?
                .as_f64()
                .ok_or(WireError::MissingField(id.to_string(), "score"))?;
            let legend = parse_string_map(field("legend")?, id, "legend")?;
            let probabilities = parse_f64_map(field("probabilities")?, id, "probabilities")?;
            let confidence = field("confidence")?
                .as_f64()
                .ok_or(WireError::MissingField(id.to_string(), "confidence"))?;
            Ok(Answer::Score {
                score,
                legend,
                probabilities,
                confidence,
            })
        }
        "noul" => {
            let noul = field("noul")?
                .as_f64()
                .ok_or(WireError::MissingField(id.to_string(), "noul"))?;
            Ok(Answer::Noul {
                noul,
                certainty: (2.0 * noul - 1.0).abs(),
            })
        }
        other => Err(WireError::UnknownKind(id.to_string(), other.to_string())),
    }
}

/// Reads a JSON object of `string -> number` fields, the shape both
/// `probabilities` maps share.
fn parse_f64_map(
    value: &Value,
    id: &str,
    field: &'static str,
) -> Result<BTreeMap<String, f64>, WireError> {
    value
        .as_object()
        .ok_or_else(|| WireError::MissingField(id.to_string(), field))?
        .iter()
        .map(|(k, v)| {
            v.as_f64()
                .map(|n| (k.clone(), n))
                .ok_or_else(|| WireError::MissingField(id.to_string(), field))
        })
        .collect()
}

/// Reads a JSON object of `string -> string` fields (`legend`).
fn parse_string_map(
    value: &Value,
    id: &str,
    field: &'static str,
) -> Result<BTreeMap<String, String>, WireError> {
    value
        .as_object()
        .ok_or_else(|| WireError::MissingField(id.to_string(), field))?
        .iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|s| (k.clone(), s.to_string()))
                .ok_or_else(|| WireError::MissingField(id.to_string(), field))
        })
        .collect()
}

/// A byte-counted token estimate, matching `crates/cox-tokens::estimate`'s
/// no-tokenizer fallback (`text.len() / BYTES_PER_TOKEN`, rounded up).
fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 / BYTES_PER_TOKEN).ceil() as usize
}

/// `state` as the text `estimate_tokens` measures: the raw string for a
/// `Value::String` (what actually reaches the wire's `state` field has no
/// surrounding quotes to count), or the compact JSON rendering of an
/// object or array (J16).
fn render_state(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Caps `state` so that its estimated tokens plus the longest question's
/// estimated tokens stay at or under [`MAX_STATE_AND_QUESTION_TOKENS`]
/// (J6). Only `state` is shortened: a truncated question could silently
/// change what is being asked, which no cap may do. A state that had to be
/// cut becomes a plain string (an object or array cut mid-structure would
/// not parse back as one) with a trailing marker noting the cut.
fn cap_state(state: Value, questions: &BTreeMap<String, Question>) -> Value {
    let longest_question_tokens = questions
        .values()
        .map(|q| estimate_tokens(&serde_json::to_string(q).unwrap_or_default()))
        .max()
        .unwrap_or(0);
    let budget = MAX_STATE_AND_QUESTION_TOKENS.saturating_sub(longest_question_tokens);
    let rendered = render_state(&state);
    if estimate_tokens(&rendered) <= budget {
        return state;
    }
    const MARKER: &str = "\n\n[state truncated: over the 32k-token state+question cap (J6)]";
    let budget_bytes = (budget as f64 * BYTES_PER_TOKEN) as usize;
    let keep = budget_bytes.saturating_sub(MARKER.len());
    let mut truncated = truncate_at_char_boundary(&rendered, keep).to_string();
    truncated.push_str(MARKER);
    Value::String(truncated)
}

/// `&s[..max_bytes]`, walked back to the nearest char boundary so a
/// multi-byte UTF-8 character is never split (which would panic).
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn noul_question() -> Question {
        Question::Noul {
            instructions: json!("Is this state worth a typed decision?"),
            criteria: None,
        }
    }

    #[test]
    fn choice_answer_parses_with_probabilities() {
        let answers = parse(
            r#"{"model":"jev-1.13.0","answers":{"department":{"type":"choice","choice":"billing","probabilities":{"billing":0.88,"technical":0.12},"confidence":0.81}},"usage":{"input_tokens":318,"output_tokens":34}}"#,
        )
        .expect("well-formed choice parses");
        let answer = answers.get("department").expect("department answered");
        match answer {
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            } => {
                assert_eq!(choice, "billing");
                assert_eq!(probabilities.get("billing"), Some(&0.88));
                assert_eq!(probabilities.get("technical"), Some(&0.12));
                assert_eq!(*confidence, 0.81);
            }
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn score_answer_parses_with_legend() {
        let answers = parse(
            r#"{"model":"jev-1.13.0","answers":{"frustration":{"type":"score","score":1.05,"legend":{"0":"Calm","1":"Frustrated"},"probabilities":{"0":0.0,"1":0.95,"2":0.05},"confidence":0.92}},"usage":{"input_tokens":304,"output_tokens":18}}"#,
        )
        .expect("well-formed score parses");
        let answer = answers.get("frustration").expect("frustration answered");
        match answer {
            Answer::Score {
                score,
                legend,
                probabilities,
                confidence,
            } => {
                assert_eq!(*score, 1.05);
                assert_eq!(legend.get("0"), Some(&"Calm".to_string()));
                assert_eq!(legend.get("1"), Some(&"Frustrated".to_string()));
                assert_eq!(probabilities.get("1"), Some(&0.95));
                assert_eq!(*confidence, 0.92);
            }
            other => panic!("expected Score, got {other:?}"),
        }
    }

    #[test]
    fn noul_certainty_is_distance_from_half() {
        // No "confidence" field: the API sends none for Noul (J11). A low
        // `noul` (a confident "no") must still read as high certainty.
        let answers = parse(
            r#"{"model":"jev-1.13.0","answers":{"is_urgent":{"type":"noul","noul":0.05}},"usage":{"input_tokens":307,"output_tokens":0}}"#,
        )
        .expect("well-formed noul parses");
        let answer = answers.get("is_urgent").expect("is_urgent answered");
        match answer {
            Answer::Noul { noul, certainty } => {
                assert_eq!(*noul, 0.05);
                assert!(
                    (*certainty - 0.9).abs() < 1e-9,
                    "|2*0.05-1| = 0.9, got {certainty}"
                );
            }
            other => panic!("expected Noul, got {other:?}"),
        }
    }

    #[test]
    fn missing_answers_is_an_error_not_a_guess() {
        let err = parse(r#"{"model":"jev-1.13.0","answers":{}}"#)
            .expect_err("empty answers must fail, not default to nothing decided");
        assert_eq!(err, WireError::NoAnswers);
    }

    #[test]
    fn unknown_answer_kind_is_an_error() {
        let err = parse(r#"{"model":"jev-1.13.0","answers":{"q":{"type":"oracle","choice":"x"}}}"#)
            .expect_err("an unknown kind must fail, not guess a kind");
        assert_eq!(err, WireError::UnknownKind("q".into(), "oracle".into()));
    }

    #[test]
    fn state_over_32k_tokens_is_truncated_with_marker() {
        let oversized = "x".repeat(400_000); // ~105k estimated tokens, well over 32k.
        let mut questions = BTreeMap::new();
        questions.insert("relevant".to_string(), noul_question());

        let req = SystemOneRequest::new(json!(oversized), "jev-latest", questions.clone());

        let state = req.state.as_str().expect("truncated state is a string");
        assert!(
            state.contains("[state truncated"),
            "truncated state carries the marker: {state}"
        );
        assert!(state.len() < oversized.len(), "state must have shrunk");

        let longest_question_tokens = questions
            .values()
            .map(|q| estimate_tokens(&serde_json::to_string(q).unwrap_or_default()))
            .max()
            .unwrap_or(0);
        assert!(
            estimate_tokens(state) + longest_question_tokens <= MAX_STATE_AND_QUESTION_TOKENS,
            "state+question must fit the J6 cap after truncation"
        );
    }

    #[test]
    fn state_under_cap_is_untouched() {
        let mut questions = BTreeMap::new();
        questions.insert("relevant".to_string(), noul_question());
        let req = SystemOneRequest::new(json!("small state"), "", questions);
        assert_eq!(req.state, json!("small state"));
        assert_eq!(req.model, DEFAULT_MODEL);
    }
}
