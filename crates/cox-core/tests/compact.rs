// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Compaction integration tests (T8.1 §1.10).
//!
//! Names use `compact_` so `cargo test -p cox-core compact_` matches them;
//! the plan's `compaction_…` names do not contain that substring.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{drain, spawn_turn};
use cox_core::{History, MemoryStore, Session};
use cox_protocol::errors::{ProviderError, ToolError};
use cox_protocol::traits::{Provider, Store as _, Tool, ToolCx};
use cox_protocol::types::{
    Caps, CompactReason, Concurrency, Content, Event, ItemKind, Job, Level, ModelId, ProviderEvent,
    ProviderId, Request, Risk, StopReason, Submission, ToolOutput, ToolSpec, Usage,
};
use cox_provider::scripted::Scripted;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const THREE_PLUS_SUMMARY: &str = concat!(
    "[[turn]]\ntext = \"first\"\n",
    "[[turn]]\ntext = \"second\"\n",
    "[[turn]]\ntext = \"third\"\n",
    "[[turn]]\ntext = \"## Goal\\nstuff\\n## Decisions\\nnone\\n## Files touched\\na.rs\\n## Open todo\\nnone\\n## Errors seen\\nnone\\n## Next step\\ngo\"\n",
);

fn open_three() -> (Session, Arc<MemoryStore>, mpsc::Receiver<Event>) {
    let config = cox_protocol::Config::default();
    let provider = Arc::new(Scripted::from_toml(THREE_PLUS_SUMMARY, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        common::tools(),
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let rx = session.events().expect("events once");
    (session, store, rx)
}

async fn user_turn(session: &Session, rx: &mut mpsc::Receiver<Event>, text: &str) -> Vec<Event> {
    let running = spawn_turn(session, text);
    let events = drain(rx).await;
    running.await.expect("join").expect("turn");
    events
}

async fn three_turns() -> (Session, Arc<MemoryStore>, mpsc::Receiver<Event>) {
    let (session, store, mut rx) = open_three();
    for t in ["t0", "t1", "t2"] {
        user_turn(&session, &mut rx, t).await;
    }
    (session, store, rx)
}

#[tokio::test]
async fn compact_keeps_last_two_turns_verbatim() {
    let (session, _, mut rx) = three_turns().await;
    let before = session.history().await;
    assert_eq!(before.len(), 6, "3 turns x user+assistant");
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    // Compaction emits ItemStarted/Done + Compacted, no TurnDone.
    let mut saw_compacted = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, Event::Compacted { .. }) {
            saw_compacted = true;
        }
    }
    assert!(saw_compacted, "Compacted event emitted");
    let after = session.history().await;
    assert_eq!(after.len(), 5, "summary + 2 kept turns");
    assert!(matches!(
        &after[0].content[0],
        cox_protocol::types::Content::Text { text } if text.contains("stuff")
    ));
    assert_eq!(&after[1..], &before[2..], "kept turns byte-identical");
}

#[tokio::test]
async fn compact_is_append_only_in_rollout() {
    use cox_protocol::ids::SessionId;
    let (session, store, _) = three_turns().await;
    let dummy = SessionId::new();
    // MemoryStore ignores the session key, so a dummy id reads everything.
    let n_before = store.rollout_read(&dummy).expect("read").len();
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    let events = store.rollout_read(&dummy).expect("read");
    assert!(events.len() > n_before, "rollout only grows");
    let compacted = events
        .iter()
        .find_map(|e| match e {
            Event::Compacted { dropped, .. } => Some(dropped.clone()),
            _ => None,
        })
        .expect("Compacted in rollout");
    assert_eq!(compacted.len(), 1, "3 turns keep 2, drop 1");
    // Original user items are still in the rollout.
    let users = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::ItemStarted {
                    kind: ItemKind::UserMessage { .. },
                    ..
                }
            )
        })
        .count();
    assert!(users >= 3, "dropped turn still on disk");
    // Rebuild matches live memory.
    let rebuilt = History::from_events(&events).messages;
    assert_eq!(rebuilt, session.history().await);
}

