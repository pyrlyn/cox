// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Compaction (plan.md §1.10, T8.1): replaces every turn but the last
//! `keep_turns` with one summary from the `compact` job. Append-only (D6f):
//! the rollout keeps every original event and `Compacted.dropped` says which
//! turns a rebuild skips. Also keeps a request under the threshold before
//! it is sent (T28.3). Separate from `session.rs` because it is the only
//! place history is ever rewritten in memory.

use cox_protocol::config::CompactionStrategy;
use cox_protocol::errors::CoreError;
use cox_protocol::ids::ItemId;
use cox_protocol::types::{
    CompactReason, Content, Event, HookEvent, HookOutcome, ItemKind, Job, Level, Message,
    ProviderEvent, RepoMapReason, Request, Role, SystemBlock,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::budget;
use crate::hooks;
use crate::session::{Session, State};

const PROMPT: &str = include_str!("prompts/compact.md");
/// §1.10 step 3: the summary itself is capped.
const MAX_SUMMARY_TOKENS: u32 = 2048;

/// Why compaction ran; reaches `PreCompact` hooks as `trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// `context_tokens_last_call ≥ compact_at × max_context`.
    Auto,
    /// T28.3: the next request's estimate is over that threshold.
    PreCall,
    /// `/compact` or `Submission::Compact`.
    Manual,
    /// The provider rejected the request as too long.
    ContextTooLong,
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            // Both are automatic; Claude Code hook matchers know only
            // `auto`/`manual`, so pre-call reads as `auto` to a hook.
            Self::Auto | Self::PreCall => "auto",
            Self::Manual => "manual",
            Self::ContextTooLong => "context_too_long",
        }
    }

    fn reason(self) -> CompactReason {
        match self {
            Self::Auto => CompactReason::PostTurn,
            Self::PreCall => CompactReason::PreCall,
            Self::Manual => CompactReason::Manual,
            Self::ContextTooLong => CompactReason::ContextTooLong,
        }
    }
}

/// What `fit_request` did with a request (T28.3).
pub(crate) enum Fit {
    /// Under the threshold, possibly after microcompaction or compaction.
    Fits(Request),
    /// Still over after both; the estimate in tokens.
    TooBig(u32),
}

/// Where a turn starts in the in-memory history, and the user item that
/// started it (the id a rollout rebuild drops the whole turn by).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnMark {
    pub item: ItemId,
    pub start: usize,
    /// The turn's `seq` (`Event::TurnStarted`); `0` for the synthetic
    /// summary turn compaction leaves behind. `/rewind` cuts here.
    pub seq: u32,
}

/// The history index the kept turns start at, and the turns before it.
pub(crate) fn split(marks: &[TurnMark], keep_turns: u32) -> Option<(usize, Vec<ItemId>)> {
    let keep = keep_turns as usize;
    if marks.len() <= keep {
        return None;
    }
    let first_kept = marks.len() - keep;
    let cut = marks[first_kept].start;
    let dropped = marks[..first_kept].iter().map(|m| m.item).collect();
    Some((cut, dropped))
}

pub(crate) fn needs_compaction(
    last_context_tokens: u32,
    max_context: u32,
    compact_at: f64,
) -> bool {
    threshold(max_context, compact_at).is_some_and(|limit| f64::from(last_context_tokens) >= limit)
}

/// ⌈bytes/4⌉ of the serialised messages: the same heuristic `truncate` and
/// the subagent cap use, so `before`/`after` compare across features.
pub(crate) fn estimate_tokens(messages: &[Message]) -> u32 {
    quarter(serde_json::to_vec(messages).map(|v| v.len()).unwrap_or(0))
}

/// The same heuristic over a whole request: cox-core may not call
/// `cox_provider::tokens::estimate` (`crates/cox/tests/deps.rs`), and the
/// exact count, when it matters, comes through `Provider::count_tokens`.
pub(crate) fn estimate(req: &Request) -> u32 {
    quarter(serde_json::to_vec(req).map(|v| v.len()).unwrap_or(0))
}

