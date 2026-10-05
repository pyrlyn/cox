// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T5.8: the real `cox` binary under a PTY renders a scripted turn, shows
//! the model and cost in the status line, and exits cleanly on Ctrl+C ×2.
//! Everything below the binary is the same path a user gets; only the
//! provider is a scripted double.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cox_protocol::traits::Store as _;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

type Screen = Arc<Mutex<vt100::Parser>>;
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

const ROWS: u16 = 30;
const COLS: u16 = 100;

/// One `cox` process on a PTY, with a thread keeping a vt100 screen of it.
struct Tui {
    screen: Screen,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
    /// Held so the PTY stays open for the reader and the child.
    _master: Box<dyn MasterPty + Send>,
}

impl Tui {
    fn spawn(home: &Path, work: &Path, scenario: &Path, args: &[&str]) -> Tui {
        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cox"));
        cmd.args(["--cwd", work.to_str().unwrap(), "--model", "scripted"]);
        cmd.args(args);
        cmd.cwd(work);
        cmd.env("COX_HOME", home);
        // The real `~/.claude.json` / `~/.claude/settings.json` must not leak
        // MCP servers or hooks into the test (T7.5/T7.6 read `$HOME`).
        cmd.env("HOME", home);
        cmd.env("COX_PROVIDER", "scripted");
        cmd.env("COX_SCENARIO", scenario);
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);

        let screen: Screen = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer: Writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let (sink, replier) = (screen.clone(), writer.clone());
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Carries the tail of the previous read so a query split across
            // two reads is still seen.
            let mut raw: Vec<u8> = Vec::new();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut parser = sink.lock().unwrap();
                parser.process(&buf[..n]);
                raw.extend_from_slice(&buf[..n]);
                // The inline viewport queries the cursor (CSI 6n) at start and
                // on every insert_before; a real terminal answers, so this one
                // must too or the binary times out.
                let queries = raw.windows(4).filter(|w| *w == b"\x1b[6n").count();
                if queries > 0 {
                    let (row, col) = parser.screen().cursor_position();
                    let reply = format!("\x1b[{};{}R", row + 1, col + 1).repeat(queries);
                    let _ = replier.lock().unwrap().write_all(reply.as_bytes());
                }
                let keep = raw.len().saturating_sub(3);
                raw.drain(..keep);
            }
        });
        Tui {
            screen,
            writer,
            child,
            _master: pty.master,
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }

    /// Polls the screen until `ok` holds; a snapshot can land between
    /// `insert_before` and the next draw, so callers check whole states.
    ///
    /// The deadline is generous on purpose: a passing run never waits for
    /// it, and the debug binary's first paint (SQLite, syntax themes, the
    /// workspace walk) takes well over a second on a box that is also
    /// running two other cargo builds — 5 s made this the one flaky test in
    /// the suite.
    fn wait_until(&self, what: &str, ok: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let text = self.screen.lock().unwrap().screen().contents();
            if ok(&text) {
                return text;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "{what} never appeared within 30s; screen was:\n{text}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Sends one prompt and waits for its reply with the turn finished (a
    /// Ctrl+C or a command while `working` would act on the turn instead).
    fn turn(&self, prompt: &str, reply: &str) {
        self.send(format!("{prompt}\r").as_bytes());
        self.wait_until("finished scripted turn", |t| {
            t.contains(reply) && !t.contains("working") && t.contains("scripted · ")
        });
    }

    /// A slash command picked through the `/` palette: `/` opens it, the
    /// name filters it to that row, Enter closes it with `/<name> ` in the
    /// composer, then `args` and Enter submit.
    fn command(&self, name: &str, args: &str) {
        self.send(b"/");
        self.wait_until("command palette", |t| t.contains("\u{25b8} "));
        self.send(name.as_bytes());
        let row = format!("\u{25b8} {name}");
        self.wait_until("palette row", |t| t.contains(&row));
        self.send(b"\r");
        self.wait_until("palette closed", |t| !t.contains("\u{25b8} "));
        self.send(format!("{args}\r").as_bytes());
    }

    /// Ctrl+C twice on an idle TUI, then a clean exit.
    fn quit(mut self) {
        self.send(b"\x03");
        self.wait_until("quit prompt", |t| t.contains("again to quit"));
        self.send(b"\x03");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "cox did not exit");
            thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "exit status {status:?}");
    }
}

#[test]
fn tui_renders_scripted_turn_and_exits_on_double_ctrl_c() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let scenario = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cox-core/tests/scenarios/text_only.toml"
    ));
    let tui = Tui::spawn(home.path(), work.path(), scenario, &[]);
    // The status line is the first thing the TUI paints.
    tui.wait_until("status line", |t| t.contains("$0.00"));
    // Reply rendered, turn finished, status shows the model and the cost.
    tui.turn("hello", "hello from scripted");
    tui.wait_until("cost in the status line", |t| t.contains("$0.00"));
    tui.quit();
}

/// T26.3: `/fork` after a turn reopens the TUI in a child session, and
/// `/handoff` from that child reopens it in a grandchild seeded with a
/// cheap summary; the store records the chain as depths 0, 1, 2.
#[test]
fn tui_fork_and_handoff_start_child_sessions() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    // Every session the loop builds gets a fresh scripted provider; a second
    // turn covers a provider that is shared instead, so the handoff summary
    // is never "exhausted".
    let scenario = home.path().join("scenario.toml");
    std::fs::write(
        &scenario,
        "[[turn]]\ntext = \"hello from scripted\"\n[[turn]]\ntext = \"we said hello\"\n",
    )
    .unwrap();
    let tui = Tui::spawn(home.path(), work.path(), &scenario, &[]);

    tui.wait_until("status line", |t| t.contains("$0.00"));
    tui.turn("hello", "hello from scripted");

    tui.command("fork", "");
    tui.wait_until("fork notice", |t| {
        t.contains("fork at the latest turn: session") && t.contains("$0.00")
    });

    tui.command("handoff", "finish the demo");
    // A notice without "(no summary" means the cheap summariser answered.
    tui.wait_until("handoff notice", |t| {
        t.contains("handoff: session") && t.contains("$0.00")
    });
    tui.quit();

    let store = cox_store::Store::open(home.path()).unwrap();
    let depths: Vec<usize> = store
        .sessions_tree(10)
        .unwrap()
        .iter()
        .map(|r| r.depth)
        .collect();
    assert_eq!(depths, [0, 1, 2], "parent, fork, handoff");
}

