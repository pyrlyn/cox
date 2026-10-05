// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The ACP client's terminals (T35.11, EA§4): the commands an external agent
//! asks cox to run with `terminal/create`, and their output, exit and kill.
//! Separate from `client.rs`, which judges each command and routes the
//! requests, because this is the only part of the client that owns running
//! processes.
//!
//! No spawn path of its own: a command runs through `bash`'s runner
//! (`cox_tools::bash::run_line`) under the sandbox policy that already
//! governs the agent's process. Dropping `Terminals` — the connection ending
//! with the session — stops every command still running.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CreateTerminalRequest, TerminalExitStatus, TerminalId, TerminalOutputResponse,
};
use cox_protocol::types::SandboxPolicy;
use cox_tools::bash::{Exit, run_line};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

/// The most output a terminal keeps when the agent names no smaller
/// `outputByteLimit`: a chatty command cannot grow cox's memory without end.
const MAX_OUTPUT: usize = 1 << 20;
/// A terminal the agent never kills still ends; the session end kills it sooner.
const MAX_RUN: Duration = Duration::from_secs(30 * 60);

/// The command line a `terminal/create` names. `command` is taken as a
/// shell fragment, as editors run it, and each `args` entry is quoted so it
/// stays one word.
pub(crate) fn command_line(req: &CreateTerminalRequest) -> String {
    let mut line = req.command.clone();
    for arg in &req.args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

fn quote(arg: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Output kept so far: the tail within the limit, and whether anything
/// before it was dropped.
struct Tail {
    text: String,
    limit: usize,
    truncated: bool,
}

impl Tail {
    fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        let over = self.text.len().saturating_sub(self.limit);
        if over > 0 {
            // ACP wants the cut on a character boundary.
            let cut = (over..=self.text.len())
                .find(|&i| self.text.is_char_boundary(i))
                .unwrap_or(self.text.len());
            self.text.drain(..cut);
            self.truncated = true;
        }
    }
}

struct Terminal {
    tail: Arc<Mutex<Tail>>,
    exit: watch::Receiver<Option<TerminalExitStatus>>,
    stop: CancellationToken,
    /// Stops the command when the terminal is released or the map dropped.
    _guard: DropGuard,
}

/// Every live terminal of one ACP connection.
#[derive(Default)]
pub(crate) struct Terminals {
    next: AtomicU64,
    live: Mutex<HashMap<TerminalId, Terminal>>,
}

impl Terminals {
    /// Starts `line`, already judged by the engine, and returns its id at once.
    pub(crate) fn create(
        &self,
        line: String,
        cwd: PathBuf,
        roots: Vec<PathBuf>,
        policy: SandboxPolicy,
        limit: Option<u64>,
    ) -> TerminalId {
        let limit = limit.map_or(MAX_OUTPUT, |l| (l as usize).min(MAX_OUTPUT));
        let tail = Arc::new(Mutex::new(Tail {
            text: String::new(),
            limit,
            truncated: false,
        }));
        let (done, exit) = watch::channel(None);
        let stop = CancellationToken::new();
        let (sink, cancel) = (tail.clone(), stop.clone());
        tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel::<String>(64);
            let runner = async move {
                let exit = run_line(&line, &cwd, &roots, &policy, &cancel, &tx, MAX_RUN).await;
                drop(tx);
                exit
            };
            let drain = async {
                while let Some(chunk) = rx.recv().await {
                    if let Ok(mut t) = sink.lock() {
                        t.push(&chunk);
                    }
                }
            };
            let (exit, ()) = tokio::join!(runner, drain);
            let status = match exit {
                Ok(exit) => exit_status(exit),
                Err(e) => {
                    if let Ok(mut t) = sink.lock() {
                        t.push(&format!("cox could not start the command: {e}"));
                    }
                    TerminalExitStatus::new()
                }
            };
            let _ = done.send(Some(status));
        });
        let id = TerminalId::new(format!("cox-{}", self.next.fetch_add(1, Ordering::Relaxed)));
        let terminal = Terminal {
            tail,
            exit,
            _guard: stop.clone().drop_guard(),
            stop,
        };
        if let Ok(mut live) = self.live.lock() {
            live.insert(id.clone(), terminal);
        }
        id
    }

    /// The output kept so far, sanitized because the agent may show it.
    pub(crate) fn output(&self, id: &TerminalId) -> Option<TerminalOutputResponse> {
        self.with(id, |t| {
            let tail = t.tail.lock().ok()?;
            let text = cox_sanitize::sanitize(&tail.text);
            let exit = t.exit.borrow().clone();
            Some(TerminalOutputResponse::new(text, tail.truncated).exit_status(exit))
        })
        .flatten()
    }

    /// Waits for the command to end, off the lock so `kill` can still reach it.
    pub(crate) async fn wait(&self, id: &TerminalId) -> Option<TerminalExitStatus> {
        let mut exit = self.with(id, |t| t.exit.clone())?;
        exit.wait_for(Option::is_some).await.ok()?.clone()
    }

    /// Stops the command; the terminal stays for `output` and `wait`.
    pub(crate) fn kill(&self, id: &TerminalId) -> Option<()> {
        self.with(id, |t| t.stop.cancel())
    }

    /// Stops the command and forgets the terminal.
    pub(crate) fn release(&self, id: &TerminalId) -> Option<()> {
        self.live.lock().ok()?.remove(id).map(drop)
    }

    fn with<T>(&self, id: &TerminalId, f: impl FnOnce(&Terminal) -> T) -> Option<T> {
        self.live.lock().ok()?.get(id).map(f)
    }
}

/// A stopped command reports the signal the runner stops it with; its group
/// is SIGKILLed after that either way.
fn exit_status(exit: Exit) -> TerminalExitStatus {
    let status = TerminalExitStatus::new();
    match exit.stopped {
        Some(_) => status.signal("SIGTERM".to_owned()),
        None => status.exit_code(exit.code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::SessionId;

    #[test]
    fn args_stay_one_word_each_and_command_stays_a_fragment() {
        let req = CreateTerminalRequest::new(SessionId::new("s"), "git log").args(vec![
            "-n".into(),
            "it's two".into(),
            String::new(),
        ]);
        assert_eq!(command_line(&req), r"git log -n 'it'\''s two' ''");
    }

    #[test]
    fn tail_keeps_the_end_on_a_char_boundary() {
        let mut tail = Tail {
            text: String::new(),
            limit: 3,
            truncated: false,
        };
        tail.push("aé");
        assert!(!tail.truncated);
        // Five bytes over a limit of three: the cut would split `é`, so it
        // moves past it.
        tail.push("bc");
        assert_eq!(tail.text, "bc");
        assert!(tail.truncated);
    }
}
