// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T68.8: the built `cox app-server --stdio` over real pipes, the way
//! Windows OpenSSH starts it. The in-process `serve` tests use a duplex;
//! only this one runs the process's own stdin and stdout handles, which is
//! where a CRLF translation or a console-mode quirk would show. It runs on
//! every OS; `app_server_round_trips_on_windows` adds what only Windows can
//! assert (no sandbox, so D7's `on-request`).

#![cfg(feature = "app-server")]

use std::io::Write as _;
use std::process::{Command, Stdio};

use cox_app::wire::{self, Call, HostInfo, Line, Outcome, Reply, Request, Response};

/// Sends `calls`, each ended with `eol`, closes stdin and returns the raw
/// bytes the server wrote plus its exit success.
fn exchange(calls: Vec<Call>, eol: &str) -> (Vec<u8>, bool) {
    let home = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cox"))
        .args(["app-server", "--stdio"])
        .env("COX_HOME", home.path())
        // A plain login shell: the user's rc files are not under test, and
        // they would slow the capture of the login environment.
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawns cox");
    let mut stdin = child.stdin.take().expect("stdin");
    for (id, call) in (1..).zip(calls) {
        let request = Line::Request(Request {
            v: wire::VERSION,
            id,
            call,
        });
        let text = wire::encode(&request).expect("encodes");
        let text = text.trim_end().to_owned() + eol;
        stdin.write_all(text.as_bytes()).expect("write");
    }
    drop(stdin);
    let out = child.wait_with_output().expect("exits");
    (out.stdout, out.status.success())
}

fn responses(bytes: &[u8]) -> Vec<Response> {
    let text = std::str::from_utf8(bytes).expect("utf-8");
    text.lines()
        .map(|line| match wire::decode(line).expect("a protocol line") {
            Line::Response(response) => response,
            other => panic!("not a response: {other:?}"),
        })
        .collect()
}

fn hello(response: &Response) -> &HostInfo {
    match &response.outcome {
        Outcome::Ok(Reply::Hello(info)) => info,
        other => panic!("not a hello reply: {other:?}"),
    }
}

/// A Windows project path as a client on Windows sends it; on another OS it
/// is only a string that must come back unchanged.
const WINDOWS_PROJECT: &str = "C:\\srv\\app";

fn round_trip(eol: &str) -> Vec<Response> {
    let calls = vec![
        Call::Hello,
        Call::Sessions {
            project: WINDOWS_PROJECT.into(),
            limit: 5,
        },
    ];
    let (bytes, ok) = exchange(calls, eol);
    assert!(ok, "the server exits cleanly when stdin closes");
    assert!(!bytes.contains(&b'\r'), "a CR leaked into stdout");
    assert!(bytes.ends_with(b"\n"), "the last line is whole");
    let responses = responses(&bytes);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].id, 1);
    assert_eq!(hello(&responses[0]).wire, wire::VERSION);
    assert!(matches!(
        responses[1].outcome,
        Outcome::Ok(Reply::Sessions(ref sessions)) if sessions.is_empty()
    ));
    responses
}

#[test]
fn app_server_stdio_round_trips_with_lf_and_crlf_clients() {
    round_trip("\n");
    round_trip("\r\n");
}

#[cfg(windows)]
#[test]
fn app_server_round_trips_on_windows() {
    let responses = round_trip("\r\n");
    let info = hello(&responses[0]);
    assert_eq!(info.os, "windows");
    // No backend: the client shows this and the session stays on `on-request`.
    assert_eq!(info.sandbox, None);
}
