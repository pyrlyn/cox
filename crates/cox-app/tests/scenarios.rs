// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The timeline fold over real event streams (T37.8, T37.38, DT§8): each
//! scripted scenario runs a live cox-core session over the Scripted
//! provider, its patches are snapshotted, and the rollout read back through
//! JSONL — as `cox resume` reads it — folds to the very same patches.

#[path = "../../cox-core/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_app::{Block, BlockKind, Controller, Meter, Tally, Timeline, TimelinePatch, coalesce};
use cox_core::{MemoryStore, Session};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Store, Tool, ToolCx};
use cox_protocol::types::{
    Concurrency, Decision, Diff, Event, Risk, Submission, ToolOutput, ToolSpec,
};
use cox_protocol::{Before, Change, Checkpointer, Config, PreImage, Snapshot, UsageRow};
use cox_provider::scripted::Scripted;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// What the test does when the turn stops for the person.
#[derive(Clone)]
enum Act {
    Nothing,
    Approve(Decision),
    InterruptAtFirstCall,
    /// Runs a second user turn, then rewinds code and conversation to it.
    RewindSecondTurn,
}

struct Case {
    name: &'static str,
    /// The crate whose `tests/scenarios` holds the TOML.
    home: &'static str,
    config: Config,
    act: Act,
}

fn core(name: &'static str, config: Config, act: Act) -> Case {
    let home = "cox-core";
    Case {
        name,
        home,
        config,
        act,
    }
}

/// Scenarios from `crates/cox-core/tests/scenarios`, with the config and
/// reaction their own core tests use, then this crate's own.
fn cases() -> Vec<Case> {
    let mut big = Config::default();
    big.context.tool_output_visible_bytes = 120;
    big.context.tool_output_head_lines = 2;
    big.context.tool_output_tail_lines = 2;
    let mut one_turn = Config::default();
    one_turn.core.max_turns = 1;
    let mut writes = Config::default();
    writes.permissions.allow = vec!["touch".into()];
    let plain = Config::default;
    let deny = Decision::Deny {
        reason: "no".into(),
    };
    vec![
        core("text_only", plain(), Act::Nothing),
        core("one_tool", plain(), Act::Nothing),
        core("three_parallel", plain(), Act::Nothing),
        core("big_tool_output", big, Act::Nothing),
        core("provider_error", plain(), Act::Nothing),
        core("max_turns", one_turn, Act::Nothing),
        core("interrupt", plain(), Act::InterruptAtFirstCall),
        core("ask_then_approve", plain(), Act::Approve(Decision::Allow)),
        core("ask_then_deny", plain(), Act::Approve(deny)),
        core(
            "allow_for_session",
            plain(),
            Act::Approve(Decision::AllowForSession),
        ),
        core("subagent_explore", plain(), Act::Nothing),
        Case {
            home: "cox-app",
            ..core("explore", plain(), Act::Nothing)
        },
        Case {
            home: "cox-app",
            ..core("checkpoint_rewind", writes, Act::RewindSecondTurn)
        },
        Case {
            home: "cox-app",
            ..core("edit", plain(), Act::Nothing)
        },
    ]
}

/// A read-only stub under a real tool's name, whose `structured` payload
/// and diff are the ones the real tool sends (`cox-tools` `read`/`grep`/`edit`).
struct Stub {
    name: &'static str,
    structured: Value,
    diff: Option<Diff>,
}

#[async_trait]
impl Tool for Stub {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "exploring stub".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        let field = input.get("path").or_else(|| input.get("pattern"));
        field.and_then(Value::as_str).unwrap_or("").into()
    }
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            text: "1\tfn main() {}".into(),
            is_error: false,
            diff: self.diff.clone(),
            structured: Some(self.structured.clone()),
        })
    }
}

/// Every path held "old"; restoring always works; no shell snapshots.
struct Fake;

