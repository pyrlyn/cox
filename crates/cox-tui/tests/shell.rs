// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Git-aware completion (T15.4): `Tab` on a `git` line opens the picker over
//! subcommands, branches or paths by position, and a choice replaces the
//! word being typed. Any other line keeps Tab's old meaning.

use cox_protocol::types::{PermissionMode, SandboxMode};
use cox_tui::picker::Kind;
use cox_tui::state::{Modal, Msg, State, update};
use crossterm::event::{KeyCode, KeyEvent};

/// Types `line` into a fresh composer and presses Tab.
fn tab_after(line: &str) -> State {
    let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
    state.files = vec!["src/lib.rs".into(), "README.md".into()];
    state.git_branches = vec!["main".into(), "feature/x".into()];
    for c in line.chars() {
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Tab)));
    state
}

fn offered(state: &State) -> Vec<String> {
    match &state.modal {
        Some(Modal::Picker(p)) if p.kind == Kind::Shell => p.matches.clone(),
        other => panic!("no shell picker: {other:?}"),
    }
}

#[test]
fn shell_tab_offers_a_subcommand_a_branch_or_a_path_by_position() {
    assert_eq!(offered(&tab_after("git ch"))[0], "checkout");
    assert_eq!(offered(&tab_after("git checkout ma")), ["main"]);
    assert_eq!(offered(&tab_after("git add sr")), ["src/lib.rs"]);
}

#[test]
fn shell_tab_on_another_command_opens_nothing() {
    assert!(tab_after("ls ").modal.is_none());
}

#[test]
fn shell_choice_replaces_the_word_being_typed() {
    let mut state = tab_after("git checkout ma");
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(state.modal.is_none());
    assert_eq!(state.composer.text(), "git checkout main ");
}

/// T23.1: `cox-tui` has no binary of its own to spawn under a PTY (unlike
/// `crates/cox/tests/tui_e2e.rs`, which spawns the real `cox`), so
/// `src/bin/kitty_probe.rs` — which exists purely for these tests — stands
/// in: it drives `cox_tui::app::run` for real and quits itself once its
/// scenario is fed.
mod pty {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

    /// Generous on purpose: a slow CI box must never turn into a flake.
    const DEADLINE: Duration = Duration::from_secs(30);

    /// The terminal side of the PTY: every raw byte, a vt100 screen, and
    /// the two behaviours of real terminals that `vt100` 0.16 lacks, so a
    /// test measures cox rather than the fixture (T23.7).
    pub struct Term {
        pub raw: Vec<u8>,
        pub parser: vt100::Parser,
        /// Lines scrolled off a DECSTBM region whose top is row 1. xterm,
        /// kitty and tmux keep these in scrollback — the premise of
        /// ratatui's `scrolling-regions` — but `vt100` drops them.
        pub evicted: Vec<String>,
        /// `CSI a;b r` last seen, 1-based; `None` once reset.
        region: Option<(u16, u16)>,
        /// An escape sequence or UTF-8 char split across two reads, held
        /// back so `resize` never lands in the middle of one.
        pending: Vec<u8>,
    }

    impl Term {
        /// Feeds whatever complete input `bytes` finishes and returns how
        /// many `CSI 6n` cursor queries it held.
        fn feed(&mut self, bytes: &[u8]) -> usize {
            self.raw.extend_from_slice(bytes);
            self.pending.extend_from_slice(bytes);
            let buf = std::mem::take(&mut self.pending);
            let complete = complete_len(&buf);
            let mut queries = 0;
            let mut fed = 0;
            let mut i = 0;
            while i < complete {
                if buf[i] != 0x1b || buf.get(i + 1) != Some(&b'[') {
                    i += 1;
                    continue;
                }
                let params_at = i + 2;
                let end = (params_at..complete)
                    .find(|&j| (0x40..=0x7e).contains(&buf[j]))
                    .unwrap_or(complete);
                let params = std::str::from_utf8(&buf[params_at..end]).unwrap_or("");
                match buf.get(end) {
                    Some(b'n') if params == "6" => queries += 1,
                    Some(b'r') => {
                        let mut it = params.split(';').map(|p| p.parse::<u16>().ok());
                        self.region = match (it.next().flatten(), it.next().flatten()) {
                            (Some(top), Some(bottom)) => Some((top, bottom)),
                            _ => None,
                        };
                    }
                    Some(b'S') => {
                        let rows = self.parser.screen().size().0;
                        if let Some((1, bottom)) = self.region.filter(|r| r.1 < rows) {
                            self.parser.process(&buf[fed..i]);
                            fed = i;
                            let n = params.parse::<u16>().unwrap_or(1).min(bottom);
                            let cols = self.parser.screen().size().1;
                            let lines = self.parser.screen().rows(0, cols).take(n.into());
                            self.evicted.extend(lines);
                        }
                    }
                    _ => {}
                }
                i = end + 1;
            }
            self.parser.process(&buf[fed..complete]);
            self.pending = buf[complete..].to_vec();
            queries
        }