#[tokio::test]
async fn compact_request_after_compaction_keeps_cached_prefix() {
    let (session, _, _) = three_turns().await;
    let tools = common::tools();
    let prefix_of = |h: &[cox_protocol::types::Message]| {
        cox_core::assemble(
            h,
            &cox_protocol::Config::default(),
            &tools,
            std::path::Path::new("/tmp/cox-turn"),
            "",
        )
        .system[..=2]
            .iter()
            .map(|b| b.text.clone())
            .collect::<Vec<_>>()
    };
    let before = prefix_of(&session.history().await);
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    assert_eq!(before, prefix_of(&session.history().await));
}

/// Records the summariser's system prompt so the focus test sees it.
struct Probe {
    system: Mutex<String>,
}

#[async_trait]
impl Provider for Probe {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }
    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: true,
            max_context: u32::MAX,
        }
    }
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        if req.job == Job::Compact {
            *self.system.lock().expect("lock") = req.system[0].text.clone();
        }
        let text = if req.job == Job::Compact {
            "compacted-summary"
        } else {
            "ok"
        };
        for ev in [
            ProviderEvent::MessageStart {
                model: ModelId("probe".into()),
            },
            ProviderEvent::TextDelta { text: text.into() },
            ProviderEvent::Stop {
                stop: cox_protocol::types::StopReason::EndTurn,
            },
            ProviderEvent::Usage {
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    estimated: true,
                    cost_usd: 0.0,
                    latency_ms: 0,
                },
            },
        ] {
            sink.send(ev).await.map_err(|_| ProviderError::Cancelled)?;
        }
        Ok(Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: true,
            cost_usd: 0.0,
            latency_ms: 0,
        })
    }
    async fn count_tokens(&self, _req: &Request) -> Result<u32, ProviderError> {
        Ok(10)
    }
}

#[tokio::test]
async fn compact_focus_is_passed_to_summarizer() {
    let probe = Arc::new(Probe {
        system: Mutex::new(String::new()),
    });
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        cox_protocol::Config::default(),
        probe.clone(),
        common::tools(),
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    for t in ["a", "b", "c"] {
        user_turn(&session, &mut rx, t).await;
    }
    session
        .submit(Submission::Compact {
            focus: Some("auth flow".into()),
        })
        .await
        .expect("compact");
    assert!(
        probe.system.lock().expect("lock").contains("auth flow"),
        "focus reaches summariser system prompt"
    );
}

/// What the summariser is sent for `config`, after three turns and a compact.
async fn compact_system_prompt(config: cox_protocol::Config) -> String {
    let probe = Arc::new(Probe {
        system: Mutex::new(String::new()),
    });
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        probe.clone(),
        common::tools(),
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    for t in ["a", "b", "c"] {
        user_turn(&session, &mut rx, t).await;
    }
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    probe.system.lock().expect("lock").clone()
}

#[tokio::test]
async fn compact_default_strategy_is_state_llm_and_adds_the_narrative_only_note() {
    let system = compact_system_prompt(cox_protocol::Config::default()).await;
    assert!(system.contains("do not write"), "{system}");
}

#[tokio::test]
async fn compact_llm_strategy_still_sends_exactly_the_old_prompt() {
    let mut config = cox_protocol::Config::default();
    config.compaction.strategy = cox_protocol::config::CompactionStrategy::Llm;
    let system = compact_system_prompt(config).await;
    assert!(!system.contains("do not write"), "{system}");
    assert!(system.ends_with("## Next step\n"), "{system}");
}

// (MemoryStore ignores the session key, so tests read with a dummy id.)

/// `Scripted` behind a finite window, recording every request it is sent;
/// `Scripted` itself reports an unbounded one, so nothing would trigger.
struct Capped {
    inner: Scripted,
    max_context: u32,
    sent: Mutex<Vec<Request>>,
}

#[async_trait]
impl Provider for Capped {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self) -> Caps {
        // No exact count: the heuristic alone decides, so the thresholds
        // below are deterministic.
        Caps {
            max_context: self.max_context,
            count_tokens: false,
            ..self.inner.capabilities()
        }
    }
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        self.sent.lock().expect("lock").push(req.clone());
        self.inner.stream(req, sink, cancel).await
    }
    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        self.inner.count_tokens(req).await
    }
}

