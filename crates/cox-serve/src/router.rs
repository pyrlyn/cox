// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The router behind `cox serve` (T66.22, `docs/design/serve.md` S7, D1, D4,
//! D6): every connection on the supervisor's socket speaks the app-server
//! wire. Workspace calls are answered here through the injected
//! [`Workspace`] (the supervisor's own `App`, which opens SQLite and nothing
//! else); a call that names a session goes to that session's worker under a
//! request id the router owns, and the worker's answer goes back under the
//! client's. One client is attached to a session at a time, and only it gets
//! the session's notifications. Separate from `supervisor` because it is the
//! socket's side of the trust boundary: `admit_peer` runs before a
//! connection's first line, and every line is decoded and refused as the
//! app-server would. The supervisor owns no `Session` and claims no session
//! lock; it only spawns workers, which do both.
//!
//! The lines are `serde_json` values rather than `cox-app`'s `wire` types:
//! the router reads only the envelope (`type`, `v`, `id`, `method`,
//! `session`), the worker decodes the whole call again, and linking the app
//! core here would put it inside the trust boundary (`deps.rs`).

// Only the socket drives the connection side, and it is unix-only; the
// worker side (`Routes::link`, `deliver`) builds everywhere.
#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::supervisor::SuperviseError;
use crate::worker::{OWN_ID, VERSION};

/// Lines waiting for one client's writer, as the app-server's own queue: a
/// stalled client fills it, then its sessions' workers wait and their
/// controllers coalesce.
const OUT_QUEUE: usize = 64;

/// Answered by the supervisor's own `App`; they need no worker.
const WORKSPACE: [&str; 3] = ["projects", "sessions", "search"];

/// The supervisor's read-only view of the workspace. T66.23 implements it
/// over `cox_app`'s `App`, which this crate may not link.
pub trait Workspace: Send + Sync {
    /// Answers `call` (`{"method", "params"}`, one of `projects`, `sessions`
    /// or `search`) with its reply (`{"kind", "value"}`) or an error text.
    fn call(&self, call: &Value) -> Result<Value, String>;
}

/// A decoded request.
#[derive(Debug, PartialEq)]
enum Call {
    Workspace(Value),
    Inbox,
    /// `cwd`, `resume`, `theme`.
    Open(PathBuf, Option<String>, String),
    /// The session, whether it is `attach`, and the call to forward.
    Session(String, bool, Value),
}

/// An `open` for the run loop, which owns the supervisor.
pub(crate) type Open = (
    PathBuf,
    Option<String>,
    String,
    oneshot::Sender<Result<String, SuperviseError>>,
);

/// A client's call waiting for its worker. `attach` marks a call whose
/// success takes the session's stream over.
struct Caller {
    client: u64,
    id: u64,
    attach: bool,
    done: oneshot::Sender<()>,
}

enum Waiter {
    /// Its response is written to the client under the client's id.
    Client(Caller),
    /// The router's own call; it gets the outcome.
    Own(oneshot::Sender<Value>),
}

#[derive(Default)]
struct Table {
    /// Each session's current worker: its serial and a weak stdin handle.
    workers: HashMap<String, (u64, mpsc::WeakUnboundedSender<String>)>,
    clients: HashMap<u64, mpsc::Sender<String>>,
    /// Each session's client, and what wakes a forward stuck on that client
    /// once another takes the stream over.
    attached: HashMap<String, (u64, Arc<Notify>)>,
    /// Per session, in request order: a worker answers in order, so a
    /// response it could not pair (id 0) belongs to the oldest.
    pending: HashMap<String, BTreeMap<u64, Waiter>>,
    next: u64,
}

impl Table {
    /// A fresh client or request id, above the supervisor's own request id
    /// so its replies are never paired with a client.
    fn next(&mut self) -> u64 {
        self.next += 1;
        OWN_ID + self.next
    }
}