        /// Every line the user could scroll back to, oldest first within
        /// each source: evicted region lines, then `vt100`'s own
        /// scrollback, then the screen.
        pub fn transcript(&mut self) -> Vec<String> {
            let mut lines = self.evicted.clone();
            let cols = self.parser.screen().size().1;
            let screen = self.parser.screen_mut();
            screen.set_scrollback(usize::MAX);
            for offset in (1..=screen.scrollback()).rev() {
                screen.set_scrollback(offset);
                lines.extend(screen.rows(0, cols).next());
            }
            screen.set_scrollback(0);
            lines.extend(screen.rows(0, cols));
            lines
        }
    }

    /// Whether `raw` stops right after a draw: ratatui shows or hides the
    /// cursor and then moves it to its resting cell, and nothing follows.
    fn ends_on_frame(raw: &[u8]) -> bool {
        let Some(at) = raw
            .windows(6)
            .rposition(|w| w == b"\x1b[?25h" || w == b"\x1b[?25l")
        else {
            return false;
        };
        let rest = &raw[at + 6..];
        rest.len() > 3
            && rest.starts_with(b"\x1b[")
            && rest.ends_with(b"H")
            && rest[2..rest.len() - 1]
                .iter()
                .all(|b| b.is_ascii_digit() || *b == b';')
    }

    /// Length of the prefix of `buf` that ends on a whole escape sequence
    /// and a whole UTF-8 char.
    fn complete_len(buf: &[u8]) -> usize {
        if let Some(esc) = buf.iter().rposition(|&b| b == 0x1b) {
            let rest = &buf[esc + 1..];
            let done = match rest.first() {
                None => false,
                Some(b'[') => rest[1..].iter().any(|b| (0x40..=0x7e).contains(b)),
                Some(b']') => rest.contains(&0x07),
                Some(_) => true,
            };
            if !done {
                return esc;
            }
        }
        let lead = buf.iter().rposition(|&b| b & 0xc0 != 0x80).unwrap_or(0);
        let want = match buf.get(lead) {
            Some(b) if b & 0xe0 == 0xc0 => 2,
            Some(b) if b & 0xf0 == 0xe0 => 3,
            Some(b) if b & 0xf8 == 0xf0 => 4,
            _ => 1,
        };
        if buf.len() - lead < want {
            lead
        } else {
            buf.len()
        }
    }

    pub struct Probe {
        pub term: Arc<Mutex<Term>>,
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
        reading: thread::JoinHandle<()>,
    }

