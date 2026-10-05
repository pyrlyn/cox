// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Background tasks (T9.2, T27.1): `agent` with `background: true` runs its
//! child session concurrently, and any running `bash` or `agent` call can be
//! detached into a task — at once for `bash(background: true)`, mid-run on
//! `Submission::Background` (`Ctrl+B`). The contract, in both directions:
//! the model sees a short pointer line in history and the user sees a
//! bounded notice — the full output lives only in the archive (or the
//! child's rollout) and is never smuggled into context silently.
//!
//! Cancellation is turn-scoped: a background task clones the spawning
//! turn's token, so `Interrupt` stops it only while that turn is current;
//! a later turn does not cancel work it did not start. Every turn token is
//! a child of the session's own (T38.2), so `Session::end` still stops it.
//!
//! T34.5: the same registry routes follow-up messages (SM§2). A subagent
//! stays addressable by its `TaskId` after it finishes: live, a message
//! queues behind its current turn; finished, it is reduced to its session
//! id plus what a resume needs, and a message wakes it.

use std::collections::VecDeque;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cox_protocol::ArchivePut;
use cox_protocol::errors::{CoreError, ToolError};
use cox_protocol::ids::{ArchiveId, CallId, ItemId, TaskId, TurnId};
use cox_protocol::traits::ToolCx;
use cox_protocol::types::{Content, Event, Level, Message, Role, Tier, ToolOutput, ToolResult};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::checkpoint::{self, Pending};
use crate::session::Session;
use crate::subagent::{Dormant, MAX_MESSAGES_PER_TASK};

/// What a task runs. A subagent reports its own `TaskCreated`/
/// `TaskCompleted` pair (`subagent.rs`); a detached shell call gets its pair
/// here, with an exit code and the archive row of its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// A subagent child session.
    Agent,
    /// A detached `bash` call.
    Shell,
}

impl TaskKind {
    /// `bash` is the one shell tool; everything else detachable is an agent.
    pub fn of(tool: &str) -> Self {
        if tool == "bash" {
            Self::Shell
        } else {
            Self::Agent
        }
    }
}

/// A spawned call: its output plus the context back, so a sandbox-denied
/// retry can run again with the same context.
pub(crate) type Running = JoinHandle<(ToolOutput, ToolCx)>;

/// The call a detached task stands for.
pub(crate) struct Detachable {
    pub turn: TurnId,
    pub call: CallId,
    pub tool: String,
    pub subject: String,
}

/// T34.2: holds one of the session's `core.max_concurrent_subagents`
/// slots; frees it on drop. `AgentTool::call` binds this to a local that
/// outlives every early return in the foreground path (Rust runs its
/// `Drop` regardless of which `?` exits the function), and moves it into
/// the spawned task's own `async move` block for a `background: true`
/// call, so the slot is held until that child's run actually finishes
/// rather than until `call()` returns its "started" pointer.
pub(crate) struct AgentSlotGuard {
    slots: Arc<AtomicU32>,
}

impl Drop for AgentSlotGuard {
    fn drop(&mut self) {
        self.slots.fetch_sub(1, Ordering::AcqRel);
    }
}

/// History pointer on completion: label, id, detail (cost, or exit code and
/// archive id) and the output's first line, capped so a multi-kilobyte
/// answer cannot leak in through it.
pub fn pointer_line(label: &str, task: TaskId, detail: &str, answer: &str) -> String {
    let first: String = answer
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    format!("background `{label}` finished (task {task}, {detail}): {first}")
}

/// How much of the answer the completion notice shows the user.
pub const NOTICE_CAP: usize = 2000;

/// Notice text on completion: label, detail and the answer, truncated with
/// a marker rather than cut silently.
pub fn notice_text(label: &str, task: TaskId, answer: &str, detail: &str) -> String {
    let short = capped(answer);
    format!("background task finished: {label} (task {task}, {detail})\n{short}")
}

fn capped(text: &str) -> String {
    let mut short: String = text.chars().take(NOTICE_CAP).collect();
    if text.chars().count() > NOTICE_CAP {
        short.push_str("\n[truncated]");
    }
    short
}