/// What every connection and every worker's reader share.
#[derive(Default)]
pub(crate) struct Routes {
    table: Mutex<Table>,
}

impl Routes {
    fn table(&self) -> MutexGuard<'_, Table> {
        // A panic elsewhere leaves the maps consistent: each edit is one
        // insert or remove.
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `session`'s worker is up; a restart links its new process here.
    pub(crate) fn link(
        &self,
        session: &str,
        serial: u64,
        stdin: mpsc::WeakUnboundedSender<String>,
    ) {
        let mut table = self.table();
        table.workers.insert(session.to_owned(), (serial, stdin));
    }

    /// `session`'s worker `serial` closed its stdout: its callers get an
    /// error and its client `ended`, since nothing will answer them.
    pub(crate) async fn unlink(&self, session: &str, serial: u64) {
        let (waiters, attached) = {
            let mut table = self.table();
            if table.workers.get(session).is_none_or(|(s, _)| *s != serial) {
                return;
            }
            table.workers.remove(session);
            let waiters = table.pending.remove(session).unwrap_or_default();
            (waiters, table.attached.remove(session))
        };
        for waiter in waiters.into_values() {
            if let Waiter::Client(caller) = waiter {
                let gone = Err("the session's worker exited".to_owned());
                self.send(caller.client, response(caller.id, gone)).await;
                let _ = caller.done.send(());
            }
        }
        if let Some((client, _)) = attached {
            self.send(client, ended(session)).await;
        }
    }

    /// One line from `session`'s worker after its `open` reply.
    pub(crate) async fn deliver(&self, session: &str, text: &str) {
        let Ok(line) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match line["type"].as_str() {
            Some("response") => self.respond(session, line).await,
            Some("notification") => {
                let is_end = line["event"]["event"] == "ended";
                self.notify(session, text, is_end).await;
            }
            _ => {}
        }
    }

    async fn respond(&self, session: &str, mut line: Value) {
        let waiter = {
            let mut table = self.table();
            let pending = table.pending.entry(session.to_owned()).or_default();
            match line["id"].as_u64() {
                Some(0) => pending.pop_first().map(|(_, waiter)| waiter),
                id => id.and_then(|id| pending.remove(&id)),
            }
        };
        match waiter {
            Some(Waiter::Client(caller)) => {
                if caller.attach && line["outcome"]["status"] == "ok" {
                    self.take_over(session, caller.client);
                }
                line["id"] = json!(caller.id);
                self.send(caller.client, format!("{line}\n")).await;
                let _ = caller.done.send(());
            }
            Some(Waiter::Own(tx)) => {
                let _ = tx.send(line["outcome"].take());
            }
            None => {}
        }
    }

    /// `client` now gets `session`'s stream; the one it replaces is told
    /// `ended`, so two people never answer one approval (D6).
    fn take_over(&self, session: &str, client: u64) {
        let mut table = self.table();
        let fresh = (client, Arc::new(Notify::new()));
        let Some((previous, moved)) = table.attached.insert(session.to_owned(), fresh) else {
            return;
        };
        moved.notify_waiters();
        if previous != client
            && let Some(queue) = table.clients.get(&previous).cloned()
        {
            // Off this reader: a stalled previous client must not hold up
            // the stream it just lost.
            let line = ended(session);
            tokio::spawn(async move { queue.send(line).await });
        }
    }

    async fn notify(&self, session: &str, text: &str, is_end: bool) {
        // Nobody attached: the controller keeps folding, and `attach` and
        // `inbox` answer from it later.
        let Some((queue, moved)) = self.attached(session) else {
            return;
        };
        let notified = moved.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Checked again once `notified` listens, so a takeover in between
        // is not missed.
        let same =
            |now: Option<(_, Arc<Notify>)>| now.is_some_and(|(_, m)| Arc::ptr_eq(&m, &moved));
        if !same(self.attached(session)) {
            return;
        }
        tokio::select! {
            _ = queue.send(format!("{text}\n")) => {}
            () = notified => return,
        }
        // Under one lock, so a takeover that lands meanwhile is kept.
        let mut table = self.table();
        if is_end && (table.attached.get(session)).is_some_and(|(_, m)| Arc::ptr_eq(m, &moved)) {
            table.attached.remove(session);
        }
    }

