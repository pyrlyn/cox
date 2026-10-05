// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox run -p`: the headless surface (plan §1.12, T6.1). One consumer of
//! the same `Event` stream the TUI reads, printed in one of three shapes;
//! the exit code tells a script what happened without parsing anything.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cox_core::Session;
use cox_protocol::Event;
use cox_protocol::ids::{CallId, ItemId, SessionId};
use cox_protocol::image::{self, ImageError, MAX_IMAGE_BYTES};
use cox_protocol::traits::Store as _;
use cox_protocol::types::{
    ApprovalPolicy, Attachment, Decision, ItemKind, Mode, StopReason, Submission, Tier,
};
use cox_store::Store;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::cli::{Cli, RunArgs};
use crate::config_load;
use crate::{resume, session};

/// Exit codes from §1.12: `0 ok · 1 error · 2 denied · 3 budget · 4 interrupted`.
pub const EXIT_OK: i32 = 0;

/// Not a new user-facing timeout config: a fixed safety margin around the
/// `bash` tool's own SIGTERM→SIGKILL grace (`crates/cox-tools/src/bash/
/// mod.rs`'s `TERM_GRACE` plus its PTY-drain `REAP_GRACE`, ~2.5s) so
/// `Session::wait_tasks_cleared` (T34.9 follow-up) cannot hang the
/// headless exit (or the TUI's, T34.11) on a `TaskKind::Shell` task that
/// does not die in time.
pub(crate) const SHELL_CANCEL_GRACE: Duration = Duration::from_secs(5);
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_DENIED: i32 = 2;
pub const EXIT_BUDGET: i32 = 3;
pub const EXIT_INTERRUPTED: i32 = 4;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Text,
    Json,
    StreamJson,
}

/// What one headless turn produced, folded from the event stream.
#[derive(Default)]
struct Outcome {
    session: Option<SessionId>,
    texts: HashMap<ItemId, String>,
    result: String,
    // input, output, cache read, cache write — summed over provider calls.
    tokens: [u32; 4],
    cost_usd: f64,
    turns: u32,
    denied: u32,
    stop: Option<StopReason>,
    failed: bool,
}

impl Outcome {
    fn exit_code(&self) -> i32 {
        match &self.stop {
            _ if self.failed => EXIT_ERROR,
            None | Some(StopReason::Error) => EXIT_ERROR,
            Some(StopReason::Refusal { .. }) => EXIT_DENIED,
            Some(StopReason::Budget) => EXIT_BUDGET,
            Some(StopReason::Interrupted) => EXIT_INTERRUPTED,
            Some(StopReason::EndTurn | StopReason::MaxTurns) if self.denied > 0 => EXIT_DENIED,
            Some(StopReason::EndTurn | StopReason::MaxTurns) => EXIT_OK,
        }
    }

    /// The `json` shape and the `stream-json` `result` alias share one body.
    fn summary(&self) -> Value {
        let session = self.session.map(|s| s.to_string());
        json!({
            "session": session,
            "result": self.result,
            "usage": {
                "input_tokens": self.tokens[0],
                "output_tokens": self.tokens[1],
                "cache_read_tokens": self.tokens[2],
                "cache_write_tokens": self.tokens[3],
            },
            "cost_usd": self.cost_usd,
            "turns": self.turns,
            "stop": self.stop,
            "denied": self.denied,
            "exit_code": self.exit_code(),
        })
    }

    /// T27.6: folds one `--loop` iteration's outcome into the running
    /// total — the ledger fields sum, the latest text/session/stop win,
    /// matching what `Outcome::summary()` already prints for a single run.
    fn merge(&mut self, other: Outcome) {
        self.session = other.session.or(self.session);
        self.result = other.result;
        for (total, iter) in self.tokens.iter_mut().zip(other.tokens) {
            *total += iter;
        }
        self.cost_usd += other.cost_usd;
        self.turns += other.turns;
        self.denied += other.denied;
        self.stop = other.stop;
        self.failed = other.failed;
    }