fn quarter(bytes: usize) -> u32 {
    u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX)
}

/// The threshold in tokens; `None` when the provider reports no window.
fn threshold(max_context: u32, compact_at: f64) -> Option<f64> {
    (max_context > 0).then(|| compact_at * f64::from(max_context))
}

/// Whether the estimate is close enough to `limit` that the heuristic's
/// error could flip the answer, so an exact count is worth asking for.
fn near(estimate: u32, limit: f64) -> bool {
    (f64::from(estimate) - limit).abs() <= 0.1 * limit
}

/// The summariser's input: one line per block, archived results as pointers.
pub(crate) fn transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        for c in &m.content {
            match c {
                Content::Text { text } => out.push_str(&format!("{role}: {text}\n")),
                Content::ToolUse { name, input, .. } => {
                    out.push_str(&format!("tool_use {name} {input}\n"));
                }
                Content::ToolResult {
                    content, is_error, ..
                } => {
                    let tag = if *is_error {
                        "tool_error"
                    } else {
                        "tool_result"
                    };
                    out.push_str(&format!("{tag}: {content}\n"));
                }
                Content::Pointer { summary, archive } => {
                    out.push_str(&format!(
                        "tool_result (archived {}): {summary}\n",
                        archive.id
                    ));
                }
                Content::Thinking { .. } | Content::Image { .. } => {}
            }
        }
    }
    out
}

/// Files, failing commands and the last user request, in transcript order.
/// Taken from tool calls, so the summary cannot drop a path the model forgot.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WorkingState {
    /// `(path, read|edited|created)`. A later edit keeps the first slot.
    files: Vec<(String, &'static str)>,
    /// `(command, exit code, last error line)`.
    errors: Vec<(String, Option<u32>, String)>,
    last_request: Option<String>,
}

/// `read` / `edited` / `created`. Created outranks edited, which outranks read.
fn rank(action: &str) -> u8 {
    match action {
        "created" => 2,
        "edited" => 1,
        _ => 0,
    }
}

pub(crate) fn working_state(messages: &[Message]) -> WorkingState {
    let mut calls = Vec::new();
    let mut state = WorkingState::default();
    for message in messages {
        for block in &message.content {
            match block {
                Content::Text { text } if message.role == Role::User && !text.trim().is_empty() => {
                    state.last_request = Some(text.trim().to_string());
                }
                Content::ToolUse { id, name, input } => calls.push((*id, name.as_str(), input)),
                Content::ToolResult {
                    call_id,
                    content,
                    is_error,
                } => {
                    if let Some((_, name, input)) =
                        calls.iter().rev().find(|(id, _, _)| id == call_id)
                    {
                        record(&mut state, name, input, content, *is_error);
                    }
                }
                _ => {}
            }
        }
    }
    state
}