    fn attached(&self, session: &str) -> Option<(mpsc::Sender<String>, Arc<Notify>)> {
        let table = self.table();
        let (client, moved) = table.attached.get(session)?;
        Some((table.clients.get(client)?.clone(), Arc::clone(moved)))
    }

    /// Sends `call` to `session`'s worker under a fresh id, `waiter` first
    /// registered so the answer cannot outrun it.
    fn call(&self, session: &str, call: Value, waiter: Waiter) -> Result<(), String> {
        let mut table = self.table();
        let stdin = table.workers.get(session).and_then(|(_, w)| w.upgrade());
        let Some(stdin) = stdin else {
            return Err(format!("session {session} is not open in cox serve"));
        };
        let id = table.next();
        let pending = table.pending.entry(session.to_owned()).or_default();
        pending.insert(id, waiter);
        let line = json!({"type": "request", "v": VERSION, "id": id, "call": call});
        if stdin.send(format!("{line}\n")).is_err() {
            pending.remove(&id);
            return Err(format!("session {session}'s worker is gone"));
        }
        Ok(())
    }

    async fn send(&self, client: u64, line: String) {
        let queue = self.table().clients.get(&client).cloned();
        if let Some(queue) = queue {
            let _ = queue.send(line).await;
        }
    }

    fn add_client(&self, queue: mpsc::Sender<String>) -> u64 {
        let mut table = self.table();
        let client = table.next();
        table.clients.insert(client, queue);
        client
    }

    /// The client is gone: each session it had attached is detached with
    /// `close`, which stops the worker's pump and keeps its controller
    /// folding, so approvals wait in the worker's inbox (D6).
    fn drop_client(&self, client: u64) {
        let mine: Vec<String> = {
            let mut table = self.table();
            table.clients.remove(&client);
            let mine = (table.attached.iter())
                .filter(|(_, (c, _))| *c == client)
                .map(|(s, _)| s.clone())
                .collect();
            table.attached.retain(|_, (c, _)| *c != client);
            mine
        };
        for session in mine {
            let close = json!({"method": "close", "params": {"session": session}});
            let _ = self.call(&session, close, Waiter::Own(oneshot::channel().0));
        }
    }

    /// Every worker's inbox in one list: each holds only its own session's.
    async fn inbox(&self) -> Value {
        let sessions: Vec<String> = self.table().workers.keys().cloned().collect();
        let mut items = Vec::new();
        for session in sessions {
            let (tx, rx) = oneshot::channel();
            let asked = self.call(&session, json!({"method": "inbox"}), Waiter::Own(tx));
            if asked.is_ok()
                && let Ok(outcome) = rx.await
                && let Some(list) = outcome["value"]["value"].as_array()
            {
                items.extend(list.iter().cloned());
            }
        }
        json!({"kind": "inbox", "value": items})
    }
}

/// Serves `bound` until `shutdown` resolves, then shuts the workers down.
/// Returns how many needed SIGKILL.
#[cfg(unix)]
pub async fn serve(
    options: crate::supervisor::Options,
    workspace: Arc<dyn Workspace>,
    bound: crate::socket::Bound,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<usize> {
    use crate::socket::admit_peer;
    use crate::supervisor::Supervisor;

    bound.listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(bound.listener)?;
    let routes = Arc::new(Routes::default());
    let mut supervisor = Supervisor::new(options);
    supervisor.route_through(Arc::clone(&routes));
    let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                // Before the first line is read: a peer of another uid
                // never reaches the decoder (D4).
                Ok((stream, _)) if admit_peer(&stream, bound.uid) => {
                    let (routes, workspace) = (Arc::clone(&routes), Arc::clone(&workspace));
                    connections.spawn(connection(stream, routes, workspace, opens.clone()));
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "could not accept a connection"),
            },
            // Opens run one at a time here, beside the supervisor's events,
            // because the supervisor has one owner.
            Some((cwd, resume, theme, reply)) = opened.recv() => {
                let _ = reply.send(supervisor.open(cwd, resume, theme).await);
            }
            event = supervisor.next_event() => supervisor.handle(event).await,
        }
    }
    // The clients' writers end with the workers' readers, which hold the
    // last references to their queues.
    connections.abort_all();
    Ok(supervisor.shutdown().await)
}

