// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The "Needs you" inbox and each session's activity (DT§4.3, §5.1): one
//! fold over the event streams of every session this process drives, so
//! the sidebar, the Dock badge and notifications agree. Derived from
//! events, never stored: a restart rebuilds it by replaying. Separate from
//! the timeline because it spans sessions and keeps only what waits on the
//! person.

use std::collections::HashMap;

use cox_protocol::types::{Event, Job, Source, StopReason, ToolCall, Why};
use cox_protocol::{CallId, SessionId, TaskId};
use serde::{Deserialize, Serialize};

/// What one inbox item asks of the person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Need {
    Approval {
        call: ToolCall,
        why: Why,
    },
    Question {
        call_id: CallId,
        question: String,
        options: Vec<String>,
    },
    /// A turn stopped on an error or a refusal.
    Failed {
        text: String,
    },
    /// A background task finished; `ok` is false on a non-zero exit.
    TaskDone {
        task: TaskId,
        label: String,
        ok: bool,
    },
}

impl Need {
    /// The line a row and a notification show: the tool and its subject
    /// (the tool alone without one), the question, the error, the task.
    fn title(&self) -> String {
        match self {
            Need::Approval { call, .. } if call.subject.is_empty() => call.name.clone(),
            Need::Approval { call, .. } => format!("{} {}", call.name, call.subject),
            Need::Question { question, .. } => question.clone(),
            Need::Failed { text } => text.clone(),
            Need::TaskDone { label, .. } => label.clone(),
        }
    }

    /// The row's status dot and what it waits for.
    fn wait(&self) -> (InboxStatus, &'static str) {
        match self {
            Need::Approval { .. } => (InboxStatus::Waiting, "approval waiting"),
            Need::Question { .. } => (InboxStatus::Waiting, "question waiting"),
            Need::Failed { .. } => (InboxStatus::Error, "turn failed"),
            Need::TaskDone { ok: true, .. } => (InboxStatus::Idle, "task done"),
            Need::TaskDone { ok: false, .. } => (InboxStatus::Error, "task failed"),
        }
    }
}

/// An inbox row's status dot, named as the clients' status glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Waiting,
    Idle,
    Error,
}

/// One inbox row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxItem {
    /// The session the answer goes to.
    pub session: SessionId,
    /// The subagent that asked, when not the main agent.
    pub source: Option<Source>,
    pub need: Need,
    /// The session closed or moved to another process: shown, not
    /// answerable from here.
    pub expired: bool,
    /// Arrival order across all sessions: "oldest first" within a rank.
    pub seq: u64,
    /// The row's line and the notification's text, built here so no client
    /// words it (T58.4.1).
    pub title: String,
    /// What it waits for after the subagent that asked
    /// (`explore-2 · approval waiting`), `expired` once expired.
    pub subtitle: String,
    pub status: InboxStatus,
}

impl InboxItem {
    fn new(session: SessionId, source: Option<Source>, need: Need, seq: u64) -> Self {
        let mut item = InboxItem {
            session,
            source,
            title: need.title(),
            need,
            expired: false,
            seq,
            subtitle: String::new(),
            status: InboxStatus::Idle,
        };
        item.word();
        item
    }

    /// Sets `status` and `subtitle` from the need and whether it expired.
    fn word(&mut self) {
        let (status, wait) = if self.expired {
            (InboxStatus::Idle, "expired")
        } else {
            self.need.wait()
        };
        let agent = self.source.as_ref().and_then(|s| s.agent.as_deref());
        self.status = status;
        self.subtitle = match agent {
            Some(agent) => format!("{agent} · {wait}"),
            None => wait.to_string(),
        };
    }

    /// 0 blocks a turn (approval, question), 1 a failed turn, 2 news.
    pub fn urgency(&self) -> u8 {
        match self.need {
            Need::Approval { .. } | Need::Question { .. } => 0,
            Need::Failed { .. } => 1,
            Need::TaskDone { .. } => 2,
        }
    }

    fn waits_on(&self, id: CallId) -> bool {
        match &self.need {
            Need::Approval { call, .. } => call.id == id,
            Need::Question { call_id, .. } => *call_id == id,
            _ => false,
        }
    }
}

/// The sidebar glyph of a session (● running, ◐ waiting, ○ idle, ✕ error);
/// "busy elsewhere" comes from `SessionEntry::held_by`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Idle,
    Running,
    WaitingOnYou,
    Failed,
}