fn record(state: &mut WorkingState, name: &str, input: &Value, content: &str, is_error: bool) {
    if name == "bash" {
        // A zero exit is not an error the summary has to keep.
        if !is_error {
            return;
        }
        let Some(command) = input
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
        else {
            return;
        };
        let last_line = content
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && !(line.starts_with('[') && line.ends_with(']')))
            .unwrap_or("")
            .to_string();
        let exit_code = content.lines().rev().find_map(|line| {
            line.trim()
                .strip_prefix("[exit ")
                .and_then(|rest| rest.split([' ', ']']).next())
                .and_then(|token| token.parse().ok())
        });
        state
            .errors
            .push((command.to_string(), exit_code, last_line));
        return;
    }
    // A denied or failed call never touched the file.
    if is_error {
        return;
    }
    let mut note = |path: &str, action: &'static str| {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        if let Some(existing) = state.files.iter_mut().find(|(got, _)| got == path) {
            if rank(action) > rank(existing.1) {
                existing.1 = action;
            }
        } else {
            state.files.push((path.to_string(), action));
        }
    };
    if let Some(action) = match name {
        "read" => Some("read"),
        "edit" => Some("edited"),
        "write" => Some("created"),
        _ => None,
    } {
        note(
            input.get("path").and_then(Value::as_str).unwrap_or(""),
            action,
        );
        return;
    }
    if name != "apply_patch" {
        return;
    }
    // Delete has no action of its own; the path still has to be listed.
    for line in input
        .get("patch")
        .and_then(Value::as_str)
        .unwrap_or("")
        .lines()
    {
        let line = line.trim();
        let marked = [
            ("*** Add File: ", "created"),
            ("*** Move to: ", "created"),
            ("*** Update File: ", "edited"),
            ("*** Delete File: ", "edited"),
        ]
        .into_iter()
        .find_map(|(prefix, action)| line.strip_prefix(prefix).map(|path| (path, action)));
        if let Some((path, action)) = marked {
            note(path, action);
        }
    }
}

/// `## Files touched` and `## Errors seen`, the sections the prompt names,
/// plus the open task (the last user request).
fn render_state(state: &WorkingState) -> String {
    let files = if state.files.is_empty() {
        "(none)\n".to_string()
    } else {
        state
            .files
            .iter()
            .map(|(path, action)| format!("- `{path}` ({action})\n"))
            .collect()
    };
    let errors = if state.errors.is_empty() {
        "(none)\n".to_string()
    } else {
        state
            .errors
            .iter()
            .map(|(command, code, line)| {
                let code = code.map(|n| format!(" exit {n}")).unwrap_or_default();
                let line = if line.is_empty() {
                    String::new()
                } else {
                    format!(": {line}")
                };
                format!("- `{command}`{code}{line}\n")
            })
            .collect()
    };
    let task = state
        .last_request
        .as_deref()
        .map(|task| format!("Open task: {task}\n\n"))
        .unwrap_or_default();
    format!("{task}## Files touched\n{files}\n## Errors seen\n{errors}")
}

impl Session {
    /// §1.10 steps 1–5. `Ok(true)` when history changed; every failure is a
    /// notice and `Ok(false)`, since a session that cannot compact still runs.
    pub(crate) async fn compact(
        &self,
        trigger: Trigger,
        focus: Option<String>,
    ) -> Result<bool, CoreError> {
        let payload = json!({ "trigger": trigger.name(), "focus": focus });
        if let HookOutcome::Block { reason } =
            hooks::fire(self, HookEvent::PreCompact, payload.clone()).await
        {
            return self
                .compaction_notice(&format!("skipped by hook: {reason}"))
                .await;
        }
        let (history, marks, state) = {
            let inner = self.inner.lock().await;
            (inner.history.clone(), inner.turn_marks.clone(), inner.state)
        };
        let Some((cut, dropped)) = split(&marks, self.config.context.keep_turns) else {
            return self
                .compaction_notice(&format!(
                    "nothing to compact: {} turn(s) fit in keep_turns={}",
                    marks.len(),
                    self.config.context.keep_turns
                ))
                .await;
        };
        self.set_state(State::Compacting).await;
        let summary = self.summarise(&history[..cut], focus.as_deref()).await;
        // Back to where it was: a pre-call compaction runs mid-turn, and an
        // `Idle` there would let `/rewind` in before the turn's next call.
        self.set_state(state).await;
        let Some(summary) = summary else {
            return self.compaction_notice("summariser returned nothing").await;
        };
        let item = ItemId::new();
        let text = format!(
            "[Compacted summary of {} earlier turn(s)]\n\n{summary}",
            dropped.len()
        );
        self.emit(Event::ItemStarted {
            item,
            kind: ItemKind::Summary { text: text.clone() },
        })
        .await?;
        self.emit(Event::ItemDone { item }).await?;
        let (before, after) = {
            let mut inner = self.inner.lock().await;
            // A turn that ran meanwhile changed what `cut` means; give up
            // rather than splice the wrong messages.
            if inner.history.len() != history.len() {
                drop(inner);
                return self
                    .compaction_notice("history changed while summarising")
                    .await;
            }
            let before = estimate_tokens(&inner.history);
            let mut kept = inner.history.split_off(cut);
            let mut next = vec![Message {
                role: Role::User,
                content: vec![Content::Text { text }],
            }];
            next.append(&mut kept);
            inner.history = next;
            let mut kept_marks: Vec<TurnMark> = inner
                .turn_marks
                .split_off(marks.len() - marks.len().min(self.config.context.keep_turns as usize));
            for m in &mut kept_marks {
                m.start = m.start - cut + 1;
            }
            inner.turn_marks = vec![TurnMark {
                item,
                start: 0,
                seq: 0,
            }];
            inner.turn_marks.append(&mut kept_marks);
            let after = estimate_tokens(&inner.history);
            inner.last_context_tokens = after;
            (before, after)
        };
        self.emit(Event::Compacted {
            summary: item,
            dropped,
            before_tokens: before,
            after_tokens: after,
            reason: trigger.reason(),
        })
        .await?;
        let _ = hooks::fire(self, HookEvent::PostCompact, payload).await;
        // §1.10 step 5 (P43): the prefix restarts here anyway, so the repo
        // map is rebuilt with it; no notice of its own.
        self.rebuild_repomap(RepoMapReason::Compaction).await?;
        Ok(true)
    }

