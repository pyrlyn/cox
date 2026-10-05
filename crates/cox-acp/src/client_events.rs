// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The fold from an external agent's ACP `session/update` notifications to
//! cox `Event`s (T52.3, DT§3.3.1). A top-level session driven by Claude
//! Agent, Codex, Gemini CLI or Cursor streams through the same `Timeline`,
//! patch coalescer and rollout as a cox session, so there is no second fold
//! (DT-7). Its own module beside `client.rs`: the client answers what the
//! agent asks, this only reads what the agent reports.
//!
//! Pure: no clock (the caller passes a millisecond timestamp with each
//! update), no I/O, no archive. Every string the agent wrote passes
//! `cox_sanitize::sanitize` here, so no consumer sees raw agent text.
//! Anything it does not know is one `Notice(Info)`, never a failure (fail
//! open, EA§7): a newer agent must not break a turn by adding a kind.

use std::collections::HashMap;
use std::path::Path;

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v1::{
    ContentBlock, Plan, PlanEntryStatus, SessionUpdate, StopReason as AcpStop, ToolCall as AcpCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind, UsageUpdate,
};
use cox_protocol::{
    CallId, ContextBreakdown, CoreError, Diff, Event, ItemId, ItemKind, Level, Risk, StopReason,
    TodoItem, TodoState, ToolCall, ToolResult, TurnId,
};
use cox_sanitize::sanitize;
use serde_json::Value;

use crate::client::kind_tool;

/// The item that is still growing: ACP streams text and thoughts as chunks
/// with no end marker, so one ends when another kind arrives or the turn does.
enum Open {
    None,
    Message(ItemId),
    Thought {
        item: ItemId,
        first_ms: u64,
        last_ms: u64,
    },
}

/// One of the agent's tool calls, by its ACP `toolCallId`.
struct Call {
    item: ItemId,
    id: CallId,
    started_ms: u64,
    /// The text content so far; ACP replaces a call's content on each
    /// update, so this is what the next `ToolCallOutput` delta extends.
    output: String,
    diff: Option<Diff>,
    done: bool,
}

/// What the agent reports that is not an `Event` (DT§3.3.1): its slash
/// commands for the composer's completion, and its current mode for the
/// Inspector's Info tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentInfo {
    /// `/name` of each command the agent advertised, sanitized.
    pub commands: Vec<String>,
    /// The agent's own mode id, sanitized.
    pub mode: Option<String>,
}

/// Folds one turn's `session/update`s into cox events. One per session; the
/// caller starts each turn with [`UpdateFold::start_turn`].
pub struct UpdateFold {
    agent: String,
    turn: TurnId,
    open: Open,
    calls: HashMap<String, Call>,
    plan: Option<(ItemId, CallId)>,
    replaying: bool,
    info: AgentInfo,
}

impl UpdateFold {
    /// A fold for the session driven by `agent` (its display name).
    pub fn new(agent: impl Into<String>, turn: TurnId) -> Self {
        Self {
            agent: agent.into(),
            turn,
            open: Open::None,
            calls: HashMap::new(),
            plan: None,
            replaying: false,
            info: AgentInfo::default(),
        }
    }

    /// Starts folding for the next turn. The caller emits `TurnStarted` and
    /// the user item itself: the agent does not echo a live prompt.
    pub fn start_turn(&mut self, turn: TurnId) {
        self.turn = turn;
        self.plan = None;
    }

    /// While `on`, `user_message_chunk`s become user items: that is how a
    /// `session/load` replays history. Off, they are the agent echoing the
    /// prompt cox already recorded, and are dropped.
    pub fn set_replaying(&mut self, on: bool) {
        self.replaying = on;
    }

    /// The commands and mode the agent reported so far.
    pub fn info(&self) -> &AgentInfo {
        &self.info
    }