#[derive(Debug, Default)]
pub struct Inbox {
    items: Vec<InboxItem>,
    activity: HashMap<SessionId, Activity>,
    labels: HashMap<TaskId, String>,
    last_error: HashMap<SessionId, String>,
    next: u64,
}

impl Inbox {
    /// Folds one event of `session` in.
    pub fn apply(&mut self, session: SessionId, event: &Event) {
        match event {
            Event::TurnStarted { job: Job::Main, .. } => {
                self.items
                    .retain(|i| !(i.session == session && matches!(i.need, Need::Failed { .. })));
                self.activity.insert(session, Activity::Running);
            }
            Event::ApprovalRequired { call, why, source } => {
                let need = Need::Approval {
                    call: call.clone(),
                    why: why.clone(),
                };
                self.push(session, source.clone(), need);
            }
            Event::QuestionAsked {
                call_id,
                question,
                options,
                source,
            } => {
                let need = Need::Question {
                    call_id: *call_id,
                    question: question.clone(),
                    options: options.clone(),
                };
                self.push(session, source.clone(), need);
            }
            Event::ApprovalDecided { call_id, .. } | Event::ToolCallDone { call_id, .. } => {
                self.items
                    .retain(|i| !(i.session == session && i.waits_on(*call_id)));
                self.settle(session, Activity::Running);
            }
            Event::TaskCreated { task, label, .. } => {
                self.labels.insert(*task, label.clone());
            }
            Event::TaskCompleted {
                task, exit_code, ..
            } => {
                let label = self.labels.remove(task).unwrap_or_default();
                let ok = exit_code.is_none_or(|c| c == 0);
                self.push(
                    session,
                    None,
                    Need::TaskDone {
                        task: *task,
                        label,
                        ok,
                    },
                );
            }
            Event::Error { error, .. } => {
                self.last_error.insert(session, error.to_string());
            }
            Event::TurnDone { stop, .. } => {
                // Whatever still waited belonged to the turn that ended.
                self.items
                    .retain(|i| !(i.session == session && i.urgency() == 0));
                let failed = match stop {
                    StopReason::Error => Some(self.last_error.remove(&session).unwrap_or_default()),
                    StopReason::Refusal { detail } => Some(detail.clone()),
                    _ => None,
                };
                let activity = match failed {
                    Some(text) => {
                        self.push(session, None, Need::Failed { text });
                        Activity::Failed
                    }
                    None => Activity::Idle,
                };
                self.activity.insert(session, activity);
            }
            _ => {}
        }
    }

    /// Everything waiting, most urgent first, oldest first within a rank.
    pub fn items(&self) -> Vec<&InboxItem> {
        let mut out: Vec<&InboxItem> = self.items.iter().collect();
        out.sort_by_key(|i| (i.urgency(), i.seq));
        out
    }

