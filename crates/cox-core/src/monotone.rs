// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The decision points after `route` (PL§4 "Decision points", T33.21,
//! T33.21.1): `risk`, `approve_hint`, `compact`, `rank` and `salience`. Each
//! asks the advisor `[plugins.decide]` names and applies the point's
//! monotone rule here, in the core, so no answer moves a decision the unsafe
//! way: risk only up, a caution only, compaction only earlier, rank only
//! over cox's own candidates, salience only orders or drops memory items
//! already extracted. The permission engine still makes every tool
//! decision; `risk` only changes the call it judges. Separate from
//! `advise.rs` so these guards and routing change independently.

use std::time::Duration;

use cox_protocol::errors::CoreError;
use cox_protocol::plugin::{Advice, Answer, DecidePoint, Question, QuestionItem};
use cox_protocol::types::{Event, Level, Risk, ToolCall, Why};
use cox_sanitize::redact::scrub;
use cox_sanitize::sanitize;
use serde_json::json;

use crate::compact;
use crate::memory_extract::Fact;
use crate::permission::Outcome;
use crate::session::Session;

/// How much of a call's input a question carries (J5.1: never tool output).
const INPUT_CHARS: usize = 2_000;
/// How much of a caution note reaches the user.
const NOTE_CHARS: usize = 200;

/// What asking a point came to. `Late` is no answer like `Silent`; it is
/// kept apart only so a point can say its deadline passed (A163).
enum Reply {
    Silent,
    Late,
    Advice(String, Advice),
}

impl Session {
    /// One answer from `plugin` for `point` within `ms`, or `None` when the
    /// point is off, the plugin is not live, or it is silent or late.
    async fn ask_point(
        &self,
        plugin: Option<&str>,
        ms: u64,
        question: Question,
    ) -> Option<(String, Advice)> {
        match self.ask_timed(plugin, ms, question).await {
            Reply::Advice(plugin, advice) => Some((plugin, advice)),
            Reply::Silent | Reply::Late => None,
        }
    }

    async fn ask_timed(&self, plugin: Option<&str>, ms: u64, question: Question) -> Reply {
        let Some((plugin, advisor)) = plugin.and_then(|p| Some((p, self.advisor(p)?))) else {
            return Reply::Silent;
        };
        let budget = Duration::from_millis(ms);
        // The core holds the budget itself, as for `route`.
        match tokio::time::timeout(budget, advisor.advise(question, budget)).await {
            Err(_) => Reply::Late,
            Ok(None) => Reply::Silent,
            Ok(Some(advice)) => Reply::Advice(plugin.to_string(), advice),
        }
    }

    async fn advised(
        &self,
        point: DecidePoint,
        plugin: String,
        mut advice: Advice,
        applied: bool,
    ) -> Result<(), CoreError> {
        // Only an applied `approve_hint` or `risk` note is kept: the first is
        // shown, the second carries the probabilities behind a raise (A163).
        // Any other note is untrusted text with no use and stays out of the
        // rollout.
        let kept = matches!(point, DecidePoint::ApproveHint | DecidePoint::Risk);
        advice.note = clean_note(advice.note.filter(|_| kept && applied));
        self.emit(Event::Advised {
            point,
            plugin,
            advice,
            applied,
        })
        .await
    }

