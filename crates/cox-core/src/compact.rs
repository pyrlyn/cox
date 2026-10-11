// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Compaction (plan.md §1.10, T8.1): replaces every turn but the last
//! `keep_turns` with one summary from the `compact` job. Append-only (D6f):
//! the rollout keeps every original event and `Compacted.dropped` says which
//! turns a rebuild skips. Also keeps a request under the threshold before
//! it is sent (T28.3). Separate from `session.rs` because it is the only
//! place history is ever rewritten in memory.

use std::collections::HashMap;

use cox_protocol::config::CompactionStrategy;
use cox_protocol::errors::CoreError;
use cox_protocol::ids::{CallId, ItemId};
use cox_protocol::types::{
    CompactReason, Content, Event, HookEvent, HookOutcome, ItemKind, Job, Level, Message,
    ProviderEvent, RepoMapReason, Request, Role, SystemBlock,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::budget;
use crate::hooks;
use crate::session::{Session, State};
use crate::subagent::first_line;

/// The summariser prompt and, after its `---` line, the note `state+llm`
/// appends: one file so the two texts are reviewed together.
const PROMPT: &str = include_str!("prompts/compact.md");
/// §1.10 step 3: the summary itself is capped.
const MAX_SUMMARY_TOKENS: u32 = 2048;
/// How `compact` opens the summary message; a later compaction finds the
/// earlier state block by it.
const SUMMARY_HEAD: &str = "[Compacted summary of ";
const FILES: &str = "## Files touched";
const ERRORS: &str = "## Errors seen";
const REQUEST: &str = "## Last request";
/// A pasted log as the request would crowd out the rest of the state.
const REQUEST_CHARS: usize = 500;

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

/// What a successful tool call did to a path (T59.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Read,
    Edited,
    Created,
    Deleted,
}

impl Action {
    const ALL: [Self; 4] = [Self::Read, Self::Edited, Self::Created, Self::Deleted];

    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edited => "edited",
            Self::Created => "created",
            Self::Deleted => "deleted",
        }
    }
}

/// The facts a summary states from the transcript instead of the model's
/// recall (T59.1), each list in order of first appearance so the same
/// transcript always renders the same block.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct WorkingState {
    pub files: Vec<(String, Vec<Action>)>,
    /// One rendered line per distinct failing command.
    pub errors: Vec<String>,
    pub request: Option<String>,
}

impl WorkingState {
    fn touch(&mut self, path: &str, action: Action) {
        if path.is_empty() {
            return;
        }
        match self.files.iter_mut().find(|(p, _)| p == path) {
            Some((_, actions)) if !actions.contains(&action) => actions.push(action),
            Some(_) => {}
            None => self.files.push((path.to_string(), vec![action])),
        }
    }

    fn error(&mut self, line: String) {
        if !self.errors.contains(&line) {
            self.errors.push(line);
        }
    }

    fn result(&mut self, name: &str, input: &Value, content: &str, is_error: bool) {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match (name, is_error) {
            ("bash", true) => self.failure(input, content),
            (_, true) => {}
            ("read" | "outline", _) => self.touch(path, Action::Read),
            ("edit", _) => self.touch(path, Action::Edited),
            // `write` says nothing about whether the file existed; a path
            // the session already saw was there before.
            ("write", _) if self.files.iter().any(|(p, _)| p == path) => {
                self.touch(path, Action::Edited);
            }
            ("write", _) => self.touch(path, Action::Created),
            // The tool's own `A`/`M`/`D`/`R` lines, so a patch is not parsed
            // a second time here.
            ("apply_patch", _) => content.lines().for_each(|l| self.patched(l)),
            _ => {}
        }
    }

    fn patched(&mut self, line: &str) {
        match line.split_once(' ') {
            Some(("A", path)) => self.touch(path, Action::Created),
            Some(("M", path)) => self.touch(path, Action::Edited),
            Some(("D", path)) => self.touch(path, Action::Deleted),
            Some(("R", moved)) => {
                if let Some((from, to)) = moved.split_once(" -> ") {
                    self.touch(from, Action::Deleted);
                    self.touch(to, Action::Created);
                }
            }
            _ => {}
        }
    }

    /// `bash` ends its output with `[exit N in Tms]` or `[why after Tms;
    /// killed]`; the timing is dropped so a rerun of the same failure is
    /// one line.
    fn failure(&mut self, input: &Value, content: &str) {
        let command = first_line(input.get("command").and_then(Value::as_str).unwrap_or("?"));
        let mut lines = content
            .lines()
            .rev()
            .map(str::trim)
            .filter(|l| !l.is_empty());
        let status = lines.next().unwrap_or("failed");
        let status = status.trim_start_matches('[').trim_end_matches(']');
        let status = status.split_once(" in ").map_or(status, |(s, _)| s);
        self.error(match lines.next() {
            Some(last) => format!("`{command}` → {status}: {}", first_line(last)),
            None => format!("`{command}` → {status}"),
        });
    }