    impl Probe {
        /// Spawns `kitty_probe` with `env` on a `rows`×`cols` PTY and
        /// answers its `CSI 6n` cursor queries the same way `tui_e2e.rs`
        /// does for the real binary (the inline viewport needs an answer
        /// or it stalls).
        pub fn spawn(env: &[(&str, &str)], rows: u16, cols: u16) -> Self {
            let pty = NativePtySystem::default()
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_kitty_probe"));
            for (key, value) in env {
                cmd.env(key, value);
            }
            cmd.env("TERM", "xterm-256color");
            let child = pty.slave.spawn_command(cmd).expect("spawn kitty_probe");
            drop(pty.slave);

            let term = Arc::new(Mutex::new(Term {
                raw: Vec::new(),
                parser: vt100::Parser::new(rows, cols, 10_000),
                evicted: Vec::new(),
                region: None,
                pending: Vec::new(),
            }));
            let mut reader = pty.master.try_clone_reader().expect("clone reader");
            let mut writer = pty.master.take_writer().expect("writer");
            let shared = term.clone();
            let reading = thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut term = shared.lock().unwrap();
                    let queries = term.feed(&buf[..n]);
                    if queries > 0 {
                        let (row, col) = term.parser.screen().cursor_position();
                        let reply = format!("\x1b[{};{}R", row + 1, col + 1).repeat(queries);
                        let _ = writer.write_all(reply.as_bytes());
                    }
                }
            });
            Probe {
                term,
                master: pty.master,
                child,
                reading,
            }
        }

        /// Waits until the screen shows `text`.
        pub fn wait_for(&self, text: &str) {
            let start = Instant::now();
            while !self
                .term
                .lock()
                .unwrap()
                .parser
                .screen()
                .contents()
                .contains(text)
            {
                assert!(
                    start.elapsed() < DEADLINE,
                    "{text:?} never reached the screen"
                );
                thread::sleep(Duration::from_millis(20));
            }
        }

        /// Resizes the terminal the way xterm does, then tells the probe:
        /// a shrink that would cut off the cursor row first scrolls the top
        /// rows into scrollback (`vt100` would drop the bottom rows). It
        /// waits for a frame boundary so the cursor is where the app last
        /// parked it rather than wherever a half-written frame left it —
        /// the one moment a resize has a single right answer.
        pub fn resize(&self, rows: u16, cols: u16) {
            let start = Instant::now();
            let mut term = loop {
                let term = self.term.lock().unwrap();
                if term.pending.is_empty() && ends_on_frame(&term.raw) {
                    break term;
                }
                drop(term);
                assert!(start.elapsed() < DEADLINE, "no frame boundary to resize at");
                thread::sleep(Duration::from_millis(2));
            };
            let (row, col) = term.parser.screen().cursor_position();
            if row >= rows {
                let lift = row + 1 - rows;
                let keep = format!("\x1b[{lift}S\x1b[{};{}H", row - lift + 1, col + 1);
                term.parser.process(keep.as_bytes());
            }
            term.parser.screen_mut().set_size(rows, cols);
            self.master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("resize pty");
        }

        /// Waits for the probe to quit itself and for the reader to drain
        /// what it wrote last (`restore()` writes right as it exits).
        pub fn finish(mut self) -> Arc<Mutex<Term>> {
            let start = Instant::now();
            while self.child.try_wait().expect("try_wait").is_none() {
                assert!(start.elapsed() < DEADLINE, "kitty_probe did not exit");
                thread::sleep(Duration::from_millis(20));
            }
            // The master reports EOF/EIO once the last slave fd closes.
            while !self.reading.is_finished() {
                assert!(start.elapsed() < DEADLINE, "PTY reader did not drain");
                thread::sleep(Duration::from_millis(10));
            }
            self.term
        }
    }

    /// Runs a scenario that needs no interaction and returns its raw bytes.
    pub fn run_probe(env: &[(&str, &str)], rows: u16, cols: u16) -> Vec<u8> {
        let term = Probe::spawn(env, rows, cols).finish();
        term.lock().unwrap().raw.clone()
    }
}

#[test]
fn pty_pops_keyboard_flags_on_exit() {
    let with = pty::run_probe(&[("COX_KITTY_PROBE_KITTY", "1")], 24, 80);
    assert!(
        with.windows(5).any(|w| w == b"\x1b[>3u"),
        "push flags missing: {:?}",
        String::from_utf8_lossy(&with)
    );
    assert!(
        with.windows(5).any(|w| w == b"\x1b[<1u"),
        "pop flags missing: {:?}",
        String::from_utf8_lossy(&with)
    );

    let without = pty::run_probe(&[("COX_KITTY_PROBE_KITTY", "0")], 24, 80);
    assert!(
        !without.windows(5).any(|w| w == b"\x1b[>3u"),
        "push flags present without the capability: {:?}",
        String::from_utf8_lossy(&without)
    );
    assert!(
        !without.windows(5).any(|w| w == b"\x1b[<1u"),
        "pop flags present without the capability: {:?}",
        String::from_utf8_lossy(&without)
    );
}