/// How a follow-up reads to its addressee, child turn or parent history
/// (SM§2); capped like a notice, since a child's text is untrusted.
pub fn message_line(from: Option<TaskId>, text: &str) -> String {
    let sender = from.map_or_else(|| "parent".to_string(), |t| format!("task {t}"));
    format!("[message from {sender}] {}", capped(text))
}

/// A message waiting for its addressee's next turn.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Queued {
    pub from: Option<TaskId>,
    pub hop: u32,
    pub text: String,
}

/// A subagent its parent can still address (SM§2).
pub(crate) enum Child {
    /// Its driver takes `queue` one turn at a time; `hop` is the hop of
    /// the message that started the current turn (SM§5).
    Running { queue: VecDeque<Queued>, hop: u32 },
    /// Downgraded from a live handle to what a resume needs.
    Finished(Box<Dormant>),
}

/// A subagent's detail: what it cost.
pub fn cost_detail(cost_usd: f64) -> String {
    format!("${cost_usd:.4}")
}

/// A detached call's detail: its exit code, if it has one, and how to get
/// the whole output back.
pub fn detached_detail(exit_code: Option<i32>, archive: Option<ArchiveId>) -> String {
    let exit = exit_code.map(|c| format!("exit {c}, ")).unwrap_or_default();
    match archive {
        Some(id) => format!("{exit}full output: expand {id}"),
        None => format!("{exit}output could not be archived"),
    }
}

/// What holding `inner`'s lock decided for a `deliver` call (T34.6, SM§5):
/// acted on after the lock drops, so a notice/emit/spawn never runs while
/// `inner` is still held.
enum Delivery {
    /// No such addressee.
    Unknown,
    /// `MAX_MESSAGES_PER_TASK` was already reached; nothing queued.
    Flooded,
    /// The addressee is dormant, but the concurrency cap (T34.2) has no
    /// slot free to wake it with; nothing queued.
    AtCapacity(u32, u32),
    /// Queued behind a running turn.
    Queued,
    /// A finished child is woken, holding this slot for its run.
    Wake(Box<Dormant>, AgentSlotGuard),
}

impl Session {
    /// Registers a running task; `/tasks` and the status count read the
    /// `TaskCreated`/`TaskCompleted` events, this is the core's own view.
    pub(crate) async fn register_task(
        &self,
        task: TaskId,
        label: String,
        tier: Tier,
        kind: TaskKind,
    ) {
        self.inner
            .lock()
            .await
            .tasks
            .insert(task, (label, tier, kind));
    }

    /// Forgets a finished task and wakes anyone in `wait_idle`, below.
    /// Callers only reach this once a task is truly done — a follow-up
    /// message that raced in keeps its entry in place instead (`subagent`'s
    /// `drive`/`AgentTool::call`, and `deliver`'s own `Wake` case below),
    /// so this never fires mid-relay just because one cycle ended.
    pub(crate) async fn complete_task(&self, task: TaskId) {
        self.inner.lock().await.tasks.remove(&task);
        self.tasks_idle.notify_waiters();
    }

    /// Awaits until no `TaskKind::Agent` task — a backgrounded `agent`, or
    /// a subagent a follow-up message just woke — is registered, or the
    /// session is cancelled (Ctrl-C, `Session::interrupt`), whichever comes
    /// first (T34.9). The headless surface's `run()` calls this before its
    /// own `process::exit`, which otherwise abandons a still-running
    /// background chain mid-relay the instant the top-level turn's own
    /// last event is handled.
    ///
    /// Deliberately blind to `TaskKind::Shell`: a detached `bash` (a dev
    /// server, a `sleep`, anything the model backgrounded on purpose) is
    /// meant to outlive the run exactly as it already does today — waiting
    /// for it here would turn "start a server in the background" into "hang
    /// the headless run forever" (T34.9 follow-up). `notified()` is
    /// created while `inner` is still locked, the documented race-free
    /// pattern for `Notify::notify_waiters` (no permit is stored for a
    /// waiter that registers after the fact, unlike `notify_one`), so a
    /// `complete_task` that empties out the last `Agent` entry between this
    /// check and the `.await` below can never be missed.
    pub async fn wait_idle(&self) {
        let cancel = self.cancel_token();
        loop {
            let notified = {
                let inner = self.inner.lock().await;
                let idle = inner
                    .tasks
                    .values()
                    .all(|(_, _, kind)| *kind != TaskKind::Agent);
                if idle {
                    return;
                }
                self.tasks_idle.notified()
            };
            tokio::select! {
                () = notified => {}
                () = cancel.cancelled() => return,
            }
        }
    }