/// One client until its stream closes. Calls run one at a time, as the
/// app-server runs them, so a response is written before the next line is
/// read.
async fn connection<S>(
    stream: S,
    routes: Arc<Routes>,
    workspace: Arc<dyn Workspace>,
    opens: mpsc::UnboundedSender<Open>,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (input, mut output) = tokio::io::split(stream);
    let (queue, mut queued) = mpsc::channel::<String>(OUT_QUEUE);
    let client = routes.add_client(queue.clone());
    let writer = tokio::spawn(async move {
        while let Some(line) = queued.recv().await {
            if output.write_all(line.as_bytes()).await.is_err() || output.flush().await.is_err() {
                return;
            }
        }
        let _ = output.shutdown().await;
    });
    let mut lines = BufReader::new(input).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        if text.trim().is_empty() {
            continue;
        }
        // A refused line gets id 0, as the app-server answers it.
        let answer = match decode(&text) {
            Ok((id, call)) => route(&routes, &*workspace, &opens, client, id, call).await,
            Err(refusal) => Some(response(0, Err(refusal))),
        };
        if let Some(line) = answer
            && queue.send(line).await.is_err()
        {
            break;
        }
    }
    routes.drop_client(client);
    drop(queue);
    let _ = writer.await;
}

/// The response line to write, or `None` when the worker's reader wrote it.
async fn route(
    routes: &Routes,
    workspace: &dyn Workspace,
    opens: &mpsc::UnboundedSender<Open>,
    client: u64,
    id: u64,
    call: Call,
) -> Option<String> {
    let (session, attach, call) = match call {
        Call::Workspace(call) => return Some(response(id, workspace.call(&call))),
        Call::Inbox => return Some(response(id, Ok(routes.inbox().await))),
        // The supervisor returns a live session's own id instead of a second
        // worker, so `open` of a held session attaches to it (F1, D6).
        Call::Open(cwd, resume, theme) => {
            let (reply, opened) = oneshot::channel();
            let _ = opens.send((cwd, resume, theme, reply));
            let session = match opened.await {
                Ok(Ok(session)) => session,
                Ok(Err(error)) => return Some(response(id, Err(error.to_string()))),
                Err(_) => return Some(response(id, Err("cox serve is shutting down".into()))),
            };
            let call = json!({"method": "attach", "params": {"session": session}});
            (session, true, call)
        }
        Call::Session(session, attach, call) => (session, attach, call),
    };
    let (done, answered) = oneshot::channel();
    let caller = Caller {
        client,
        id,
        attach,
        done,
    };
    if let Err(error) = routes.call(&session, call, Waiter::Client(caller)) {
        return Some(response(id, Err(error)));
    }
    let _ = answered.await;
    None
}