    /// The events one raw `update` object stands for. A kind the SDK cannot
    /// parse (a newer agent's, or an unstable one cox does not enable) is a
    /// notice naming its tag, never an error.
    pub fn update_json(&mut self, update: &Value, at_ms: u64) -> Vec<Event> {
        match serde_json::from_value::<SessionUpdate>(update.clone()) {
            Ok(update) => self.update(update, at_ms),
            Err(_) => {
                let tag = update["sessionUpdate"].as_str().unwrap_or("(no kind)");
                vec![self.unknown(tag)]
            }
        }
    }

    /// The events one `session/update` stands for.
    pub fn update(&mut self, update: SessionUpdate, at_ms: u64) -> Vec<Event> {
        let mut out = Vec::new();
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => match text_of(&chunk.content) {
                Some(text) => self.message(&mut out, &text),
                None => out.push(self.non_text("message")),
            },
            SessionUpdate::AgentThoughtChunk(chunk) => match text_of(&chunk.content) {
                Some(text) => self.thought(&mut out, &text, at_ms),
                None => out.push(self.non_text("thought")),
            },
            SessionUpdate::UserMessageChunk(chunk) if self.replaying => {
                self.close(&mut out);
                if let Some(text) = text_of(&chunk.content) {
                    let item = ItemId::new();
                    let kind = ItemKind::UserMessage {
                        text,
                        attachments: Vec::new(),
                    };
                    out.extend([Event::ItemStarted { item, kind }, Event::ItemDone { item }]);
                }
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::ToolCall(call) => {
                self.close(&mut out);
                self.tool_call(&mut out, call, at_ms);
            }
            SessionUpdate::ToolCallUpdate(update) => {
                self.close(&mut out);
                self.tool_update(&mut out, update, at_ms);
            }
            SessionUpdate::Plan(plan) => {
                self.close(&mut out);
                self.plan(&mut out, &plan);
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                if let MaybeUndefined::Value(title) = info.title {
                    let title = sanitize(title.lines().next().unwrap_or_default());
                    out.push(Event::TitleSet {
                        title,
                        by_user: false,
                    });
                }
            }
            SessionUpdate::UsageUpdate(usage) => out.push(self.context(&usage)),
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.info.commands = update
                    .available_commands
                    .iter()
                    .map(|c| sanitize(&format!("/{}", c.name)))
                    .collect();
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                self.info.mode = Some(sanitize(&update.current_mode_id.to_string()));
            }
            SessionUpdate::ConfigOptionUpdate(_) => {}
            other => {
                let tag = serde_json::to_value(&other)
                    .ok()
                    .and_then(|v| v["sessionUpdate"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| String::from("(unknown)"));
                out.push(self.unknown(&tag));
            }
        }
        out
    }

    /// The prompt's end: open items close, calls the agent never finished
    /// close (failed unless the turn ended normally), and `TurnDone` follows.
    /// A stop other than `end_turn` or `cancelled` is named in a warning.
    pub fn stop(&mut self, reason: AcpStop, at_ms: u64) -> Vec<Event> {
        let mut out = Vec::new();
        self.close(&mut out);
        let (stop, warn) = match reason {
            AcpStop::EndTurn => (StopReason::EndTurn, None),
            AcpStop::Cancelled => (StopReason::Interrupted, None),
            AcpStop::MaxTokens => (StopReason::EndTurn, Some("its output token limit")),
            AcpStop::MaxTurnRequests => (StopReason::MaxTurns, Some("its request limit")),
            AcpStop::Refusal => (
                StopReason::Refusal {
                    detail: String::new(),
                },
                Some("a refusal"),
            ),
            _ => (StopReason::EndTurn, Some("an unknown stop reason")),
        };
        self.finish_calls(&mut out, stop == StopReason::EndTurn, at_ms);
        if let Some(why) = warn {
            out.push(Event::Notice {
                level: Level::Warn,
                text: format!("{} stopped the turn: {why}", self.agent),
            });
        }
        out.push(Event::TurnDone {
            turn: self.turn,
            stop,
        });
        out
    }

