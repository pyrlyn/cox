// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T66.24: the point of `cox serve` end to end (`docs/design/serve.md` S9,
//! D6). The real `cox serve` runs a session in a worker with the scripted
//! provider; a client that bridges through `cox attach --stdio` runs a turn
//! and is killed; a second client attaches and finds the same timeline, and
//! the rollout holds the turn's events once: a reattach writes nothing.

#![cfg(all(unix, feature = "app-server"))]

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use cox_app::coalesce::apply;
use cox_app::{Block, BlockKind, Intent, TimelinePatch};
use cox_protocol::Store as _;
use cox_protocol::ids::SessionId;
use cox_store::Store;
use serde_json::{Value, json};

/// One plain reply.
const REPLY: &str = r#"
[[turn]]
text = "Hello from the resident session."
"#;

/// The machine is often loaded while the whole workspace is tested, so every
/// wait has a deadline far past the normal case; a pass never waits for it.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often a poll looks again.
const POLL: Duration = Duration::from_millis(25);

/// Everything a test creates, under a short path: the socket is
/// `home/run/serve.sock` and `sun_path` holds 104 bytes on macOS, which the
/// default temporary directory there already exceeds.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cox-e2e-")
            .tempdir_in("/tmp")
            .expect("scratch under /tmp");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("home")).expect("home");
        std::fs::create_dir_all(root.join("work/.git")).expect("work");
        std::fs::write(root.join("scenario.toml"), REPLY).expect("scenario");
        Self { _dir: dir, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn socket(&self) -> PathBuf {
        self.home().join("run/serve.sock")
    }

    fn log(&self) -> PathBuf {
        self.root.join("serve.log")
    }

    /// A `cox` command with the scratch home and the scripted provider. The
    /// scenario is for the worker, which inherits this environment from the
    /// supervisor. Cargo sets `COX_KEYRING=off`, and `HOME` is the scratch
    /// too, so nothing reaches a real keychain or `~/.cox`.
    fn cox(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(self.work())
            .env("COX_HOME", self.home())
            .env("HOME", self.home())
            .env("COX_PROVIDER", "scripted")
            .env("COX_SCENARIO", self.root.join("scenario.toml"))
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY");
        cmd
    }
}

fn signal(name: &str, target: &str) {
    // Through kill(1) rather than a new dependency for one signal.
    let _ = Command::new("/bin/kill")
        .args([name, "--", target])
        .stderr(Stdio::null())
        .status();
}

/// `cox serve` in its own process group, so that its workers (which share the
/// group) die with it however the test ends.
struct Serve {
    child: Option<Child>,
    pid: u32,
    log: PathBuf,
}

impl Serve {
    fn start(scratch: &Scratch) -> Self {
        let log = std::fs::File::create(scratch.log()).expect("log");
        let child = scratch
            .cox()
            .arg("serve")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("start cox serve");
        let pid = child.id();
        let mut serve = Self {
            child: Some(child),
            pid,
            log: scratch.log(),
        };
        serve.wait_for_socket(&scratch.socket());
        serve
    }

