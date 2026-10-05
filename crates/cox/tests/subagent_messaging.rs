// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T34.9 e2e: two subagents messaging through the parent, driven headless
//! (`cox run -p --output-format stream-json`) against a real `COX_HOME`
//! scratch tree and the `Scripted` provider — no network, no API key
//! (D12). The card names `tests/subagent_messaging.rs`, but this repo has
//! no root-level `tests/` directory; `crates/cox/tests/run_cli.rs` is
//! where this repo's other real-binary `cox run` e2e tests live, so this
//! file joins them there instead.
//!
//! A background subagent chain keeps the binary's tokio runtime alive
//! until every child finishes (`Runtime::drop` waits for spawned tasks),
//! so a scenario or hop-limit bug would hang the test forever rather than
//! fail fast. `run_scripted` below reads stdout on its own thread and
//! enforces a wall-clock timeout itself instead of trusting `assert_cmd`
//! (which has none) or the child process to exit.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

const TWO_CHILDREN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/subagent_messaging_two_children.toml"
);
const PING_PONG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/subagent_messaging_ping_pong.toml"
);
const BACKGROUND_WAIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/subagent_background_wait.toml"
);
const BACKGROUND_SHELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/subagent_background_shell.toml"
);

/// `.cox/agents/talker.md` (T34.1 format, discovered from the work tree
/// like `run_cli.rs`'s `ext_lists_commands_and_agents_from_the_project_tree`):
/// a minimal custom preset whose only tool is `send_message`.
fn write_talker_agent(work: &Path) {
    std::fs::create_dir_all(work.join(".cox/agents")).expect("mkdir .cox/agents");
    std::fs::write(
        work.join(".cox/agents/talker.md"),
        "---\nname: talker\ndescription: sends a message for a test\ntools: send_message\n---\nSend the requested message.\n",
    )
    .expect("write talker.md");
}

/// Runs the real binary against a scripted scenario and returns its exit
/// code and full stdout, or panics if it outruns `timeout` — the guard
/// against a hung background chain the card asks for. Reads stdout on a
/// separate thread so a full pipe buffer can never make the child block
/// while this side waits. `extra_args` go after `--output-format
/// stream-json`; every existing scenario passes `&[]` and gets today's
/// default headless permission mode, so `--permission-mode bypass` stays
/// opt-in for the one test that needs a `bash` call to actually run.
fn run_scripted(
    work: &Path,
    home: &Path,
    scenario: &str,
    timeout: Duration,
    extra_args: &[&str],
) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cox"))
        .current_dir(work)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", scenario)
        .args([
            "--cwd",
            work.to_str().expect("utf8 path"),
            "run",
            "-p",
            "go",
            "--output-format",
            "stream-json",
        ])
        .args(extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cox");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let out = match rx.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            panic!("cox did not finish within {timeout:?} (scenario: {scenario})");
        }
    };
    let status = child.wait().expect("wait for exit");
    (status.code().unwrap_or(-1), out)
}

fn events(out: &str) -> Vec<Value> {
    out.lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad json line {line:?}: {e}"))
        })
        .collect()
}

/// The `task` id of the `task_created` event whose `label` contains
/// `label_part` (a substring of that subagent's own `label`, built from
/// the *first* line of its `task` text — the scenario's `when_contains`
/// marker deliberately lives on a second line instead, see the scenario
/// file's own comment, so this can't just reuse it) — a way to identify
/// which spawned task is which without depending on dispatch timing.
fn task_named(events: &[Value], label_part: &str) -> String {
    events
        .iter()
        .find(|e| {
            e["type"] == "task_created"
                && e["label"].as_str().is_some_and(|l| l.contains(label_part))
        })
        .unwrap_or_else(|| panic!("no task_created labeled with {label_part:?}: {events:#?}"))
        ["task"]
        .as_str()
        .expect("task id")
        .to_string()
}

fn task_messages(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| e["type"] == "task_message")
        .collect()
}

/// The parent's own persisted rollout (`cox-store`'s one JSONL per
/// session, `$COX_HOME/sessions/<id>.jsonl`) — reading it back proves a
/// `task_message` is not just a live console notification but part of the
/// parent's own recorded history, the "history pointer line" the card's
/// Check asks for. Each rollout line wraps the bare `Event` (stdout's own
/// shape) in `{"seq", "ts", "event": ...}`, so this unwraps it before
/// returning.
fn rollout_events(home: &Path, session_id: &str) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session_id}.jsonl"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read rollout {}: {e}", path.display()));
    events(&text)
        .into_iter()
        .map(|line| line["event"].clone())
        .collect()
}