    /// The Dock badge: items that block a turn and can still be answered.
    pub fn badge(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.urgency() == 0 && !i.expired)
            .count()
    }

    pub fn activity(&self, session: SessionId) -> Activity {
        self.activity.get(&session).copied().unwrap_or_default()
    }

    /// The person saw a non-blocking item (a failure, a finished task).
    pub fn dismiss(&mut self, session: SessionId, seq: u64) {
        self.items
            .retain(|i| !(i.session == session && i.seq == seq && i.urgency() > 0));
    }

    /// `session` closed here or another process took it: its items stay
    /// visible but can no longer be answered from this one.
    pub fn expire(&mut self, session: SessionId) {
        for item in self.items.iter_mut().filter(|i| i.session == session) {
            item.expired = true;
            item.word();
        }
    }

    fn push(&mut self, session: SessionId, source: Option<Source>, need: Need) {
        self.next += 1;
        let item = InboxItem::new(session, source, need, self.next);
        if item.urgency() == 0 {
            self.activity.insert(session, Activity::WaitingOnYou);
        }
        self.items.push(item);
    }

    /// Back to `to` once nothing blocks `session` any more.
    fn settle(&mut self, session: SessionId, to: Activity) {
        let waiting = self
            .items
            .iter()
            .any(|i| i.session == session && i.urgency() == 0);
        if !waiting && self.activity(session) == Activity::WaitingOnYou {
            self.activity.insert(session, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::errors::CoreError;
    use cox_protocol::types::{ModelId, Risk, Tier};
    use cox_protocol::{ItemId, TurnId};

    use super::*;

    fn started() -> Event {
        Event::TurnStarted {
            turn: TurnId::new(),
            seq: 1,
            job: Job::Main,
            tier: Tier::Code,
            model: ModelId("m".into()),
        }
    }

    fn approval(id: CallId) -> Event {
        Event::ApprovalRequired {
            call: ToolCall {
                id,
                name: "bash".into(),
                input: serde_json::json!({"command": "rm -rf target"}),
                risk: Risk::Exec,
                subject: "rm -rf target".into(),
                segments: None,
            },
            why: Why::Risk { risk: Risk::Exec },
            source: None,
        }
    }

    fn question(id: CallId) -> Event {
        Event::QuestionAsked {
            call_id: id,
            question: "which branch?".into(),
            options: vec!["main".into()],
            source: None,
        }
    }

    fn done(stop: StopReason) -> Event {
        Event::TurnDone {
            turn: TurnId::new(),
            stop,
        }
    }

    fn kinds(inbox: &Inbox) -> Vec<(SessionId, &'static str)> {
        let kind = |n: &Need| match n {
            Need::Approval { .. } => "approval",
            Need::Question { .. } => "question",
            Need::Failed { .. } => "failed",
            Need::TaskDone { .. } => "task",
        };
        inbox
            .items()
            .iter()
            .map(|i| (i.session, kind(&i.need)))
            .collect()
    }

    #[test]
    fn inbox_orders_blocking_first_then_failures_then_news_oldest_first() {
        let (a, b, c) = (SessionId::new(), SessionId::new(), SessionId::new());
        let task = TaskId::new();
        let mut inbox = Inbox::default();
        inbox.apply(
            b,
            &Event::TaskCreated {
                task,
                label: "tests".into(),
                tier: Tier::Cheap,
            },
        );
        inbox.apply(
            b,
            &Event::TaskCompleted {
                task,
                result_item: ItemId::new(),
                cost_usd: 0.0,
                exit_code: Some(1),
                archive: None,
            },
        );
        inbox.apply(a, &started());
        let first = CallId::new();
        inbox.apply(a, &approval(first));
        inbox.apply(b, &started());
        inbox.apply(
            b,
            &Event::Error {
                error: CoreError::Interrupted,
                fatal: true,
            },
        );
        inbox.apply(b, &done(StopReason::Error));
        inbox.apply(c, &started());
        inbox.apply(c, &question(CallId::new()));
        inbox.apply(a, &approval(CallId::new()));

        assert_eq!(
            kinds(&inbox),
            vec![
                (a, "approval"),
                (c, "question"),
                (a, "approval"),
                (b, "failed"),
                (b, "task"),
            ]
        );
        assert_eq!(inbox.badge(), 3);
        assert_eq!(inbox.activity(a), Activity::WaitingOnYou);
        assert_eq!(inbox.activity(b), Activity::Failed);
        let failed = &inbox.items()[3].need;
        assert_eq!(
            failed,
            &Need::Failed {
                text: "interrupted".into()
            }
        );
        let task_row = &inbox.items()[4].need;
        assert!(matches!(task_row, Need::TaskDone { label, ok: false, .. } if label == "tests"));
    }

    #[test]
    fn answered_items_leave_and_the_session_runs_again() {
        let s = SessionId::new();
        let (call, ask) = (CallId::new(), CallId::new());
        let mut inbox = Inbox::default();
        inbox.apply(s, &started());
        inbox.apply(s, &approval(call));
        inbox.apply(s, &question(ask));
        inbox.apply(
            s,
            &Event::ApprovalDecided {
                call_id: call,
                decision: cox_protocol::Decision::Allow,
                by: cox_protocol::DecidedBy::User,
            },
        );
        assert_eq!(
            inbox.activity(s),
            Activity::WaitingOnYou,
            "the question still waits"
        );
        inbox.expire(s);
        assert_eq!(inbox.badge(), 0, "expired items cannot be answered here");
        let result = cox_protocol::ToolResult {
            ok: true,
            visible: "main".into(),
            archive: None,
            bytes: 4,
            duration_ms: 1,
            diff: None,
            structured: None,
        };
        inbox.apply(
            s,
            &Event::ToolCallDone {
                call_id: ask,
                result,
            },
        );
        assert!(inbox.items().is_empty());
        assert_eq!(inbox.activity(s), Activity::Running);
        inbox.apply(s, &done(StopReason::EndTurn));
        assert_eq!(inbox.activity(s), Activity::Idle);
    }

    #[test]
    fn a_new_turn_clears_the_last_failure_and_dismiss_drops_news() {
        let s = SessionId::new();
        let mut inbox = Inbox::default();
        inbox.apply(
            s,
            &done(StopReason::Refusal {
                detail: "no".into(),
            }),
        );
        assert_eq!(kinds(&inbox), vec![(s, "failed")]);
        inbox.apply(s, &started());
        assert!(inbox.items().is_empty());
        inbox.apply(s, &approval(CallId::new()));
        let seq = inbox.items()[0].seq;
        inbox.dismiss(s, seq);
        assert_eq!(
            inbox.items().len(),
            1,
            "a blocking item is answered, not dismissed"
        );
    }

    #[test]
    fn an_approval_names_its_tool_and_subject() {
        let mut inbox = Inbox::default();
        inbox.apply(SessionId::new(), &approval(CallId::new()));
        let item = inbox.items()[0].clone();
        assert_eq!(item.title, "bash rm -rf target");
        assert_eq!(item.subtitle, "approval waiting");
        assert_eq!(item.status, InboxStatus::Waiting);
    }

    #[test]
    fn an_approval_without_a_subject_names_the_tool_and_its_agent() {
        let s = SessionId::new();
        let mut inbox = Inbox::default();
        let Event::ApprovalRequired { mut call, why, .. } = approval(CallId::new()) else {
            unreachable!("approval() builds an approval")
        };
        call.subject.clear();
        let source = Source {
            session: s,
            agent: Some("explore-2".into()),
            preset: None,
        };
        let event = Event::ApprovalRequired {
            call,
            why,
            source: Some(source),
        };
        inbox.apply(s, &event);
        let item = inbox.items()[0].clone();
        assert_eq!(item.title, "bash");
        assert_eq!(item.subtitle, "explore-2 · approval waiting");
    }

    #[test]
    fn each_need_has_its_words() {
        let s = SessionId::new();
        let (ok, bad) = (TaskId::new(), TaskId::new());
        let mut inbox = Inbox::default();
        inbox.apply(s, &question(CallId::new()));
        for (task, code) in [(ok, 0), (bad, 2)] {
            let label = if code == 0 { "build" } else { "tests" };
            inbox.apply(
                s,
                &Event::TaskCreated {
                    task,
                    label: label.into(),
                    tier: Tier::Cheap,
                },
            );
            inbox.apply(
                s,
                &Event::TaskCompleted {
                    task,
                    result_item: ItemId::new(),
                    cost_usd: 0.0,
                    exit_code: Some(code),
                    archive: None,
                },
            );
        }
        inbox.apply(
            s,
            &done(StopReason::Refusal {
                detail: "no".into(),
            }),
        );
        let words: Vec<(String, String, InboxStatus)> = inbox
            .items()
            .iter()
            .map(|i| (i.title.clone(), i.subtitle.clone(), i.status))
            .collect();
        let row = |t: &str, w: &str, s| (t.to_string(), w.to_string(), s);
        assert_eq!(
            words,
            vec![
                row("no", "turn failed", InboxStatus::Error),
                row("build", "task done", InboxStatus::Idle),
                row("tests", "task failed", InboxStatus::Error),
            ],
            "the question left with the turn that asked it"
        );
        inbox.apply(s, &started());
        inbox.apply(s, &question(CallId::new()));
        let asked = inbox.items()[0].clone();
        assert_eq!(asked.title, "which branch?");
        assert_eq!(asked.subtitle, "question waiting");
        assert_eq!(asked.status, InboxStatus::Waiting);
    }

    #[test]
    fn an_expired_item_reads_expired() {
        let s = SessionId::new();
        let mut inbox = Inbox::default();
        inbox.apply(s, &approval(CallId::new()));
        inbox.expire(s);
        let item = inbox.items()[0].clone();
        assert_eq!(item.subtitle, "expired");
        assert_eq!(item.status, InboxStatus::Idle);
        assert_eq!(item.title, "bash rm -rf target", "the line stays");
    }
}