    /// An earlier compaction's state block, so a second compaction keeps
    /// the paths only the first summary still names.
    fn carry(&mut self, summary: &str) {
        for line in section(summary, FILES).filter_map(|l| l.strip_prefix("- ")) {
            if let Some((path, actions)) = line.rsplit_once(": ") {
                for name in actions.split(", ") {
                    if let Some(a) = Action::ALL.into_iter().find(|a| a.name() == name) {
                        self.touch(path, a);
                    }
                }
            }
        }
        for line in section(summary, ERRORS).filter_map(|l| l.strip_prefix("- ")) {
            self.error(line.to_string());
        }
        if let Some(request) = section(summary, REQUEST).next().filter(|r| *r != "none") {
            self.request = Some(request.to_string());
        }
    }

    pub(crate) fn render(&self) -> String {
        let files = self.files.iter().map(|(path, actions)| {
            let names: Vec<&str> = actions.iter().map(|a| a.name()).collect();
            format!("{path}: {}", names.join(", "))
        });
        format!(
            "{FILES}\n{}{ERRORS}\n{}{REQUEST}\n{}\n",
            bullets(files),
            bullets(self.errors.iter().cloned()),
            self.request.as_deref().unwrap_or("none")
        )
    }
}

/// `none` rather than an empty section, so the model never reads a
/// missing list as one it should fill in.
fn bullets(items: impl Iterator<Item = String>) -> String {
    let out: String = items.map(|i| format!("- {i}\n")).collect();
    if out.is_empty() {
        "none\n".to_string()
    } else {
        out
    }
}

/// The lines under `heading`, up to the next `##` heading.
fn section<'a>(text: &'a str, heading: &'static str) -> impl Iterator<Item = &'a str> {
    text.lines()
        .skip_while(move |l| *l != heading)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
}