#[test]
fn parent_relays_a_message_between_two_children() {
    let work = tempfile::tempdir().expect("work tempdir");
    let home = tempfile::tempdir().expect("home tempdir");
    write_talker_agent(work.path());
    let (code, out) = run_scripted(
        work.path(),
        home.path(),
        TWO_CHILDREN,
        Duration::from_secs(30),
        &[],
    );
    assert_eq!(code, 0, "{out}");
    let events = events(&out);

    // Registry names come from spawn order, not marker order: the
    // scenario dispatches TALKB (the message's recipient) first so its
    // name is registered before TALKA's background task can possibly
    // race ahead and address it (see the scenario file's own comment).
    // `talka`/`talkb` below are `TaskId`s looked up by marker, so the
    // assertions read by role regardless of which registry name each got.
    let talka = task_named(&events, "send talker-1 a hello");
    let talkb = task_named(&events, "reply to the parent");

    // TALKA -> TALKB, relayed through the parent: a fresh message, hop 1.
    let messages = task_messages(&events);
    assert!(
        messages.iter().any(|m| m["task"] == talkb
            && m["from"] == talka
            && m["hop"] == 1
            && m["text"] == "hi from talker-2"),
        "no relayed TALKA -> TALKB message in {messages:#?}"
    );

    // TALKB -> parent: SM§3's "to its own task id means the parent"
    // convention, so `task` and `from` are both TALKB's own id.
    assert!(
        messages.iter().any(|m| m["task"] == talkb
            && m["from"] == talkb
            && m["hop"] == 1
            && m["text"] == "hi from talker-1"),
        "no TALKB -> parent message in {messages:#?}"
    );

    // The parent's own history pointer line: the same to-parent message,
    // persisted to the parent's rollout, not just streamed live.
    let session_id = events[0]["session"].as_str().expect("session id");
    let rollout = rollout_events(home.path(), session_id);
    assert!(
        rollout_has(&rollout, &talkb, &talkb, 1, "hi from talker-1"),
        "the to-parent message never reached the parent's own rollout"
    );
}

fn rollout_has(rollout: &[Value], task: &str, from: &str, hop: u64, text: &str) -> bool {
    rollout.iter().any(|e| {
        e["type"] == "task_message"
            && e["task"] == task
            && e["from"] == from
            && e["hop"] == hop
            && e["text"] == text
    })
}

#[test]
fn hop_limit_stops_a_scripted_ping_pong() {
    let work = tempfile::tempdir().expect("work tempdir");
    let home = tempfile::tempdir().expect("home tempdir");
    write_talker_agent(work.path());
    let (code, out) = run_scripted(
        work.path(),
        home.path(),
        PING_PONG,
        Duration::from_secs(30),
        &[],
    );
    assert_eq!(code, 0, "{out}");
    let events = events(&out);

    let talker_a = task_named(&events, "send talker-1 a ping");
    let talker_b = task_named(&events, "wait for a ping and reply");
    let messages = task_messages(&events);

    // Four relays get through, one per hop, alternating direction.
    for (hop, (from, to, text)) in [
        (1u64, (&talker_a, &talker_b, "ping1")),
        (2, (&talker_b, &talker_a, "pong1")),
        (3, (&talker_a, &talker_b, "ping2")),
        (4, (&talker_b, &talker_a, "pong2")),
    ] {
        assert!(
            messages.iter().any(|m| m["task"] == *to
                && m["from"] == *from
                && m["hop"] == hop
                && m["text"] == text),
            "missing hop {hop} ({text}) in {messages:#?}"
        );
    }

    // The fifth hop (A's third ping) never arrives: no hop-5 message at
    // all, from either side.
    assert!(
        !messages.iter().any(|m| m["hop"] == 5),
        "a hop-5 message was delivered despite the MAX_HOPS cap: {messages:#?}"
    );

    // The parent dropped it instead, with a warning that names the cap.
    let dropped = events.iter().any(|e| {
        e["type"] == "notice"
            && e["level"] == "warn"
            && e["text"].as_str().is_some_and(|t| t.contains("hop limit"))
    });
    assert!(dropped, "no hop-limit drop notice in {events:#?}");

    // The run still terminates cleanly (already implied by `code == 0`
    // above without hitting `run_scripted`'s timeout, but the final event
    // shape is worth pinning too).
    assert_eq!(events.last().unwrap()["type"], "result");
}

