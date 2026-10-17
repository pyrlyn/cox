// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T68.9: a scripted turn through a fake `ssh`, `docker`, `limactl` and
//! `colima`. The command is the one `cox_target::command` builds for the
//! kind; the fake launcher refuses any other argv prefix, drops it and runs
//! the built `cox app-server --stdio`, and this file speaks the public
//! `cox_app::wire` to it, so the argv, the scrubbed environment, the wire and
//! the session are the shipped ones, only the machine or container is
//! missing. The `#[ignore]` tests at the end do the same against a real
//! Colima, Lima or Docker: run them by hand with `--run-ignored only` and
//! the `COX_TEST_*` variables set.

#![cfg(all(unix, feature = "app-server"))]

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use cox_app::wire::{self, Call, Line, Notification, Outcome, Reply, Request, ServerEvent};
use cox_app::{BlockKind, Intent, TimelinePatch};
use cox_protocol::types::StopReason;
use cox_target::{Kind, Target, Tty};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

// why: `remote.rs` shares this file; not every helper is used by both.
#[allow(dead_code)]
#[path = "support/fake_launcher.rs"]
mod fake_launcher;

/// One plain reply.
const REPLY: &str = r#"
[[turn]]
text = "Hello from the target."
"#;

/// Long enough for a cold `cox` start on a loaded runner, short enough that
/// a hung launcher fails the test instead of the job.
const WAIT: Duration = Duration::from_secs(60);

fn ends_turn(patch: &TimelinePatch) -> bool {
    matches!(patch, TimelinePatch::Upsert { block, .. }
        if matches!(block.kind, BlockKind::TurnMeta { stop: Some(StopReason::EndTurn), .. }))
}

/// A client of one `cox app-server --stdio`, started the way the app starts
/// it: through the launcher, with its stdio piped.
struct Client {
    child: Child,
    stdin: ChildStdin,
    out: Lines<BufReader<ChildStdout>>,
    err: ChildStderr,
    next: u64,
    turn_ended: bool,
}

impl Client {
    fn start(target: &Target, launcher: &Path) -> Self {
        let command =
            cox_target::command(target, launcher, &["cox", "app-server", "--stdio"], Tty::No)
                .expect("a valid target");
        let mut child = Command::from(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start the launcher");
        let stdin = child.stdin.take().expect("stdin");
        let out = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let err = child.stderr.take().expect("stderr");
        Self {
            child,
            stdin,
            out,
            err,
            next: 0,
            turn_ended: false,
        }
    }

    /// The next protocol line; a closed pipe fails with what the launcher
    /// said, which is how a refused argv shows up.
    async fn line(&mut self) -> Line {
        let read = tokio::time::timeout(WAIT, self.out.next_line())
            .await
            .expect("the server sent nothing in time")
            .expect("read the server's stdout");
        let Some(text) = read else {
            let status = self.child.wait().await.expect("wait");
            let mut said = String::new();
            self.err.read_to_string(&mut said).await.expect("stderr");
            panic!("the server closed its output ({status}): {said}");
        };
        wire::decode(&text).expect("a protocol line")
    }

    fn note(&mut self, line: Line) {
        if let Line::Notification(Notification {
            event: ServerEvent::Patches { patches, .. },
            ..
        }) = line
        {
            self.turn_ended |= patches.iter().any(ends_turn);
        }
    }

    /// Sends `call` and returns its reply, noting the notifications that
    /// arrive first.
    async fn call(&mut self, call: Call) -> Reply {
        self.next += 1;
        let id = self.next;
        let text = wire::encode(&Line::Request(Request {
            v: wire::VERSION,
            id,
            call,
        }))
        .expect("encode");
        self.stdin
            .write_all(text.as_bytes())
            .await
            .expect("write the request");
        self.stdin.flush().await.expect("flush");
        loop {
            match self.line().await {
                Line::Response(response) if response.id == id => match response.outcome {
                    Outcome::Ok(reply) => return reply,
                    Outcome::Err(error) => panic!("the server refused the call: {error}"),
                },
                other => self.note(other),
            }
        }
    }

    async fn turn_end(&mut self) {
        while !self.turn_ended {
            let line = self.line().await;
            self.note(line);
        }
    }

    /// Closes stdin, which is how the app ends a link, and waits for the
    /// server to leave so no `cox app-server` outlives the test.
    async fn finish(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(WAIT, self.child.wait())
            .await
            .expect("the server did not exit on a closed stdin")
            .expect("wait");
        assert!(status.success(), "the server exited with {status}");
    }
}

/// A scripted turn on `target` through its fake launcher; returns the
/// directory the launcher recorded into.
async fn run_turn(target: &Target) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let project = root.join("project");
    let home = root.join("remote");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&home).expect("remote home");
    let scenario = root.join("scenario.toml");
    std::fs::write(&scenario, REPLY).expect("scenario");
    let fake = fake_launcher::write(
        root,
        target.kind(),
        Path::new(env!("CARGO_BIN_EXE_cox")),
        &home,
        &scenario,
    );

    let mut client = Client::start(target, &fake);
    let Reply::Opened { session, .. } = client
        .call(Call::Open {
            cwd: project,
            resume: None,
            theme: "base16-ocean.dark".into(),
        })
        .await
    else {
        panic!("open did not answer with a session");
    };
    let sent = client
        .call(Call::Send {
            session,
            intent: Intent::Send {
                text: "hi".into(),
                attachments: vec![],
                confirm_think: false,
            },
        })
        .await;
    assert_eq!(sent, Reply::Sent { child: None });
    client.turn_end().await;
    client.finish().await;

    let args = fake_launcher::recorded_args(root, target.kind());
    assert!(
        args.iter().any(|a| a == target.name()),
        "{} was not passed to the launcher: {args:?}",
        target.name()
    );
    dir
}