    /// `calls` (one tool batch, after `PreToolUse`) with `risk` advice
    /// applied, before `Engine::decide` judges them: one question for the
    /// batch (J5.1). A call is an item only when raising it to
    /// `Destructive` would turn the engine's `Allow` into `Ask` or `Deny`,
    /// so a read-only call, a call already asked, or `Bypass` never waits on
    /// a plugin.
    pub(crate) async fn advise_risk(
        &self,
        mut calls: Vec<ToolCall>,
    ) -> Result<Vec<ToolCall>, CoreError> {
        let decide = &self.config.plugins.decide;
        if decide.risk.is_none() {
            return Ok(calls);
        }
        let allowed = |o: Outcome| matches!(o, Outcome::Allow { .. });
        let mut asked = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if matches!(call.risk, Risk::ReadOnly | Risk::Destructive) {
                continue;
            }
            let raised = ToolCall {
                risk: Risk::Destructive,
                ..call.clone()
            };
            if allowed(self.decide(call).await) && !allowed(self.decide(&raised).await) {
                asked.push(i);
            }
        }
        if asked.is_empty() {
            return Ok(calls);
        }
        let task = compact::working_state(&self.inner.lock().await.history).request;
        let question = Question {
            point: DecidePoint::Risk,
            state: json!({
                "task": clip(task.as_deref().unwrap_or_default(), INPUT_CHARS),
                "cwd": self.cwd.to_string_lossy(),
                "workspace_roots": self
                    .config
                    .core
                    .workspace_roots
                    .iter()
                    .map(|r| r.to_string_lossy())
                    .collect::<Vec<_>>(),
                "sandbox": self.config.sandbox.mode,
            }),
            options: vec![],
            items: asked
                .iter()
                .map(|&i| QuestionItem {
                    state: json!({
                        "tool": calls[i].name,
                        "subject": clip(&calls[i].subject, INPUT_CHARS),
                        "input": clip(&calls[i].input.to_string(), INPUT_CHARS),
                        "classifier_risk": calls[i].risk,
                    }),
                    options: vec![],
                })
                .collect(),
        };
        let fits = question.clone();
        let plugin = decide.risk.as_deref();
        let (plugin, advice) = match self.ask_timed(plugin, decide.risk_ms, question).await {
            Reply::Advice(plugin, advice) => (plugin, advice),
            Reply::Silent => return Ok(calls),
            Reply::Late => {
                let text = format!(
                    "plugin {} gave no risk advice within {} ms; the classifier's risk stands",
                    plugin.unwrap_or_default(),
                    decide.risk_ms
                );
                self.emit(Event::Notice {
                    level: Level::Info,
                    text,
                })
                .await?;
                return Ok(calls);
            }
        };
        let mut applied = false;
        // The host checks the shape too; checked again so an advisor that is
        // not a plugin cannot pin one call's answer on another.
        if let Answer::Items { answers } = &advice.answer
            && fits.fits(&advice.answer)
        {
            for (&i, answer) in asked.iter().zip(answers) {
                let one = Advice {
                    answer: answer.clone(),
                    ..advice.clone()
                };
                let (risk, raised) = raise_risk(calls[i].risk, &one, decide.min_confidence);
                calls[i].risk = risk;
                applied |= raised;
            }
        }
        self.advised(DecidePoint::Risk, plugin, advice, applied)
            .await?;
        Ok(calls)
    }

    /// `approve_hint` for a call about to be shown for approval: a caution
    /// notice the core frames itself, or nothing. Warning-only (§14
    /// decision 10), so no answer can read as "looks safe".
    pub(crate) async fn advise_approval(
        &self,
        call: &ToolCall,
        why: &Why,
    ) -> Result<(), CoreError> {
        let decide = &self.config.plugins.decide;
        let question = Question {
            point: DecidePoint::ApproveHint,
            state: json!({
                "tool": call.name,
                "subject": clip(&call.subject, INPUT_CHARS),
                "input": clip(&call.input.to_string(), INPUT_CHARS),
                "risk": call.risk,
                "why": why,
            }),
            options: vec![],
            items: vec![],
        };
        let Some((plugin, mut advice)) = self
            .ask_point(
                decide.approve_hint.as_deref(),
                decide.approve_hint_ms,
                question,
            )
            .await
        else {
            return Ok(());
        };
        let applied = yes(&advice, decide.min_confidence);
        advice.note = clean_note(advice.note);
        if applied {
            let reason = advice.note.as_deref().unwrap_or("check this call closely");
            self.emit(Event::Notice {
                level: Level::Warn,
                text: format!("caution from plugin {plugin}: {reason}"),
            })
            .await?;
        }
        self.advised(DecidePoint::ApproveHint, plugin, advice, applied)
            .await
    }

    /// Whether to compact at a turn's start. A `mandatory` compaction runs
    /// without asking, so no answer can skip it; otherwise `compact` advice
    /// may only bring it earlier, and is asked only when there is history
    /// beyond `keep_turns` to compact.
    pub(crate) async fn compact_now(
        &self,
        mandatory: bool,
        context_tokens: u32,
        max_context: u32,
    ) -> Result<bool, CoreError> {
        if mandatory {
            return Ok(true);
        }
        let decide = &self.config.plugins.decide;
        let turns = self.inner.lock().await.turn_marks.len();
        if decide.compact.is_none() || turns <= self.config.context.keep_turns as usize {
            return Ok(false);
        }
        let question = Question {
            point: DecidePoint::Compact,
            state: json!({
                "context_tokens": context_tokens,
                "max_context": max_context,
                "compact_at": self.config.context.compact_at,
                "turns": turns,
            }),
            options: vec![],
            items: vec![],
        };
        let Some((plugin, advice)) = self
            .ask_point(decide.compact.as_deref(), decide.compact_ms, question)
            .await
        else {
            return Ok(false);
        };
        let now = yes(&advice, decide.min_confidence);
        self.advised(DecidePoint::Compact, plugin, advice, now)
            .await?;
        Ok(now)
    }

    /// `found` (what `tool_search` returned for `query`) after `rank`
    /// advice: reordered or filtered, never added to.
    pub(crate) async fn advise_rank(&self, query: &str, found: Vec<String>) -> Vec<String> {
        let decide = &self.config.plugins.decide;
        let question = Question {
            point: DecidePoint::Rank,
            state: json!({ "query": clip(query, INPUT_CHARS) }),
            options: found.clone(),
            items: vec![],
        };
        let Some((plugin, advice)) = self
            .ask_point(decide.rank.as_deref(), decide.rank_ms, question)
            .await
        else {
            return found;
        };
        let (ranked, applied) = rank(&found, &advice, decide.min_confidence);
        // A rollout write failure is the emitter's to report; the call's
        // result stands either way.
        let _ = self
            .advised(DecidePoint::Rank, plugin, advice, applied)
            .await;
        ranked
    }

    /// `facts` (what memory extraction parsed) after `salience` advice:
    /// items below `[memory].salience_min` dropped, never added to or
    /// edited. One question carries every item, scrubbed like any other
    /// question text (`Question` has no batch `items` field yet, T33.40.1,
    /// so they travel in `state["items"]`).
    pub(crate) async fn advise_salience(&self, facts: Vec<Fact>) -> Vec<Fact> {
        let decide = &self.config.plugins.decide;
        if facts.is_empty() {
            return facts;
        }
        let items: Vec<_> = facts
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "kind": f.kind,
                    "body": clip(&f.body, INPUT_CHARS),
                })
            })
            .collect();
        let question = Question {
            point: DecidePoint::Salience,
            state: json!({ "items": items }),
            options: vec![],
            items: vec![],
        };
        let Some((plugin, advice)) = self
            .ask_point(decide.salience.as_deref(), decide.salience_ms, question)
            .await
        else {
            return facts;
        };
        let (kept, applied) = keep_salient(
            &facts,
            &advice,
            decide.min_confidence,
            self.config.memory.salience_min,
        );
        // A rollout write failure is the emitter's to report; extraction
        // stands either way.
        let _ = self
            .advised(DecidePoint::Salience, plugin, advice, applied)
            .await;
        kept
    }
}

