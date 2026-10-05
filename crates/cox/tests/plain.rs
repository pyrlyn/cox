// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T29.1: the real `cox --plain` under a PTY — a scripted turn with an
//! approval, a markdown table and Ctrl+C twice — prints flat labelled
//! lines and never moves the cursor. Only the provider is a double.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

const SCENARIO: &str = r#"[[turn]]
text = "writing"
tool_calls = [{ name = "write", input = { path = "a.txt", content = "x" } }]

[[turn]]
text = "| file | state |\n|---|---|\n| a.txt | written |"
"#;

/// Everything `cox --plain` wrote to the PTY, raw bytes included.
fn transcript() -> String {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let scenario = home.path().join("scenario.toml");
    std::fs::write(&scenario, SCENARIO).unwrap();
    let pty = NativePtySystem::default()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cox"));
    cmd.args(["--plain", "--cwd", work.path().to_str().unwrap()]);
    cmd.args(["--model", "scripted"]);
    cmd.cwd(work.path());
    cmd.env("COX_HOME", home.path());
    // No real `~/.claude.json` MCP servers or hooks in the test.
    cmd.env("HOME", home.path());
    cmd.env("COX_PROVIDER", "scripted");
    cmd.env("COX_SCENARIO", &scenario);
    cmd.env("TERM", "dumb");
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);

    let raw = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut reader = pty.master.try_clone_reader().unwrap();
    let sink = raw.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let mut writer = pty.master.take_writer().unwrap();
    let mut send = |bytes: &[u8]| {
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    };
    let text = || String::from_utf8_lossy(&raw.lock().unwrap()).into_owned();
    let wait_until = |what: &str, ok: &dyn Fn(&str) -> bool| {
        let start = Instant::now();
        while !ok(&text()) {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "{what} never appeared; output was:\n{}",
                text()
            );
            thread::sleep(Duration::from_millis(50));
        }
    };

    wait_until("first prompt", &|t| t.ends_with("you: "));
    send(b"go\r");
    wait_until("approval prompt", &|t| t.ends_with("[3] deny "));
    send(b"1\r");
    wait_until("turn done", &|t| {
        t.contains("cost: ") && t.ends_with("you: ")
    });
    send(b"\x03");
    wait_until("quit notice", &|t| {
        t.contains("again to quit") && t.ends_with("you: ")
    });
    send(b"\x03");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "cox did not exit"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status:?}");
    // The reader may still hold the last bytes of the exit.
    thread::sleep(Duration::from_millis(200));
    let out = text();
    redact(&out, work.path())
}

/// Temp paths and token counts vary per machine; everything else is fixed.
fn redact(out: &str, work: &Path) -> String {
    let mut out = out.replace("\r\n", "\n").replace('\x07', "<BEL>");
    let real = std::fs::canonicalize(work).unwrap();
    for path in [real.as_path(), work] {
        out = out.replace(path.to_str().unwrap(), "[WORK]");
    }
    out.lines()
        .map(|line| match line.strip_prefix("cost: ") {
            Some(rest) => format!("cost: {}", rest.replace(|c: char| c.is_ascii_digit(), "#")),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn plain_transcript_snapshot() {
    insta::assert_snapshot!("plain_transcript", transcript());
}

/// No `CSI … H/J/K` (position, erase display, erase line) nor any other
/// cursor movement: every byte is appended below what came before.
#[test]
fn plain_has_no_csi_cursor_moves() {
    let out = transcript();
    let bytes = out.as_bytes();
    for (i, w) in bytes.windows(2).enumerate() {
        if w != b"\x1b[" {
            continue;
        }
        let end = bytes[i + 2..]
            .iter()
            .find(|b| (0x40..=0x7e).contains(*b))
            .copied();
        assert!(
            !matches!(end, Some(b'A'..=b'H' | b'J' | b'K' | b'S' | b'T' | b'f')),
            "cursor-moving CSI at byte {i}: {out:?}"
        );
    }
    assert!(out.contains("you: go"), "{out}");
    assert!(out.contains("cox: file: a.txt; state: written"), "{out}");
}