/// Read-only; returns `self.0` bytes on one line.
struct Big(usize);

#[async_trait]
impl Tool for Big {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "big".into(),
            description: "large output".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        "big".into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: "y".repeat(self.0),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// ⌈bytes/4⌉ of a request: the heuristic the pre-call check uses.
fn tokens(req: &Request) -> u32 {
    (serde_json::to_vec(req).expect("json").len() as u32).div_ceil(4)
}

/// Three turns of `big_tool_output_mid_turn`: a 16 000-byte first prompt,
/// then turn 3 calls `big` (`output` bytes). Returns turn 3's events.
async fn big_mid_turn(
    output: usize,
    max_context: u32,
) -> (Vec<Event>, Arc<Capped>, Arc<MemoryStore>, Session) {
    let mut config = cox_protocol::Config::default();
    config.context.tool_output_visible_bytes = 128_000;
    let provider = Arc::new(Capped {
        inner: Scripted::from_toml(&common::scenario("big_tool_output_mid_turn"), "")
            .expect("scenario"),
        max_context,
        sent: Mutex::new(Vec::new()),
    });
    let mut tools = common::tools();
    tools.push(Arc::new(Big(output)));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider.clone(),
        tools,
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    user_turn(&session, &mut rx, &"x".repeat(16_000)).await;
    user_turn(&session, &mut rx, "t1").await;
    let events = user_turn(&session, &mut rx, "t2").await;
    (events, provider, store, session)
}

/// 0.75 × 8 800 = 6 600 tokens. The three turn-3 requests measure ~4 600
/// (first call), ~8 700 (after a 16 000-byte result) and ~4 700 once the
/// first turn's 16 000-byte prompt is summarised, so only the second
/// crosses the threshold and compaction brings it back under.
const WINDOW: u32 = 8_800;

#[tokio::test]
async fn big_tool_output_mid_turn_compacts_before_call() {
    let (events, provider, store, session) = big_mid_turn(16_000, WINDOW).await;
    let at = |want: &dyn Fn(&Event) -> bool| events.iter().position(want).expect("event");
    let done = at(&|e| matches!(e, Event::ToolCallDone { .. }));
    let compacted = at(&|e| {
        matches!(
            e,
            Event::Compacted {
                reason: CompactReason::PreCall,
                ..
            }
        )
    });
    let next_call = events[done..]
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::ItemStarted {
                    kind: ItemKind::AssistantMessage { .. },
                    ..
                }
            )
        })
        .expect("a call after the tool")
        + done;
    assert!(
        done < compacted && compacted < next_call,
        "compacted mid-turn, before the call"
    );
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    let sent = provider.sent.lock().expect("lock").clone();
    let jobs: Vec<Job> = sent.iter().map(|r| r.job.clone()).collect();
    assert_eq!(
        jobs,
        [Job::Main, Job::Main, Job::Main, Job::Compact, Job::Main]
    );
    let last = sent.last().expect("final call");
    assert!(f64::from(tokens(last)) < 0.75 * f64::from(WINDOW));
    // The summary replaced turn 1; turns 2 and 3 went out verbatim, the big
    // result included (never a pointer: it is inside the last two turns).
    let history = session.history().await;
    assert_eq!(&last.messages[..], &history[..last.messages.len()]);
    assert!(matches!(&history[0].content[0], Content::Text { text } if text.contains("read big")));
    assert!(last.messages.iter().all(|m| {
        m.content
            .iter()
            .all(|c| !matches!(c, Content::Text { text } if text.starts_with("xxx")))
    }));
    assert!(last.messages.iter().any(|m| {
        m.content
            .iter()
            .any(|c| matches!(c, Content::ToolResult { content, .. } if content.len() >= 16_000))
    }));
    // The summary call is billed like every other request.
    assert_eq!(store.usage_rows().len(), sent.len());
}