/// T59.1: every path the session read, edited or created, every failing
/// `bash` command and the last user request, read off `messages` alone.
/// Only calls that succeeded touch a path; a failed `read` read nothing.
pub(crate) fn working_state(messages: &[Message]) -> WorkingState {
    let mut state = WorkingState::default();
    let mut calls: HashMap<CallId, (&str, &Value)> = HashMap::new();
    for m in messages {
        // A user message's first text is what was typed; hook context and
        // attached files follow it as further text blocks.
        let typed = m.content.iter().find_map(|c| match c {
            Content::Text { text } if m.role == Role::User => Some(text),
            _ => None,
        });
        match typed {
            Some(text) if text.starts_with(SUMMARY_HEAD) => state.carry(text),
            Some(text) => {
                let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
                state.request = Some(flat.chars().take(REQUEST_CHARS).collect());
            }
            None => {}
        }
        for c in &m.content {
            match c {
                Content::ToolUse { id, name, input } => {
                    calls.insert(*id, (name, input));
                }
                Content::ToolResult {
                    call_id,
                    content,
                    is_error,
                } => {
                    if let Some((name, input)) = calls.get(call_id) {
                        state.result(name, input, content, *is_error);
                    }
                }
                _ => {}
            }
        }
    }
    state
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
            "{SUMMARY_HEAD}{} earlier turn(s)]\n\n{summary}",
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
        let state = (self.config.compaction.strategy == CompactionStrategy::StateLlm)
            .then(|| working_state(messages).render());
        let (base, note) = PROMPT.split_once("\n---\n").unwrap_or((PROMPT, ""));
        let mut system = format!("{base}\n");
        if state.is_some() {
            system.push_str(note);
        }
        if let Some(focus) = focus {
            system.push_str(&format!("\nFocus on: {focus}\n"));
        }
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
                content: vec![Content::Text {
                    text: transcript(messages),
                }],
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
        let out = out.trim();
        if out.is_empty() {
            return None;
        }
        Some(match state {
            Some(state) => format!("{state}\n{out}"),
            None => out.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
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

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Content::Text { text: text.into() }],
        }
    }

    /// One assistant message with the calls, one user message with their
    /// results: `(tool, input, result, is_error)`.
    fn round(calls: &[(&str, Value, &str, bool)]) -> [Message; 2] {
        let ids: Vec<CallId> = calls.iter().map(|_| CallId::new()).collect();
        let uses = calls
            .iter()
            .zip(&ids)
            .map(|((name, input, ..), id)| Content::ToolUse {
                id: *id,
                name: (*name).into(),
                input: input.clone(),
            });
        let results = calls
            .iter()
            .zip(&ids)
            .map(|((_, _, out, err), id)| Content::ToolResult {
                call_id: *id,
                content: (*out).into(),
                is_error: *err,
            });
        [
            Message {
                role: Role::Assistant,
                content: uses.collect(),
            },
            Message {
                role: Role::User,
                content: results.collect(),
            },
        ]
    }

    fn scripted() -> Vec<Message> {
        let mut m = vec![user("fix the parser")];
        m.extend(round(&[
            ("read", json!({"path": "src/lib.rs"}), "1 fn parse()", false),
            ("read", json!({"path": "missing.rs"}), "no such file", true),
            ("grep", json!({"pattern": "parse"}), "src/lib.rs:1", false),
        ]));
        m.extend(round(&[
            ("edit", json!({"path": "src/lib.rs"}), "edited", false),
            ("write", json!({"path": "tests/parse.rs"}), "wrote", false),
            ("write", json!({"path": "src/lib.rs"}), "wrote", false),
            (
                "bash",
                json!({"command": "cargo test\n  --all"}),
                "running 3 tests\nerror: test parse failed\n[exit 101 in 2300ms]",
                true,
            ),
            (
                "bash",
                json!({"command": "ls"}),
                "a\n[exit 0 in 1ms]",
                false,
            ),
        ]));
        m.extend(round(&[(
            "apply_patch",
            json!({"patch": "*** Begin Patch"}),
            "A docs/a.md\nM src/lib.rs\nR old.rs -> new.rs\nD gone.rs",
            false,
        )]));
        m.push(user("now  run\nclippy"));
        m
    }

    #[test]
    fn working_state_lists_every_touched_path_failing_command_and_request() {
        use Action::*;
        let state = working_state(&scripted());
        let files: Vec<(&str, &[Action])> = state
            .files
            .iter()
            .map(|(p, a)| (p.as_str(), a.as_slice()))
            .collect();
        assert_eq!(
            files,
            [
                ("src/lib.rs", &[Read, Edited][..]),
                ("tests/parse.rs", &[Created][..]),
                ("docs/a.md", &[Created][..]),
                ("old.rs", &[Deleted][..]),
                ("new.rs", &[Created][..]),
                ("gone.rs", &[Deleted][..]),
            ]
        );
        assert_eq!(
            state.errors,
            ["`cargo test` → exit 101: error: test parse failed"]
        );
        assert_eq!(state.request.as_deref(), Some("now run clippy"));
    }

    #[test]
    fn working_state_renders_the_same_block_for_the_same_transcript() {
        let block = working_state(&scripted()).render();
        assert_eq!(block, working_state(&scripted()).render());
        assert!(block.starts_with("## Files touched\n- src/lib.rs: read, edited\n"));
        assert!(block.contains("## Errors seen\n- `cargo test` → exit 101"));
        assert!(block.ends_with("## Last request\nnow run clippy\n"));
        assert_eq!(
            WorkingState::default().render(),
            "## Files touched\nnone\n## Errors seen\nnone\n## Last request\nnone\n"
        );
    }

    #[test]
    fn second_compaction_keeps_the_paths_only_the_first_summary_names() {
        let first = working_state(&scripted());
        let summary = format!(
            "{SUMMARY_HEAD}3 earlier turn(s)]\n\n{}## Goal\nship it\n",
            first.render()
        );
        let mut later = vec![user(&summary)];
        later.extend(round(&[(
            "edit",
            json!({"path": "src/main.rs"}),
            "edited",
            false,
        )]));
        let state = working_state(&later);
        assert_eq!(state.files[..first.files.len()], first.files[..]);
        assert_eq!(
            state.files.last().map(|(p, _)| p.as_str()),
            Some("src/main.rs")
        );
        assert_eq!(state.errors, first.errors);
        assert_eq!(state.request, first.request, "no newer request typed");
    }

    #[test]
    fn llm_prompt_is_unchanged_and_the_state_note_is_separate() {
        let (base, note) = PROMPT.split_once("\n---\n").expect("note after ---");
        assert!(base.ends_with("## Next step"));
        assert!(!base.contains("do not write"));
        assert!(note.contains("do not write") && note.contains("## Goal"));
    }

    #[test]
    fn pre_call_asks_for_an_exact_count_only_within_ten_percent() {
        assert!(near(900, 1000.0) && near(1100, 1000.0));
        assert!(!near(899, 1000.0) && !near(1101, 1000.0));
        assert_eq!(threshold(0, 0.75), None);
    }
}