/// T34.9 regression, direct and minimal: the two relay tests above prove
/// `Session::wait_idle` through a marker-dependent multi-hop chain, but a
/// single background child with exactly one round is enough on its own —
/// without the fix this reliably failed, since `cox run -p` exited the
/// instant the parent's own "done" turn concluded, before the runtime
/// ever got around to running the child's still-unscheduled `drive` task
/// on its own tokio task.
#[test]
fn headless_run_waits_for_background_subagents() {
    let work = tempfile::tempdir().expect("work tempdir");
    let home = tempfile::tempdir().expect("home tempdir");
    write_talker_agent(work.path());
    let (code, out) = run_scripted(
        work.path(),
        home.path(),
        BACKGROUND_WAIT,
        Duration::from_secs(30),
        &[],
    );
    assert_eq!(code, 0, "{out}");
    let events = events(&out);
    let child = task_named(&events, "say hello");
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "task_completed" && e["task"] == child),
        "the background child's task_completed never reached stream-json before exit: {events:#?}"
    );
}

/// T34.9 follow-up: `wait_idle` (`crates/cox-core/src/tasks.rs`) must stay
/// blind to `TaskKind::Shell` — only a backgrounded `agent` should hold the
/// headless exit open. A detached `bash` (a dev server, a long `sleep`) is
/// meant to outlive the *run*; if `wait_idle` ever started counting it too,
/// the run would last as long as the shell's 4001-second `sleep`. But
/// outliving the run is not the same as outliving the *process*: `run()`
/// (`crates/cox/src/run.rs`) ends the session and kills any still-running
/// shell task before it exits (`Session::end` + `wait_tasks_cleared`), so
/// this also checks that the real OS process is gone afterward, not merely
/// orphaned (ppid 1) — the actual bug in an earlier version of this fix.
///
/// T50.6: both checks are built to hold under full-workspace load. The
/// bound is `DID_NOT_WAIT`, a fraction of the shell's own duration, not a
/// guess at how fast the run is: idle it takes about 1.5 s, but under the
/// parallel agents' builds process start-up and the `git` spawns of the
/// workspace checkpoint around the call took it past 10 s and even 30 s,
/// none of which is waiting on the shell. The leak check looks for this
/// run's own command line (`sleep 4001.<test pid>`, written into a copy of
/// the scenario), since a machine-wide `sleep 4001` also matched the same
/// test running at that moment in another worktree, and it polls until a
/// deadline because the killed `sleep` is reaped asynchronously.
/// `--permission-mode bypass` is what actually lets the scripted `bash`
/// call run rather than being denied by the default headless mode (as in
/// `tui_e2e.rs`'s `tui_ctrl_b_backgrounds_sleep_and_composer_accepts_input`);
/// a denied call would prove nothing, so the `task_created` event for the
/// shell task is asserted too.
#[test]
#[cfg(unix)]
fn headless_run_does_not_wait_for_a_background_shell() {
    /// Far above any load-induced slowness seen, far below the 4001 s the
    /// run would take if it waited on the shell.
    const DID_NOT_WAIT: Duration = Duration::from_secs(300);
    let work = tempfile::tempdir().expect("work tempdir");
    let home = tempfile::tempdir().expect("home tempdir");
    let command = format!("sleep 4001.{}", std::process::id());
    let reap = KillOnDrop(command.replace('.', "\\."));
    let scenario = home.path().join("scenario.toml");
    let template = std::fs::read_to_string(BACKGROUND_SHELL).expect("read scenario");
    assert!(
        template.contains("\"sleep 4001\""),
        "scenario lost its command"
    );
    std::fs::write(&scenario, template.replace("sleep 4001", &command)).expect("write scenario");
    let (code, out) = run_scripted(
        work.path(),
        home.path(),
        scenario.to_str().expect("utf8 path"),
        DID_NOT_WAIT,
        &["--permission-mode", "bypass"],
    );
    assert_eq!(code, 0, "{out}");
    let events = events(&out);
    assert!(
        events.iter().any(|e| e["type"] == "task_created"
            && e["label"].as_str().is_some_and(|l| l.contains(&command))),
        "the detached shell's task_created never reached stream-json: {events:#?}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while running(&reap.0) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !running(&reap.0),
        "`{command}` outlived the headless run instead of being killed"
    );
}

/// Whether a process whose command line matches `pattern` is alive:
/// `pgrep -f` exits 0 if it finds a match, 1 if not.
#[cfg(unix)]
fn running(pattern: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", pattern])
        .status()
        .expect("run pgrep")
        .success()
}

/// Kills whatever still matches the pattern when the test ends, pass or
/// panic, so a failed run never leaves a 4001-second `sleep` behind.
#[cfg(unix)]
struct KillOnDrop(String);

#[cfg(unix)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-f", &self.0]).status();
    }
}