#[tokio::test]
async fn pre_call_still_over_after_compaction_stops_with_budget() {
    // 40 000 bytes stay in the kept turns, so no compaction can fit them.
    let (events, provider, _, _) = big_mid_turn(40_000, WINDOW).await;
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Compacted {
            reason: CompactReason::PreCall,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Notice { level: Level::Budget, text } if text.contains("request not sent")
    )));
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::Budget,
            ..
        })
    ));
    let jobs: Vec<Job> = provider
        .sent
        .lock()
        .expect("lock")
        .iter()
        .map(|r| r.job.clone())
        .collect();
    assert_eq!(
        jobs,
        [Job::Main, Job::Main, Job::Main, Job::Compact],
        "nothing sent after"
    );
}

/// A stub answering under a real tool's name, so `working_state` reads
/// its calls the way it reads the built-in tools'.
struct Named(&'static str, &'static str, bool);

#[async_trait]
impl Tool for Named {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.into(),
            description: "named stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Exclusive,
        }
    }
    fn subject(&self, _input: &Value) -> String {
        self.0.into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: self.1.into(),
            is_error: self.2,
            diff: None,
            structured: None,
        })
    }
}

const STATE_SCENARIO: &str = concat!(
    "[[turn]]\ntext = \"working\"\ntool_calls = [\n",
    "  { name = \"read\", input = { path = \"src/lib.rs\" } },\n",
    "  { name = \"edit\", input = { path = \"src/lib.rs\" } },\n",
    "  { name = \"bash\", input = { command = \"cargo test\" } },\n",
    "]\n",
    "[[turn]]\ntext = \"first done\"\n",
    "[[turn]]\ntext = \"second\"\n",
    "[[turn]]\ntext = \"third\"\n",
    "[[turn]]\ntext = \"## Goal\\nfix the parser\\n## Next step\\nrerun\"\n",
);

#[tokio::test]
async fn compact_state_llm_summary_lists_touched_paths_and_failing_command() {
    let mut config = cox_protocol::Config::default();
    config.compaction.strategy = cox_protocol::config::CompactionStrategy::StateLlm;
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(Named("read", "1 fn parse()", false)),
        Arc::new(Named("edit", "edited", false)),
        Arc::new(Named(
            "bash",
            "error: test parse failed\n[exit 101 in 9ms]",
            true,
        )),
    ];
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        Arc::new(Scripted::from_toml(STATE_SCENARIO, "").expect("scenario")),
        tools,
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    for t in ["fix the parser", "t1", "t2"] {
        user_turn(&session, &mut rx, t).await;
    }
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    let history = session.history().await;
    let Content::Text { text } = &history[0].content[0] else {
        panic!("summary first: {:?}", history[0]);
    };
    assert!(text.contains("- src/lib.rs: read, edited"), "{text}");
    assert!(
        text.contains("- `cargo test` → exit 101: error: test parse failed"),
        "{text}"
    );
    assert!(text.contains("## Last request\nfix the parser"), "{text}");
    assert!(
        text.contains("## Goal\nfix the parser"),
        "model text follows: {text}"
    );
}

/// A scenario of one tool-calling turn per entry of `turns`, each `(tool,
/// calls)`, then the summariser's reply. A tool turn is two `[[turn]]`s:
/// the calls, then the answer once the results are in.
fn archived_scenario(turns: &[(&str, usize)], summary: &str) -> String {
    let mut toml = String::new();
    for (tool, calls) in turns {
        let call = format!("  {{ name = \"{tool}\", input = {{ path = \"a.rs\" }} }},\n");
        toml.push_str(&format!(
            "[[turn]]\ntext = \"working\"\ntool_calls = [\n{}]\n[[turn]]\ntext = \"done\"\n",
            call.repeat(*calls)
        ));
    }
    toml.push_str(&format!("[[turn]]\ntext = \"{summary}\"\n"));
    toml
}

