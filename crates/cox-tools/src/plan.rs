// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `exit_plan`: leave plan mode only when the person approves (T67.7).
//! `Risk::ReadOnly`, so plan mode allows the call. It asks through the same
//! question relay as `ask_user`. A headless run, and any session opened
//! without a question surface (ACP with no client prompt), declines without
//! asking. The tool never changes the permission mode: an approval is
//! `structured.exit_plan = "approve"`, and the session applies `Default`.

use async_trait::async_trait;
use cox_protocol::types::{EXIT_PLAN_APPROVED, EXIT_PLAN_TOOL, Source};
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// What the ask_user modal shows. Anything other than [`APPROVE`] is feedback
/// and stays in plan mode.
const QUESTION: &str = "Approve this plan and leave plan mode?";
const APPROVE: &str = "Approve";
const KEEP: &str = "Keep planning";

/// `headless` declines. `surface` asks through `cx.relay` (TUI, `--plain`).
pub struct ExitPlanTool {
    surface: bool,
}

impl ExitPlanTool {
    /// Headless and ACP without a prompt: decline, never ask.
    pub fn headless() -> Self {
        Self { surface: false }
    }

    /// A surface that answers `Event::QuestionAsked`.
    pub fn surface() -> Self {
        Self { surface: true }
    }
}

/// The question the modal shows: the fixed prompt plus the plan's first line.
pub fn exit_plan_question(plan: &str) -> String {
    let line = plan
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let shown: String = line.chars().take(80).collect();
    format!("{QUESTION} {shown}")
}

#[async_trait]
impl Tool for ExitPlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: EXIT_PLAN_TOOL.to_string(),
            description: "Leave plan mode. Call this only while planning, with the plan in \
                `plan`. The user must approve in the question modal; a headless run declines. \
                Approval is applied by the session, not by this tool. Feedback means keep \
                planning."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan": {"type": "string", "description": "The plan the user is asked to approve"}
                },
                "required": ["plan"]
            }),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("plan")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
            .chars()
            .take(80)
            .collect()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let plan = str_field(&input, "plan")?;
        if plan.trim().is_empty() {
            return Err(ToolError::Denied {
                why: "exit_plan needs a non-empty plan".into(),
            });
        }
        if !self.surface {
            return Err(declined());
        }
        let relay = cx.relay.as_ref().ok_or_else(declined)?;
        let question = exit_plan_question(&plan);
        let options = [APPROVE.to_string(), KEEP.to_string()];
        let source = cx.agent.clone().map(|agent| Source {
            session: cx.session,
            agent: Some(agent),
            preset: cx.preset.clone(),
        });
        let answer = tokio::select! {
            biased;
            _ = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            answer = relay.ask(cx.call, &question, &options, source) => answer?,
        };
        let Some(answer) = answer else {
            return Err(ToolError::Denied {
                why: "the question was dismissed without an answer".into(),
            });
        };
        if answer == APPROVE {
            return Ok(ToolOutput {
                text: "The user approved this plan.".into(),
                is_error: false,
                diff: None,
                structured: Some(json!({ EXIT_PLAN_TOOL: EXIT_PLAN_APPROVED })),
            });
        }
        Ok(ToolOutput {
            text: format!("The user did not approve the plan. Feedback: {answer}"),
            is_error: false,
            diff: None,
            structured: Some(json!({ EXIT_PLAN_TOOL: "revise", "feedback": answer })),
        })
    }
}

fn declined() -> ToolError {
    ToolError::Denied {
        why: "exit_plan is declined: no one can approve leaving plan mode".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use cox_protocol::{
        Archive, ArchiveId, ArchivePut, CallId, Relay, SandboxMode, SandboxPolicy, SessionId,
        StoreError,
    };
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

    fn cx() -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
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
            CancellationToken::new(),
            tx,
            SessionId::new(),
            CallId::new(),
        )
    }

    struct FakeRelay {
        answer: Option<String>,
        asked: Mutex<Vec<String>>,
    }

    impl FakeRelay {
        fn answering(answer: Option<String>) -> Arc<Self> {
            Arc::new(Self {
                answer,
                asked: Mutex::new(Vec::new()),
            })
        }

        fn questions(&self) -> Vec<String> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    #[async_trait]
    impl Relay for FakeRelay {
        async fn send_message(&self, _to: &str, _text: &str) -> Result<(), ToolError> {
            Ok(())
        }
        async fn ask(
            &self,
            _call_id: CallId,
            question: &str,
            _options: &[String],
            _source: Option<Source>,
        ) -> Result<Option<String>, ToolError> {
            self.asked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(question.to_string());
            Ok(self.answer.clone())
        }
    }

    fn with_relay(mut cx: ToolCx, relay: Arc<FakeRelay>) -> ToolCx {
        cx.relay = Some(relay);
        cx
    }

    #[test]
    fn exit_plan_spec_is_read_only_and_ignores_the_surface() {
        let headless = ExitPlanTool::headless().spec();
        assert_eq!(headless.name, EXIT_PLAN_TOOL);
        assert_eq!(headless.risk, Risk::ReadOnly);
        assert!(!headless.deferred);
        assert_eq!(headless, ExitPlanTool::surface().spec());
    }

    /// T67.7: a headless tool declines even when a relay would approve, and
    /// it never asks.
    #[tokio::test]
    async fn headless_declines_exit_plan() {
        let relay = FakeRelay::answering(Some(APPROVE.into()));
        let err = ExitPlanTool::headless()
            .call(
                json!({"plan": "ship the ledger"}),
                &with_relay(cx(), relay.clone()),
            )
            .await
            .expect_err("headless");
        assert!(matches!(err, ToolError::Denied { .. }));
        assert!(relay.questions().is_empty(), "headless must not ask");
    }

    /// T67.7: approval is the exact option, and only after the relay was
    /// asked. Feedback and a dismissal do not approve.
    #[tokio::test]
    async fn exit_plan_requires_approval() {
        let plan = json!({"plan": "ship the ledger"});
        let missing = ExitPlanTool::surface()
            .call(json!({}), &cx())
            .await
            .expect_err("missing plan");
        assert!(matches!(missing, ToolError::Denied { .. }));

        let dismiss = FakeRelay::answering(None);
        let err = ExitPlanTool::surface()
            .call(plan.clone(), &with_relay(cx(), dismiss.clone()))
            .await
            .expect_err("dismissed");
        assert!(matches!(err, ToolError::Denied { .. }));
        assert_eq!(dismiss.questions().len(), 1);

        let revise = FakeRelay::answering(Some("add a test".into()));
        let out = ExitPlanTool::surface()
            .call(plan.clone(), &with_relay(cx(), revise.clone()))
            .await
            .expect("feedback");
        let structured = out.structured.expect("structured");
        assert_eq!(structured[EXIT_PLAN_TOOL], "revise");
        assert_eq!(structured["feedback"], "add a test");
        assert!(out.text.contains("add a test"), "{}", out.text);
        assert_eq!(
            revise.questions(),
            vec![exit_plan_question("ship the ledger")]
        );

        let approve = FakeRelay::answering(Some(APPROVE.into()));
        let out = ExitPlanTool::surface()
            .call(plan, &with_relay(cx(), approve.clone()))
            .await
            .expect("approved");
        assert_eq!(
            out.structured.expect("structured")[EXIT_PLAN_TOOL],
            EXIT_PLAN_APPROVED
        );
        assert_eq!(approve.questions().len(), 1);
    }
}