    /// T28.3, §1.10 pre-call: keeps a request under `compact_at ×
    /// max_context` before it is sent. Microcompaction first (every archived
    /// result outside `keep_turns` → pointer, request-only), then one full
    /// compaction and one re-assembly. `build(history, turn_starts,
    /// microcompact_after_turns)` is the caller's assembly, so the retried
    /// request is built exactly like the first.
    pub(crate) async fn fit_request(
        &self,
        req: Request,
        build: impl Fn(&[Message], &[usize], u32) -> Request,
    ) -> Result<Fit, CoreError> {
        let Some(limit) = threshold(
            self.provider.capabilities().max_context,
            self.config.context.compact_at,
        ) else {
            return Ok(Fit::Fits(req));
        };
        if self.request_tokens(&req, limit).await.is_none() {
            return Ok(Fit::Fits(req));
        }
        let req = self.rebuild(&build).await;
        if self.request_tokens(&req, limit).await.is_none() {
            return Ok(Fit::Fits(req));
        }
        let req = if self.compact(Trigger::PreCall, None).await? {
            self.rebuild(&build).await
        } else {
            req
        };
        Ok(match self.request_tokens(&req, limit).await {
            None => Fit::Fits(req),
            Some(tokens) => Fit::TooBig(tokens),
        })
    }

    /// The current history, assembled with every result outside
    /// `keep_turns` microcompacted (the last turns are never touched).
    async fn rebuild(&self, build: &impl Fn(&[Message], &[usize], u32) -> Request) -> Request {
        let (history, starts) = {
            let inner = self.inner.lock().await;
            let starts: Vec<usize> = inner.turn_marks.iter().map(|m| m.start).collect();
            (inner.history.clone(), starts)
        };
        build(&history, &starts, 0)
    }

    /// `Some(tokens)` when `req` is at or over `limit`. The heuristic
    /// decides unless it is within 10 % of `limit` and the provider can
    /// count; a failed count falls back to the estimate rather than
    /// blocking the call.
    async fn request_tokens(&self, req: &Request, limit: f64) -> Option<u32> {
        let mut tokens = estimate(req);
        if self.provider.capabilities().count_tokens
            && near(tokens, limit)
            && let Ok(n) = self.provider.count_tokens(req).await
        {
            tokens = n;
        }
        (f64::from(tokens) >= limit).then_some(tokens)
    }

