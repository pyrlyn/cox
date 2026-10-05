// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Unconditional redaction of what leaves a session (T28.4): `scrub_event`,
//! the per-event copy every output boundary — rollout lines, logs, headless
//! output, exports — writes. The pattern table and `scrub` itself live in
//! `cox_sanitize::redact` (T33.9) so they exist exactly once; what the model
//! needs for the task never passes through either (redacting model input is
//! out of scope).

use std::borrow::Cow;

use cox_protocol::types::Event;
// T33.9: the pattern table moved to `cox-sanitize` so the plugin host
// redacts with the same one; re-exported so `cox_core::redact::scrub` and
// `REDACTED` keep working.
pub use cox_sanitize::redact::{REDACTED, scrub};

/// The copy of `ev` for output boundaries: its `TextDelta`,
/// `ToolCallOutput` and `ToolCallDone` text scrubbed. Everything the model
/// needs — user text, tool-call input, thinking — is left verbatim (out of
/// scope), and an unchanged event is returned by borrow.
pub fn scrub_event(ev: &Event) -> Cow<'_, Event> {
    match ev {
        Event::TextDelta { item, text } => match changed(text) {
            Some(text) => Cow::Owned(Event::TextDelta { item: *item, text }),
            None => Cow::Borrowed(ev),
        },
        Event::ToolCallOutput { call_id, delta } => match changed(delta) {
            Some(delta) => Cow::Owned(Event::ToolCallOutput {
                call_id: *call_id,
                delta,
            }),
            None => Cow::Borrowed(ev),
        },
        Event::ToolCallDone { call_id, result } => {
            let visible = changed(&result.visible);
            let unified = result.diff.as_ref().and_then(|d| changed(&d.unified));
            if visible.is_none() && unified.is_none() {
                return Cow::Borrowed(ev);
            }
            let mut result = result.clone();
            if let Some(visible) = visible {
                result.visible = visible;
            }
            if let (Some(unified), Some(diff)) = (unified, result.diff.as_mut()) {
                diff.unified = unified;
            }
            Cow::Owned(Event::ToolCallDone {
                call_id: *call_id,
                result,
            })
        }
        _ => Cow::Borrowed(ev),
    }
}

/// `Some(scrubbed)` only when `scrub` changed something.
fn changed(text: &str) -> Option<String> {
    let scrubbed = scrub(text);
    (scrubbed.as_ref() != text).then(|| scrubbed.into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use cox_protocol::errors::ToolError;
    use cox_protocol::ids::SessionId;
    use cox_protocol::traits::{Store as _, Tool, ToolCx};
    use cox_protocol::types::{Concurrency, Level, Risk, Submission, ToolOutput, ToolSpec};
    use cox_provider::scripted::Scripted;
    use serde_json::Value;

    use crate::{MemoryStore, Session};

    use super::*;

    /// A tool whose *output* is secret-shaped: the scenario scripts the
    /// model, this scripts the tool side (a secret in `input` would be
    /// model input — out of scope — and legitimately stay verbatim).
    struct Leaky;

    #[async_trait::async_trait]
    impl Tool for Leaky {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "leak".into(),
                description: "returns a secret-shaped string".into(),
                input_schema: serde_json::json!({"type": "object"}),
                deferred: false,
                risk: Risk::ReadOnly,
                concurrency: Concurrency::Parallel,
            }
        }
        fn subject(&self, _input: &Value) -> String {
            String::new()
        }
        async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                text: "key sk-abc12345678 end".into(),
                is_error: false,
                diff: None,
                structured: None,
            })
        }
    }

    #[tokio::test]
    async fn rollout_never_contains_key_patterns() {
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
        let provider = Arc::new(
            Scripted::from_toml(
                include_str!("../tests/scenarios/secret_tool_output.toml"),
                "",
            )
            .expect("scenario"),
        );
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            config,
            provider,
            vec![Arc::new(Leaky)],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-turn"),
        )
        .expect("session");
        session
            .submit(Submission::UserTurn {
                text: "run the tool".into(),
                attachments: vec![],
                confirm_think: false,
            })
            .await
            .expect("turn");
        let events = store.rollout_read(&SessionId::new()).expect("rollout");
        let rollout = serde_json::to_string(&events).expect("rollout json");
        assert!(
            !rollout.contains("sk-abc12345678"),
            "the rollout kept the secret-shaped tool output"
        );
        assert!(rollout.contains(REDACTED), "nothing was redacted at all");
        assert_eq!(
            scrub(&rollout).as_ref(),
            rollout.as_str(),
            "the rollout still contains a key pattern"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Notice {
                    level: Level::Security,
                    text
                } if text.contains("redacted in the rollout")
            )),
            "no Notice(Security) after the redaction"
        );
        // The model's own input is out of scope: it still sees the original.
        let history = serde_json::to_string(&session.history().await).expect("history json");
        assert!(
            history.contains("sk-abc12345678"),
            "the model lost the tool output"
        );
    }
}