/// T37.34: a second TUI that resumes the session the first one drives
/// gets the busy notice naming the holder and its options, and exits
/// instead of opening a second writer on the same rollout.
#[test]
fn tui_resume_of_a_driven_session_prints_busy_notice() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let scenario = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cox-core/tests/scenarios/text_only.toml"
    ));
    let driver = Tui::spawn(home.path(), work.path(), scenario, &[]);
    driver.wait_until("status line", |t| t.contains("$0.00"));
    driver.turn("hello", "hello from scripted");
    let store = cox_store::Store::open(home.path()).unwrap();
    let id = store.sessions_tree(10).unwrap()[0].info.id.clone();

    let mut second = Tui::spawn(home.path(), work.path(), scenario, &["--resume", &id]);
    let notice = format!("session {id} is open in cox tui (pid");
    second.wait_until("busy notice", |t| {
        let t = t.replace('\n', "");
        t.contains(&notice) && t.contains("fork it into a new session")
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = second.child.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "cox did not exit");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(!status.success(), "a busy session is an error exit");
    driver.quit();
}

/// T27.1: `Ctrl+B` on a running `sleep` moves it to the background; the
/// turn ends and the composer takes input while the task still runs.
#[test]
fn tui_ctrl_b_backgrounds_sleep_and_composer_accepts_input() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let scenario = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/scenarios/bash_sleep_then_done.toml"
    ));
    let tui = Tui::spawn(
        home.path(),
        work.path(),
        scenario,
        &["--permission-mode", "bypass"],
    );
    tui.wait_until("status line", |t| t.contains("$0.00"));
    tui.send(b"go\r");
    tui.wait_until("sleep running", |t| {
        t.contains("begin") && t.contains("working")
    });
    tui.send(b"\x02");
    tui.wait_until("turn done, task listed", |t| {
        t.contains("moved on") && !t.contains("working") && t.contains("1 tasks")
    });
    tui.send(b"typed while it sleeps");
    tui.wait_until("composer text while the task runs", |t| {
        t.contains("typed while it sleeps") && t.contains("1 tasks")
    });
    tui.wait_until("task finished", |t| {
        t.contains("background task finished") && t.contains("0 tasks")
    });
    tui.quit();
}

/// Quitting the TUI while a detached `bash` still runs kills its process
/// instead of leaving it orphaned (ppid 1): `run_tui` does what headless
/// `run` does (`interrupt` + `wait_tasks_cleared`, then
/// `shutdown_background`). `pgrep -f` exits 0 if it finds a match.
#[test]
#[cfg(unix)]
fn tui_quit_kills_a_running_background_shell() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let scenario = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/scenarios/tui_background_shell.toml"
    ));
    let tui = Tui::spawn(
        home.path(),
        work.path(),
        scenario,
        &["--permission-mode", "bypass"],
    );
    tui.wait_until("status line", |t| t.contains("$0.00"));
    tui.send(b"go\r");
    tui.wait_until("turn done, task listed", |t| {
        t.contains("detached it") && !t.contains("working") && t.contains("1 tasks")
    });
    tui.quit();
    let leaked = std::process::Command::new("pgrep")
        .args(["-f", "sleep 4002"])
        .status()
        .unwrap()
        .success();
    assert!(!leaked, "a `sleep 4002` process outlived the TUI");
}

/// T25.5: `send = "ctrl+enter"` in `keybindings.toml` shows in the hints,
/// turns plain Enter into a newline and sends on Ctrl+Enter. A plain PTY
/// writes `\r` for both keys, so Ctrl+Enter goes in as the kitty
/// keyboard protocol's `CSI 13;5u`, which is what a terminal that can tell
/// them apart sends.
#[test]
fn tui_keybindings_toml_rebinds_send_to_ctrl_enter() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("keybindings.toml"),
        "send = \"ctrl+enter\"\n",
    )
    .unwrap();
    let scenario = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cox-core/tests/scenarios/text_only.toml"
    ));
    let tui = Tui::spawn(home.path(), work.path(), scenario, &[]);
    tui.wait_until("rebound hint", |t| {
        t.contains("Ctrl+Enter send") && t.contains("$0.00")
    });
    tui.send(b"hello\r");
    tui.send(b"again");
    tui.wait_until("two composer lines, nothing sent", |t| {
        t.contains("hello") && t.contains("again") && !t.contains("working")
    });
    // Enter had its chance to send; the scripted reply is instant, so half
    // a second without it means the `\r` stayed in the composer.
    thread::sleep(Duration::from_millis(500));
    let before = tui.wait_until("screen", |_| true);
    assert!(
        !before.contains("hello from scripted"),
        "Enter sent:\n{before}"
    );
    tui.send(b"\x1b[13;5u");
    let text = tui.wait_until("finished scripted turn", |t| {
        t.contains("hello from scripted") && !t.contains("working")
    });
    assert_eq!(text.matches("hello from scripted").count(), 1, "{text}");
    tui.quit();
}