/// A plugin's note as the user or the rollout may see it: no escapes, at
/// most `NOTE_CHARS`, and none when only blanks are left.
fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| sanitize(&n).chars().take(NOTE_CHARS).collect::<String>())
        .filter(|n| !n.trim().is_empty())
}

/// The same redaction the rollout gets, clipped to a question's size.
fn clip(text: &str, chars: usize) -> String {
    scrub(text).chars().take(chars).collect()
}

fn confident(advice: &Advice, min_confidence: f64) -> bool {
    advice.confidence.is_some_and(|c| c >= min_confidence)
}

/// A yes/no point's verdict: yes only when `p_yes` reaches `min_confidence`
/// and a stated confidence does too (J11: a yes/no may give none).
fn yes(advice: &Advice, min_confidence: f64) -> bool {
    let sure = advice.confidence.is_none_or(|c| c >= min_confidence);
    matches!(advice.answer, Answer::Noul { p_yes } if p_yes >= min_confidence) && sure
}

/// `Risk` on the J5.1 severity scale: 0 read-only … 3 destructive.
fn level(risk: Risk) -> u8 {
    match risk {
        Risk::ReadOnly => 0,
        Risk::Write => 1,
        Risk::Exec => 2,
        Risk::Destructive => 3,
    }
}

/// `max(builtin, advised)`: a confident severity `Score` rounded onto the
/// scale; anything else keeps `builtin`. Returns the risk and whether the
/// advice raised it.
fn raise_risk(builtin: Risk, advice: &Advice, min_confidence: f64) -> (Risk, bool) {
    let advised = match advice.answer {
        Answer::Score { value } if confident(advice, min_confidence) && value.is_finite() => {
            [Risk::ReadOnly, Risk::Write, Risk::Exec, Risk::Destructive]
                .into_iter()
                .rev()
                .find(|r| f64::from(level(*r)) - 0.5 <= value)
        }
        _ => None,
    };
    match advised {
        Some(risk) if level(risk) > level(builtin) => (risk, true),
        _ => (builtin, false),
    }
}