#[async_trait]
impl Checkpointer for Fake {
    async fn preimages(&self, roots: &[PathBuf], _cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        let image = |p: &String| PreImage {
            path: roots[0].join(p),
            before: Before::Bytes(b"old".to_vec()),
        };
        paths.iter().map(image).collect()
    }
    async fn snapshot(&self, _roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        Ok(Snapshot { trees: vec![] })
    }
    async fn changes(&self, _: &Snapshot, _: &Snapshot) -> Result<Vec<Change>, ToolError> {
        Ok(vec![])
    }
    async fn restore(
        &self,
        _roots: &[PathBuf],
        _cwd: &Path,
        _path: &Path,
        _bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        Ok(())
    }
}

/// `common::open` plus the exploring stubs and, for the rewind case, the
/// fake checkpointer.
fn open(
    toml: &str,
    mut config: Config,
    act: &Act,
) -> (Session, Arc<MemoryStore>, mpsc::Receiver<Event>) {
    config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-turn")];
    let provider = Arc::new(Scripted::from_toml(toml, "").expect("scenario"));
    let mut tools = common::tools();
    tools.push(Arc::new(Stub {
        name: "read",
        structured: json!({"lines": 1}),
        diff: None,
    }));
    tools.push(Arc::new(Stub {
        name: "grep",
        structured: json!({"matches": 1}),
        diff: None,
    }));
    // Two hunks: numbering restarts at each header; a file marker is dropped.
    let unified = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n use std::io;\n\
        -fn old() {}\n+fn new() -> u8 { 1 }\n@@ -40,2 +40,3 @@ impl Backoff\n     let x = 2;\n\
        +    // more\n }\n";
    tools.push(Arc::new(Stub {
        name: "edit",
        structured: json!({}),
        diff: Some(Diff {
            path: "src/lib.rs".into(),
            unified: unified.into(),
        }),
    }));
    let store = Arc::new(MemoryStore::new());
    let cwd = PathBuf::from("/tmp/cox-turn");
    let session =
        Session::new(config, provider, tools, store.clone(), store.clone(), cwd).expect("session");
    // The other scenarios keep the default checkpointer, as in cox-core.
    if matches!(act, Act::RewindSecondTurn) {
        session.set_checkpointer(Arc::new(Fake));
    }
    let rx = session.events().expect("events once");
    (session, store, rx)
}

/// The scenario's user turns: the live events, then the rollout.
async fn run(case: &Case) -> (Vec<Event>, Vec<Event>) {
    let (session, store, mut rx) = open(&scenario(case), case.config.clone(), &case.act);
    let mut running = common::spawn_turn(&session, case.name);
    let mut live = Vec::new();
    let mut turns = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event timeout")
            .expect("event stream open");
        match (&event, &case.act) {
            (Event::ApprovalRequired { call, .. }, Act::Approve(decision)) => {
                let approve = Submission::Approve {
                    call_id: call.id,
                    decision: decision.clone(),
                };
                session.submit(approve).await.expect("approve");
            }
            (Event::ToolCallRequested { .. }, Act::InterruptAtFirstCall) => {
                session
                    .submit(Submission::Interrupt)
                    .await
                    .expect("interrupt");
            }
            _ => {}
        }
        let done = matches!(event, Event::TurnDone { .. });
        // A rewind ends with the `Notice` that says what it did.
        let rewound = matches!(event, Event::Notice { .. })
            && live.iter().any(|e| matches!(e, Event::Rewound { .. }));
        live.push(event);
        if rewound {
            break;
        }
        if !done {
            continue;
        }
        let _ = (&mut running).await.expect("join");
        turns += 1;
        match (&case.act, turns) {
            (Act::RewindSecondTurn, 1) => running = common::spawn_turn(&session, "second turn"),
            (Act::RewindSecondTurn, _) => {
                let rewind = Submission::Rewind {
                    to_turn: 2,
                    code: true,
                    conversation: true,
                };
                session.submit(rewind).await.expect("rewind");
            }
            _ => break,
        }
    }
    let rollout = store.rollout_read(&session.id()).expect("rollout");
    (live, rollout)
}

/// The case's TOML, from the `tests/scenarios` of the crate it lives in.
fn scenario(case: &Case) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../{}/tests/scenarios", case.home))
        .join(format!("{}.toml", case.name));
    std::fs::read_to_string(&path).expect("scenario file")
}

