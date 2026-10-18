// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One worker process (`docs/design/serve.md` D3): spawn it with piped stdio,
//! write the few app-server wire lines the supervisor itself sends (`open`,
//! `end`), pick the `open` reply out of its stdout, hand every later line to
//! the router (S7), and report its exit. Separate from `supervisor` so the
//! process plumbing stays apart from the restart bookkeeping. The lines are built with `serde_json` instead of
//! `cox-app`'s `wire` types: the supervisor sends two calls and reads one
//! reply, which does not justify linking the app core into this crate.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use crate::router::Routes;

/// The request id of the supervisor's own calls. A worker is a fresh process
/// whose first request comes from the supervisor, so no client id is in use
/// yet; the router (S7) maps client ids around it.
pub(crate) const OWN_ID: u64 = 1;

/// The wire version `cox-app`'s `wire::VERSION` names.
pub(crate) const VERSION: u32 = 1;

/// The worker's answer to `open`: the session id, or the error text.
pub(crate) type Opened = Result<String, String>;

/// A started worker and the receiver of its `open` reply.
pub(crate) type Spawned = (Worker, oneshot::Receiver<Opened>);

/// A running worker. Dropping it kills the process, so a worker the
/// supervisor forgot never outlives it.
pub(crate) struct Worker {
    pub(crate) serial: u64,
    pub(crate) started: Instant,
    /// Lines for stdin. The router holds only weak handles, so dropping this
    /// one is what closes the pipe.
    stdin: Option<mpsc::UnboundedSender<String>>,
    kill: Option<oneshot::Sender<()>>,
}

impl Worker {
    /// Start `program`, send it `open`, and call `on_exit` with whether it
    /// exited with status 0. The receiver yields the `open` reply, or closes
    /// if the worker exits without one. With `routes`, the session the reply
    /// names is linked there and every later line goes to the router.
    pub(crate) async fn spawn(
        program: &Path,
        args: &[OsString],
        serial: u64,
        open: &str,
        routes: Option<Arc<Routes>>,
        on_exit: impl FnOnce(bool) + Send + 'static,
    ) -> std::io::Result<Spawned> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (stdin, queued) = mpsc::unbounded_channel();
        if let Some(pipe) = child.stdin.take() {
            tokio::spawn(write(pipe, queued));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if let Some(out) = child.stdout.take() {
            let link = routes.map(|routes| (routes, serial, stdin.downgrade()));
            tokio::spawn(read(out, reply_tx, link));
        }
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                // A send or a dropped `Worker` both mean: stop it now.
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            on_exit(status.is_ok_and(|s| s.success()));
        });
        let worker = Self {
            serial,
            started: Instant::now(),
            stdin: Some(stdin),
            kill: Some(kill_tx),
        };
        worker.send(open);
        Ok((worker, reply_rx))
    }

    /// Write one line. A worker that already died cannot take it, and its
    /// exit is reported through `on_exit`, so a write error needs no answer.
    fn send(&self, line: &str) {
        if let Some(stdin) = &self.stdin {
            let _ = stdin.send(format!("{line}\n"));
        }
    }

    /// Ask the worker to end `session`, then close its stdin: a worker that
    /// misses the line still ends the session on EOF (T66.18).
    pub(crate) async fn end(&mut self, session: &str) {
        self.send(&end_line(session));
        self.stdin = None;
    }

    /// SIGKILL on Unix, which a stuck worker cannot ignore.
    pub(crate) fn kill(&mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
    }
}

/// The `open` request; `resume` reopens a session from its rollout.
pub(crate) fn open_line(cwd: &Path, resume: Option<&str>, theme: &str) -> String {
    request(json!({"method": "open", "params": {"cwd": cwd, "resume": resume, "theme": theme}}))
}

fn end_line(session: &str) -> String {
    request(json!({"method": "end", "params": {"session": session}}))
}

fn request(call: Value) -> String {
    json!({"type": "request", "v": VERSION, "id": OWN_ID, "call": call}).to_string()
}

/// Writes each queued line; ends, closing the pipe, once every strong sender
/// is gone or the worker stops reading.
async fn write(mut pipe: ChildStdin, mut queued: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = queued.recv().await {
        if pipe.write_all(line.as_bytes()).await.is_err() || pipe.flush().await.is_err() {
            return;
        }
    }
}

/// Where a worker's lines go once its session is known.
type Link = (Arc<Routes>, u64, mpsc::WeakUnboundedSender<String>);

/// Read stdout to its end. Everything after the `open` reply is the router's
/// to forward (S7); without a router it is dropped, so a full pipe never
/// blocks the worker.
async fn read(out: ChildStdout, reply: oneshot::Sender<Opened>, link: Option<Link>) {
    let mut lines = BufReader::new(out).lines();
    let mut reply = Some(reply);
    let mut session: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if let (Some(id), Some((routes, ..))) = (&session, &link) {
            routes.deliver(id, &line).await;
        } else if reply.is_some()
            && let Some(opened) = opened(&line)
            && let Some(tx) = reply.take()
        {
            // Linked before the reply is told, so the router can reach the
            // session as soon as the supervisor's `open` returns.
            if let (Ok(id), Some((routes, serial, stdin))) = (&opened, &link) {
                routes.link(id, *serial, stdin.clone());
                session = Some(id.clone());
            }
            let _ = tx.send(opened);
        }
    }
    if let (Some(id), Some((routes, serial, _))) = (session, link) {
        routes.unlink(&id, serial).await;
    }
}

/// The session id or error text of the response to the supervisor's call,
/// or `None` for any other line.
fn opened(line: &str) -> Option<Opened> {
    let line: Value = serde_json::from_str(line).ok()?;
    if line["type"] != "response" || line["id"] != OWN_ID {
        return None;
    }
    let outcome = &line["outcome"];
    let text = |v: &Value| v.as_str().map(str::to_owned);
    Some(match outcome["status"].as_str() {
        Some("ok") => text(&outcome["value"]["value"]["session"])
            .ok_or_else(|| "the worker's reply names no session".to_owned()),
        _ => Err(text(&outcome["value"]).unwrap_or_else(|| "the worker refused".to_owned())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_end_lines_match_the_wire() {
        let line: Value =
            serde_json::from_str(&open_line(Path::new("/w"), Some("s1"), "dark")).expect("json");
        assert_eq!(line["type"], "request");
        assert_eq!(line["call"]["method"], "open");
        assert_eq!(line["call"]["params"]["resume"], "s1");
        let line: Value = serde_json::from_str(&end_line("s1")).expect("json");
        assert_eq!(
            line["call"],
            json!({"method": "end", "params": {"session": "s1"}})
        );
    }

    #[test]
    fn only_the_reply_to_the_supervisor_counts() {
        let ok = r#"{"type":"response","v":1,"id":1,"outcome":{"status":"ok","value":{"kind":"opened","value":{"session":"s1","blocks":[],"warnings":[]}}}}"#;
        assert_eq!(opened(ok), Some(Ok("s1".to_owned())));
        let err = r#"{"type":"response","v":1,"id":1,"outcome":{"status":"err","value":"locked"}}"#;
        assert_eq!(opened(err), Some(Err("locked".to_owned())));
        assert_eq!(opened(&ok.replace("\"id\":1", "\"id\":7")), None);
        assert_eq!(opened("not json"), None);
    }
}