    /// Folds one event in; returns a Claude-compatible alias line to print
    /// under `stream-json`, when this event has one.
    fn fold(&mut self, ev: &Event) -> Option<Value> {
        match ev {
            Event::SessionStarted { session, .. } => self.session = Some(*session),
            Event::ItemStarted {
                item,
                kind: ItemKind::AssistantMessage { text },
            } => {
                self.texts.insert(*item, text.clone());
            }
            Event::TextDelta { item, text } => {
                if let Some(buf) = self.texts.get_mut(item) {
                    buf.push_str(text);
                }
            }
            Event::ItemDone { item } => {
                if let Some(text) = self.texts.remove(item) {
                    self.result = text.clone();
                    return Some(json!({
                        "type": "assistant",
                        "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
                        "session_id": self.session.map(|s| s.to_string()),
                    }));
                }
            }
            Event::Usage { usage, .. } => {
                self.turns += 1;
                self.tokens[0] += usage.input_tokens;
                self.tokens[1] += usage.output_tokens;
                self.tokens[2] += usage.cache_read_tokens;
                self.tokens[3] += usage.cache_write_tokens;
                self.cost_usd += usage.cost_usd;
            }
            Event::ApprovalDecided {
                decision: Decision::Deny { .. },
                ..
            } => self.denied += 1,
            Event::TurnDone { stop, .. } => self.stop = Some(stop.clone()),
            Event::Error { fatal: true, .. } => self.failed = true,
            _ => {}
        }
        None
    }
}

/// Runs one prompt to completion and returns the process exit code.
/// Without `-p` this is the resume/continue listing from T2.4.
pub fn run(cli: &Cli, args: &RunArgs, cwd: &Path) -> anyhow::Result<i32> {
    let Some(prompt) = args.prompt.clone() else {
        resume::run(cli, args)?;
        return Ok(EXIT_OK);
    };
    let format = match args.output_format.as_deref().unwrap_or("text") {
        "text" => Format::Text,
        "json" => Format::Json,
        "stream-json" => Format::StreamJson,
        other => anyhow::bail!("unknown --output-format `{other}` (text | json | stream-json)"),
    };
    // T27.6: `--loop`/`--max-iterations` are a pair; the interval reuses
    // the TUI `/loop`'s own grammar (T27.4) instead of a second parser.
    let loop_spec = match (&args.r#loop, args.max_iterations) {
        (Some(interval), Some(max_iterations)) => {
            let interval = cox_tui::commands::parse_interval(interval).ok_or_else(|| {
                anyhow::anyhow!("invalid --loop interval `{interval}` (expected <n>s|m|h)")
            })?;
            Some((interval, max_iterations))
        }
        (None, None) => None,
        _ => anyhow::bail!("--loop and --max-iterations must be given together"),
    };
    // T40.7: every `--image` is read and checked before any request, so a
    // bad one costs nothing.
    let attachments = match read_images(&args.images) {
        Ok(attachments) => attachments,
        Err(e) => {
            eprintln!("cox: {e}");
            return Ok(EXIT_DENIED);
        }
    };
    // Headless defaults to `never`: nobody is there to answer an ask.
    let approve_default = cli.approve.is_none();
    let rt = tokio::runtime::Runtime::new()?;
    let resume_spec = if args.r#continue || args.resume.is_some() {
        let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
        let id = if args.r#continue {
            Store::open(&home)?.latest_session_for_cwd(cwd)?
        } else {
            let raw = args
                .resume
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--resume requires a session id"))?;
            raw.parse()?
        };
        let history = resume::from_home(&home, &id.to_string())?;
        Some((id, history))
    } else {
        None
    };
    let (session, loaded) = rt.block_on(session::open(
        cli,
        cwd,
        args.answer.clone(),
        false,
        |config| {
            if approve_default {
                config.permissions.approval = ApprovalPolicy::Never;
            }
        },
        resume_spec,
        false,
        None,
    ))?;
    // T6.3: with any other policy a driver answers asks on stdin, within
    // `hooks.timeout_s`; `never` never asks, so stdin is left alone.
    let approvals = (loaded.config.permissions.approval != ApprovalPolicy::Never)
        .then(|| Duration::from_secs(u64::from(loaded.config.hooks.timeout_s)));
    // P42, invariant 9: only an explicit flag is think consent here —
    // `--deep`, or `--mode architect` like it. `core.mode = architect` from
    // a config file alone never confirms: that run is refused below.
    let confirm_think = args.deep || cli.mode.as_deref() == Some("architect");
    let unconfirmed_architect = loaded.config.core.mode == Mode::Architect && !confirm_think;
    // The event receiver is taken once and, for `--loop`, shared across
    // every iteration's `drive` call on the same session — that is also
    // what lets the core's own `budget.session_usd` tracking (already
    // cumulative per session) double as the loop's spend cap, instead of
    // a second one (plan.md §6 A31).
    let outcome = rt.block_on(async move {
        let mut rx = session
            .events()
            .ok_or_else(|| anyhow::anyhow!("session events already taken"))?;
        let interrupter = session.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                interrupter.interrupt();
            }
        });
        // T9.1: `--deep` routes the run through think; the flag itself is
        // the confirmation the gate requires. Once per run, `--loop` too.
        if args.deep {
            session
                .submit(Submission::SwitchModel {
                    tier: Tier::Think,
                    model: None,
                })
                .await?;
        }
        let outcome = match loop_spec {
            Some(loop_spec) => {
                run_loop(
                    &session,
                    &mut rx,
                    (prompt, attachments),
                    format,
                    approvals,
                    confirm_think,
                    loop_spec,
                )
                .await
            }
            None => {
                let prompt = (prompt, attachments);
                drive(&session, &mut rx, prompt, format, approvals, confirm_think).await
            }
        };
        // T34.9 follow-up: `drive`/`run_loop` above already awaited
        // `session.wait_idle()`, so every `TaskKind::Agent` chain is done.
        // A `TaskKind::Shell` task (a detached `bash`) is deliberately not
        // waited for there, but leaving it running and only abandoning the
        // OS process (`shutdown_background`, in `run`) leaks it as an
        // orphan once this process exits. `end()` cancels the parent of
        // every turn token (T38.2), so it reaches the token each detached
        // call's `ToolCx::cancel` cloned — including one from an older
        // `--loop` iteration — which trips the shell tool's own
        // SIGTERM-then-SIGKILL logic (`crates/cox-tools/src/bash/mod.rs`)
        // and kills its process group. `wait_tasks_cleared` then waits for
        // that kill to actually land before `shutdown_background` runs.
        session.end();
        session.wait_tasks_cleared(SHELL_CANCEL_GRACE).await;
        outcome
    })?;
    if unconfirmed_architect && matches!(outcome.stop, Some(StopReason::Refusal { .. })) {
        eprintln!(
            "cox: architect mode runs on the think tier; pass --mode architect to confirm it"
        );
    }
    let mut out = std::io::stdout().lock();
    match format {
        Format::Text => writeln!(out, "{}", outcome.result)?,
        Format::Json => writeln!(out, "{}", outcome.summary())?,
        Format::StreamJson => {
            let mut result = outcome.summary();
            result["type"] = json!("result");
            result["is_error"] = json!(outcome.exit_code() != EXIT_OK);
            writeln!(out, "{result}")?;
        }
    }
    // By this point every `TaskKind::Agent` chain is done (`wait_idle`)
    // and any `TaskKind::Shell` task this same session could still reach
    // has already been cancelled and killed (`end` +
    // `wait_tasks_cleared`, above) — so nothing meaningful is left
    // outstanding. `rt`'s own `Drop` does not know that: left to run
    // normally, it shuts down by "waiting until all [spawned] tasks have
    // completed" (`tokio::runtime::Runtime`'s own docs), which would hang
    // this process on a leftover `wait_tasks_cleared`'s own deadline gave
    // up on (a shell that did not die in time). `shutdown_background`
    // returns immediately instead, abandoning only that rare case rather
    // than every ordinary exit.
    rt.shutdown_background();
    Ok(outcome.exit_code())
}