/// T22.4: `tui.mouse = false` (the default probe state) must leave the
/// terminal's mouse reporting untouched; `true` enables it on start and
/// disables it on exit, exactly once each.
#[test]
fn pty_no_mouse_capture_when_disabled() {
    let count =
        |raw: &[u8], needle: &[u8]| raw.windows(needle.len()).filter(|w| *w == needle).count();

    let without = pty::run_probe(&[], 24, 80);
    assert_eq!(
        count(&without, b"\x1b[?1000h") + count(&without, b"\x1b[?1006h"),
        0,
        "mouse capture sent without tui.mouse: {:?}",
        String::from_utf8_lossy(&without)
    );

    let with = pty::run_probe(&[("COX_PROBE_MOUSE", "1")], 24, 80);
    let shown = String::from_utf8_lossy(&with);
    assert_eq!(count(&with, b"\x1b[?1000h"), 1, "{shown:?}");
    assert_eq!(count(&with, b"\x1b[?1006h"), 1, "{shown:?}");
    assert_eq!(
        count(&with, b"\x1b[?1000l"),
        1,
        "not disabled on exit: {shown:?}"
    );
    assert_eq!(
        count(&with, b"\x1b[?1006l"),
        1,
        "not disabled on exit: {shown:?}"
    );
}

/// Rows `app.rs` keeps for the live viewport (`VIEWPORT_ROWS`).
const VIEWPORT_ROWS: usize = 15;

/// A sentinel no cox frame draws, so a cell still holding it was not
/// touched by the frame under test.
const SENTINEL: &str = "\u{a4}";

/// Replays `raw` offline and counts the frames that rewrote the whole
/// viewport. ratatui ends every `draw` by showing or hiding the cursor and
/// `insert_before` never does, so each `?25h`/`?25l` closes one frame (the
/// inserts that preceded that draw included). Before each frame every
/// screen cell is overwritten with `SENTINEL` in a copy of the screen; a
/// row with no sentinel left afterwards was erased or fully redrawn, and a
/// frame that did that to at least `VIEWPORT_ROWS` rows repainted the
/// viewport rather than diffing it.
fn full_repaints(raw: &[u8], rows: u16, cols: u16) -> usize {
    let mut live = vt100::Parser::new(rows, cols, 0);
    let mut seed = b"\x1b7".to_vec();
    for row in 1..=rows {
        seed.extend(format!("\x1b[{row};1H{}", SENTINEL.repeat(cols.into())).bytes());
    }
    seed.extend(b"\x1b8");
    let mut repaints = 0;
    let mut rest = raw;
    while !rest.is_empty() {
        let end = rest
            .windows(6)
            .position(|w| w == b"\x1b[?25h" || w == b"\x1b[?25l")
            .map_or(rest.len(), |at| at + 6);
        let (frame, next) = rest.split_at(end);
        rest = next;
        let mut copy = vt100::Parser::new(rows, cols, 0);
        copy.process(&live.screen().state_formatted());
        copy.process(&seed);
        copy.process(frame);
        live.process(frame);
        let screen = copy.screen();
        let rewritten = (0..rows)
            .filter(|&r| {
                (0..cols).all(|c| screen.cell(r, c).is_none_or(|x| x.contents() != SENTINEL))
            })
            .count();
        if rewritten >= VIEWPORT_ROWS {
            repaints += 1;
        }
    }
    repaints
}

/// T23.2: 40 finished cells stream into scrollback on an 80×24 PTY.
/// Without `scrolling-regions` every `insert_before` clears the viewport
/// and the next draw repaints all of it (measured: 40 repaints for 40
/// cells); with the feature the region above the viewport scrolls and the
/// viewport is only diffed (measured: 0). The bound is the card's "at most
/// once per cell" tightened to one for the whole run, so the test fails if
/// the feature is ever dropped.
#[test]
fn pty_insert_before_repaints_at_most_once_per_cell() {
    let raw = pty::run_probe(&[("COX_PROBE_SCENARIO", "cells")], 24, 80);
    let shown = String::from_utf8_lossy(&raw);
    assert!(
        shown.contains("cell-40"),
        "the last cell never reached the screen"
    );
    let repaints = full_repaints(&raw, 24, 80);
    assert!(
        repaints <= 1,
        "{repaints} full-viewport repaints for 40 inserted cells"
    );
}