/// Reads one client line as the app-server's `wire::decode` would, with the
/// same refusals, and checks the fields the router routes by. Any other
/// method must name its session; its worker decodes the rest and refuses
/// what it does not know.
fn decode(text: &str) -> Result<(u64, Call), String> {
    let bad = |what: &str| format!("not a protocol line: {what}");
    let line: Value = serde_json::from_str(text.trim_end()).map_err(|e| bad(&e.to_string()))?;
    match line["v"].as_u64() {
        Some(v) if v == u64::from(VERSION) => {}
        Some(got) => return Err(format!("protocol version {got}, expected {VERSION}")),
        None => return Err(bad("no version")),
    }
    match line["type"].as_str() {
        Some("request") => {}
        Some("response" | "notification") => return Err("the server reads only requests".into()),
        _ => return Err(bad("unknown type")),
    }
    let id = line["id"].as_u64().ok_or_else(|| bad("no id"))?;
    let call = &line["call"];
    let method = call["method"].as_str().ok_or_else(|| bad("no method"))?;
    let params = &call["params"];
    let text = |key: &str| params[key].as_str().map(str::to_owned);
    let call = match method {
        m if WORKSPACE.contains(&m) => Call::Workspace(call.clone()),
        "inbox" => Call::Inbox,
        "open" => {
            let cwd = text("cwd").ok_or_else(|| bad("open needs a cwd"))?;
            let theme = text("theme").ok_or_else(|| bad("open needs a theme"))?;
            let resume = &params["resume"];
            let id = resume.as_str().map(str::to_owned);
            if id.is_none() && !resume.is_null() {
                return Err(bad("resume is not a session id"));
            }
            Call::Open(cwd.into(), id, theme)
        }
        m => {
            let named = text("session").ok_or_else(|| bad(&format!("`{m}` names no session")));
            Call::Session(named?, m == "attach", call.clone())
        }
    };
    Ok((id, call))
}

fn response(id: u64, outcome: Result<Value, String>) -> String {
    let outcome = match outcome {
        Ok(reply) => json!({"status": "ok", "value": reply}),
        Err(error) => json!({"status": "err", "value": error}),
    };
    let line = json!({"type": "response", "v": VERSION, "id": id, "outcome": outcome});
    format!("{line}\n")
}

fn ended(session: &str) -> String {
    let event = json!({"event": "ended", "data": {"session": session}});
    let line = json!({"type": "notification", "v": VERSION, "event": event});
    format!("{line}\n")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{BufReader, Lines};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use crate::supervisor::Options;

    /// Stands in for `cox serve --worker`, as `supervisor`'s fake does: it
    /// logs every line and answers by method under the request's own id.
    /// `attach` is answered like `open` and followed by one batch of patches,
    /// as the worker's new pump would send it.
    const FAKE: &str = r#"log="$1"
s="s$$"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s\n' "$line" | sed -n 's/^{"type":"request","v":1,"id":\([0-9][0-9]*\).*/\1/p')
  reply() { printf '{"type":"response","v":1,"id":%s,"outcome":{"status":"ok","value":%s}}\n' "$id" "$1"; }
  opened="{\"kind\":\"opened\",\"value\":{\"session\":\"$s\",\"blocks\":[],\"warnings\":[]}}"
  case "$line" in
    *'"method":"open"'*) reply "$opened" ;;
    *'"method":"attach"'*)
      reply "$opened"
      printf '{"type":"notification","v":1,"event":{"event":"patches","data":{"session":"%s","patches":[]}}}\n' "$s" ;;
    *'"method":"snapshot"'*) reply '{"kind":"snapshot","value":[]}' ;;
    *'"method":"inbox"'*) reply "{\"kind\":\"inbox\",\"value\":[{\"session\":\"$s\"}]}" ;;
    *'"method":"end"'*) exit 0 ;;
  esac
