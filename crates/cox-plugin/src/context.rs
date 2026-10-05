// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The context snapshot `cox_context` answers (PL§5, T33.9): session id,
//! cwd, tier and model, usage totals, the last 50 transcript items with
//! their assembled text, the todo list and the compactions — folded by the
//! host from the session's events, never the assembled `Request` (PL§12).
//! Its own module because it is state the host keeps between calls, shared
//! by every plugin of a session, while `hostfn` only answers calls.
//!
//! Redaction happens when the snapshot is read, over the assembled text: a
//! secret streamed across two `TextDelta`s is whole by then, where scrubbing
//! each event alone would miss it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use cox_protocol::types::{CompactReason, Event, ItemKind, Job, ModelId, Tier};
use cox_protocol::{CallId, ItemId, SessionId};
use cox_sanitize::redact::scrub;
use serde::Serialize;
use serde_json::Value;

/// How many transcript items the snapshot keeps (PL§5).
pub const ITEMS: usize = 50;

/// One transcript item as a plugin sees it.
#[derive(Debug, Clone, Serialize)]
struct Entry {
    /// The item id, or the call id for a tool result.
    id: String,
    /// `user`, `assistant`, `tool_call`, `tool_result`, `summary` or `notice`.
    kind: &'static str,
    /// The tool, for a call.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    /// Whether a tool result succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    /// The assembled text: message text, a call's subject, a result's
    /// visible output.
    text: String,
    #[serde(skip)]
    item: Option<ItemId>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct Totals {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
struct Compaction {
    before_tokens: u32,
    after_tokens: u32,
    reason: CompactReason,
}

#[derive(Debug, Clone, Default, Serialize)]
struct Snapshot {
    session: Option<SessionId>,
    cwd: Option<PathBuf>,
    tier: Option<Tier>,
    model: Option<ModelId>,
    usage: Totals,
    items: VecDeque<Entry>,
    /// The `items` of the last successful `todo` call.
    todo: Value,
    compactions: Vec<Compaction>,
    /// A `todo` call waiting for its result.
    #[serde(skip)]
    pending_todo: Option<(CallId, Value)>,
}

/// The session's folded context. One per session, shared by its plugins.
#[derive(Debug, Default)]
pub struct Context {
    state: Mutex<Snapshot>,
}

impl Context {
    /// An empty snapshot; `fold` fills it.
    pub fn new() -> Self {
        Self::default()
    }

    // A poisoned lock only means a fold panicked mid-update; every field
    // stays valid on its own, so keep serving.
    fn lock(&self) -> MutexGuard<'_, Snapshot> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Folds one event, in rollout order.
    pub fn fold(&self, ev: &Event) {
        let mut s = self.lock();
        match ev {
            Event::SessionStarted { session, cwd, .. } => {
                s.session = Some(*session);
                s.cwd = Some(cwd.clone());
            }
            Event::TurnStarted {
                job: Job::Main,
                tier,
                model,
                ..
            } => {
                s.tier = Some(*tier);
                s.model = Some(model.clone());
            }
            Event::ModelSwitched { tier, to, .. } if s.tier == Some(*tier) => {
                s.model = Some(to.clone());
            }
            Event::ItemStarted { item, kind } => {
                if let Some(entry) = entry(*item, kind) {
                    if let ItemKind::ToolCall { call } = kind
                        && call.name == "todo"
                    {
                        s.pending_todo = Some((call.id, call.input["items"].clone()));
                    }
                    s.push(entry);
                }
            }
            Event::TextDelta { item, text } => {
                if let Some(e) = s.items.iter_mut().rev().find(|e| e.item == Some(*item)) {
                    e.text.push_str(text);
                }
            }
            Event::ToolCallDone { call_id, result } => {
                if result.ok
                    && let Some((id, items)) = s.pending_todo.take()
                {
                    if id == *call_id {
                        s.todo = items;
                    } else {
                        s.pending_todo = Some((id, items));
                    }
                }
                s.push(Entry {
                    id: call_id.to_string(),
                    kind: "tool_result",
                    tool: None,
                    ok: Some(result.ok),
                    text: result.visible.clone(),
                    item: None,
                });
            }
            Event::Usage { usage, .. } => {
                let t = &mut s.usage;
                t.input_tokens += u64::from(usage.input_tokens);
                t.output_tokens += u64::from(usage.output_tokens);
                t.cache_read_tokens += u64::from(usage.cache_read_tokens);
                t.cache_write_tokens += u64::from(usage.cache_write_tokens);
                t.cost_usd += usage.cost_usd;
            }
            Event::Compacted {
                before_tokens,
                after_tokens,
                reason,
                ..
            } => s.compactions.push(Compaction {
                before_tokens: *before_tokens,
                after_tokens: *after_tokens,
                reason: *reason,
            }),
            _ => {}
        }
    }

    /// The snapshot as `cox_context` returns it, every string redacted.
    pub fn snapshot(&self) -> Value {
        let mut v = serde_json::to_value(&*self.lock()).unwrap_or(Value::Null);
        redact(&mut v);
        v
    }
}

impl Snapshot {
    fn push(&mut self, entry: Entry) {
        if self.items.len() == ITEMS {
            self.items.pop_front();
        }
        self.items.push_back(entry);
    }
}

/// The entry an item starts, or `None` for thinking, which is not part of
/// the visible transcript.
fn entry(item: ItemId, kind: &ItemKind) -> Option<Entry> {
    let (kind, tool, text) = match kind {
        ItemKind::UserMessage { text, .. } => ("user", None, text.clone()),
        ItemKind::AssistantMessage { text } => ("assistant", None, text.clone()),
        ItemKind::ToolCall { call } => ("tool_call", Some(call.name.clone()), call.subject.clone()),
        ItemKind::Summary { text } => ("summary", None, text.clone()),
        ItemKind::Notice { text, .. } => ("notice", None, text.clone()),
        ItemKind::Thinking { .. } | ItemKind::ToolResult { .. } => return None,
    };
    Some(Entry {
        id: item.to_string(),
        kind,
        tool,
        ok: None,
        text,
        item: Some(item),
    })
}

/// Scrubs every string in `v` on its own, so a replacement can never cut
/// through JSON syntax the way scrubbing the serialised text could.
fn redact(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let std::borrow::Cow::Owned(clean) = scrub(s) {
                *s = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::Object(map) => map.values_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::types::{Risk, ToolCall, ToolResult};
    use serde_json::json;

    use super::*;

    fn done(call_id: CallId, visible: &str) -> Event {
        Event::ToolCallDone {
            call_id,
            result: ToolResult {
                ok: true,
                visible: visible.into(),
                archive: None,
                bytes: visible.len() as u64,
                duration_ms: 1,
                diff: None,
                structured: None,
            },
        }
    }

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: name.into(),
            input,
            risk: Risk::ReadOnly,
            subject: name.into(),
            segments: None,
        }
    }

    #[test]
    fn context_snapshot_is_redacted() {
        let ctx = Context::new();
        let secret = "sk-abcdefghijklmnop";
        let bash = call("bash", json!({ "command": "env" }));
        ctx.fold(&Event::ItemStarted {
            item: ItemId::new(),
            kind: ItemKind::ToolCall { call: bash.clone() },
        });
        ctx.fold(&done(bash.id, &format!("OPENAI_API_KEY={secret}\n")));
        // A secret split across two deltas is whole in the assembled text.
        let reply = ItemId::new();
        ctx.fold(&Event::ItemStarted {
            item: reply,
            kind: ItemKind::AssistantMessage {
                text: String::new(),
            },
        });
        ctx.fold(&Event::TextDelta {
            item: reply,
            text: "your key is sk-abcdefg".into(),
        });
        ctx.fold(&Event::TextDelta {
            item: reply,
            text: "hijklmnop, rotate it".into(),
        });

        let snap = ctx.snapshot();
        let text = snap.to_string();
        assert!(!text.contains(secret), "{text}");
        assert!(!text.contains("sk-abcdefg"), "{text}");
        let items = snap["items"].as_array().expect("items");
        assert_eq!(items[1]["kind"], "tool_result");
        assert_eq!(items[1]["text"], "OPENAI_API_KEY=«redacted»\n");
        assert_eq!(items[2]["text"], "your key is «redacted», rotate it");
    }

    #[test]
    fn snapshot_keeps_the_last_fifty_items_and_the_todo_list() {
        let ctx = Context::new();
        for i in 0..(ITEMS + 5) {
            ctx.fold(&Event::ItemStarted {
                item: ItemId::new(),
                kind: ItemKind::UserMessage {
                    text: format!("m{i}"),
                    attachments: Vec::new(),
                },
            });
        }
        let todo = call(
            "todo",
            json!({ "items": [{ "id": "1", "text": "t", "state": "done" }] }),
        );
        ctx.fold(&Event::ToolCallRequested { call: todo.clone() });
        ctx.fold(&Event::ItemStarted {
            item: ItemId::new(),
            kind: ItemKind::ToolCall { call: todo.clone() },
        });
        ctx.fold(&done(todo.id, "1 [x] t"));

        let snap = ctx.snapshot();
        let items = snap["items"].as_array().expect("items");
        assert_eq!(items.len(), ITEMS);
        assert_eq!(items[0]["text"], "m7");
        assert_eq!(snap["todo"][0]["state"], "done");
    }
}