/// `candidates` in a confident `Choice`'s order, keeping only the indices
/// it names once each; anything else keeps `candidates` as they are.
fn rank(candidates: &[String], advice: &Advice, min_confidence: f64) -> (Vec<String>, bool) {
    let Answer::Choice { order } = &advice.answer else {
        return (candidates.to_vec(), false);
    };
    if !confident(advice, min_confidence) {
        return (candidates.to_vec(), false);
    }
    let mut out: Vec<String> = Vec::new();
    for i in order {
        let Some(name) = usize::try_from(*i).ok().and_then(|i| candidates.get(i)) else {
            continue;
        };
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    (out, true)
}

/// `facts` after a confident `Scores` answer: items whose score clears
/// `threshold` (a `[memory]` config value the advice never moves), in their
/// original order. Anything else — a different answer shape, low or missing
/// confidence, or a `values` length that does not match `facts` — keeps
/// every fact.
fn keep_salient(
    facts: &[Fact],
    advice: &Advice,
    min_confidence: f64,
    threshold: f64,
) -> (Vec<Fact>, bool) {
    let Answer::Scores { values } = &advice.answer else {
        return (facts.to_vec(), false);
    };
    if !confident(advice, min_confidence) || values.len() != facts.len() {
        return (facts.to_vec(), false);
    }
    let kept = facts
        .iter()
        .zip(values)
        .filter(|(_, score)| score.is_finite() && **score >= threshold)
        .map(|(fact, _)| fact.clone())
        .collect();
    (kept, true)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cox_protocol::errors::ToolError;
    use cox_protocol::ids::{CallId, ItemId, TurnId};
    use cox_protocol::traits::{Advisor, Tool, ToolCx};
    use cox_protocol::types::{
        Concurrency, Content, Message, PermissionMode, Role, ToolOutput, ToolSpec,
    };
    use cox_provider::scripted::Scripted;
    use tokio::sync::mpsc;

    use super::*;
    use crate::compact::TurnMark;
    use crate::session::MemoryStore;
    use crate::turn::run_tools;

    struct Fake {
        advice: Advice,
        /// How long `advise` takes before answering; `ZERO` for every test
        /// but the ones proving a late answer is no answer.
        delay: Duration,
        asked: Mutex<Vec<Question>>,
    }

    #[async_trait]
    impl Advisor for Fake {
        fn id(&self) -> &str {
            "fake"
        }

        async fn advise(&self, question: Question, _budget: Duration) -> Option<Advice> {
            self.asked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(question);
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            Some(self.advice.clone())
        }
    }

    impl Fake {
        fn asked(&self) -> usize {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).len()
        }
    }

    fn advice(answer: Answer, confidence: Option<f64>, note: &str) -> Advice {
        Advice {
            answer,
            confidence,
            note: Some(note.into()),
        }
    }

    /// A session whose every decision point is answered by one `Fake`.
    fn open(mode: PermissionMode, advice: Advice) -> (Session, Arc<Fake>, mpsc::Receiver<Event>) {
        open_delayed(mode, advice, Duration::ZERO)
    }

    /// `open`, but the `Fake` takes `delay` before answering.
    fn open_delayed(
        mode: PermissionMode,
        advice: Advice,
        delay: Duration,
    ) -> (Session, Arc<Fake>, mpsc::Receiver<Event>) {
        let (session, fake, rx, _) = open_with(mode, advice, delay, vec![]);
        (session, fake, rx)
    }

    /// `open_delayed` with `tools` and the store the rollout goes to.
    fn open_with(
        mode: PermissionMode,
        advice: Advice,
        delay: Duration,
        tools: Vec<Arc<dyn Tool>>,
    ) -> (Session, Arc<Fake>, mpsc::Receiver<Event>, Arc<MemoryStore>) {
        let mut config = cox_protocol::Config::default();
        config.permissions.mode = mode;
        let decide = &mut config.plugins.decide;
        for point in [
            &mut decide.risk,
            &mut decide.approve_hint,
            &mut decide.compact,
            &mut decide.rank,
            &mut decide.salience,
        ] {
            *point = Some("fake".into());
        }
        let provider = Arc::new(Scripted::from_toml("", "").expect("scenario"));
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            config,
            provider,
            tools,
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-monotone"),
        )
        .expect("session");
        let fake = Arc::new(Fake {
            advice,
            delay,
            asked: Mutex::new(Vec::new()),
        });
        session.set_advisors(vec![fake.clone()]);
        let rx = session.events().expect("events once");
        (session, fake, rx, store)
    }

    fn call(risk: Risk) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "bash".into(),
            input: json!({ "command": "git push --force origin main" }),
            subject: "git push --force origin main".into(),
            risk,
            segments: None,
        }
    }

    fn drain(rx: &mut mpsc::Receiver<Event>) -> Vec<Event> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// A batched `risk` answer: one severity `Score` per item.
    fn items(values: &[f64], confidence: Option<f64>) -> Advice {
        let answers = values
            .iter()
            .map(|&value| Answer::Score { value })
            .collect();
        advice(Answer::Items { answers }, confidence, "")
    }

    /// `advise_risk` for a batch of one call.
    async fn advise_one(session: &Session, call: ToolCall) -> ToolCall {
        let mut out = session.advise_risk(vec![call]).await.expect("risk");
        out.pop().expect("one call in, one out")
    }

    fn advised(events: &[Event]) -> Vec<(bool, Option<String>)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Advised {
                    applied, advice, ..
                } => Some((*applied, advice.note.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn risk_advice_cannot_lower_risk() {
        let score = |value, confidence| advice(Answer::Score { value }, confidence, "");
        assert_eq!(
            raise_risk(Risk::Exec, &score(0.0, Some(1.0)), 0.6),
            (Risk::Exec, false)
        );
        assert_eq!(
            raise_risk(Risk::Exec, &score(-9.0, Some(1.0)), 0.6),
            (Risk::Exec, false)
        );
        assert_eq!(
            raise_risk(Risk::Write, &score(3.0, Some(0.5)), 0.6),
            (Risk::Write, false)
        );
        assert_eq!(
            raise_risk(Risk::Write, &score(3.0, None), 0.6),
            (Risk::Write, false)
        );
        let noul = advice(Answer::Noul { p_yes: 0.0 }, Some(1.0), "");
        assert_eq!(raise_risk(Risk::Exec, &noul, 0.6), (Risk::Exec, false));
        assert_eq!(
            raise_risk(Risk::Write, &score(2.5, Some(0.9)), 0.6),
            (Risk::Destructive, true)
        );

        // Through the session: a "harmless" answer leaves the call as the
        // classifier left it, and a raise is judged by the engine.
        let (session, fake, mut rx) = open(PermissionMode::Auto, items(&[0.0], Some(1.0)));
        let kept = advise_one(&session, call(Risk::Write)).await;
        assert_eq!((kept.risk, fake.asked()), (Risk::Write, 1));
        assert!(matches!(session.decide(&kept).await, Outcome::Allow { .. }));
        assert_eq!(advised(&drain(&mut rx)), vec![(false, None)]);

        let (session, _, mut rx) = open(PermissionMode::Auto, items(&[3.0], Some(0.9)));
        let raised = advise_one(&session, call(Risk::Write)).await;
        assert_eq!(raised.risk, Risk::Destructive);
        assert!(matches!(session.decide(&raised).await, Outcome::Ask(_)));
        assert_eq!(advised(&drain(&mut rx)), vec![(true, None)]);
    }

    #[tokio::test]
    async fn risk_not_asked_when_outcome_would_not_change() {
        let raise = items(&[3.0], Some(1.0));
        // Already asked (Write outside `Auto`), always allowed (`Bypass`),
        // read-only, or already destructive: the answer could change nothing.
        for (mode, risk) in [
            (PermissionMode::Default, Risk::Write),
            (PermissionMode::Bypass, Risk::Write),
            (PermissionMode::Auto, Risk::ReadOnly),
            (PermissionMode::Auto, Risk::Destructive),
        ] {
            let (session, fake, mut rx) = open(mode, raise.clone());
            let out = advise_one(&session, call(risk)).await;
            assert_eq!((out.risk, fake.asked()), (risk, 0), "{mode:?} {risk:?}");
            assert!(advised(&drain(&mut rx)).is_empty());
        }
    }

    /// A `Write` tool whose subject is its `path`; `"ro": true` makes a
    /// call read-only.
    struct Writes;

    #[async_trait]
    impl Tool for Writes {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "w".into(),
                description: "writes".into(),
                input_schema: json!({ "type": "object" }),
                deferred: false,
                risk: Risk::Write,
                concurrency: Concurrency::Parallel,
            }
        }
        fn subject(&self, input: &serde_json::Value) -> String {
            input["path"].as_str().unwrap_or_default().into()
        }
        fn risk(&self, input: &serde_json::Value) -> Risk {
            if input["ro"] == true {
                Risk::ReadOnly
            } else {
                Risk::Write
            }
        }
        fn touches(&self, input: &serde_json::Value) -> Option<Vec<String>> {
            Some(vec![self.subject(input)])
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _cx: &ToolCx,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                text: "SECRET-TOOL-OUTPUT".into(),
                is_error: false,
                diff: None,
                structured: None,
            })
        }
    }

    fn write(path: &str) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: "w".into(),
            input: json!({ "path": path }),
            subject: path.into(),
            risk: Risk::Write,
            segments: None,
        }
    }

    #[tokio::test]
    async fn risk_batch_asks_once_with_one_item_per_call() {
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Writes)];
        let (session, fake, _rx, _) = open_with(
            PermissionMode::Auto,
            items(&[0.0, 0.0], Some(1.0)),
            Duration::ZERO,
            tools,
        );
        session.inner.lock().await.history.push(Message {
            role: Role::User,
            content: vec![Content::Text {
                text: "tidy the docs".into(),
            }],
        });
        let batch = vec![
            (CallId::new(), "w".into(), json!({ "path": "a.md" })),
            (
                CallId::new(),
                "w".into(),
                json!({ "path": "r.md", "ro": true }),
            ),
            (CallId::new(), "w".into(), json!({ "path": "b.md" })),
        ];
        let ran = run_tools(&session, TurnId::new(), batch)
            .await
            .expect("batch");
        assert!(ran.iter().all(|(_, r)| r.ok), "{ran:?}");
        assert_eq!(fake.asked(), 1, "one question for the whole batch");
        let asked = fake.asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let question = &asked[0];
        assert_eq!(question.point, DecidePoint::Risk);
        assert_eq!(question.state["task"], "tidy the docs");
        assert_eq!(question.state["cwd"], "/tmp/cox-monotone");
        assert!(question.state["workspace_roots"].is_array());
        assert!(question.state.get("sandbox").is_some());
        // The read-only call could change nothing, so it is not an item.
        let subjects: Vec<_> = question.items.iter().map(|i| &i.state["subject"]).collect();
        assert_eq!(subjects, ["a.md", "b.md"]);
        let item = question.items[0].state.as_object().expect("item");
        let mut keys: Vec<_> = item.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["classifier_risk", "input", "subject", "tool"]);
        let sent = serde_json::to_string(question).expect("json");
        assert!(!sent.contains("SECRET-TOOL-OUTPUT"), "never tool output");
    }

    #[tokio::test]
    async fn risk_items_raise_only_their_own_call() {
        let batch = || vec![write("a.md"), write("b.md"), write("c.md")];
        let (session, _, mut rx) = open(PermissionMode::Auto, items(&[0.0, 3.0, 1.0], Some(1.0)));
        let out = session.advise_risk(batch()).await.expect("risk");
        let risks: Vec<_> = out.iter().map(|c| c.risk).collect();
        assert_eq!(risks, [Risk::Write, Risk::Destructive, Risk::Write]);
        assert_eq!(advised(&drain(&mut rx)), vec![(true, None)]);

        // A flat answer, a short list, or low confidence: nothing moves.
        for wrong in [
            advice(Answer::Score { value: 3.0 }, Some(1.0), ""),
            items(&[3.0, 3.0], Some(1.0)),
            items(&[3.0, 3.0, 3.0], Some(0.1)),
        ] {
            let (session, fake, mut rx) = open(PermissionMode::Auto, wrong.clone());
            let out = session.advise_risk(batch()).await.expect("risk");
            assert!(out.iter().all(|c| c.risk == Risk::Write), "{wrong:?}");
            assert_eq!(fake.asked(), 1);
            assert_eq!(advised(&drain(&mut rx)), vec![(false, None)], "{wrong:?}");
        }
    }

    #[tokio::test]
    async fn late_risk_answer_records_a_notice() {
        // Later than `risk_ms` (200 by default): every call keeps its
        // classifier risk, and the notice names the point and the deadline.
        let (session, fake, mut rx) = open_delayed(
            PermissionMode::Auto,
            items(&[3.0], Some(1.0)),
            Duration::from_millis(500),
        );
        let ms = session.config.plugins.decide.risk_ms;
        let out = advise_one(&session, write("a.md")).await;
        assert_eq!((out.risk, fake.asked()), (Risk::Write, 1));
        let events = drain(&mut rx);
        assert!(advised(&events).is_empty(), "a late answer is no answer");
        let notices: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Notice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            vec![format!(
                "plugin fake gave no risk advice within {ms} ms; the classifier's risk stands"
            )]
        );
    }

    #[tokio::test]
    async fn risk_note_reaches_the_rollout() {
        use cox_protocol::traits::Store as _;
        let note = "p(irreversible)=0.91 \u{1b}[31mexternal=0.12";
        let mut raise = items(&[3.0], Some(1.0));
        raise.note = Some(note.into());
        let (session, _, _rx, store) =
            open_with(PermissionMode::Auto, raise, Duration::ZERO, vec![]);
        assert_eq!(
            advise_one(&session, write("a.md")).await.risk,
            Risk::Destructive
        );
        let rollout = store.rollout_read(&session.id()).expect("rollout");
        let notes: Vec<_> = rollout
            .iter()
            .filter_map(|e| match e {
                Event::Advised {
                    point: DecidePoint::Risk,
                    applied: true,
                    advice,
                    ..
                } => Some(advice.note.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes,
            vec![Some("p(irreversible)=0.91 external=0.12".to_string())]
        );
    }

    #[tokio::test]
    async fn approve_hint_cannot_say_looks_safe() {
        let why = Why::Risk { risk: Risk::Exec };
        for safe in [
            advice(Answer::Noul { p_yes: 0.02 }, Some(0.99), "looks safe"),
            advice(Answer::Score { value: 0.0 }, Some(0.99), "looks safe"),
            advice(Answer::Choice { order: vec![0] }, Some(0.99), "looks safe"),
            advice(Answer::Noul { p_yes: 0.9 }, Some(0.1), "looks safe"),
        ] {
            let (session, _, mut rx) = open(PermissionMode::Default, safe);
            session
                .advise_approval(&call(Risk::Exec), &why)
                .await
                .expect("hint");
            let events = drain(&mut rx);
            assert!(!events.iter().any(|e| matches!(e, Event::Notice { .. })));
            // The unused note never reaches the rollout either.
            assert_eq!(advised(&events), vec![(false, None)]);
        }
        let caution = advice(
            Answer::Noul { p_yes: 0.9 },
            None,
            "pushes \u{1b}[31mto main",
        );
        let (session, _, mut rx) = open(PermissionMode::Default, caution);
        session
            .advise_approval(&call(Risk::Exec), &why)
            .await
            .expect("hint");
        let events = drain(&mut rx);
        let notices: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Notice {
                    level: Level::Warn,
                    text,
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notices, vec!["caution from plugin fake: pushes to main"]);
    }

    #[tokio::test]
    async fn compact_advice_cannot_skip_mandatory_compaction() {
        let never = advice(Answer::Noul { p_yes: 0.0 }, Some(1.0), "");
        let (session, fake, _rx) = open(PermissionMode::Default, never);
        let mark = TurnMark {
            item: ItemId::new(),
            start: 0,
            seq: 1,
        };
        session.inner.lock().await.turn_marks = vec![mark; 3];
        assert!(session.compact_now(true, 900, 1000).await.expect("compact"));
        assert_eq!(fake.asked(), 0, "a due compaction is not up for a vote");
        assert!(
            !session
                .compact_now(false, 100, 1000)
                .await
                .expect("compact")
        );
        assert_eq!(fake.asked(), 1);

        // Earlier is allowed, but only when there is something to compact.
        let now = advice(Answer::Noul { p_yes: 0.9 }, Some(0.9), "");
        let (session, fake, _rx) = open(PermissionMode::Default, now);
        assert!(
            !session
                .compact_now(false, 100, 1000)
                .await
                .expect("compact")
        );
        assert_eq!(fake.asked(), 0, "nothing beyond keep_turns");
        session.inner.lock().await.turn_marks = vec![mark; 3];
        assert!(
            session
                .compact_now(false, 100, 1000)
                .await
                .expect("compact")
        );
    }

    #[tokio::test]
    async fn rank_advice_cannot_add_tools() {
        let found: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let choice =
            |order: Vec<u32>, confidence| advice(Answer::Choice { order }, Some(confidence), "");
        assert_eq!(
            rank(&found, &choice(vec![2, 7, 0, 2, u32::MAX], 0.9), 0.6),
            (vec!["c".to_string(), "a".to_string()], true)
        );
        assert_eq!(rank(&found, &choice(vec![], 0.9), 0.6), (vec![], true));
        assert_eq!(
            rank(&found, &choice(vec![1], 0.1), 0.6),
            (found.clone(), false)
        );

        let (session, fake, mut rx) = open(PermissionMode::Default, choice(vec![9, 1], 0.9));
        let ranked = session.advise_rank("github issue", found.clone()).await;
        assert_eq!(ranked, vec!["b".to_string()]);
        let asked = fake.asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(asked[0].options, found);
        assert_eq!(advised(&drain(&mut rx)), vec![(true, None)]);
    }

    fn fact(name: &str) -> Fact {
        Fact {
            name: name.into(),
            kind: "fact".into(),
            body: format!("body of {name}"),
        }
    }

    #[tokio::test]
    async fn salience_advice_cannot_add_memory_items() {
        let facts = vec![fact("a"), fact("b"), fact("c")];
        let scores =
            |values: Vec<f64>, confidence| advice(Answer::Scores { values }, Some(confidence), "");

        // Pure rule: the survivors are a subset of what was sent, never a
        // new name, and never more of them.
        assert_eq!(
            keep_salient(&facts, &scores(vec![1.0, 0.0, 1.0], 0.9), 0.6, 0.3),
            (vec![facts[0].clone(), facts[2].clone()], true)
        );
        // A `values` list that does not match the items sent — as if the
        // plugin answered about a different batch — is out of shape and
        // changes nothing, so it cannot smuggle an extra item in either.
        assert_eq!(
            keep_salient(&facts, &scores(vec![1.0, 1.0, 1.0, 1.0], 0.9), 0.6, 0.3),
            (facts.clone(), false)
        );

        let (session, fake, mut rx) =
            open(PermissionMode::Default, scores(vec![1.0, 0.0, 1.0], 0.9));
        let kept = session.advise_salience(facts.clone()).await;
        assert_eq!(kept, vec![facts[0].clone(), facts[2].clone()]);
        assert_eq!(fake.asked(), 1);
        let asked = fake.asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(asked[0].point, DecidePoint::Salience);
        assert_eq!(asked[0].state["items"].as_array().expect("items").len(), 3);
        assert_eq!(advised(&drain(&mut rx)), vec![(true, None)]);

        // An empty extraction is never asked about and never grows one.
        let (session, fake, _rx) = open(PermissionMode::Default, scores(vec![], 0.9));
        assert_eq!(session.advise_salience(vec![]).await, vec![]);
        assert_eq!(fake.asked(), 0);
    }

    #[tokio::test]
    async fn salience_thresholds_stay_in_config() {
        let facts = vec![fact("a"), fact("b")];
        let scores = advice(
            Answer::Scores {
                values: vec![0.5, 0.99],
            },
            Some(0.9),
            "",
        );

        // Pure rule: the same advice, a different `[memory]` bar, a
        // different outcome — the threshold argument, not the advice,
        // decides.
        assert_eq!(keep_salient(&facts, &scores, 0.6, 0.3).0, facts.clone());
        assert_eq!(
            keep_salient(&facts, &scores, 0.6, 0.6).0,
            vec![facts[1].clone()]
        );
        assert_eq!(keep_salient(&facts, &scores, 0.6, 1.0).0, vec![]);

        // Through the session: `config.memory.salience_min` is the only
        // knob, the plugin's answer carries no threshold of its own.
        let (mut session, fake, _rx) = open(PermissionMode::Default, scores.clone());
        session.config.memory.salience_min = 0.9;
        let kept = session.advise_salience(facts.clone()).await;
        assert_eq!(
            kept,
            vec![facts[1].clone()],
            "only b clears the configured bar"
        );
        assert_eq!(fake.asked(), 1);

        let (mut session, _fake, _rx) = open(PermissionMode::Default, scores);
        session.config.memory.salience_min = 0.3;
        let kept = session.advise_salience(facts.clone()).await;
        assert_eq!(kept, facts, "the same low bar keeps both");
    }

    #[tokio::test]
    async fn late_salience_keeps_every_item() {
        let facts = vec![fact("a"), fact("b")];
        let drop_all = advice(
            Answer::Scores {
                values: vec![0.0, 0.0],
            },
            Some(0.9),
            "",
        );

        // Later than `salience_ms` (300 by default): the answer is no
        // answer, every item survives.
        let (session, fake, mut rx) = open_delayed(
            PermissionMode::Default,
            drop_all.clone(),
            Duration::from_millis(500),
        );
        assert_eq!(session.advise_salience(facts.clone()).await, facts);
        assert_eq!(fake.asked(), 1, "the point was asked");
        assert!(
            advised(&drain(&mut rx)).is_empty(),
            "a late answer is no answer"
        );

        // No plugin configured for the point: never asked, every item kept.
        let (mut session, fake, _rx) = open(PermissionMode::Default, drop_all.clone());
        session.config.plugins.decide.salience = None;
        assert_eq!(session.advise_salience(facts.clone()).await, facts);
        assert_eq!(fake.asked(), 0);

        // Confidence below `min_confidence`: the drop is not trusted, every
        // item is kept.
        let unsure = advice(
            Answer::Scores {
                values: vec![0.0, 0.0],
            },
            Some(0.1),
            "",
        );
        let (session, fake, mut rx) = open(PermissionMode::Default, unsure);
        assert_eq!(session.advise_salience(facts.clone()).await, facts);
        assert_eq!(
            advised(&drain(&mut rx)),
            vec![(false, None)],
            "recorded, but not applied"
        );
        assert_eq!(fake.asked(), 1);
    }
}