    /// A JSON-RPC error or the process exiting mid-turn (DT§3.3.1): open
    /// items and calls close, then `Error(ExternalAgent)` and `TurnDone`.
    /// `message` is the error and the stderr tail; it is sanitized here.
    pub fn failed(&mut self, message: &str, at_ms: u64) -> Vec<Event> {
        let mut out = Vec::new();
        self.close(&mut out);
        self.finish_calls(&mut out, false, at_ms);
        out.push(Event::Error {
            error: CoreError::ExternalAgent {
                agent: self.agent.clone(),
                message: sanitize(message),
            },
            fatal: false,
        });
        out.push(Event::TurnDone {
            turn: self.turn,
            stop: StopReason::Error,
        });
        out
    }

    fn message(&mut self, out: &mut Vec<Event>, text: &str) {
        let item = match self.open {
            Open::Message(item) => item,
            _ => {
                self.close(out);
                let item = ItemId::new();
                let kind = ItemKind::AssistantMessage {
                    text: String::new(),
                };
                out.push(Event::ItemStarted { item, kind });
                self.open = Open::Message(item);
                item
            }
        };
        out.push(Event::TextDelta {
            item,
            text: sanitize(text),
        });
    }

    fn thought(&mut self, out: &mut Vec<Event>, text: &str, at_ms: u64) {
        let item = match &mut self.open {
            Open::Thought { item, last_ms, .. } => {
                *last_ms = at_ms;
                *item
            }
            _ => {
                self.close(out);
                let item = ItemId::new();
                let kind = ItemKind::Thinking {
                    text: String::new(),
                    signature: None,
                };
                out.push(Event::ItemStarted { item, kind });
                self.open = Open::Thought {
                    item,
                    first_ms: at_ms,
                    last_ms: at_ms,
                };
                item
            }
        };
        out.push(Event::ThinkingDelta {
            item,
            text: sanitize(text),
        });
    }

    /// Ends the growing message or thought, if any.
    fn close(&mut self, out: &mut Vec<Event>) {
        match std::mem::replace(&mut self.open, Open::None) {
            Open::None => {}
            Open::Message(item) => out.push(Event::ItemDone { item }),
            Open::Thought {
                item,
                first_ms,
                last_ms,
            } => {
                let duration_ms = last_ms.saturating_sub(first_ms);
                out.extend([
                    Event::ThinkingDone { item, duration_ms },
                    Event::ItemDone { item },
                ]);
            }
        }
    }

    /// A new call: shown as run by the agent, never an approval (the
    /// agent's own tool calls run inside its sandboxed process, EA§2).
    fn tool_call(&mut self, out: &mut Vec<Event>, call: AcpCall, at_ms: u64) {
        let key = call.tool_call_id.0.to_string();
        if !self.calls.contains_key(&key) {
            let input = call.raw_input.clone();
            self.open_call(out, &key, &call.title, Some(call.kind), input, at_ms);
        }
        self.apply(out, &key, Some(&call.content), Some(call.status), at_ms);
    }

    fn tool_update(&mut self, out: &mut Vec<Event>, update: ToolCallUpdate, at_ms: u64) {
        let key = update.tool_call_id.0.to_string();
        let f = update.fields;
        if !self.calls.contains_key(&key) {
            // An update for a call it never announced still becomes one item.
            let title = f.title.clone().unwrap_or_else(|| String::from("tool call"));
            self.open_call(out, &key, &title, f.kind, f.raw_input.clone(), at_ms);
        }
        self.apply(out, &key, f.content.as_ref(), f.status, at_ms);
    }

    fn open_call(
        &mut self,
        out: &mut Vec<Event>,
        key: &str,
        title: &str,
        kind: Option<ToolKind>,
        input: Option<Value>,
        at_ms: u64,
    ) {
        let (_, risk) = kind_tool(kind);
        let title = sanitize(title);
        let call = ToolCall {
            id: CallId::new(),
            name: title.clone(),
            input: input.unwrap_or(Value::Null),
            risk,
            subject: title,
            segments: None,
        };
        let item = ItemId::new();
        self.calls.insert(
            key.to_owned(),
            Call {
                item,
                id: call.id,
                started_ms: at_ms,
                output: String::new(),
                diff: None,
                done: false,
            },
        );
        out.extend([
            Event::ItemStarted {
                item,
                kind: ItemKind::ToolCall { call: call.clone() },
            },
            Event::ToolCallRequested { call },
        ]);
    }