/// Runs one user turn per entry of `turns`, compacts, and returns the
/// history before and after with the archive ids each turn wrote.
async fn compact_archived(
    turns: &[(&str, usize)],
    strategy: cox_protocol::config::CompactionStrategy,
) -> (
    Vec<cox_protocol::types::Message>,
    Vec<cox_protocol::types::Message>,
    Vec<Vec<String>>,
) {
    let mut config = cox_protocol::Config::default();
    config.compaction.strategy = strategy;
    let tools: Vec<Arc<dyn Tool>> = ["read", "grep", "edit"]
        .into_iter()
        .map(|name| Arc::new(Named(name, "output", false)) as Arc<dyn Tool>)
        .collect();
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        Arc::new(
            Scripted::from_toml(&archived_scenario(turns, "## Goal\\nship"), "").expect("scenario"),
        ),
        tools,
        store.clone(),
        store.clone(),
        PathBuf::from("/tmp/cox-turn"),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    let mut ids = Vec::new();
    for t in 0..turns.len() {
        let events = user_turn(&session, &mut rx, &format!("t{t}")).await;
        ids.push(
            events
                .iter()
                .filter_map(|e| match e {
                    Event::ToolCallDone { result, .. } => result.archive.as_ref(),
                    _ => None,
                })
                .map(|a| a.id.to_string())
                .collect(),
        );
        // Archive ids are ULIDs: distinct milliseconds keep id order equal
        // to turn order, which the snapshot depends on.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let before = session.history().await;
    session
        .submit(Submission::Compact { focus: None })
        .await
        .expect("compact");
    (before, session.history().await, ids)
}

fn summary_text(history: &[cox_protocol::types::Message]) -> String {
    let Content::Text { text } = &history[0].content[0] else {
        panic!("summary first: {:?}", history[0]);
    };
    text.clone()
}

/// Archive ids are random; the snapshot names them by position instead.
fn redact_ids(mut text: String, ids: &[String]) -> String {
    for (n, id) in ids.iter().enumerate() {
        text = text.replace(id, &format!("ID{}", n + 1));
    }
    text
}

#[tokio::test]
async fn compaction_notice_lists_pointer_ids_and_keeps_last_turns_verbatim() {
    use cox_protocol::config::CompactionStrategy::{Llm, StateLlm};
    // Turns 1-3 are dropped (three archived outputs), turns 4-5 are kept.
    let turns = [
        ("read", 1),
        ("grep", 1),
        ("edit", 1),
        ("read", 1),
        ("grep", 1),
    ];
    let (before, after, ids) = compact_archived(&turns, StateLlm).await;
    let text = summary_text(&after);
    let flat: Vec<String> = ids.concat();
    for id in &flat[..3] {
        assert!(text.contains(id.as_str()), "{id} missing from {text}");
    }
    for id in &flat[3..] {
        assert!(
            !text.contains(id.as_str()),
            "kept turn's {id} named: {text}"
        );
    }
    let kept = before.len() - after.len() + 1;
    assert_eq!(
        &after[1..],
        &before[kept..],
        "last two turns byte-identical"
    );
    insta::assert_snapshot!(redact_ids(text, &flat));

    // The model's text is last under `strategy = llm` too.
    let (_, after_llm, ids) = compact_archived(&turns, Llm).await;
    let text = summary_text(&after_llm);
    assert!(
        text.contains("## Goal\nship\n\n## Archived outputs\n"),
        "{text}"
    );
    assert!(
        text.lines()
            .last()
            .is_some_and(|l| l.starts_with("- #") && l.ends_with(" edit")),
        "{text}"
    );
    assert!(
        ids.concat()[..3]
            .iter()
            .all(|id| text.contains(id.as_str()))
    );
}

#[tokio::test]
async fn compaction_notice_shows_32_of_40_ids() {
    let turns = [("read", 40), ("grep", 1), ("edit", 1)];
    let (_, after, _) =
        compact_archived(&turns, cox_protocol::config::CompactionStrategy::Llm).await;
    let text = summary_text(&after);
    let listed = text.lines().filter(|l| l.starts_with("- #")).count();
    assert_eq!(listed, 32, "{text}");
    assert!(text.ends_with("\n… and 8 more"), "{text}");
}