/// Reads each `--image` path and checks it with `image::attachment`. The
/// path is the user's own argument, and the user is the trust root, so it
/// is not confined like a path from the model. A file over the cap is
/// refused from its size, before it is read.
fn read_images(paths: &[PathBuf]) -> Result<Vec<Attachment>, String> {
    paths
        .iter()
        .map(|path| {
            let why = |e: &dyn std::fmt::Display| format!("--image {}: {e}", path.display());
            let len = std::fs::metadata(path).map_err(|e| why(&e))?.len();
            let bytes = usize::try_from(len).unwrap_or(usize::MAX);
            if bytes > MAX_IMAGE_BYTES {
                let cap = MAX_IMAGE_BYTES;
                return Err(why(&ImageError::TooLarge { bytes, cap }));
            }
            let data = std::fs::read(path).map_err(|e| why(&e))?;
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            image::attachment(name, &data).map_err(|e| why(&e))
        })
        .collect()
}

/// Runs one turn to completion on an already-open `session`, reading its
/// events off the caller's `rx` (taken once — see `run`'s comment — so
/// `--loop` can call this repeatedly on the same receiver). `prompt` is the
/// text and its attachments (T40.7).
async fn drive(
    session: &Session,
    rx: &mut mpsc::Receiver<Event>,
    prompt: (String, Vec<Attachment>),
    format: Format,
    approvals: Option<Duration>,
    confirm_think: bool,
) -> anyhow::Result<Outcome> {
    // The core runs the turn inside `submit`, so it must live on its own
    // task or nothing could answer an `ApprovalRequired` mid-turn.
    let (text, attachments) = prompt;
    let turn = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .submit(Submission::UserTurn {
                    text,
                    attachments,
                    confirm_think,
                })
                .await
        }
    });
    let mut outcome = Outcome::default();
    let mut out = std::io::stdout().lock();
    let mut driver = approvals.map(|_| spawn_stdin());
    let mut pending: Vec<(CallId, Instant)> = Vec::new();
    // T34.9: the top-level turn ending is not the whole run ending — a
    // background subagent (or a detached `bash`) this turn started can
    // still be mid-relay. `ended` remembers that the terminal event
    // already arrived; the loop keeps draining `rx` exactly as before
    // (still printing stream-json, still answering approvals for a
    // relayed child call) until `session.wait_idle()` also agrees nothing
    // is left running, so a still-running chain's own `task_message`/
    // `notice`/`task_completed` events are never dropped. `rx.recv` stays
    // the first (biased) branch so a message already sitting in the
    // channel is always drained before this decides the run is idle.
    let mut ended = false;
    loop {
        let deadline = pending.iter().map(|(_, at)| *at).min();
        let ev = tokio::select! {
            biased;
            ev = rx.recv() => match ev {
                Some(ev) => ev,
                None => break,
            },
            line = recv_or_pend(&mut driver) => {
                match line {
                    Some((call_id, decision)) => {
                        pending.retain(|(id, _)| *id != call_id);
                        session.submit(Submission::Approve { call_id, decision }).await?;
                    }
                    // stdin closed: whatever is still pending times out.
                    None => driver = None,
                }
                continue;
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                let now = Instant::now();
                let (expired, kept): (Vec<_>, Vec<_>) = pending.drain(..).partition(|(_, at)| *at <= now);
                pending = kept;
                for (call_id, _) in expired {
                    let reason = format!("no decision from the driver within {}s", approvals.unwrap_or_default().as_secs());
                    session.submit(Submission::Approve { call_id, decision: Decision::Deny { reason } }).await?;
                }
                continue;
            }
            () = session.wait_idle(), if ended => break,
        };
        // T28.4: this surface prints, so every event passes through the one
        // redaction helper first; folding the scrubbed copy keeps `result`,
        // the alias and `text` output scrubbed without a second pass.
        let ev = cox_core::redact::scrub_event(&ev).into_owned();
        if format == Format::StreamJson {
            writeln!(out, "{}", serde_json::to_string(&ev)?)?;
        }
        if let Event::ApprovalRequired { call, .. } = &ev {
            match approvals {
                Some(timeout) => pending.push((call.id, Instant::now() + timeout)),
                None => {
                    session
                        .submit(Submission::Approve {
                            call_id: call.id,
                            decision: Decision::Deny {
                                reason: "no approver in headless mode".into(),
                            },
                        })
                        .await?;
                }
            }
        }
        let alias = outcome.fold(&ev);
        if let (Format::StreamJson, Some(alias)) = (format, alias) {
            writeln!(out, "{alias}")?;
        }
        if matches!(
            ev,
            Event::TurnDone { .. } | Event::Error { fatal: true, .. }
        ) {
            ended = true;
        }
    }
    if let Ok(Err(e)) = turn.await {
        return Err(e.into());
    }
    Ok(outcome)
}