    /// New content becomes output (only the part past what was shown), a
    /// diff is kept for `ToolCallDone`, and a final status finishes the call.
    fn apply(
        &mut self,
        out: &mut Vec<Event>,
        key: &str,
        content: Option<&Vec<ToolCallContent>>,
        status: Option<ToolCallStatus>,
        at_ms: u64,
    ) {
        let Some(call) = self.calls.get_mut(key) else {
            return;
        };
        if call.done {
            return;
        }
        if let Some(content) = content {
            let mut text = String::new();
            for part in content {
                match part {
                    ToolCallContent::Content(c) => {
                        if let Some(t) = text_of(&c.content) {
                            text.push_str(&t);
                        }
                    }
                    ToolCallContent::Diff(d) => {
                        call.diff = Some(unified(&d.path, d.old_text.as_deref(), &d.new_text));
                    }
                    ToolCallContent::Terminal(t) => {
                        text.push_str(&format!("[terminal {}]\n", t.terminal_id));
                    }
                    _ => {}
                }
            }
            let text = sanitize(&text);
            let delta = text.strip_prefix(call.output.as_str()).unwrap_or(&text);
            if !delta.is_empty() {
                out.push(Event::ToolCallOutput {
                    call_id: call.id,
                    delta: delta.to_owned(),
                });
            }
            if !text.is_empty() {
                call.output = text;
            }
        }
        match status {
            Some(ToolCallStatus::Completed) => finish(out, call, true, at_ms),
            Some(ToolCallStatus::Failed) => finish(out, call, false, at_ms),
            _ => {}
        }
    }

    /// Calls still open when the turn ends.
    fn finish_calls(&mut self, out: &mut Vec<Event>, ok: bool, at_ms: u64) {
        let mut open: Vec<&mut Call> = self.calls.values_mut().filter(|c| !c.done).collect();
        open.sort_by_key(|c| c.started_ms);
        for call in open {
            finish(out, call, ok, at_ms);
        }
        self.calls.clear();
    }

    /// The agent's plan as a `todo` result, which the Inspector's Plan tab
    /// already reads (`ToolResult::todo_list`, G3). One call per turn; a
    /// later plan in the same turn replaces it.
    fn plan(&mut self, out: &mut Vec<Event>, plan: &Plan) {
        let items: Vec<TodoItem> = plan
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| TodoItem {
                id: (i + 1).to_string(),
                text: sanitize(&e.content),
                state: match e.status {
                    PlanEntryStatus::InProgress => TodoState::InProgress,
                    PlanEntryStatus::Completed => TodoState::Done,
                    _ => TodoState::Pending,
                },
            })
            .collect();
        let (item, id) = self.plan.unwrap_or_else(|| (ItemId::new(), CallId::new()));
        if self.plan.is_none() {
            let call = ToolCall {
                id,
                name: String::from("todo"),
                input: Value::Null,
                risk: Risk::ReadOnly,
                subject: format!("{} plan", self.agent),
                segments: None,
            };
            out.extend([
                Event::ItemStarted {
                    item,
                    kind: ItemKind::ToolCall { call: call.clone() },
                },
                Event::ToolCallRequested { call },
            ]);
            self.plan = Some((item, id));
        }
        let visible = items
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let structured = serde_json::to_value(&items).ok().map(Box::new);
        out.push(Event::ToolCallDone {
            call_id: id,
            result: ToolResult {
                ok: true,
                bytes: visible.len() as u64,
                visible,
                archive: None,
                duration_ms: 0,
                diff: None,
                structured,
            },
        });
    }

    /// `usage_update` feeds the context ring only: no `Usage` event and no
    /// cost, since the agent's usage is its own billing (DT§3.3.1).
    fn context(&self, usage: &UsageUpdate) -> Event {
        let used = u32::try_from(usage.used).unwrap_or(u32::MAX);
        Event::ContextBreakdown {
            turn: self.turn,
            breakdown: ContextBreakdown {
                window: u32::try_from(usage.size).ok(),
                total: used,
                history: used,
                ..ContextBreakdown::default()
            },
        }
    }

    fn unknown(&self, tag: &str) -> Event {
        Event::Notice {
            level: Level::Info,
            text: sanitize(&format!(
                "{} sent an update cox does not show: {tag}",
                self.agent
            )),
        }
    }

    fn non_text(&self, what: &str) -> Event {
        Event::Notice {
            level: Level::Info,
            text: format!("{} sent a {what} part cox does not show", self.agent),
        }
    }
}