fn fold(events: &[Event]) -> Vec<TimelinePatch> {
    let mut timeline = Timeline::new("base16-ocean.dark");
    events.iter().flat_map(|e| timeline.apply(e)).collect()
}

/// The rollout as `cox resume` gets it: one JSON line per event, parsed.
fn through_jsonl(events: &[Event]) -> Vec<Event> {
    let jsonl: String = events
        .iter()
        .map(|e| serde_json::to_string(e).expect("serialize") + "\n")
        .collect();
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse rollout line"))
        .collect()
}

fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !b"ILOU".contains(&b)))
}

/// Numbers ULIDs by first appearance (`#1`, `#2`, …) — so the snapshot still
/// shows which blocks share a key — and zeroes timings and token estimates.
fn normalize(value: &mut Value, ids: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            let words: Vec<String> = s
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| is_ulid(w))
                .map(str::to_owned)
                .collect();
            for word in words {
                let n = match ids.iter().position(|id| *id == word) {
                    Some(n) => n + 1,
                    None => {
                        ids.push(word.clone());
                        ids.len()
                    }
                };
                *s = s.replace(&word, &format!("#{n}"));
            }
        }
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match k.as_str() {
                    "duration_ms" | "input_tokens" | "output_tokens" | "cache_read_tokens"
                    | "cache_write_tokens" | "latency_ms" => *v = Value::from(0),
                    "cost_usd" => *v = Value::from(0.0),
                    _ => normalize(v, ids),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| normalize(v, ids)),
        _ => {}
    }
}

#[tokio::test]
async fn replay_equals_live() {
    for case in cases() {
        let (live, rollout) = run(&case).await;
        assert_eq!(
            fold(&live),
            fold(&through_jsonl(&rollout)),
            "{}: the replayed rollout folds differently",
            case.name
        );
    }
}

#[tokio::test]
async fn patches_match_snapshot_per_scenario() {
    for case in cases() {
        let (live, _) = run(&case).await;
        let mut ids = Vec::new();
        let lines: Vec<String> = fold(&live)
            .iter()
            .map(|patch| {
                let mut value = serde_json::to_value(patch).expect("patch json");
                normalize(&mut value, &mut ids);
                value.to_string()
            })
            .collect();
        insta::assert_snapshot!(case.name, lines.join("\n"));
    }
}

/// Sixty tool rounds in one user turn: more events than the core's channel
/// (256) holds, so a drain that waited on the consumer would stall the turn.
fn flood() -> String {
    let mut toml = String::new();
    for i in 0..60 {
        toml += &format!(
            "[[turn]]\ntext = \"round {i}\"\ntool_calls = [{{ name = \"echo\", input = {{ text = \"e{i}\" }} }}]\n\n"
        );
    }
    toml + "[[turn]]\ntext = \"done\"\n"
}

fn turn_over(blocks: &[Block]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(&b.kind, BlockKind::TurnMeta { stop: Some(_), .. }))
}

fn mirror(patches: Vec<TimelinePatch>) -> Vec<Block> {
    let mut blocks = Vec::new();
    patches
        .into_iter()
        .for_each(|p| coalesce::apply(&mut blocks, p));
    blocks
}

