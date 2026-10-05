// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Event` → ACP `SessionUpdate` mapping (T11.2 in plan numbering, T11.1
//! task): agent message and thought chunks, tool call start/progress/done
//! with locations, and the `todo` plan. Pure over one event plus the call
//! table the forwarder keeps; approvals travel the permission-request flow
//! in `server.rs`, never as updates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, MessageId, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    SessionUpdate, TextContent, ToolCall, ToolCallContent, ToolCallId, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use cox_protocol::ids::CallId;
use cox_protocol::types::{
    Event, Risk, StopReason as CoxStop, TodoItem, TodoState, ToolCall as CoxCall,
};

/// Remembers each live call's name and subject between `ToolCallRequested`
/// and `ToolCallDone` (the latter carries only an id).
#[derive(Debug, Default)]
pub struct CallTable {
    inner: HashMap<CallId, (String, String)>,
}

impl CallTable {
    /// Empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a requested call; returns its updates (start + locations).
    pub fn requested(&mut self, call: &CoxCall, cwd: &Path) -> Vec<SessionUpdate> {
        self.inner
            .insert(call.id, (call.name.clone(), call.subject.clone()));
        let mut tool = ToolCall::new(ToolCallId::new(call.id.to_string()), &call.subject)
            .kind(kind_for(&call.name, call.risk))
            .status(ToolCallStatus::InProgress);
        tool.locations = locations_for(&call.name, &call.subject, cwd);
        tool.raw_input = Some(call.input.clone());
        vec![SessionUpdate::ToolCall(tool)]
    }

    /// Looks a finished call up by id.
    pub fn get(&self, id: &CallId) -> Option<&(String, String)> {
        self.inner.get(id)
    }

    /// Forgets a finished call.
    pub fn remove(&mut self, id: &CallId) {
        self.inner.remove(id);
    }
}

/// Maps one `Event` to zero or more updates. `todo` results become `Plan`
/// updates; approvals map to nothing here (see `server.rs`).
pub fn updates_for(calls: &mut CallTable, event: &Event, cwd: &Path) -> Vec<SessionUpdate> {
    match event {
        Event::TextDelta { item, text } => {
            let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text.clone())))
                .message_id(MessageId::new(item.to_string()));
            vec![SessionUpdate::AgentMessageChunk(chunk)]
        }
        Event::ThinkingDelta { item, text } => {
            let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text.clone())))
                .message_id(MessageId::new(item.to_string()));
            vec![SessionUpdate::AgentThoughtChunk(chunk)]
        }
        Event::ToolCallRequested { call } => calls.requested(call, cwd),
        // Streaming stdout would spam one update per delta; the Done update
        // carries the result.
        Event::ToolCallOutput { .. } => vec![],
        Event::ToolCallDone { call_id, result } => {
            let name = calls
                .get(call_id)
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
            calls.remove(call_id);
            let mut fields = ToolCallUpdateFields::new().status(Some(if result.ok {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            }));
            fields.content = Some(vec![ToolCallContent::from(ContentBlock::Text(
                TextContent::new(result.visible.clone()),
            ))]);
            let update = ToolCallUpdate::new(ToolCallId::new(call_id.to_string()), fields);
            let mut out = vec![SessionUpdate::ToolCallUpdate(update)];
            if name == "todo"
                && result.ok
                && let Some(plan) = result.todo_list().and_then(plan_from)
            {
                out.push(SessionUpdate::Plan(plan));
            }
            out
        }
        _ => vec![],
    }
}

/// cox stop reasons to ACP stop reasons. `Budget` and `Error` have no ACP
/// counterpart, so the driver sends the detail as a final message chunk and
/// reports `Refusal`.
pub fn map_stop(stop: &CoxStop) -> agent_client_protocol::schema::v1::StopReason {
    use agent_client_protocol::schema::v1::StopReason as AcpStop;
    match stop {
        CoxStop::EndTurn => AcpStop::EndTurn,
        CoxStop::MaxTurns => AcpStop::MaxTurnRequests,
        CoxStop::Interrupted => AcpStop::Cancelled,
        CoxStop::Budget | CoxStop::Refusal { .. } | CoxStop::Error => AcpStop::Refusal,
    }
}