    /// Waits, bounded by `deadline`, for every task of any kind — including
    /// `TaskKind::Shell`, which `wait_idle` above deliberately ignores — to
    /// leave the registry (T34.9 follow-up). The headless surface calls
    /// `end()` first, then this, right before `shutdown_background`: a
    /// still-running detached `bash` gets killed rather than merely
    /// abandoned as an orphan process. Unlike `wait_idle`, this cannot use
    /// `cancel.cancelled()` as an exit condition — the caller just set it,
    /// so it is already true and would make this return immediately,
    /// before the kill it triggered actually lands. `end()` reaches a shell
    /// detached in any turn (T38.2); `deadline` is the safety net for a
    /// shell that does not die in time.
    pub async fn wait_tasks_cleared(&self, deadline: Duration) {
        let _ = tokio::time::timeout(deadline, async {
            loop {
                let notified = {
                    let inner = self.inner.lock().await;
                    if inner.tasks.is_empty() {
                        return;
                    }
                    self.tasks_idle.notified()
                };
                notified.await;
            }
        })
        .await;
    }

    /// Makes a freshly spawned subagent addressable; its task is hop 0.
    pub(crate) async fn track_child(&self, task: TaskId) {
        let queue = VecDeque::new();
        let child = Child::Running { queue, hop: 0 };
        self.inner.lock().await.children.insert(task, child);
    }

    /// The message a running child's next turn carries, if any; its hop
    /// becomes the child's current one (SM§5).
    pub(crate) async fn next_queued(&self, task: TaskId) -> Option<Queued> {
        match self.inner.lock().await.children.get_mut(&task) {
            Some(Child::Running { queue, hop }) => {
                let next = queue.pop_front()?;
                *hop = next.hop;
                Some(next)
            }
            _ => None,
        }
    }

    /// The hop a message sent by `task` during its current turn builds on.
    pub(crate) async fn child_hop(&self, task: TaskId) -> u32 {
        match self.inner.lock().await.children.get(&task) {
            Some(Child::Running { hop, .. }) => *hop,
            _ => 0,
        }
    }

    /// A run ended: under one lock, either a message that raced in hands
    /// the child back for another run, or it is downgraded to `dormant`.
    pub(crate) async fn park_child(
        &self,
        task: TaskId,
        dormant: Dormant,
    ) -> Option<(Box<Dormant>, Queued)> {
        let mut inner = self.inner.lock().await;
        if let Some(Child::Running { queue, hop }) = inner.children.get_mut(&task)
            && let Some(next) = queue.pop_front()
        {
            *hop = next.hop;
            return Some((Box::new(dormant), next));
        }
        let child = Child::Finished(Box::new(dormant));
        inner.children.insert(task, child);
        None
    }

    /// A child that cannot run again is no longer addressable.
    pub(crate) async fn forget_child(&self, task: TaskId) {
        self.inner.lock().await.children.remove(&task);
    }

