// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `exit_plan`: the model proposes leaving Plan mode (T67.7). Separate from
//! `ask_user` because the decision is fixed — approve, or keep planning —
//! and because a headless run or an ACP client with no prompt declines it
//! instead of taking `--answer`. The tool never changes the permission
//! mode. It reports the person's decision; the session applies `Default`
//! only after an approval.

use async_trait::async_trait;
use cox_protocol::types::{EXIT_PLAN_APPROVE, EXIT_PLAN_KEEP, Source};
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// Where the decision comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Audience {
    /// No client prompt: the call is declined, not answered by `--answer`.
    Headless,
    /// A surface that answers `QuestionAsked`.
    Surface,
}

/// Asks the person to leave Plan mode.
pub struct ExitPlanTool {
    audience: Audience,
}

impl ExitPlanTool {
    /// Headless and ACP without a question surface.
    pub fn headless() -> Self {
        Self {
            audience: Audience::Headless,
        }
    }

    /// The TUI and the desktop app, which answer `QuestionAsked`.
    pub fn surface() -> Self {
        Self {
            audience: Audience::Surface,
        }
    }
}

/// The session switches to `Default` only when the tool says the person
/// approved. An error result never does: declining is not approval.
pub fn approved_exit(output: &ToolOutput) -> bool {
    !output.is_error
        && output
            .structured
            .as_ref()
            .and_then(|v| v.get("approved"))
            .and_then(Value::as_bool)
            == Some(true)
}

const DECLINED: &str =
    "leaving Plan mode needs a person's approval, and this run has no one to give it";

#[async_trait]
impl Tool for ExitPlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exit_plan".to_string(),
            description: "Leave Plan mode. Call this only while permission_mode is Plan, \
                with the plan in `plan`, once it is ready to carry out. The person approves \
                or asks you to keep planning; you cannot change the mode yourself. A headless \
                run and a client with no prompt decline the call."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan": {"type": "string", "description": "The plan the person is asked to approve"}
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
            .and_then(|plan| plan.lines().next())
            .map(|line| line.chars().take(80).collect())
            .filter(|line: &String| !line.is_empty())
            .unwrap_or_else(|| "exit_plan".to_string())
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let plan = str_field(&input, "plan")?;
        let plan = plan.trim();
        if plan.is_empty() {
            return Err(ToolError::Denied {
                why: "missing or empty \"plan\" field".into(),
            });
        }
        match self.audience {
            Audience::Headless => Err(ToolError::Denied {
                why: DECLINED.into(),
            }),
            Audience::Surface => ask(plan, cx).await,
        }
    }
}

async fn ask(plan: &str, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
    let relay = cx.relay.as_ref().ok_or_else(|| ToolError::Denied {
        why: DECLINED.into(),
    })?;
    let source = cx.agent.clone().map(|agent| Source {
        session: cx.session,
        agent: Some(agent),
        preset: cx.preset.clone(),
    });
    let question = format!("Leave Plan mode and carry out this plan?\n{plan}");
    let options = [EXIT_PLAN_APPROVE.to_string(), EXIT_PLAN_KEEP.to_string()];
    let answer = tokio::select! {
        biased;
        _ = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
        answer = relay.ask(cx.call, &question, &options, source) => answer?,
    };
    let Some(answer) = answer else {
        return Err(ToolError::Denied {
            why: "leaving Plan mode was dismissed without an answer".into(),
        });
    };
    let approved = answer == EXIT_PLAN_APPROVE;
    let text = if approved {
        "The user approved the plan. The session leaves Plan mode; proceed with it.\n".to_string()
    } else if answer == EXIT_PLAN_KEEP {
        "The user wants to keep planning. Stay in Plan mode and revise the plan.\n".to_string()
    } else {
        format!("The user wants to keep planning. Stay in Plan mode.\nFeedback: {answer}\n")
    };
    Ok(ToolOutput {
        text,
        is_error: false,
        diff: None,
        structured: Some(json!({
            "approved": approved,
            "plan": plan,
        })),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

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

    /// T67.7: a headless tool declines even when a relay is attached, and
    /// never reports approval, so the session has nothing to switch on.
    #[tokio::test]
    async fn headless_declines_exit_plan() {
        let plan = json!({"plan": "rename the helper"});
        let mut with_relay = cx();
        with_relay.relay = Some(Arc::new(PanicIfAsked));
        let err = ExitPlanTool::headless()
            .call(plan, &with_relay)
            .await
            .expect_err("headless");
        assert!(matches!(err, ToolError::Denied { .. }), "{err}");
        let spec = ExitPlanTool::headless().spec();
        assert_eq!(spec.name, "exit_plan");
        assert_eq!(spec.risk, Risk::ReadOnly);
        assert!(!approved_exit(&ToolOutput {
            text: err.to_string(),
            is_error: true,
            diff: None,
            structured: Some(json!({"approved": true})),
        }));
    }

    struct PanicIfAsked;

    #[async_trait]
    impl Relay for PanicIfAsked {
        async fn send_message(&self, _to: &str, _text: &str) -> Result<(), ToolError> {
            Ok(())
        }
        async fn ask(
            &self,
            _call_id: CallId,
            _question: &str,
            _options: &[String],
            _source: Option<Source>,
        ) -> Result<Option<String>, ToolError> {
            panic!("headless exit_plan must not ask");
        }
    }
}