/// Paused time: the consumer's 2 s naps cost nothing, and the clock only
/// advances past one when every task is idle, so a core blocked on a full
/// channel would finish at 2 s or later.
#[tokio::test(start_paused = true)]
async fn slow_consumer_never_stalls_the_core() {
    let nap = Duration::from_secs(2);
    let mut runs: Vec<(String, String, Config)> = cases()
        .into_iter()
        .filter(|case| matches!(case.act, Act::Nothing))
        .map(|case| (case.name.to_owned(), scenario(&case), case.config))
        .collect();
    runs.push(("flood".into(), flood(), Config::default()));
    for (name, toml, config) in runs {
        let (session, store, rx) = open(&toml, config, &Act::Nothing);
        let controller = Controller::spawn(Timeline::new("base16-ocean.dark"), rx);
        let start = tokio::time::Instant::now();
        let turn = common::spawn_turn(&session, &name);
        let finished = tokio::spawn(async move {
            let _ = turn.await;
            start.elapsed()
        });
        let (mut blocks, mut widest, mut meter) = (Vec::new(), 0, None);
        while !turn_over(&blocks) {
            let batch = tokio::time::timeout(Duration::from_secs(60), controller.next_patches())
                .await
                .expect("patches in time")
                .expect("stream open");
            // The meter's and the status's patches sit beside the blocks.
            let beside = |p: &&TimelinePatch| {
                matches!(
                    p,
                    TimelinePatch::Usage { .. } | TimelinePatch::Status { .. }
                )
            };
            widest = widest.max(batch.len() - batch.iter().filter(beside).count());
            for patch in batch {
                match patch {
                    TimelinePatch::Usage { usage } => meter = Some(usage.session),
                    p => coalesce::apply(&mut blocks, p),
                }
            }
            tokio::time::sleep(nap).await;
        }
        let took = finished.await.expect("join");
        assert!(
            took < nap,
            "{name}: the turn waited on the consumer ({took:?})"
        );
        let events = store.rollout_read(&session.id()).expect("rollout");
        assert_eq!(
            meter,
            Some(ledger(&rows_of(&store, &session))),
            "{name}: the last queued meter is not the ledger's"
        );
        if name == "flood" {
            assert!(events.len() > 256, "flood emitted only {}", events.len());
        }
        assert!(
            widest <= blocks.len(),
            "{name}: a batch of {widest} patches for {} blocks",
            blocks.len()
        );
        assert_eq!(
            blocks,
            mirror(fold(&events)),
            "{name}: coalesced state differs from applying every patch"
        );
    }
}

/// This session's ledger rows; a subagent writes its own under its own id.
fn rows_of(store: &MemoryStore, session: &Session) -> Vec<UsageRow> {
    let mut rows = store.usage_rows();
    rows.retain(|r| r.session_id == session.id());
    rows
}

/// The DS§7 figures summed straight from ledger rows.
fn ledger(rows: &[UsageRow]) -> Tally {
    let sum = |f: fn(&UsageRow) -> u32| rows.iter().map(f).sum();
    Tally {
        sent: sum(|r| {
            r.usage.input_tokens + r.usage.cache_read_tokens + r.usage.cache_write_tokens
        }),
        received: sum(|r| r.usage.output_tokens),
        cache_read: sum(|r| r.usage.cache_read_tokens),
        cache_write: sum(|r| r.usage.cache_write_tokens),
        uncached: sum(|r| r.usage.input_tokens),
        cost_usd: rows.iter().map(|r| r.usage.cost_usd).sum(),
        calls: rows.len() as u32,
        estimated: rows.iter().any(|r| r.usage.estimated),
    }
}

/// Two user turns — a tool round then a reply (two calls), and a reply (one
/// call): each turn's meter and the session's equal the rows it wrote.
#[tokio::test]
async fn meter_totals_equal_the_ledger_rows() {
    let toml = "[[turn]]\ntext = \"echoing\"\ntool_calls = [{ name = \"echo\", input = { text = \"hi\" } }]\n\n[[turn]]\ntext = \"done\"\n\n[[turn]]\ntext = \"again\"\n";
    let (session, store, mut rx) = open(toml, Config::default(), &Act::Nothing);
    for text in ["first", "second"] {
        let _ = common::spawn_turn(&session, text).await.expect("join");
    }
    let (mut meter, mut turns) = (Meter::default(), Vec::new());
    while let Ok(event) = rx.try_recv() {
        meter.apply(&event, Duration::ZERO);
        if matches!(event, Event::TurnDone { .. }) {
            turns.extend(meter.view().turn.as_ref().map(|t| t.tally));
        }
    }
    let rows = rows_of(&store, &session);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.usage.output_tokens > 0));
    assert_eq!(turns, vec![ledger(&rows[..2]), ledger(&rows[2..])]);
    assert_eq!(meter.view().session, ledger(&rows));
}