/// `cox run --loop <interval> -p "…" --max-iterations N` (T27.6, the
/// headless counterpart of the TUI's `/loop`, T27.4): repeats `drive` on
/// the same session and folds each iteration's `Outcome` into a running
/// total. Stops after `max_iterations` turns, the moment a turn's own
/// stop reason is `StopReason::Budget` (the core already tracks
/// `budget.session_usd` cumulatively per session, so this needs no
/// second cap) or fails fatally, or when `Ctrl+C` fires during the wait
/// between iterations — that exit is clean, not an error. The attachments
/// go with the first turn only (T40.7); later turns already have them in
/// history.
async fn run_loop(
    session: &Session,
    rx: &mut mpsc::Receiver<Event>,
    prompt: (String, Vec<Attachment>),
    format: Format,
    approvals: Option<Duration>,
    confirm_think: bool,
    // `(interval, max_iterations)`, bundled so the function stays under
    // clippy's 7-argument limit.
    loop_spec: (Duration, u32),
) -> anyhow::Result<Outcome> {
    let (interval, max_iterations) = loop_spec;
    let (text, mut attachments) = prompt;
    let mut total = Outcome::default();
    for i in 0..max_iterations {
        let prompt = (text.clone(), std::mem::take(&mut attachments));
        let iteration = drive(session, rx, prompt, format, approvals, confirm_think).await?;
        total.merge(iteration);
        if total.failed || matches!(total.stop, Some(StopReason::Budget)) {
            break;
        }
        let last = i + 1 == max_iterations;
        if !last {
            let interrupted = tokio::select! {
                _ = tokio::time::sleep(interval) => false,
                _ = tokio::signal::ctrl_c() => true,
            };
            if interrupted {
                break;
            }
        }
    }
    Ok(total)
}

