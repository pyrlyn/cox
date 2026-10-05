// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `send_message`: a subagent's follow-up to a sibling or the parent, or
//! the parent's own follow-up to a child (T34.6, SM§4). Thin glue only —
//! parsing plus one call through `cx.relay`. Stateless (T34.6 review): the
//! tool holds no `Relay` of its own, since one shared instance is handed
//! to every session's tool list (`crates/cox-session`'s `open`, the same
//! `tools()` list `ask_user` is built into) and a child may in future get
//! it too (T34.1 custom presets can list any built-in tool by name). The
//! `Relay` that answers a given call must be *that call's own session* —
//! `cox-core/src/turn.rs` stamps `ToolCx.relay` with the session that
//! built it, the same pattern `agent`/`preset` (T34.3) already use — never
//! a handle fixed at construction time, or two children sharing this tool
//! would misroute through whichever session built it first. It never
//! touches the registry itself.

use async_trait::async_trait;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// `send_message { to, text }`.
pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_message".to_string(),
            description: "Send a follow-up message to another subagent task or to the \
                parent session. Pass `to` (\"parent\", a sibling's registry name such as \
                `explore-2`, or its task id) and `text`. The addressee gets it as its next \
                turn; a finished subagent is woken to answer it, capped by the session's \
                concurrency limit."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string"},
                    "text": {"type": "string"}
                },
                "required": ["to", "text"]
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let to = str_field(&input, "to")?;
        let text = str_field(&input, "text")?;
        let relay = cx.relay.as_ref().ok_or_else(|| ToolError::Denied {
            why: "send_message is not available here".into(),
        })?;
        relay.send_message(&to, &text).await?;
        Ok(ToolOutput {
            text: format!("message sent to {to}"),
            is_error: false,
            diff: None,
            structured: Some(json!({"to": to})),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use cox_protocol::{Archive, ArchiveId, ArchivePut, Relay, StoreError};

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

    /// A fake `Relay` recording what it was asked to send, so this crate's
    /// own test proves the tool's parsing/plumbing without needing a real
    /// `Session` (`cox-core` depends on `cox-tools`, never the reverse).
    struct FakeRelay {
        name: &'static str,
        seen: StdMutex<Vec<(String, String)>>,
        deny: bool,
    }

    #[async_trait]
    impl Relay for FakeRelay {
        async fn send_message(&self, to: &str, text: &str) -> Result<(), ToolError> {
            if self.deny {
                return Err(ToolError::Denied {
                    why: "denied by fake".into(),
                });
            }
            self.seen
                .lock()
                .expect("lock")
                .push((to.to_string(), text.to_string()));
            Ok(())
        }
    }

    fn cx(relay: Option<Arc<dyn Relay>>) -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        ToolCx {
            relay,
            ..crate::tool_cx(
                vec![],
                std::path::PathBuf::from("/tmp"),
                cox_protocol::SandboxPolicy {
                    mode: cox_protocol::SandboxMode::ReadOnly,
                    network: false,
                    writable: vec![],
                    readonly_in_workspace: vec![],
                    linux_backend: Default::default(),
                },
                Arc::new(NoopArchive),
                tokio_util::sync::CancellationToken::new(),
                tx,
                cox_protocol::SessionId::new(),
                cox_protocol::CallId::new(),
            )
        }
    }

    #[tokio::test]
    async fn send_message_calls_relay_with_parsed_fields() {
        let relay = Arc::new(FakeRelay {
            name: "r",
            seen: StdMutex::new(Vec::new()),
            deny: false,
        });
        let tool = SendMessageTool;
        let out = tool
            .call(
                json!({"to": "explore-2", "text": "ping"}),
                &cx(Some(relay.clone())),
            )
            .await
            .expect("sent");
        assert!(out.text.contains("explore-2"));
        assert_eq!(
            relay.seen.lock().expect("lock").as_slice(),
            &[("explore-2".to_string(), "ping".to_string())]
        );
    }

    #[tokio::test]
    async fn send_message_propagates_a_denial_from_relay() {
        let relay = Arc::new(FakeRelay {
            name: "r",
            seen: StdMutex::new(Vec::new()),
            deny: true,
        });
        let tool = SendMessageTool;
        let err = tool
            .call(json!({"to": "parent", "text": "hi"}), &cx(Some(relay)))
            .await
            .expect_err("denied");
        assert!(matches!(err, ToolError::Denied { .. }));
    }

    #[tokio::test]
    async fn call_with_no_relay_is_denied() {
        let tool = SendMessageTool;
        let err = tool
            .call(json!({"to": "parent", "text": "hi"}), &cx(None))
            .await
            .expect_err("no relay wired");
        assert!(matches!(err, ToolError::Denied { .. }));
    }

    /// T34.6 review: the bug this guards against is a `Relay` fixed once at
    /// construction and shared — with one `SendMessageTool` handed to every
    /// session's tool list, that would route every call through whichever
    /// session happened to build it first, misrouting a second session's
    /// (e.g. a sibling child's) calls. `SendMessageTool` now holds no state
    /// at all, so the *same* instance must reach each call's own `cx.relay`
    /// and never the other's.
    #[tokio::test]
    async fn the_same_tool_instance_reaches_each_calls_own_relay() {
        let tool = SendMessageTool;
        let a = Arc::new(FakeRelay {
            name: "a",
            seen: StdMutex::new(Vec::new()),
            deny: false,
        });
        let b = Arc::new(FakeRelay {
            name: "b",
            seen: StdMutex::new(Vec::new()),
            deny: false,
        });
        tool.call(json!({"to": "x", "text": "from a"}), &cx(Some(a.clone())))
            .await
            .expect("sent via a");
        tool.call(json!({"to": "y", "text": "from b"}), &cx(Some(b.clone())))
            .await
            .expect("sent via b");
        assert_eq!(
            a.seen.lock().expect("lock").as_slice(),
            &[("x".to_string(), "from a".to_string())],
            "{}'s relay must see only its own call",
            a.name
        );
        assert_eq!(
            b.seen.lock().expect("lock").as_slice(),
            &[("y".to_string(), "from b".to_string())],
            "{}'s relay must see only its own call",
            b.name
        );
    }
}