/// Final message chunk for stops that carry detail, if any.
pub fn stop_detail(stop: &CoxStop) -> Option<String> {
    match stop {
        CoxStop::Refusal { detail } => Some(detail.clone()),
        CoxStop::Budget => Some("budget limit reached".to_string()),
        CoxStop::Error => Some("turn failed".to_string()),
        _ => None,
    }
}

fn kind_for(name: &str, risk: Risk) -> ToolKind {
    match name {
        "read" => ToolKind::Read,
        "edit" | "write" | "apply_patch" => ToolKind::Edit,
        "grep" | "glob" => ToolKind::Search,
        "bash" => ToolKind::Execute,
        "todo" => ToolKind::Think,
        "web_fetch" | "web-fetch" => ToolKind::Fetch,
        _ if name.starts_with("mcp__") => ToolKind::Other,
        _ => match risk {
            Risk::ReadOnly => ToolKind::Read,
            Risk::Write => ToolKind::Edit,
            Risk::Exec => ToolKind::Execute,
            Risk::Destructive => ToolKind::Delete,
        },
    }
}

/// Absolute file locations for file-shaped calls, so clients can follow
/// along; anything else gets no locations.
fn locations_for(name: &str, subject: &str, cwd: &Path) -> Vec<ToolCallLocation> {
    if !matches!(name, "read" | "edit" | "write" | "apply_patch") || subject.is_empty() {
        return Vec::new();
    }
    let path: PathBuf = if Path::new(subject).is_absolute() {
        PathBuf::from(subject)
    } else {
        cwd.join(subject)
    };
    vec![ToolCallLocation::new(path)]
}

/// The `todo` tool's structured list as a `Plan`.
fn plan_from(items: Vec<TodoItem>) -> Option<Plan> {
    let entries: Vec<PlanEntry> = items
        .into_iter()
        .map(|item| {
            let status = match item.state {
                TodoState::Done => PlanEntryStatus::Completed,
                TodoState::InProgress => PlanEntryStatus::InProgress,
                TodoState::Pending => PlanEntryStatus::Pending,
            };
            PlanEntry::new(item.text, PlanEntryPriority::Medium, status)
        })
        .collect();
    (!entries.is_empty()).then(|| Plan::new(entries))
}

#[cfg(test)]
mod tests {
    use cox_protocol::types::ToolResult;
    use serde_json::json;

    use super::*;

    #[test]
    fn todo_plan_comes_from_the_structured_list_not_the_text() {
        let mut calls = CallTable::default();
        let id = CallId::new();
        let call = CoxCall {
            id,
            name: "todo".into(),
            input: json!({}),
            risk: Risk::ReadOnly,
            subject: "todo".into(),
            segments: None,
        };
        updates_for(
            &mut calls,
            &Event::ToolCallRequested { call },
            Path::new("/"),
        );
        let done = Event::ToolCallDone {
            call_id: id,
            result: ToolResult {
                ok: true,
                visible: "not a parseable list".into(),
                archive: None,
                bytes: 0,
                duration_ms: 0,
                diff: None,
                structured: Some(Box::new(json!([
                    {"id": "1", "text": "read", "state": "done"},
                    {"id": "2", "text": "write", "state": "in_progress"},
                ]))),
            },
        };
        let updates = updates_for(&mut calls, &done, Path::new("/"));
        let Some(SessionUpdate::Plan(plan)) = updates.last() else {
            panic!("expected a plan update, got {updates:?}");
        };
        let got: Vec<_> = plan
            .entries
            .iter()
            .map(|e| (e.content.as_str(), e.status.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("read", PlanEntryStatus::Completed),
                ("write", PlanEntryStatus::InProgress),
            ]
        );
    }
}