done
"#;

    const WAIT: Duration = Duration::from_secs(5);

    /// Answers `projects` only, so a test sees the supervisor's `App` reply.
    struct Projects;

    impl Workspace for Projects {
        fn call(&self, call: &Value) -> Result<Value, String> {
            match call["method"].as_str() {
                Some("projects") => Ok(json!({"kind": "projects", "value": [{"root": "/w"}]})),
                _ => Err("not here".into()),
            }
        }
    }

    struct Serve {
        dir: tempfile::TempDir,
        stop: Option<oneshot::Sender<()>>,
        task: JoinHandle<std::io::Result<usize>>,
    }

    impl Serve {
        fn start() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("worker.sh"), FAKE).expect("fake worker");
            let bound = crate::socket::bind(&dir.path().join("home")).expect("bind");
            // Through `/bin/sh`, so no freshly written file is executed.
            let mut options = Options::worker("/bin/sh".into());
            options.args = vec![
                dir.path().join("worker.sh").into(),
                dir.path().join("log").into(),
            ];
            options.grace = Duration::from_millis(300);
            let (stop, stopped) = oneshot::channel::<()>();
            let shutdown = async move {
                let _ = stopped.await;
            };
            let task = tokio::spawn(serve(options, Arc::new(Projects), bound, shutdown));
            Self {
                dir,
                stop: Some(stop),
                task,
            }
        }

        async fn client(&self) -> Client {
            let path = crate::socket::path(&self.dir.path().join("home"));
            let stream = UnixStream::connect(path).await.expect("connect");
            let (read, write) = stream.into_split();
            Client {
                lines: BufReader::new(read).lines(),
                write,
            }
        }

        fn log(&self) -> Vec<String> {
            let log = std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default();
            log.lines().map(str::to_owned).collect()
        }

        async fn stop(mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            let served = timeout(WAIT, self.task).await.expect("in time");
            assert_eq!(served.expect("joined").expect("served"), 0);
        }
    }

    struct Client {
        lines: Lines<BufReader<OwnedReadHalf>>,
        write: OwnedWriteHalf,
    }

    impl Client {
        async fn line(&mut self, text: &str) {
            let line = format!("{text}\n");
            self.write.write_all(line.as_bytes()).await.expect("write");
        }

        async fn call(&mut self, id: u64, call: Value) {
            let line = json!({"type": "request", "v": 1, "id": id, "call": call});
            self.line(&line.to_string()).await;
        }

        async fn next(&mut self) -> Value {
            let line = timeout(WAIT, self.lines.next_line())
                .await
                .expect("in time");
            let line = line.expect("read").expect("a line");
            serde_json::from_str(&line).expect("json")
        }

        /// The response to `id`, past any notification.
        async fn response(&mut self, id: u64) -> Value {
            loop {
                let line = self.next().await;
                if line["type"] == "response" {
                    assert_eq!(line["id"], id, "{line}");
                    return line["outcome"].clone();
                }
            }
        }

        /// The next notification of `event`, past any other.
        async fn event(&mut self, event: &str) -> Value {
            loop {
                let line = self.next().await;
                if line["type"] == "notification" && line["event"]["event"] == event {
                    return line["event"]["data"].clone();
                }
            }
        }

        async fn open(&mut self, id: u64, cwd: &std::path::Path, resume: Option<&str>) -> String {
            let params = json!({"cwd": cwd, "resume": resume, "theme": "dark"});
            self.call(id, json!({"method": "open", "params": params}))
                .await;
            let outcome = self.response(id).await;
            assert_eq!(outcome["status"], "ok", "{outcome}");
            let session = outcome["value"]["value"]["session"].as_str();
            session.expect("a session").to_owned()
        }
    }

    fn count(log: &[String], method: &str) -> usize {
        let method = format!(r#""method":"{method}""#);
        log.iter().filter(|l| l.contains(&method)).count()
    }

    #[tokio::test]
    async fn workspace_calls_need_no_worker() {
        let serve = Serve::start();
        let mut client = serve.client().await;
        client
            .call(7, json!({"method": "projects", "params": {"limit": 5}}))
            .await;
        let outcome = client.response(7).await;
        assert_eq!(outcome["value"]["kind"], "projects", "{outcome}");
        assert_eq!(outcome["value"]["value"][0]["root"], "/w");

        // Refused as the app-server refuses it: id 0 and the reason.
        client.line("not json").await;
        let outcome = client.response(0).await;
        assert_eq!(outcome["status"], "err");
        assert!(
            outcome["value"]
                .as_str()
                .is_some_and(|e| e.starts_with("not a protocol line"))
        );
        client
            .line(r#"{"type":"request","v":2,"id":3,"call":{"method":"inbox"}}"#)
            .await;
        let outcome = client.response(0).await;
        assert_eq!(outcome["value"], "protocol version 2, expected 1");

        assert!(serve.log().is_empty(), "no worker started");
        serve.stop().await;
    }

    #[tokio::test]
    async fn session_calls_reach_their_worker() {
        let serve = Serve::start();
        let mut client = serve.client().await;
        let session = client.open(1, serve.dir.path(), None).await;
        // The client's id 9 and the worker's own ids never meet.
        let snapshot = json!({"method": "snapshot", "params": {"session": session}});
        client.call(9, snapshot).await;
        let outcome = client.response(9).await;
        assert_eq!(outcome["value"]["kind"], "snapshot", "{outcome}");

        let unknown = json!({"method": "snapshot", "params": {"session": "nobody"}});
        client.call(10, unknown).await;
        let outcome = client.response(10).await;
        assert_eq!(outcome["status"], "err", "{outcome}");

        let log = serve.log();
        assert_eq!(count(&log, "snapshot"), 1, "{log:?}");
        assert!(!log.iter().any(|l| l.contains(r#""id":9,"#)), "{log:?}");
        serve.stop().await;
    }

    #[tokio::test]
    async fn a_second_attach_ends_the_first_stream() {
        let serve = Serve::start();
        let (mut first, mut second) = (serve.client().await, serve.client().await);
        let session = first.open(1, serve.dir.path(), None).await;
        assert_eq!(first.event("patches").await["session"], session);

        second
            .call(
                1,
                json!({"method": "attach", "params": {"session": session}}),
            )
            .await;
        let outcome = second.response(1).await;
        assert_eq!(outcome["value"]["value"]["session"], session, "{outcome}");
        assert_eq!(first.event("ended").await["session"], session);
        assert_eq!(second.event("patches").await["session"], session);

        // What waited while nobody was attached is in the worker's inbox.
        second.call(2, json!({"method": "inbox"})).await;
        let outcome = second.response(2).await;
        assert_eq!(
            outcome["value"]["value"][0]["session"], session,
            "{outcome}"
        );
        serve.stop().await;
    }

    #[tokio::test]
    async fn open_resume_of_a_live_worker_attaches() {
        let serve = Serve::start();
        let (mut first, mut second) = (serve.client().await, serve.client().await);
        let session = first.open(1, serve.dir.path(), None).await;
        let resumed = second.open(1, serve.dir.path(), Some(&session)).await;
        assert_eq!(resumed, session);
        assert_eq!(first.event("ended").await["session"], session);

        let log = serve.log();
        assert_eq!(count(&log, "open"), 1, "one core: {log:?}");
        assert_eq!(count(&log, "attach"), 2, "{log:?}");
        serve.stop().await;
    }

    #[test]
    fn decode_reads_the_routing_fields() {
        let line = r#"{"type":"request","v":1,"id":4,"call":{"method":"close","params":{"session":"s1"}}}"#;
        let (id, call) = decode(line).expect("decoded");
        assert_eq!(id, 4);
        assert!(matches!(call, Call::Session(session, false, _) if session == "s1"));
        let reply = r#"{"type":"response","v":1,"id":4,"outcome":{"status":"ok","value":null}}"#;
        assert_eq!(
            decode(reply).err().as_deref(),
            Some("the server reads only requests")
        );
        let nameless = r#"{"type":"request","v":1,"id":4,"call":{"method":"send","params":{}}}"#;
        assert!(decode(nameless).is_err());
        let unknown = r#"{"type":"request","v":1,"id":4,"call":{"method":"rm","params":{}}}"#;
        assert!(decode(unknown).is_err());
    }
}