/// T23.7: a 120×40 terminal shrinks to 80×24 while a reply streams below
/// 12 finished cells; the reply and 4 more cells finish after the resize.
/// Every one of them must be in the scrollback exactly once: a stale copy
/// of the old viewport pushed up by the resize would show `reply-start`
/// twice, and a whole-screen clear would lose the cells that were still on
/// screen.
#[test]
fn pty_resize_mid_stream_keeps_scrollback_unique() {
    let probe = pty::Probe::spawn(&[("COX_PROBE_SCENARIO", "resize")], 40, 120);
    probe.wait_for("reply-start");
    probe.resize(24, 80);
    let term = probe.finish();
    let lines = term.lock().unwrap().transcript();
    let markers = (1..=16)
        .map(|n| format!("cell-{n:02}"))
        .chain(["reply-start".to_string(), "reply-end".to_string()]);
    for marker in markers {
        let seen = lines.iter().filter(|l| l.contains(&marker)).count();
        assert_eq!(
            seen,
            1,
            "{marker} appears {seen} times in:\n{}",
            lines.join("\n")
        );
    }
}

/// T23.5: with focus lost, `TurnDone` writes OSC 9 then BEL; with focus held
/// the same event writes nothing. Focus reporting is switched on and off.
#[test]
fn pty_turn_done_writes_osc9() {
    let raw = pty::run_probe(&[("COX_PROBE_SCENARIO", "notify")], 24, 80);
    let has = |needle: &[u8]| raw.windows(needle.len()).filter(|w| *w == needle).count();
    let shown = String::from_utf8_lossy(&raw);
    assert_eq!(has(b"\x1b]9;turn done\x07\x07"), 1, "{shown:?}");
    assert_eq!(has(b"\x1b]9;"), 1, "rang with focus held: {shown:?}");
    assert_eq!(
        has(b"\x1b[?1004h"),
        1,
        "focus reporting not enabled: {shown:?}"
    );
    assert_eq!(
        has(b"\x1b[?1004l"),
        1,
        "focus reporting not disabled: {shown:?}"
    );
}

/// T23.6: a terminal that draws OSC 9;4 sees one indeterminate state for
/// the turn, a clear when it ends and another on exit; one that does not
/// sees no OSC 9;4 at all.
#[test]
fn pty_progress_only_with_the_capability() {
    let scenario = ("COX_PROBE_SCENARIO", "progress");
    let with = pty::run_probe(&[scenario, ("COX_PROBE_OSC9_4", "1")], 24, 80);
    let count =
        |raw: &[u8], needle: &[u8]| raw.windows(needle.len()).filter(|w| *w == needle).count();
    let shown = String::from_utf8_lossy(&with);
    assert_eq!(count(&with, b"\x1b]9;4;3;0\x1b\\"), 1, "{shown:?}");
    assert_eq!(count(&with, b"\x1b]9;4;0;0\x1b\\"), 2, "{shown:?}");
    let without = pty::run_probe(&[scenario], 24, 80);
    assert_eq!(
        // `]` too: SGR's `39;49m` contains `9;4`.
        count(&without, b"\x1b]9;4"),
        0,
        "{:?}",
        String::from_utf8_lossy(&without)
    );
}

/// T23.4: `y` on the probe's one streaming cell writes OSC 52 with the
/// cell's text, base64-encoded, only when the terminal draws it.
#[test]
fn pty_copy_writes_osc52() {
    let scenario = ("COX_PROBE_SCENARIO", "copy");
    let with = pty::run_probe(&[scenario, ("COX_PROBE_OSC52", "1")], 24, 80);
    let shown = String::from_utf8_lossy(&with);
    assert!(shown.contains("\x1b]52;c;Y2VsbCB0ZXh0\x1b\\"), "{shown:?}");
    let without = pty::run_probe(&[scenario], 24, 80);
    assert!(
        !String::from_utf8_lossy(&without).contains("\x1b]52;c;"),
        "wrote OSC 52 without the capability: {:?}",
        String::from_utf8_lossy(&without)
    );
}