/// One driver line: `{"approve":"<call_id>"}` or `{"deny":"<call_id>","reason":"…"}`.
#[derive(serde::Deserialize)]
struct DriverLine {
    approve: Option<String>,
    deny: Option<String>,
    reason: Option<String>,
}

fn decision_line(line: &str) -> Option<(CallId, Decision)> {
    let d: DriverLine = serde_json::from_str(line).ok()?;
    if let Some(id) = d.approve {
        return Some((id.parse().ok()?, Decision::Allow));
    }
    let id: CallId = d.deny?.parse().ok()?;
    let reason = d.reason.unwrap_or_else(|| "denied by driver".into());
    Some((id, Decision::Deny { reason }))
}

/// stdin is blocking; a thread turns its lines into decisions. Unparseable
/// lines are ignored so a stray log line cannot approve anything.
fn spawn_stdin() -> mpsc::Receiver<(CallId, Decision)> {
    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Some(decision) = decision_line(&line)
                && tx.blocking_send(decision).is_err()
            {
                break;
            }
        }
    });
    rx
}

/// Pends forever without a driver so `select!` never spins on a closed side.
async fn recv_or_pend<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use cox_core::MemoryStore;
    use cox_provider::scripted::Scripted;

    use super::*;

    /// A session over a scripted scenario with `turns` scripted replies
    /// queued up, no tools, no config overrides — enough for `run_loop`
    /// to drive several turns without touching the network.
    fn open(turns: usize) -> Session {
        let scenario = "[[turn]]\ntext = \"ok\"\n".repeat(turns);
        let store = Arc::new(MemoryStore::new());
        let provider = Arc::new(Scripted::from_toml(&scenario, "").expect("scenario"));
        Session::new(
            cox_protocol::Config::default(),
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-run-loop"),
        )
        .expect("session")
    }

    /// The claim `run_loop`'s card Check names: it stops on its own once
    /// `max_iterations` turns have run, even though the scenario has more
    /// scripted replies queued than that — a tiny interval (never really
    /// slept out) keeps the test instant.
    #[tokio::test]
    async fn run_loop_stops_after_max_iterations() {
        let session = open(3);
        let mut rx = session.events().expect("events");
        let outcome = run_loop(
            &session,
            &mut rx,
            ("hi".into(), vec![]),
            Format::Text,
            None,
            false,
            (Duration::from_millis(1), 2),
        )
        .await
        .expect("run_loop");
        assert_eq!(outcome.turns, 2);
        assert_eq!(outcome.exit_code(), EXIT_OK);
    }

    /// `Outcome::merge` is what turns per-iteration ledgers into the one
    /// running total `--loop` finally prints.
    #[test]
    fn merge_sums_the_ledger_and_keeps_the_latest_text() {
        let mut total = Outcome {
            result: "first".into(),
            tokens: [1, 2, 3, 4],
            cost_usd: 0.5,
            turns: 1,
            ..Outcome::default()
        };
        total.merge(Outcome {
            result: "second".into(),
            tokens: [1, 1, 1, 1],
            cost_usd: 0.25,
            turns: 1,
            ..Outcome::default()
        });
        assert_eq!(total.result, "second");
        assert_eq!(total.tokens, [2, 3, 4, 5]);
        assert_eq!(total.cost_usd, 0.75);
        assert_eq!(total.turns, 2);
    }
}