/// Closes one call: `ToolCallDone` with its output and diff, then `ItemDone`.
fn finish(out: &mut Vec<Event>, call: &mut Call, ok: bool, at_ms: u64) {
    call.done = true;
    let visible = call.output.clone();
    out.push(Event::ToolCallDone {
        call_id: call.id,
        result: ToolResult {
            ok,
            bytes: visible.len() as u64,
            visible,
            archive: None,
            duration_ms: at_ms.saturating_sub(call.started_ms),
            diff: call.diff.take(),
            structured: None,
        },
    });
    out.push(Event::ItemDone { item: call.item });
}

/// A content block's text; `None` for images, audio and resources.
fn text_of(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(t) => Some(t.text.clone()),
        _ => None,
    }
}

/// ACP's whole-file `oldText`/`newText` as the unified diff Changes and
/// Review read; `oldText` absent means the agent created the file.
fn unified(path: &Path, old: Option<&str>, new: &str) -> Diff {
    let label = sanitize(&path.display().to_string());
    let text = similar::TextDiff::from_lines(old.unwrap_or_default(), new)
        .unified_diff()
        .context_radius(3)
        .header(&label, &label)
        .to_string();
    Diff {
        path: path.to_path_buf(),
        unified: sanitize(&text),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt::Write as _;

    use super::*;

    /// Recorded `session/update` payloads, one JSON object per line, in the
    /// order an agent sends them for one prompt.
    const FIXTURE: &str = include_str!("../tests/fixtures/acp_updates.jsonl");

    fn fold_fixture() -> Vec<Event> {
        let mut fold = UpdateFold::new("Claude Agent", TurnId::new());
        let mut events = Vec::new();
        for (i, line) in FIXTURE.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let update: Value = serde_json::from_str(line).expect("fixture line is JSON");
            events.extend(fold.update_json(&update, 100 * i as u64));
        }
        events.extend(fold.stop(AcpStop::EndTurn, 10_000));
        events
    }

    /// Events as stable text: ids become `#n` in order of first sight.
    fn render(events: &[Event]) -> String {
        let mut ids: HashMap<String, usize> = HashMap::new();
        let mut out = String::new();
        for event in events {
            let mut v = serde_json::to_value(event).expect("event serializes");
            relabel(&mut v, &mut ids);
            let _ = writeln!(out, "{v}");
        }
        out
    }

    fn relabel(v: &mut Value, ids: &mut HashMap<String, usize>) {
        match v {
            Value::Object(map) => {
                for (k, child) in map.iter_mut() {
                    let is_id = matches!(k.as_str(), "item" | "id" | "call_id" | "turn");
                    if is_id && let Value::String(s) = child {
                        let n = ids.len() + 1;
                        let n = *ids.entry(s.clone()).or_insert(n);
                        *child = Value::String(format!("#{n}"));
                    } else {
                        relabel(child, ids);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|c| relabel(c, ids)),
            _ => {}
        }
    }

    /// T52.3 Check: the fixture turn — thought, message, a tool call with
    /// output and a diff, a plan, a title, usage, commands — folds into the
    /// events a cox session emits for the same things.
    #[test]
    fn acp_updates_fold_into_events_snapshot() {
        insta::assert_snapshot!(render(&fold_fixture()));
    }

    /// T52.3 Check: a kind the schema does not know, and a non-text part,
    /// are each one `Notice(Info)`; the fold goes on.
    #[test]
    fn acp_unknown_update_is_a_notice() {
        let mut fold = UpdateFold::new("Codex", TurnId::new());
        let unknown = serde_json::json!({"sessionUpdate": "brand_new_kind", "x": 1});
        let events = fold.update_json(&unknown, 0);
        let [Event::Notice { level, text }] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!(*level, Level::Info);
        assert!(
            text.contains("Codex") && text.contains("brand_new_kind"),
            "{text}"
        );
        let image = serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "image", "data": "AA==", "mimeType": "image/png"}
        });
        let events = fold.update_json(&image, 1);
        assert!(matches!(
            events.as_slice(),
            [Event::Notice {
                level: Level::Info,
                ..
            }]
        ));
        let text = serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "still here"}
        });
        let events = fold.update_json(&text, 2);
        assert!(matches!(
            events.as_slice(),
            [Event::ItemStarted { .. }, Event::TextDelta { .. }]
        ));
    }

    /// T52.3 Check: a `tool_call_update` carrying a diff finishes the call
    /// with that diff as a unified diff, so Changes and Review list the file.
    #[test]
    fn acp_tool_call_update_carries_its_diff() {
        let mut fold = UpdateFold::new("Gemini CLI", TurnId::new());
        let start = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Edit src/lib.rs",
            "kind": "edit",
            "status": "in_progress"
        });
        let done = serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
            "content": [{
                "type": "diff",
                "path": "/w/src/lib.rs",
                "oldText": "fn a() {}\n",
                "newText": "fn a() {}\nfn b() {}\n"
            }]
        });
        let mut events = fold.update_json(&start, 0);
        events.extend(fold.update_json(&done, 250));
        let result = events
            .iter()
            .find_map(|e| match e {
                Event::ToolCallDone { result, .. } => Some(result),
                _ => None,
            })
            .expect("the call finished");
        assert!(result.ok);
        assert_eq!(result.duration_ms, 250);
        let diff = result.diff.as_ref().expect("a diff");
        assert_eq!(diff.path, Path::new("/w/src/lib.rs"));
        assert!(diff.unified.contains("+fn b() {}"), "{}", diff.unified);
        assert!(matches!(events.last(), Some(Event::ItemDone { .. })));
        let requested = events.iter().find_map(|e| match e {
            Event::ToolCallRequested { call } => Some(call),
            _ => None,
        });
        assert_eq!(
            requested.map(|c| (c.name.as_str(), c.risk)),
            Some(("Edit src/lib.rs", Risk::Write))
        );
    }

    /// Escape sequences the agent writes never reach an event.
    #[test]
    fn acp_update_text_is_sanitized() {
        let mut fold = UpdateFold::new("Cursor", TurnId::new());
        let chunk = serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hi\u{1b}[2J there"}
        });
        let events = fold.update_json(&chunk, 0);
        let Some(Event::TextDelta { text, .. }) = events.last() else {
            panic!("{events:?}");
        };
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }

    /// A cancelled prompt ends as `Interrupted`; a call still running is
    /// closed as failed, so no block keeps spinning.
    #[test]
    fn acp_cancel_closes_open_calls() {
        let mut fold = UpdateFold::new("Codex", TurnId::new());
        let start = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t9",
            "title": "cargo test",
            "kind": "execute",
            "status": "in_progress"
        });
        let mut events = fold.update_json(&start, 0);
        events.extend(fold.stop(AcpStop::Cancelled, 40));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ToolCallDone { result, .. } if !result.ok
        )));
        assert!(matches!(
            events.last(),
            Some(Event::TurnDone {
                stop: StopReason::Interrupted,
                ..
            })
        ));
    }
}