fn parse(text: &str) -> Target {
    text.parse().expect("a valid target")
}

#[tokio::test]
async fn turn_runs_through_fake_ssh() {
    let dir = run_turn(&parse("ssh:devbox")).await;
    let args = fake_launcher::recorded_args(dir.path(), Kind::Ssh);
    assert_eq!(
        args[..4],
        ["-T", "-o", "BatchMode=yes", "-o"],
        "ssh gets no terminal and never prompts: {args:?}"
    );
}

#[tokio::test]
async fn turn_runs_through_fake_docker() {
    run_turn(&parse("docker:web")).await;
    let dir = run_turn(&Target::Docker {
        container: "web".into(),
        context: Some("remote".into()),
    })
    .await;
    let args = fake_launcher::recorded_args(dir.path(), Kind::Docker);
    assert_eq!(args[..2], ["--context", "remote"], "{args:?}");
}

#[tokio::test]
async fn turn_runs_through_fake_limactl() {
    let dir = run_turn(&parse("lima:default")).await;
    let args = fake_launcher::recorded_args(dir.path(), Kind::Lima);
    assert_eq!(args[..3], ["shell", "--tty=false", "default"], "{args:?}");
}

#[tokio::test]
async fn turn_runs_through_fake_colima() {
    let dir = run_turn(&parse("colima:dev")).await;
    let args = fake_launcher::recorded_args(dir.path(), Kind::Colima);
    assert_eq!(args[..4], ["ssh", "--profile", "dev", "--"], "{args:?}");
}

#[tokio::test]
async fn fake_launcher_refuses_an_argv_it_was_not_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let fake = fake_launcher::write(
        root,
        Kind::Lima,
        Path::new(env!("CARGO_BIN_EXE_cox")),
        root,
        &root.join("scenario.toml"),
    );
    // `limactl shell` without `--tty=false` is what a terminal would get; a
    // protocol link must never be started that way.
    let status = std::process::Command::new(&fake)
        .args(["shell", "default", "cox", "app-server", "--stdio"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run the fake");
    assert_eq!(status.code(), Some(fake_launcher::BAD_ARGV));
}

#[tokio::test]
#[allow(unsafe_code)]
async fn fake_launcher_sees_no_api_key() {
    // why: env::set_var is unsafe in edition 2024; a key in the app's own
    // process must not follow the session to the target, whatever the kind.
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-must-stay-local") };
    for text in ["ssh:devbox", "docker:web", "lima:default", "colima:dev"] {
        let target = parse(text);
        let dir = run_turn(&target).await;
        let env = fake_launcher::recorded_env(dir.path(), target.kind());
        assert!(!env.contains("ANTHROPIC_API_KEY"), "a key reached {text}");
        assert!(!env.contains("sk-must-stay-local"), "a key reached {text}");
    }
}

/// `Projects` against a real launcher and the `cox` on the target's `PATH`:
/// the link starts, the server answers and exits on a closed stdin.
async fn real_round_trip(kind: Kind, var: &str, target: fn(String) -> Target) {
    let Ok(name) = std::env::var(var) else {
        eprintln!("skipped: {var} is not set");
        return;
    };
    let Ok(launcher) = cox_target::launcher(kind) else {
        eprintln!("skipped: no {} at a fixed path", kind.program());
        return;
    };
    let mut client = Client::start(&target(name), &launcher);
    let reply = client.call(Call::Projects { limit: 5 }).await;
    assert!(matches!(reply, Reply::Projects(_)), "{reply:?}");
    client.finish().await;
}

#[tokio::test]
#[ignore = "needs a running Colima profile named by COX_TEST_COLIMA_PROFILE with cox installed"]
async fn real_colima_round_trip() {
    real_round_trip(Kind::Colima, "COX_TEST_COLIMA_PROFILE", |profile| {
        Target::Colima { profile }
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a running Lima instance named by COX_TEST_LIMA_INSTANCE with cox installed"]
async fn real_lima_round_trip() {
    real_round_trip(Kind::Lima, "COX_TEST_LIMA_INSTANCE", |instance| {
        Target::Lima { instance }
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a running container named by COX_TEST_DOCKER_CONTAINER with cox installed"]
async fn real_docker_round_trip() {
    real_round_trip(Kind::Docker, "COX_TEST_DOCKER_CONTAINER", |container| {
        Target::Docker {
            container,
            context: None,
        }
    })
    .await;
}
