// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `exit_plan` (T67.7): the model asks to leave Plan mode. The tool is
//! read-only, so Plan mode's engine lets it through, and it asks the person
//! the way `ask_user` does. It never changes the permission mode itself —
//! `cox-core` does that when the answer is `EXIT_PLAN_APPROVE`. A run with
//! no question surface declines, which is every headless run and ACP without
//! a client prompt.

use async_trait::async_trait;
use cox_protocol::types::{EXIT_PLAN_APPROVE, EXIT_PLAN_KEEP, EXIT_PLAN_TOOL};
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// Whether this tool raises `QuestionAsked`. Headless and ACP without a
/// prompt construct the declining one, so they cannot approve by accident.
pub struct ExitPlanTool {
    ask: bool,
}

impl ExitPlanTool {
    /// No one to ask: every call is declined.
    pub fn declining() -> Self {
        Self { ask: false }
    }

    /// Interactive: wait on the session's question surface.
    pub fn asking() -> Self {
        Self { ask: true }
    }
}

/// The question the `ask_user` modal shows. Newlines in the plan become
/// spaces so the modal stays one header line.
pub fn prompt(plan: &str) -> String {
    let plan = plan.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("Leave plan mode and start editing? {plan}")
}

#[async_trait]
impl Tool for ExitPlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: EXIT_PLAN_TOOL.to_string(),
            description: "Ask the user to leave Plan mode and start editing. Pass `plan`, the \
                plan they are approving. Only an explicit approval changes the mode, and that \
                change is the session's, not this tool's. A headless run declines."
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
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let plan = str_field(&input, "plan")?;
        if !self.ask {
            return Err(ToolError::Denied {
                why: "exit_plan needs a person to approve leaving plan mode; this run has no \
                      prompt"
                    .into(),
            });
        }
        let relay = cx.relay.as_ref().ok_or_else(|| ToolError::Denied {
            why: "no surface is listening for the plan approval".into(),
        })?;
        let question = prompt(&plan);
        let options = [EXIT_PLAN_APPROVE.to_string(), EXIT_PLAN_KEEP.to_string()];
        let answer = tokio::select! {
            biased;
            _ = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
            answer = relay.ask(cx.call, &question, &options, None) => {
                answer?.ok_or_else(|| ToolError::Denied {
                    why: "the plan approval was dismissed".into(),
                })?
            }
        };
        let approved = answer == EXIT_PLAN_APPROVE;
        let text = if approved {
            "Approved. The session is switching to the default permission mode.".to_string()
        } else {
            format!("Still planning.\n{answer}")
        };
        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: Some(json!({
                "approved": approved,
                "plan": plan,
                "feedback": if approved { Value::Null } else { Value::String(answer) },
            })),
        })
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

    fn cx() -> ToolCx {
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
            CancellationToken::new(),
            tx,
            SessionId::new(),
            CallId::new(),
        )
    }

    struct FixedRelay(Option<String>);

    #[async_trait]
    impl Relay for FixedRelay {
        async fn send_message(&self, _to: &str, _text: &str) -> Result<(), ToolError> {
            Ok(())
        }
        async fn ask(
            &self,
            _call_id: CallId,
            _question: &str,
            _options: &[String],
            _source: Option<cox_protocol::types::Source>,
        ) -> Result<Option<String>, ToolError> {
            Ok(self.0.clone())
        }
    }

    /// Keeps the question so a test can see what the modal would show.
    struct RecordingRelay {
        answer: String,
        asked: Mutex<Vec<(String, Vec<String>)>>,
    }

    #[async_trait]
    impl Relay for RecordingRelay {
        async fn send_message(&self, _to: &str, _text: &str) -> Result<(), ToolError> {
            Ok(())
        }
        async fn ask(
            &self,
            _call_id: CallId,
            question: &str,
            options: &[String],
            _source: Option<cox_protocol::types::Source>,
        ) -> Result<Option<String>, ToolError> {
            self.asked
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push((question.to_string(), options.to_vec()));
            Ok(Some(self.answer.clone()))
        }
    }

    #[tokio::test]
    async fn headless_declines_exit_plan() {
        let err = ExitPlanTool::declining()
            .call(json!({"plan": "ship it"}), &cx())
            .await
            .expect_err("no prompt");
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
        let mut bare = cx();
        bare.relay = Some(Arc::new(FixedRelay(Some(EXIT_PLAN_APPROVE.into()))));
        let err = ExitPlanTool::declining()
            .call(json!({"plan": "ship it"}), &bare)
            .await
            .expect_err("a fixed answer is not a prompt");
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
    }

    /// Only the approve option reports approval. Feedback leaves the flag false,
    /// so the session has nothing to switch on.
    #[tokio::test]
    async fn only_the_approve_answer_reports_approval() {
        let keep = Arc::new(RecordingRelay {
            answer: "narrow the cache key first".into(),
            asked: Mutex::new(Vec::new()),
        });
        let mut cx = cx();
        cx.relay = Some(keep.clone());
        let out = ExitPlanTool::asking()
            .call(json!({"plan": "Ship the cache fix."}), &cx)
            .await
            .expect("answered");
        assert!(!out.exit_plan_approved());
        assert!(out.text.contains("narrow the cache key"));
        let (question, options) = keep
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(0);
        assert_eq!(
            question,
            "Leave plan mode and start editing? Ship the cache fix."
        );
        assert_eq!(options, [EXIT_PLAN_APPROVE, EXIT_PLAN_KEEP]);

        cx.relay = Some(Arc::new(FixedRelay(Some(EXIT_PLAN_APPROVE.into()))));
        let out = ExitPlanTool::asking()
            .call(json!({"plan": "Ship the cache fix."}), &cx)
            .await
            .expect("approved");
        assert!(out.exit_plan_approved());
        assert!(!out.is_error);
    }
}
