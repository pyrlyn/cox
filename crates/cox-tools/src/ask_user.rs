// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `ask_user`: the model asks the person a question and the turn blocks
//! until they answer (plan.md T3.8, §1.11). Separate from the other tools
//! because it is the only one whose "I/O" is a surface: an interactive
//! surface answers `Event::QuestionAsked` with `Submission::Answer` through
//! `cx.relay` (DT G4), a headless run answers with `--answer` or fails.

use async_trait::async_trait;
use cox_protocol::types::Source;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// Where answers come from.
pub enum Answers {
    /// Headless: `--answer` text, or nothing (every question fails).
    Fixed(Option<String>),
    /// Interactive: the session raises `QuestionAsked` and waits for the
    /// surface's `Submission::Answer`.
    Surface,
}

pub struct AskUserTool {
    answers: Answers,
}

impl AskUserTool {
    pub fn new(answers: Answers) -> Self {
        Self { answers }
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_user".to_string(),
            description: "Ask the user one question and wait for the answer. Use it only \
                when you cannot proceed without a decision that is theirs to make; do not \
                use it to confirm work you can verify yourself. Pass `question` and, when \
                the choice is between a few concrete alternatives, `options`. In a headless \
                run this returns the `--answer` text or an error."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "options": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["question"]
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let question = str_field(&input, "question")?;
        let options: Vec<String> = input
            .get("options")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let answer = match &self.answers {
            Answers::Fixed(Some(answer)) => answer.clone(),
            Answers::Fixed(None) => {
                return Err(ToolError::Denied {
                    why: "no one can answer: this run is headless; pass --answer or run \
                          interactively"
                        .into(),
                });
            }
            Answers::Surface => {
                let relay = cx.relay.as_ref().ok_or_else(|| ToolError::Denied {
                    why: "no surface is listening for questions".into(),
                })?;
                // T34.3: built from `cx.agent`/`cx.preset` the same way
                // `relay_approval` builds an `ApprovalRequired`'s `Source`;
                // `None` for the top-level session the user is talking to.
                let source = cx.agent.clone().map(|agent| Source {
                    session: cx.session,
                    agent: Some(agent),
                    preset: cx.preset.clone(),
                });
                tokio::select! {
                    // Cancel wins over a reply that lands in the same tick.
                    biased;
                    _ = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
                    answer = relay.ask(cx.call, &question, &options, source) => {
                        answer?.ok_or_else(|| ToolError::Denied {
                            why: "the question was dismissed without an answer".into(),
                        })?
                    }
                }
            }
        };
        Ok(ToolOutput {
            text: answer.clone(),
            is_error: false,
            diff: None,
            structured: Some(json!({
                "question": question,
                "options": options,
                "answer": answer,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use std::sync::Mutex;

    use cox_protocol::{
        Archive, ArchiveId, ArchivePut, CallId, Relay, SandboxMode, SandboxPolicy, SessionId,
        StoreError,
    };
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;

    struct NoopArchive;

    #[async_trait]
    impl Archive for NoopArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn cx(cancel: CancellationToken) -> ToolCx {
        let (tx, _rx) = mpsc::channel(1);
        crate::tool_cx(
            vec![PathBuf::from("/tmp")],
            PathBuf::from("/tmp"),
            SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            Arc::new(NoopArchive),
            cancel,
            tx,
            SessionId::new(),
            CallId::new(),
        )
    }

    #[tokio::test]
    async fn ask_user_headless_returns_the_fixed_answer_or_an_error() {
        let q = json!({"question": "which?", "options": ["a", "b"]});
        let out = AskUserTool::new(Answers::Fixed(Some("a".into())))
            .call(q.clone(), &cx(CancellationToken::new()))
            .await
            .expect("answered");
        assert_eq!(out.text, "a");
        let err = AskUserTool::new(Answers::Fixed(None))
            .call(q, &cx(CancellationToken::new()))
            .await
            .expect_err("headless without --answer");
        assert!(matches!(err, ToolError::Denied { .. }));
    }

    /// Answers every question with `answer` (or never, for `None`) and
    /// keeps what it was asked.
    struct FakeRelay {
        answer: Option<Option<String>>,
        asked: Mutex<Vec<(CallId, String, Option<Source>)>>,
    }

    impl FakeRelay {
        fn answering(answer: Option<Option<String>>) -> Arc<Self> {
            Arc::new(Self {
                answer,
                asked: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl Relay for FakeRelay {
        async fn send_message(&self, _to: &str, _text: &str) -> Result<(), ToolError> {
            Ok(())
        }
        async fn ask(
            &self,
            call_id: CallId,
            question: &str,
            _options: &[String],
            source: Option<Source>,
        ) -> Result<Option<String>, ToolError> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).push((
                call_id,
                question.to_string(),
                source,
            ));
            match &self.answer {
                Some(answer) => Ok(answer.clone()),
                None => std::future::pending().await,
            }
        }
    }

    fn with_relay(cx: ToolCx, relay: Arc<FakeRelay>) -> ToolCx {
        ToolCx {
            relay: Some(relay),
            ..cx
        }
    }

    #[tokio::test]
    async fn ask_user_surface_answer_is_the_result_dismiss_fails_and_cancel_unblocks() {
        let relay = FakeRelay::answering(Some(Some("cox".into())));
        let asking = with_relay(cx_fresh(), relay.clone());
        let out = AskUserTool::new(Answers::Surface)
            .call(json!({"question": "name?"}), &asking)
            .await
            .expect("answered");
        assert_eq!(out.text, "cox");
        let (call, question, _) = relay.asked.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
        assert_eq!(call, asking.call);
        assert_eq!(question, "name?");

        let asking = with_relay(cx_fresh(), FakeRelay::answering(Some(None)));
        let err = AskUserTool::new(Answers::Surface)
            .call(json!({"question": "skip?"}), &asking)
            .await
            .expect_err("dismissed");
        assert!(matches!(err, ToolError::Denied { .. }));

        let cancel = CancellationToken::new();
        let asking = with_relay(cx(cancel.clone()), FakeRelay::answering(None));
        cancel.cancel();
        let err = AskUserTool::new(Answers::Surface)
            .call(json!({"question": "wait?"}), &asking)
            .await
            .expect_err("cancelled");
        assert!(matches!(err, ToolError::Cancelled));
    }

    fn cx_fresh() -> ToolCx {
        cx(CancellationToken::new())
    }

    /// T34.3: a `ToolCx` labelled by `spawn_child` (`agent`/`preset` set)
    /// makes the question's `source` carry the same `Source` shape
    /// `relay_approval` already builds for a relayed approval; the
    /// top-level session's own `cx` (this file's `cx` helper) keeps
    /// `source: None`.
    #[tokio::test]
    async fn ask_user_from_subagent_carries_its_source() {
        let session = SessionId::new();
        let relay = FakeRelay::answering(Some(Some("staging".into())));
        let child_cx = ToolCx {
            session,
            agent: Some("explore-2".into()),
            preset: Some("explore".into()),
            ..with_relay(cx_fresh(), relay.clone())
        };
        let out = AskUserTool::new(Answers::Surface)
            .call(json!({"question": "which env?"}), &child_cx)
            .await
            .expect("answered");
        assert_eq!(out.text, "staging");
        tool_asked_top_level(relay.clone()).await;
        let asked = relay.asked.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            asked[0].2,
            Some(Source {
                session,
                agent: Some("explore-2".into()),
                preset: Some("explore".into()),
            })
        );
        // The top-level session's own call carries no source at all.
        assert_eq!(asked[1].2, None);
    }

    async fn tool_asked_top_level(relay: Arc<FakeRelay>) {
        AskUserTool::new(Answers::Surface)
            .call(json!({"question": "ok?"}), &with_relay(cx_fresh(), relay))
            .await
            .expect("answered");
    }
}