    /// `/handoff` (T26.3): the `compact` job over the whole history with the
    /// focus `hand off: <objective>`, recorded against this session. The
    /// history is left untouched — the summary seeds a new child session.
    pub async fn handoff_summary(&self, objective: &str) -> Option<String> {
        let history = self.inner.lock().await.history.clone();
        self.summarise(&history, Some(&format!("hand off: {objective}")))
            .await
    }

    async fn compaction_notice(&self, why: &str) -> Result<bool, CoreError> {
        self.emit(Event::Notice {
            level: Level::Warn,
            text: format!("compaction {why}"),
        })
        .await?;
        Ok(false)
    }

    /// One request on the `compact` job, recorded in the ledger like any
    /// other (D6g); `None` when the provider fails or answers nothing.
    async fn summarise(&self, messages: &[Message], focus: Option<&str>) -> Option<String> {
        // T9.1: the summary routes like any other job, so a `/model` switch
        // of the cheap tier applies here too.
        let route = self.route_for(Job::Compact, true).await.ok()?;
        let model = route.model.clone();
        // `state+llm` pre-fills files and errors from the transcript. The
        // model is still capped at `MAX_SUMMARY_TOKENS`; that cap is the
        // narrative only, so the state block does not spend it.
        let state = (self.config.compaction.strategy == CompactionStrategy::StatePlusLlm)
            .then(|| render_state(&working_state(messages)));
        let mut system = PROMPT.to_string();
        if let Some(focus) = focus {
            system.push_str(&format!("\nFocus on: {focus}\n"));
        }
        let text = match &state {
            Some(block) => format!(
                "Working state (already recorded; do not repeat these sections):\n\n{block}\nTranscript:\n{}",
                transcript(messages)
            ),
            None => transcript(messages),
        };
        let req = Request {
            tier: route.tier,
            job: Job::Compact,
            model: model.clone(),
            system: vec![SystemBlock {
                text: system,
                cache: false,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text { text }],
            }],
            effort: route.effort,
            max_tokens: MAX_SUMMARY_TOKENS.min(route.max_tokens),
            thinking: route.thinking,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        };
        let (tx, mut rx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.cancel_token();
        let join = tokio::spawn(async move { provider.stream(req, tx, cancel).await });
        let mut out = String::new();
        while let Some(ev) = rx.recv().await {
            if let ProviderEvent::TextDelta { text } = ev {
                out.push_str(&text);
            }
        }
        let usage = join.await.ok()?.ok()?;
        self.store
            .usage_insert(&cox_protocol::UsageRow {
                session_id: self.id,
                turn: 0,
                job: Job::Compact,
                tier: route.tier,
                provider: self.provider.id(),
                model,
                effort: Some(route.effort),
                usage,
            })
            .ok()?;
        if budget::counts(route.tier, self.config.budget.cheap_counts) {
            self.add_spend(usage.cost_usd).await;
        }
        let out = out.trim().to_string();
        match state {
            Some(block) if out.is_empty() => Some(block),
            Some(block) => Some(format!("{block}\n{out}")),
            None => (!out.is_empty()).then_some(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn marks(n: usize) -> Vec<TurnMark> {
        (0..n)
            .map(|i| TurnMark {
                item: ItemId::new(),
                start: i * 2,
                seq: i as u32 + 1,
            })
            .collect()
    }

    #[test]
    fn compact_split_keeps_the_last_turns_and_names_the_rest() {
        assert!(split(&marks(2), 2).is_none());
        let m = marks(5);
        let (cut, dropped) = split(&m, 2).expect("three to drop");
        assert_eq!(cut, 6);
        assert_eq!(dropped, [m[0].item, m[1].item, m[2].item]);
    }

    #[test]
    fn compact_trigger_is_a_fraction_of_max_context() {
        assert!(needs_compaction(750, 1000, 0.75));
        assert!(!needs_compaction(749, 1000, 0.75));
        assert!(!needs_compaction(1, 0, 0.75));
    }

    #[test]
    fn pre_call_asks_for_an_exact_count_only_within_ten_percent() {
        assert!(near(900, 1000.0) && near(1100, 1000.0));
        assert!(!near(899, 1000.0) && !near(1101, 1000.0));
        assert_eq!(threshold(0, 0.75), None);
    }

    fn exchange(name: &str, input: Value, content: &str, is_error: bool) -> [Message; 2] {
        use cox_protocol::ids::CallId;
        let id = CallId::new();
        [
            Message {
                role: Role::Assistant,
                content: vec![Content::ToolUse {
                    id,
                    name: name.into(),
                    input,
                }],
            },
            Message {
                role: Role::User,
                content: vec![Content::ToolResult {
                    call_id: id,
                    content: content.into(),
                    is_error,
                }],
            },
        ]
    }

    fn touched_transcript() -> Vec<Message> {
        let text = |text: &str| Message {
            role: Role::User,
            content: vec![Content::Text { text: text.into() }],
        };
        let mut messages = vec![text("rename the parser")];
        // Read then edit of the same path: one slot, action raised to edited.
        messages.extend(exchange(
            "read",
            json!({"path": "src/a.rs"}),
            "1|fn main() {}",
            false,
        ));
        messages.extend(exchange(
            "edit",
            json!({"path": "src/a.rs", "old": "a", "new": "b"}),
            "edited",
            false,
        ));
        messages.extend(exchange(
            "write",
            json!({"path": "src/c.rs"}),
            "wrote",
            false,
        ));
        messages.extend(exchange(
            "apply_patch",
            json!({"patch": "*** Add File: src/d.rs\n*** Update File: src/e.rs\n"}),
            "applied",
            false,
        ));
        messages.extend(exchange(
            "bash",
            json!({"command": "echo ok"}),
            "[exit 0 in 3ms]",
            false,
        ));
        messages.extend(exchange(
            "bash",
            json!({"command": "cargo test"}),
            "error: could not compile `cox`\n[exit 1 in 40ms]",
            true,
        ));
        messages.push(text("fix the compile error"));
        messages
    }

    async fn summary_for(strategy: CompactionStrategy, narrative: &str) -> String {
        use std::path::PathBuf;
        use std::sync::Arc;

        use cox_provider::scripted::Scripted;

        let mut config = cox_protocol::Config::default();
        config.compaction.strategy = strategy;
        let store = Arc::new(crate::session::MemoryStore::new());
        let provider = Arc::new(
            Scripted::from_toml(&format!("[[turn]]\ntext = {narrative:?}\n"), "")
                .expect("scenario"),
        );
        crate::session::Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-compact"),
        )
        .expect("session")
        .summarise(&touched_transcript(), None)
        .await
        .expect("summary")
    }

    #[tokio::test]
    async fn state_plus_llm_summary_lists_every_touched_path_and_failing_command() {
        let narrative = "## Goal\ncontinue\n## Next step\nrerun the suite";
        let plus = summary_for(CompactionStrategy::StatePlusLlm, narrative).await;
        assert!(plus.contains("`src/a.rs` (edited)"), "{plus}");
        assert!(plus.contains("`src/c.rs` (created)"), "{plus}");
        assert!(plus.contains("`src/d.rs` (created)"), "{plus}");
        assert!(plus.contains("`src/e.rs` (edited)"), "{plus}");
        assert_eq!(plus.matches("src/a.rs").count(), 1, "{plus}");
        assert!(
            plus.contains("`cargo test` exit 1: error: could not compile `cox`"),
            "{plus}"
        );
        assert!(
            plus.contains("fix the compile error") && plus.contains(narrative),
            "{plus}"
        );
        assert!(!plus.contains("echo ok"), "{plus}");
        assert_eq!(
            summary_for(CompactionStrategy::Llm, narrative).await,
            narrative
        );
    }
}