    /// The socket accepts a connection: the file appears at `bind`, a moment
    /// before `listen`, and the bridge fails when it connects too early.
    fn wait_for_socket(&mut self, socket: &Path) {
        let until = Instant::now() + DEADLINE;
        while UnixStream::connect(socket).is_err() {
            let exited = self
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten());
            assert!(exited.is_none(), "cox serve exited early: {exited:?}");
            assert!(Instant::now() < until, "no socket at {}", socket.display());
            thread::sleep(POLL);
        }
    }

    /// SIGTERM, then the exit status, within the deadline.
    fn terminate(&mut self) -> std::process::ExitStatus {
        signal("-TERM", &self.pid.to_string());
        let mut child = self.child.take().expect("still running");
        let until = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = child.try_wait().expect("wait") {
                return status;
            }
            if Instant::now() >= until {
                // Put back for the drop guard to kill the group.
                self.child = Some(child);
                panic!("cox serve did not exit on SIGTERM");
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if thread::panicking()
            && let Ok(log) = std::fs::read_to_string(&self.log)
        {
            eprintln!("--- cox serve log ---\n{log}");
        }
        // Only while it has not been reaped: a pid is not reused as long as
        // the group is alive, but nothing guards a group that is gone.
        if let Some(mut child) = self.child.take() {
            signal("-KILL", &format!("-{}", self.pid));
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A `cox attach --stdio` bridge and the timeline its patches fold into.
struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    blocks: Vec<Block>,
    turn_ended: bool,
    next_id: u64,
}

impl Client {
    fn start(scratch: &Scratch) -> Self {
        let mut child = scratch
            .cox()
            .args(["attach", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start cox attach");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, lines) = mpsc::channel();
        // Ends with the pipe, so a killed bridge leaves no thread behind.
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            blocks: Vec::new(),
            turn_ended: false,
            next_id: 0,
        }
    }

    fn next_line(&mut self) -> Value {
        match self.lines.recv_timeout(DEADLINE) {
            Ok(line) => serde_json::from_str(&line).expect("a protocol line"),
            Err(RecvTimeoutError::Timeout) => panic!("no line within the deadline"),
            Err(RecvTimeoutError::Disconnected) => panic!("the bridge closed its output"),
        }
    }

    /// Folds a `patches` notification the way a UI would; any other line
    /// changes nothing.
    fn fold(&mut self, line: &Value) {
        if line["type"] != "notification" || line["event"]["event"] != "patches" {
            return;
        }
        let patches = line["event"]["data"]["patches"].clone();
        let patches: Vec<TimelinePatch> = serde_json::from_value(patches).expect("patches");
        for patch in patches {
            self.turn_ended |= ends_turn(&patch);
            apply(&mut self.blocks, patch);
        }
    }

    /// Sends one call and returns the reply value of its response, folding
    /// the notifications that arrive first.
    fn call(&mut self, call: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"type": "request", "v": 1, "id": id, "call": call});
        writeln!(self.stdin, "{line}").expect("write the request");
        self.stdin.flush().expect("flush");
        loop {
            let line = self.next_line();
            if line["type"] != "response" {
                self.fold(&line);
                continue;
            }
            assert_eq!(line["id"], id, "{line}");
            let outcome = &line["outcome"];
            assert_eq!(outcome["status"], "ok", "{line}");
            return outcome["value"]["value"].clone();
        }
    }

    /// `open` or `attach`: the session's id, and the blocks it starts from.
    fn opened(&mut self, call: Value) -> SessionId {
        let value = self.call(call);
        self.blocks = serde_json::from_value(value["blocks"].clone()).expect("blocks");
        value["session"]
            .as_str()
            .expect("session")
            .parse()
            .expect("id")
    }

    fn open(&mut self, cwd: &Path) -> SessionId {
        let params = json!({"cwd": cwd, "resume": null, "theme": "base16-ocean.dark"});
        self.opened(json!({"method": "open", "params": params}))
    }

    fn attach(&mut self, session: SessionId) -> SessionId {
        let params = json!({"session": session});
        self.opened(json!({"method": "attach", "params": params}))
    }

    fn send(&mut self, session: SessionId, text: &str) {
        let intent = Intent::Send {
            text: text.into(),
            attachments: vec![],
            confirm_think: false,
        };
        let intent = serde_json::to_value(intent).expect("intent");
        let params = json!({"session": session, "intent": intent});
        self.turn_ended = false;
        self.call(json!({"method": "send", "params": params}));
    }

    fn until_turn_ends(&mut self) {
        while !self.turn_ended {
            let line = self.next_line();
            self.fold(&line);
        }
    }

    /// SIGKILL, as a crashed or disconnected terminal would end.
    fn kill(mut self) {
        self.child.kill().expect("kill the bridge");
        self.child.wait().expect("reap the bridge");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ends_turn(patch: &TimelinePatch) -> bool {
    matches!(patch, TimelinePatch::Upsert { block, .. }
        if matches!(block.kind, BlockKind::TurnMeta { stop: Some(_), .. }))
}

/// The rollout's complete lines. A line being written is skipped: it is not
/// yet an event.
fn rollout(store: &Store, session: &SessionId) -> Vec<Value> {
    let text = std::fs::read_to_string(store.rollout_path(session)).unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    for line in text.lines() {
        match serde_json::from_str(line) {
            Ok(row) => rows.push(row),
            Err(_) => break,
        }
    }
    rows
}

fn event_type(row: &Value) -> &str {
    row["event"]["type"].as_str().unwrap_or_default()
}

/// The turn's `TurnDone` is durable: the patches that end the turn can reach
/// the client before the core's last line is written.
fn wait_for_turn_done(store: &Store, session: &SessionId) -> Vec<Value> {
    let until = Instant::now() + DEADLINE;
    loop {
        let rows = rollout(store, session);
        if rows.iter().any(|r| event_type(r) == "turn_done") {
            return rows;
        }
        assert!(Instant::now() < until, "no turn_done in the rollout");
        thread::sleep(POLL);
    }
}

fn holder_pid(store: &Store, session: &SessionId) -> Option<u32> {
    store
        .session_holder(session)
        .expect("probe the lock")
        .map(|holder| holder.pid)
}

#[test]
fn a_reattach_after_a_killed_client_writes_nothing_to_the_rollout() {
    let scratch = Scratch::new();
    let mut serve = Serve::start(&scratch);
    let store = Store::open(&scratch.home()).expect("store");

    // Client A opens a session and runs one turn.
    let mut a = Client::start(&scratch);
    let session = a.open(&scratch.work());
    let before = rollout(&store, &session).len();
    a.send(session, "hello");
    a.until_turn_ends();
    let rows = wait_for_turn_done(&store, &session);
    let after = rows.len();
    assert!(
        after > before,
        "the turn wrote nothing: {before} -> {after}"
    );
    let bytes = std::fs::read(store.rollout_path(&session)).expect("rollout");
    let folded = a.blocks.clone();
    assert!(
        folded
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Assistant { .. })),
        "the reply never reached the timeline: {folded:?}"
    );

    // The client dies without a word; the worker, not the bridge, owns the
    // session.
    a.kill();
    let worker = holder_pid(&store, &session).expect("the worker still holds the lock");
    assert_ne!(worker, serve.pid, "the supervisor must not hold a session");

    // Client B finds the session as the turn left it.
    let mut b = Client::start(&scratch);
    assert_eq!(b.attach(session), session);
    assert_eq!(b.blocks, folded, "the attach did not replay A's timeline");
    // A call that goes through the worker after the attach: whatever the
    // reattach was going to write, it has by now.
    let snapshot = b.call(json!({"method": "snapshot", "params": {"session": session}}));
    let snapshot: Vec<Block> = serde_json::from_value(snapshot).expect("snapshot");
    assert_eq!(snapshot, folded);
    assert_eq!(holder_pid(&store, &session), Some(worker));

    let rows = rollout(&store, &session);
    assert_eq!(rows.len(), after, "the reattach wrote to the rollout");
    assert_eq!(
        std::fs::read(store.rollout_path(&session)).expect("rollout"),
        bytes,
        "the rollout's bytes changed"
    );
    let seqs: Vec<u64> = rows.iter().filter_map(|r| r["seq"].as_u64()).collect();
    let expected: Vec<u64> = (1..=after as u64).collect();
    assert_eq!(seqs, expected, "seq repeats or skips");
    let turn: Vec<&str> = rows[before..].iter().map(event_type).collect();
    assert_eq!(turn.first(), Some(&"turn_started"), "{turn:?}");
    assert_eq!(turn.last(), Some(&"turn_done"), "{turn:?}");
    for kind in ["turn_started", "turn_done"] {
        let count = rows.iter().filter(|r| event_type(r) == kind).count();
        assert_eq!(count, 1, "{kind} appears {count} times");
    }

    // Stopping the supervisor ends every session and removes its socket.
    drop(b);
    let status = serve.terminate();
    assert!(status.success(), "cox serve exited with {status}");
    assert_eq!(
        holder_pid(&store, &session),
        None,
        "the lock outlived serve"
    );
    assert!(!scratch.socket().exists(), "the socket outlived serve");
}