    /// `Submission::TaskMessage`: queued behind a running child's current
    /// turn, never mid-turn; a finished child is woken with it, holding one
    /// of T34.2's concurrency slots for that run (T34.6 point 3 — a woken
    /// child ran outside the cap before this). `MAX_MESSAGES_PER_TASK`
    /// (T34.6, SM§5) denies a flood before either happens.
    pub(crate) async fn deliver(
        &self,
        task: TaskId,
        from: Option<TaskId>,
        hop: u32,
        text: String,
    ) -> Result<(), CoreError> {
        let queued = Queued {
            from,
            hop,
            text: text.clone(),
        };
        let decision = {
            let mut inner = self.inner.lock().await;
            if !inner.children.contains_key(&task) {
                Delivery::Unknown
            } else if *inner.message_counts.get(&task).unwrap_or(&0) >= MAX_MESSAGES_PER_TASK {
                Delivery::Flooded
            } else {
                match inner.children.get_mut(&task) {
                    Some(Child::Running { queue, .. }) => {
                        queue.push_back(queued.clone());
                        *inner.message_counts.entry(task).or_insert(0) += 1;
                        Delivery::Queued
                    }
                    Some(Child::Finished(_)) => {
                        let cap = self.config.core.max_concurrent_subagents;
                        match self.try_reserve_agent_slot(cap) {
                            Ok(slot) => {
                                let running = Child::Running {
                                    queue: VecDeque::new(),
                                    hop,
                                };
                                match inner.children.insert(task, running) {
                                    Some(Child::Finished(dormant)) => {
                                        *inner.message_counts.entry(task).or_insert(0) += 1;
                                        Delivery::Wake(dormant, slot)
                                    }
                                    _ => Delivery::Queued, // unreachable: just matched Finished
                                }
                            }
                            Err(running) => Delivery::AtCapacity(running, cap),
                        }
                    }
                    None => Delivery::Unknown, // unreachable: just matched Some above
                }
            }
        };
        match decision {
            Delivery::Unknown => {
                let text = format!("no subagent task {task} to message");
                self.notice(Level::Warn, text).await
            }
            Delivery::Flooded => {
                let why = format!(
                    "message cap reached: task {task} already received \
                     {MAX_MESSAGES_PER_TASK} of {MAX_MESSAGES_PER_TASK} messages"
                );
                self.notice(Level::Warn, why.clone()).await?;
                Err(CoreError::Denied { why })
            }
            Delivery::AtCapacity(running, cap) => {
                let why = format!(
                    "subagent concurrency cap reached: {running} of {cap} `agent` tasks \
                     already running; task {task} stays dormant"
                );
                self.notice(Level::Warn, why.clone()).await?;
                Err(CoreError::Denied { why })
            }
            Delivery::Queued => {
                self.emit(Event::TaskMessage {
                    task,
                    from,
                    hop,
                    text,
                })
                .await
            }
            Delivery::Wake(dormant, slot) => {
                self.emit(Event::TaskMessage {
                    task,
                    from,
                    hop,
                    text,
                })
                .await?;
                // T34.9: register before spawning `wake`, not inside it —
                // that task is scheduled independently and may not run for
                // a moment, and `wait_idle` must never see this task as
                // idle in the gap between deciding to wake it and the
                // wake actually starting.
                let (label, tier) = dormant.label_and_tier();
                self.register_task(task, label, tier, TaskKind::Agent).await;
                let parent = self.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    crate::subagent::wake(parent, task, dormant, queued).await;
                });
                Ok(())
            }
        }
    }

    /// `send_message`'s name-or-id resolution (T34.6/T34.9, SM§4): the
    /// exact registry name `name_task` indexed, else a literal `TaskId`.
    /// Shared by the parent's own `resolve_addressee` below (further
    /// checked against `children`, since the parent must reject a name/id
    /// that names no live child) and by a child's `Relay::send_message`
    /// (`subagent.rs`), which holds the same registry read-only via
    /// `spawn_child` and has no `children` map of its own to check a
    /// literal id against.
    pub(crate) async fn resolve_name_or_id(&self, to: &str) -> Option<TaskId> {
        if let Some(id) = self.task_names.lock().await.get(to).copied() {
            return Some(id);
        }
        to.parse().ok()
    }

    /// `send_message`'s name/id resolution for a direct (parent-side) call
    /// (T34.6, SM§4): an unknown or already-forgotten id is "unknown", not
    /// silently accepted.
    pub(crate) async fn resolve_addressee(&self, to: &str) -> Option<TaskId> {
        let id = self.resolve_name_or_id(to).await?;
        let inner = self.inner.lock().await;
        inner.children.contains_key(&id).then_some(id)
    }

    /// Indexes a spawned child's registry name (`explore-2`) for
    /// `resolve_addressee`, alongside `track_child`'s id-keyed entry.
    pub(crate) async fn name_task(&self, name: String, task: TaskId) {
        self.task_names.lock().await.insert(name, task);
    }

    /// A child's message to its parent (SM§3): a line in history after the
    /// last cache breakpoint, like `publish_task_result`, never a turn.
    pub(crate) async fn message_parent(
        &self,
        from: TaskId,
        hop: u32,
        text: String,
    ) -> Result<(), CoreError> {
        let line = message_line(Some(from), &text);
        self.inner.lock().await.history.push(Message {
            role: Role::User,
            content: vec![Content::Text { text: line }],
        });
        // Addressed to the sender's own task thread: the parent has no id.
        self.emit(Event::TaskMessage {
            task: from,
            from: Some(from),
            hop,
            text,
        })
        .await
    }

    /// T34.2's `core.max_concurrent_subagents` cap, checked and reserved in
    /// one step: a burst of parallel `agent` calls in the same turn (the
    /// core's own `turn.rs` dispatches `Concurrency::Parallel` tools
    /// concurrently, `agent` among them) must not all read the same count
    /// and all pass — a separate check-then-register across an `await`
    /// races, since two calls can both observe the pre-registration count
    /// before either registers. `Err` carries how many slots were already
    /// reserved, for the denial text; `Ok` carries the guard that frees the
    /// slot on drop, so every exit (success, error, cancellation, a failed
    /// `resolve`/`spawn_child`) releases it exactly once without the
    /// caller having to remember to.
    pub(crate) fn try_reserve_agent_slot(&self, cap: u32) -> Result<AgentSlotGuard, u32> {
        loop {
            let current = self.agent_slots.load(Ordering::Acquire);
            if current >= cap {
                return Err(current);
            }
            if self
                .agent_slots
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(AgentSlotGuard {
                    slots: self.agent_slots.clone(),
                });
            }
        }
    }

    /// Completion report: the pointer line enters history for the model,
    /// the notice goes to the user. The full output stays in the archive or
    /// the child's rollout file.
    pub(crate) async fn publish_task_result(
        &self,
        task: TaskId,
        label: &str,
        answer: &str,
        detail: &str,
    ) -> Result<(), CoreError> {
        {
            let mut inner = self.inner.lock().await;
            inner.history.push(Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: pointer_line(label, task, detail, answer),
                }],
            });
        }
        self.emit(Event::Notice {
            level: Level::Info,
            text: notice_text(label, task, answer, detail),
        })
        .await
    }

    /// Arms `Submission::Background` for a call about to run. `bash` with
    /// `background: true` is the same detach pulled at once, so the flag
    /// leaves the input the tool sees: the core owns the task, not the tool.
    pub(crate) async fn arm_detach(
        &self,
        call: CallId,
        tool: &str,
        mut input: Value,
    ) -> (Value, CancellationToken) {
        let token = CancellationToken::new();
        if TaskKind::of(tool) == TaskKind::Shell
            && let Some(flag) = input.as_object_mut().and_then(|o| o.remove("background"))
            && flag.as_bool() == Some(true)
        {
            token.cancel();
        }
        self.inner.lock().await.detach.insert(call, token.clone());
        (input, token)
    }

    /// `Submission::Background`: pulls a running call's detach switch.
    pub(crate) async fn background(&self, call: CallId) -> Result<(), CoreError> {
        let token = self.inner.lock().await.detach.get(&call).cloned();
        match token {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => {
                self.emit(Event::Notice {
                    level: Level::Warn,
                    text: format!("no running call {call} to move to the background"),
                })
                .await
            }
        }
    }

    /// Waits for `running` unless it is detached first. `Ok` is the call's
    /// own output (with its context unless the call panicked); `Err` is the
    /// pointer result the model gets for a call that now runs as a task,
    /// boxed because a `ToolResult` is far larger than the `Ok` side.
    pub(crate) async fn wait_or_detach(
        &self,
        at: Detachable,
        mut running: Running,
        detach: CancellationToken,
        pending: &mut Option<Pending>,
    ) -> Result<(ToolOutput, Option<ToolCx>), Box<ToolResult>> {
        let joined = tokio::select! {
            biased;
            joined = &mut running => Some(joined),
            _ = detach.cancelled() => None,
        };
        self.inner.lock().await.detach.remove(&at.call);
        match joined {
            Some(Ok((output, cx))) => Ok((output, Some(cx))),
            Some(Err(_)) => Ok((crate::turn::error_output(ToolError::Io), None)),
            None => Err(Box::new(
                self.detach_task(at, running, pending.take()).await,
            )),
        }
    }

    /// Turns a running call into a task: registry and `TaskCreated` for a
    /// shell call, the rest of the call on its own tokio task, and the
    /// pointer result back for the turn to continue with.
    async fn detach_task(
        &self,
        at: Detachable,
        running: Running,
        pending: Option<Pending>,
    ) -> ToolResult {
        let task = TaskId::new();
        let kind = TaskKind::of(&at.tool);
        let label = format!("{}: {}", at.tool, crate::subagent::first_line(&at.subject));
        if kind == TaskKind::Shell {
            let _ = self
                .emit(Event::TaskCreated {
                    task,
                    label: label.clone(),
                    tier: self.tier,
                })
                .await;
            self.register_task(task, label.clone(), self.tier, kind)
                .await;
        }
        let session = self.clone();
        let text = format!(
            "background task {task} started: {label}\n\
             its result will arrive as a notice, not in this turn"
        );
        tokio::spawn(async move {
            let output = match running.await {
                Ok((output, _)) => output,
                Err(_) => crate::turn::error_output(ToolError::Io),
            };
            if let Some(pending) = pending {
                checkpoint::after(&session, at.turn, at.call, pending).await;
            }
            session
                .finish_detached(task, kind, &at, &label, output)
                .await;
        });
        ToolResult {
            ok: true,
            bytes: text.len() as u64,
            visible: text,
            archive: None,
            duration_ms: 0,
            diff: None,
            structured: None,
        }
    }

    /// The archive row first, then the completion pair and the pointer —
    /// the model never sees a shortened output before it is retrievable.
    async fn finish_detached(
        &self,
        task: TaskId,
        kind: TaskKind,
        at: &Detachable,
        label: &str,
        output: ToolOutput,
    ) {
        let archive = self
            .archive
            .put(ArchivePut {
                session: self.id,
                call: at.call,
                tool: at.tool.clone(),
                subject: Some(at.subject.clone()),
                bytes: output.text.as_bytes().to_vec(),
            })
            .await
            .ok();
        let exit_code = output
            .structured
            .as_ref()
            .and_then(|s| s.get("exit_code"))
            .and_then(Value::as_i64)
            .and_then(|c| i32::try_from(c).ok());
        if kind == TaskKind::Shell {
            self.complete_task(task).await;
            let _ = self
                .emit(Event::TaskCompleted {
                    task,
                    result_item: ItemId::new(),
                    cost_usd: 0.0,
                    exit_code,
                    archive,
                })
                .await;
        }
        let detail = detached_detail(exit_code, archive);
        let _ = self
            .publish_task_result(task, label, &output.text, &detail)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_pointer_line_is_bounded() {
        let long = format!("{}\nsecond line", "x".repeat(5000));
        let line = pointer_line("explore: y", TaskId::new(), &cost_detail(0.002), &long);
        assert!(line.contains("explore: y"), "{line}");
        assert!(line.contains("$0.0020"), "{line}");
        assert!(!line.contains("second line"), "first line only");
        assert!(line.len() < 300, "bounded: {}", line.len());
    }

    #[test]
    fn tasks_notice_truncates_with_a_marker() {
        let long = "y".repeat(NOTICE_CAP + 10);
        let text = notice_text("shell: make", TaskId::new(), &long, "exit 0");
        assert!(text.contains("[truncated]"), "marked, not cut silently");
        let short = notice_text("shell: make", TaskId::new(), "ok", "exit 0");
        assert!(!short.contains("[truncated]"), "{short}");
    }

    #[test]
    fn detached_detail_names_exit_code_and_expand_id() {
        let id = ArchiveId::new();
        assert_eq!(
            detached_detail(Some(2), Some(id)),
            format!("exit 2, full output: expand {id}")
        );
        assert_eq!(detached_detail(None, None), "output could not be archived");
    }

    #[test]
    fn only_bash_is_a_shell_task() {
        assert_eq!(TaskKind::of("bash"), TaskKind::Shell);
        assert_eq!(TaskKind::of("agent"), TaskKind::Agent);
    }
}
