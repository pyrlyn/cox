// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A workspace on another machine (DT§4.4, T52.20): `ssh <host> cox
//! app-server --stdio` spoken over `wire`, with the same calls a local
//! `App` and `LiveSession` answer, so the desktop app drives a remote
//! session as it drives a local one. Separate from `server` because this is
//! the client end, and from `live` because nothing here runs a session.
//!
//! Trust: the host name comes from the person and is refused if ssh could
//! read it as an option; ssh runs non-interactively with agent and port
//! forwarding off and a scrubbed environment, so no local variable (a key
//! among them) reaches the remote side. Authentication is the person's own
//! ssh configuration and agent — the app never asks for a password. A link
//! the remote asks to show is a web link or nothing, and it opens only after
//! the person confirms it (`Host::confirm_open_url`).
//!
//! The first exchange is `hello` (T68.3). The launcher's stderr is the only
//! place ssh, a shell or a container runtime say why a link never came up,
//! so it is read, kept to its last 8 KiB and passed through
//! `cox_sanitize::sanitize` before any of it reaches an error: it is text
//! the other machine wrote and is shown to the person as is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::plugin::CanvasAttach;
use cox_protocol::types::TodoItem;
use cox_target::{Resolved, Target, Tty};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::app::{App, Host};
use crate::coalesce;
use crate::plugin_ui::{self, CanvasUpdate, LinkClick, LinkTarget, PluginCanvas, link_target};
use crate::wire::{self, Call, HostInfo, Line, Outcome, Reply, Request, ServerEvent, WireError};
use crate::{Block, Changes, Completion, Intent, Project, SearchHit, SessionEntry, TimelinePatch};

/// What a remote call can fail with.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error(transparent)]
    Target(#[from] cox_target::TargetError),
    #[error("could not start the launcher: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the connection to {0} dropped; reconnect to go on")]
    Disconnected(String),
    /// The launcher ran but `cox` was not found on the target.
    #[error(
        "cox is not on the PATH of {target}; install cox there so that a non-interactive login can run `cox` (see docs/app-server.md)"
    )]
    NotInstalled { target: String },
    /// The link closed before `hello` finished, for a reason the launcher
    /// gave on stderr (sanitized, last lines only).
    #[error("could not reach {target}: {detail}")]
    Unreachable { target: String, detail: String },
    #[error(
        "{target} speaks app-server protocol {theirs} but this app speaks {ours}; update {} so the two match",
        update_side(.ours, .theirs)
    )]
    WireVersion {
        target: String,
        ours: u32,
        theirs: u32,
    },
    /// An older cox answered `hello` with an error: it has no such call.
    #[error(
        "the cox on {target} is too old for this app (protocol {ours}); update cox on that machine (see docs/app-server.md)"
    )]
    RemoteTooOld { target: String, ours: u32 },
    /// The remote cox's own error, as it worded it.
    #[error("{0}")]
    Remote(String),
    #[error("could not read the config: {0}")]
    Config(String),
    #[error("the remote cox answered another call")]
    Unexpected,
    #[error(transparent)]
    Wire(#[from] WireError),
}

/// A session's patches, created by whichever comes first: the reader
/// seeing its first batch, or the caller that opened it. Batches fold
/// through `coalesce::push` as the local controller's queue does, so a UI
/// that stops pulling costs memory per changed block, not per batch the
/// remote sends.
struct Stream {
    queue: Mutex<Vec<TimelinePatch>>,
    /// Signalled whenever patches are queued or the stream ends.
    ready: Notify,
    closed: AtomicBool,
    /// A handle holds it; a second one gets a closed stream instead.
    taken: AtomicBool,
}

/// One ssh connection: the writer half, the calls waiting for an answer,
/// and each session's patch stream.
struct Link {
    host: String,
    /// This machine's host: where a remote session's links are confirmed.
    local: Arc<dyn Host>,
    writer: tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Outcome>>>,
    streams: Mutex<HashMap<SessionId, Arc<Stream>>>,
    /// Each session's canvas updates, for its first handle only, as a
    /// stream is.
    canvases: Mutex<HashMap<SessionId, mpsc::Sender<CanvasUpdate>>>,
    connected: AtomicBool,
    /// The protocol version of a line this end refused, 0 if none: the
    /// reason a link that never answered `hello` is a wire mismatch.
    refused_version: AtomicU32,
    /// The id of the `hello` still waiting for its answer, 0 when none: the
    /// one call an unpairable (id 0) error line may be an answer to.
    hello_id: AtomicU64,
    /// Killed when the link goes (`kill_on_drop`).
    child: Mutex<Option<tokio::process::Child>>,
    /// The launcher's stderr; `None` for a link that is not a process.
    stderr: Mutex<Option<Stderr>>,
}

/// A launcher's stderr as it is read: the last `STDERR_KEEP` bytes.
struct Stderr {
    tail: Arc<Mutex<Vec<u8>>>,
    task: JoinHandle<()>,
}

/// How much of the launcher's stderr is kept; older bytes are dropped.
const STDERR_KEEP: usize = 8 * 1024;

/// The longest detail an error carries, after the last few lines are cut.
const DETAIL_CHARS: usize = 600;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Link {
    /// Starts reading `reader` on the runtime; `local` hears the remote's
    /// inbox items and links.
    fn start(
        host: &str,
        reader: impl AsyncRead + Send + Unpin + 'static,
        writer: impl AsyncWrite + Send + Unpin + 'static,
        child: Option<tokio::process::Child>,
        local: Arc<dyn Host>,
    ) -> Arc<Self> {
        let link = Arc::new(Self {
            host: host.to_owned(),
            local: Arc::clone(&local),
            writer: tokio::sync::Mutex::new(Box::new(writer)),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            canvases: Mutex::new(HashMap::new()),
            connected: AtomicBool::new(true),
            refused_version: AtomicU32::new(0),
            hello_id: AtomicU64::new(0),
            child: Mutex::new(child),
            stderr: Mutex::new(None),
        });
        tokio::spawn(read(Arc::clone(&link), reader, local));
        link
    }

    async fn call(&self, call: Call) -> Result<Reply, RemoteError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let is_hello = matches!(call, Call::Hello);
        if is_hello {
            self.hello_id.store(id, Ordering::Release);
        }
        let result = self.exchange(id, call).await;
        if is_hello {
            self.hello_id.store(0, Ordering::Release);
        }
        result
    }

    async fn exchange(&self, id: u64, call: Call) -> Result<Reply, RemoteError> {
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if !self.connected.load(Ordering::Acquire) {
            lock(&self.pending).remove(&id);
            return Err(self.dropped());
        }
        let text = wire::encode(&Line::Request(Request {
            v: wire::VERSION,
            id,
            call,
        }))?;
        let written = {
            let mut writer = self.writer.lock().await;
            match writer.write_all(text.as_bytes()).await {
                Ok(()) => writer.flush().await,
                Err(e) => Err(e),
            }
        };
        if written.is_err() {
            lock(&self.pending).remove(&id);
            return Err(self.dropped());
        }
        match rx.await {
            Ok(Outcome::Ok(reply)) => Ok(reply),
            Ok(Outcome::Err(text)) => Err(RemoteError::Remote(text)),
            Err(_) => Err(self.dropped()),
        }
    }

    fn dropped(&self) -> RemoteError {
        RemoteError::Disconnected(self.host.clone())
    }

    /// Why a link that closed before `hello` finished is gone: a line of
    /// another protocol version, else what the launcher exited with and
    /// said on stderr.
    async fn launch_failure(&self) -> RemoteError {
        let theirs = self.refused_version.load(Ordering::Acquire);
        if theirs != 0 {
            return wire_mismatch(&self.host, theirs);
        }
        let (status, text) = self.reap(false).await;
        classify_failure(&self.host, status, &text)
    }

    /// `hello` got no answer in time: the process is killed, so a stuck
    /// remote does not outlive the attempt.
    async fn hello_timed_out(&self) -> RemoteError {
        let (_, text) = self.reap(true).await;
        let mut detail = format!("no answer within {} s", HELLO_TIMEOUT.as_secs());
        let tail = tail_detail(&text);
        if !tail.is_empty() {
            detail = format!("{detail}; {tail}");
        }
        RemoteError::Unreachable {
            target: self.host.clone(),
            detail,
        }
    }

    /// The process's exit status and the stderr it wrote, once it is gone.
    async fn reap(&self, kill: bool) -> (Option<i32>, String) {
        let child = lock(&self.child).take();
        let status = match child {
            Some(mut child) => {
                if kill {
                    let _ = child.start_kill();
                }
                match tokio::time::timeout(REAP, child.wait()).await {
                    Ok(Ok(status)) => status.code(),
                    _ => {
                        // Still up with its stdout closed: nothing more will come.
                        let _ = child.start_kill();
                        None
                    }
                }
            }
            None => None,
        };
        let stderr = lock(&self.stderr).take();
        let text = match stderr {
            Some(Stderr { tail, task }) => {
                // The pipe ends with the process; the bound only guards a
                // grandchild that kept it open.
                let _ = tokio::time::timeout(REAP, task).await;
                String::from_utf8_lossy(&lock(&tail)).into_owned()
            }
            None => String::new(),
        };
        (status, text)
    }

    /// `session`'s stream, once.
    fn take_stream(&self, session: SessionId) -> Arc<Stream> {
        let mut streams = lock(&self.streams);
        let stream = streams.entry(session).or_insert_with(Stream::new);
        if stream.taken.swap(true, Ordering::AcqRel) {
            // Taken already: a second handle on the session gets an empty,
            // closed stream instead of stealing the first one's batches.
            let closed = Stream::new();
            closed.close();
            return closed;
        }
        Arc::clone(stream)
    }

    fn patches(&self, session: SessionId, patches: Vec<TimelinePatch>) {
        let stream = {
            let mut streams = lock(&self.streams);
            Arc::clone(streams.entry(session).or_insert_with(Stream::new))
        };
        stream.push(patches);
    }

    fn end(&self, session: SessionId) {
        if let Some(stream) = lock(&self.streams).remove(&session) {
            stream.close();
        }
        lock(&self.canvases).remove(&session);
    }

    /// Capped again here, since the remote's plugin drew it. The read loop
    /// must not wait, so a full queue drops the update; showing the pane
    /// again sends every frame.
    fn canvas(&self, session: SessionId, update: CanvasUpdate) {
        if let Some(tx) = lock(&self.canvases).get(&session) {
            let _ = tx.try_send(update.capped());
        }
    }

    /// The connection is gone: every waiting call fails and every stream
    /// ends, so each session shows as disconnected.
    fn drop_all(&self) {
        self.connected.store(false, Ordering::Release);
        lock(&self.pending).clear();
        for (_, stream) in lock(&self.streams).drain() {
            stream.close();
        }
        lock(&self.canvases).clear();
    }
}

impl Stream {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Vec::new()),
            ready: Notify::new(),
            closed: AtomicBool::new(false),
            taken: AtomicBool::new(false),
        })
    }

    fn push(&self, patches: Vec<TimelinePatch>) {
        {
            let mut queue = lock(&self.queue);
            for patch in patches {
                coalesce::push(&mut queue, patch);
            }
        }
        self.ready.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.notify_one();
    }

    /// Everything queued, waiting until there is some; `None` once the
    /// stream ended and everything was pulled.
    async fn next(&self) -> Option<Vec<TimelinePatch>> {
        loop {
            // Created before the check: a `notify_one` in between leaves a
            // permit, so no wake-up is lost.
            let ready = self.ready.notified();
            {
                let mut queue = lock(&self.queue);
                if !queue.is_empty() {
                    return Some(std::mem::take(&mut *queue));
                }
                if self.closed.load(Ordering::Acquire) {
                    return None;
                }
            }
            ready.await;
        }
    }
}

/// How long `hello` may wait for its answer. A launcher can connect while the
/// remote never answers (a shell rc that waits for input, a stopped
/// container's proxy), and the app must not hang on it.
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a closed link waits for its process and its stderr.
const REAP: Duration = Duration::from_secs(2);

/// Routes each line the remote writes; ends the link at EOF or an error.
async fn read(link: Arc<Link>, reader: impl AsyncRead + Unpin, local: Arc<dyn Host>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        let line = match wire::decode(&text) {
            Ok(line) => line,
            // The other end speaks another protocol: nothing it says can be
            // trusted to mean what ours does, and the pending `hello` must
            // learn why instead of waiting.
            Err(WireError::Version { got }) => {
                link.refused_version.store(got, Ordering::Release);
                break;
            }
            // A line that does not parse is skipped, not fatal: the stream
            // stays in step because every line is whole.
            Err(_) => continue,
        };
        match line {
            Line::Response(response) => {
                // Id 0 is the server's answer to a line it could not read, so
                // it cannot be paired. Only a `hello` still waiting may take
                // it (an older cox does not know that call); after that a
                // remote could complete an unrelated call with it.
                let waiting = if response.id == 0 {
                    match link.hello_id.load(Ordering::Acquire) {
                        0 => None,
                        id => lock(&link.pending).remove(&id),
                    }
                } else {
                    lock(&link.pending).remove(&response.id)
                };
                if let Some(tx) = waiting {
                    let _ = tx.send(response.outcome);
                }
            }
            Line::Notification(note) => match note.event {
                ServerEvent::Patches { session, patches } => link.patches(session, patches),
                ServerEvent::Ended { session } => link.end(session),
                ServerEvent::Inbox { item, badge } => local.notify(*item, badge),
                // The badge counts this machine's sessions; a remote count
                // would overwrite it.
                ServerEvent::Badge { .. } => {}
                ServerEvent::PluginNotice { alert } => {
                    // Nothing else shows it: the remote left it to this host.
                    local.plugin_notice(*alert);
                }
                ServerEvent::PluginCanvas { session, update } => link.canvas(session, *update),
                ServerEvent::OpenUrl { url } => {
                    if let Some(url) = web_link(&url) {
                        local.confirm_open_url(&link.host, &url);
                    }
                }
            },
            Line::Request(_) => {}
        }
    }
    link.drop_all();
}

/// `text` as a normalized `http(s)` link with a host; anything else (a
/// `file:` path, another app's scheme) is dropped, since it would launch
/// something here rather than show a page.
fn web_link(text: &str) -> Option<String> {
    let url = url::Url::parse(text).ok()?;
    let web = matches!(url.scheme(), "http" | "https") && url.host().is_some();
    web.then(|| url.into())
}

/// One target's workspace: a host over ssh, a container or a VM.
pub struct RemoteWorkspace {
    host: String,
    resolved: Resolved,
    local: Arc<dyn Host>,
    link: Mutex<Arc<Link>>,
    /// What the last `hello` answered.
    info: Mutex<HostInfo>,
}

impl RemoteWorkspace {
    /// Connects to a resolved target (T68.4): `cox app-server --stdio` run
    /// through its launcher, in its starting directory if it has one.
    pub async fn connect(
        resolved: Resolved,
        local: Arc<dyn Host>,
    ) -> Result<Arc<Self>, RemoteError> {
        // An ssh host reads as its bare name, as before targets existed.
        let host = match &resolved.target {
            Target::Ssh { host } => host.clone(),
            other => other.to_string(),
        };
        let (link, info) = establish(&resolved, &host, Arc::clone(&local)).await?;
        Ok(Arc::new(Self {
            host,
            resolved,
            local,
            link: Mutex::new(link),
            info: Mutex::new(info),
        }))
    }

    /// What the host answered to `hello`: its cox version, OS, architecture
    /// and sandbox backend. A copy, because `reconnect` replaces it.
    pub fn info(&self) -> HostInfo {
        lock(&self.info).clone()
    }

    /// Connects to ssh `host` through `ssh`: the system one, or a test's fake.
    pub async fn connect_with(
        ssh: &Path,
        host: &str,
        local: Arc<dyn Host>,
    ) -> Result<Arc<Self>, RemoteError> {
        let target = Target::Ssh {
            host: host.to_owned(),
        };
        let resolved = Resolved {
            target,
            launcher: ssh.to_owned(),
            cwd: None,
        };
        Self::connect(resolved, local).await
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn is_connected(&self) -> bool {
        self.link().connected.load(Ordering::Acquire)
    }

    /// The disconnected state's action: a fresh ssh connection. Sessions
    /// are opened again (with `resume`) by the caller.
    pub async fn reconnect(&self) -> Result<(), RemoteError> {
        let (link, info) = establish(&self.resolved, &self.host, Arc::clone(&self.local)).await?;
        *lock(&self.link) = link;
        *lock(&self.info) = info;
        Ok(())
    }

    fn link(&self) -> Arc<Link> {
        Arc::clone(&lock(&self.link))
    }

    pub async fn projects(&self, limit: i64) -> Result<Vec<Project>, RemoteError> {
        match self.link().call(Call::Projects { limit }).await? {
            Reply::Projects(rows) => Ok(rows),
            _ => Err(RemoteError::Unexpected),
        }
    }

    pub async fn sessions(
        &self,
        project: PathBuf,
        limit: i64,
    ) -> Result<Vec<SessionEntry>, RemoteError> {
        match self.link().call(Call::Sessions { project, limit }).await? {
            Reply::Sessions(rows) => Ok(rows),
            _ => Err(RemoteError::Unexpected),
        }
    }

    pub async fn search(&self, query: String, limit: i64) -> Result<Vec<SearchHit>, RemoteError> {
        match self.link().call(Call::Search { query, limit }).await? {
            Reply::Search(rows) => Ok(rows),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// A session on the remote machine, in its `cwd`; `resume` reopens one.
    pub async fn open(
        &self,
        cwd: PathBuf,
        resume: Option<SessionId>,
        theme: String,
    ) -> Result<Arc<RemoteSession>, RemoteError> {
        let link = self.link();
        let call = Call::Open { cwd, resume, theme };
        match link.call(call).await? {
            Reply::Opened {
                session,
                blocks,
                warnings,
            } => Ok(RemoteSession::new(link, session, blocks, warnings)),
            _ => Err(RemoteError::Unexpected),
        }
    }
}

fn spawn(resolved: &Resolved, host: &str, local: Arc<dyn Host>) -> Result<Arc<Link>, RemoteError> {
    // why: the directory comes from config and was checked as a token when
    // the target resolved; `command` checks every token again.
    let cwd = resolved.cwd.as_deref().map(|dir| ["--cwd", dir]);
    let mut argv = vec!["cox"];
    argv.extend(cwd.iter().flatten());
    argv.extend(["app-server", "--stdio"]);
    let command = cox_target::command(&resolved.target, &resolved.launcher, &argv, Tty::No)?;
    let mut child = tokio::process::Command::from(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let missing = || std::io::Error::other("the launcher started without its pipes");
    let stdin = child.stdin.take().ok_or_else(missing)?;
    let stdout = child.stdout.take().ok_or_else(missing)?;
    let stderr = child.stderr.take().ok_or_else(missing)?;
    let link = Link::start(host, stdout, stdin, Some(child), local);
    let tail = Arc::new(Mutex::new(Vec::new()));
    let task = tokio::spawn(collect(stderr, Arc::clone(&tail)));
    *lock(&link.stderr) = Some(Stderr { tail, task });
    Ok(link)
}

/// A started link and the answer to its `hello`. A link that cannot say
/// hello is dropped, which kills its process.
async fn establish(
    resolved: &Resolved,
    host: &str,
    local: Arc<dyn Host>,
) -> Result<(Arc<Link>, HostInfo), RemoteError> {
    let link = spawn(resolved, host, local)?;
    let info = hello(&link).await?;
    Ok((link, info))
}

/// The first exchange on a link, refused unless the host speaks our wire.
async fn hello(link: &Link) -> Result<HostInfo, RemoteError> {
    let host = &link.host;
    let answer = match tokio::time::timeout(HELLO_TIMEOUT, link.call(Call::Hello)).await {
        Ok(answer) => answer,
        Err(_) => return Err(link.hello_timed_out().await),
    };
    let info = match answer {
        Ok(Reply::Hello(info)) => info,
        Ok(_) => return Err(RemoteError::Unexpected),
        // The server answered, but with an error: it does not know `hello`.
        Err(RemoteError::Remote(_)) => {
            return Err(RemoteError::RemoteTooOld {
                target: host.clone(),
                ours: wire::VERSION,
            });
        }
        Err(RemoteError::Disconnected(_)) => return Err(link.launch_failure().await),
        Err(e) => return Err(e),
    };
    if info.wire != wire::VERSION {
        return Err(wire_mismatch(host, info.wire));
    }
    Ok(info)
}

/// Which end is behind: the one with the lower protocol version.
fn update_side(ours: &u32, theirs: &u32) -> &'static str {
    if theirs > ours {
        "this app"
    } else {
        "cox on that machine"
    }
}

fn wire_mismatch(host: &str, theirs: u32) -> RemoteError {
    RemoteError::WireVersion {
        target: host.to_owned(),
        ours: wire::VERSION,
        theirs,
    }
}

/// Reads `stderr` to its end, keeping the last `STDERR_KEEP` bytes. It keeps
/// reading past that so the process never blocks on a full pipe.
async fn collect(mut stderr: impl AsyncRead + Unpin, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        keep_last(&mut lock(&tail), &chunk[..n], STDERR_KEEP);
    }
}

fn keep_last(kept: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    kept.extend_from_slice(chunk);
    if kept.len() > cap {
        kept.drain(..kept.len() - cap);
    }
}

/// The last few non-empty lines of `text`, sanitized and cut to
/// `DETAIL_CHARS`; empty when there are none.
fn tail_detail(text: &str) -> String {
    let clean = cox_sanitize::sanitize(text);
    let lines: Vec<&str> = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let detail = lines[lines.len().saturating_sub(4)..].join("; ");
    match detail
        .char_indices()
        .rev()
        .nth(DETAIL_CHARS.saturating_sub(1))
    {
        Some((start, _)) => format!("…{}", &detail[start..]),
        None => detail,
    }
}

/// The error for a link that closed before `hello` was answered. `text` is
/// the launcher's stderr, untrusted: it is sanitized and cut to its last
/// lines here, and nothing else puts it in an error.
fn classify_failure(host: &str, status: Option<i32>, text: &str) -> RemoteError {
    let clean = cox_sanitize::sanitize(text);
    let lower = clean.to_lowercase();
    let names_cox = lower.contains("cox");
    // The third phrase is how Windows OpenSSH's default shells word it:
    // cmd.exe ("'cox' is not recognized as an internal or external
    // command") and PowerShell ("the term 'cox' is not recognized as ...").
    let not_found = [
        "command not found",
        "executable file not found",
        "is not recognized as",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || lower.contains("no such file or directory");
    // 127 is the shell's "not found" status, which ssh and `docker exec`
    // pass through; 9009 is cmd.exe's.
    if matches!(status, Some(127 | 9009)) || (names_cox && not_found) {
        return RemoteError::NotInstalled {
            target: host.to_owned(),
        };
    }
    let mut detail = tail_detail(&clean);
    if detail.is_empty() {
        detail = match status {
            Some(code) => format!("the connection closed (exit status {code})"),
            None => "the connection closed".to_owned(),
        };
    }
    RemoteError::Unreachable {
        target: host.to_owned(),
        detail,
    }
}

/// A session on the remote machine: the calls a `LiveSession` answers,
/// each a request over the link.
pub struct RemoteSession {
    id: SessionId,
    link: Arc<Link>,
    opened: Vec<Block>,
    warnings: Vec<String>,
    patches: Arc<Stream>,
    canvas: tokio::sync::Mutex<mpsc::Receiver<CanvasUpdate>>,
}

impl RemoteSession {
    fn new(link: Arc<Link>, id: SessionId, opened: Vec<Block>, warnings: Vec<String>) -> Arc<Self> {
        let patches = link.take_stream(id);
        // A second handle's sender is dropped here, so its canvas is closed.
        let (tx, canvas) = mpsc::channel(plugin_ui::CANVAS_QUEUE);
        lock(&link.canvases).entry(id).or_insert(tx);
        Arc::new(Self {
            id,
            link,
            opened,
            warnings,
            patches,
            canvas: tokio::sync::Mutex::new(canvas),
        })
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The blocks the session opened with; `snapshot` asks for the latest.
    pub fn opened(&self) -> &[Block] {
        &self.opened
    }

    /// False once the connection dropped: the session shows disconnected
    /// until the workspace reconnects and it is opened again.
    pub fn is_connected(&self) -> bool {
        self.link.connected.load(Ordering::Acquire)
    }

    /// The next batch; `None` once closed or disconnected.
    pub async fn next_patches(&self) -> Option<Vec<TimelinePatch>> {
        self.patches.next().await
    }

    pub async fn snapshot(&self) -> Result<Vec<Block>, RemoteError> {
        match self.call(Call::Snapshot { session: self.id }).await? {
            Reply::Snapshot(blocks) => Ok(blocks),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// Returns at once for a turn; a fork or handoff returns its child.
    pub async fn send(&self, intent: Intent) -> Result<Option<Arc<Self>>, RemoteError> {
        let call = Call::Send {
            session: self.id,
            intent,
        };
        match self.call(call).await? {
            Reply::Sent { child: None } => Ok(None),
            Reply::Sent { child: Some(child) } => Ok(Some(Self::new(
                Arc::clone(&self.link),
                child,
                Vec::new(),
                Vec::new(),
            ))),
            _ => Err(RemoteError::Unexpected),
        }
    }

    pub async fn complete(
        &self,
        token: String,
        limit: u32,
    ) -> Result<Vec<Completion>, RemoteError> {
        let call = Call::Complete {
            session: self.id,
            token,
            limit,
        };
        match self.call(call).await? {
            Reply::Complete(rows) => Ok(rows),
            _ => Err(RemoteError::Unexpected),
        }
    }

    pub async fn changes(&self) -> Result<Changes, RemoteError> {
        match self.call(Call::Changes { session: self.id }).await? {
            Reply::Changes(changes) => Ok(changes),
            _ => Err(RemoteError::Unexpected),
        }
    }

    pub async fn plan(&self) -> Result<Vec<TodoItem>, RemoteError> {
        match self.call(Call::Plan { session: self.id }).await? {
            Reply::Plan(items) => Ok(items),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// A truncated output in full, as `cox expand` prints it.
    pub async fn output(&self, archive: ArchiveId) -> Result<String, RemoteError> {
        let call = Call::Expand {
            session: self.id,
            archive,
        };
        match self.call(call).await? {
            Reply::Expand(text) => Ok(text),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// A plugin button or action (`plugin:<id>:<command>`) clicked: the
    /// remote session runs the command, and says whether its plugins
    /// granted it.
    pub async fn plugin_action(&self, id: &str) -> Result<bool, RemoteError> {
        let call = Call::PluginAction {
            session: self.id,
            id: id.to_owned(),
        };
        match self.call(call).await? {
            Reply::Acted(done) => Ok(done),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// A click on a link in the remote plugin's output. That machine's
    /// plugin chose it, so a web link only asks this host to confirm
    /// (`Host::confirm_open_url`), and a path comes back as written, to be
    /// read from the remote session, never from this disk.
    pub fn open_link(&self, link: &str) -> LinkClick {
        match link_target(link) {
            Some(LinkTarget::Web(url)) => {
                self.link.local.confirm_open_url(&self.link.host, &url);
                LinkClick::Opened
            }
            Some(LinkTarget::File { path, line }) => LinkClick::File { path, line },
            None => LinkClick::Refused,
        }
    }

    /// The canvases the remote session's plugins declared, sanitized here.
    pub async fn canvases(&self) -> Result<Vec<PluginCanvas>, RemoteError> {
        match self.call(Call::Canvases { session: self.id }).await? {
            Reply::Canvases(all) => Ok(all.into_iter().filter_map(PluginCanvas::checked).collect()),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// The pane shows `plugin`'s canvas at this size; its frames arrive
    /// through [`Self::next_canvas`].
    pub async fn show_canvas(
        &self,
        plugin: &str,
        width_pt: u32,
        height_pt: u32,
    ) -> Result<(), RemoteError> {
        let call = Call::ShowCanvas {
            session: self.id,
            plugin: plugin.to_owned(),
            width_pt,
            height_pt,
        };
        self.shown(call).await
    }

    /// The pane hid `plugin`'s canvas.
    pub async fn hide_canvas(&self, plugin: &str) -> Result<(), RemoteError> {
        let call = Call::HideCanvas {
            session: self.id,
            plugin: plugin.to_owned(),
        };
        self.shown(call).await
    }

    async fn shown(&self, call: Call) -> Result<(), RemoteError> {
        match self.call(call).await? {
            Reply::Shown => Ok(()),
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// The next canvas update, capped on arrival; `None` once closed or
    /// disconnected.
    pub async fn next_canvas(&self) -> Option<CanvasUpdate> {
        self.canvas.lock().await.recv().await
    }

    /// A click on a frame: the attachment for the composer, capped and
    /// sanitized again here.
    pub async fn select_canvas(
        &self,
        plugin: &str,
        frame: &str,
        value: &str,
    ) -> Result<Option<CanvasAttach>, RemoteError> {
        let call = Call::CanvasSelect {
            session: self.id,
            plugin: plugin.to_owned(),
            frame: frame.to_owned(),
            value: value.to_owned(),
        };
        match self.call(call).await? {
            Reply::CanvasAttach(attach) => {
                Ok(attach.and_then(|a| a.capped().ok()).map(plugin_ui::chip))
            }
            _ => Err(RemoteError::Unexpected),
        }
    }

    /// Stops the stream; the remote session keeps running.
    pub async fn close(&self) -> Result<(), RemoteError> {
        match self.call(Call::Close { session: self.id }).await? {
            Reply::Closed => Ok(()),
            _ => Err(RemoteError::Unexpected),
        }
    }

    async fn call(&self, call: Call) -> Result<Reply, RemoteError> {
        self.link.call(call).await
    }
}

impl App {
    /// A remote host's workspace over ssh (T52.20); its inbox items and
    /// links reach this app's host.
    pub async fn connect_remote(&self, host: &str) -> Result<Arc<RemoteWorkspace>, RemoteError> {
        self.connect_target(&format!("ssh:{host}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Remembers every link it was asked to open, and how.
    #[derive(Default)]
    struct Links {
        opened: Mutex<Vec<String>>,
        asked: Mutex<Vec<(String, String)>>,
        alerts: Mutex<Vec<crate::PluginAlert>>,
    }

    impl Host for Links {
        fn notify(&self, _: crate::InboxItem, _: u32) {}
        fn badge(&self, _: u32) {}
        fn open_url(&self, url: &str) {
            lock(&self.opened).push(url.to_owned());
        }
        fn secret(&self, _: &str) -> Option<String> {
            None
        }
        fn confirm_open_url(&self, origin: &str, url: &str) {
            lock(&self.asked).push((origin.to_owned(), url.to_owned()));
        }
        fn plugin_notice(&self, alert: crate::PluginAlert) -> bool {
            lock(&self.alerts).push(alert);
            true
        }
    }

    #[tokio::test]
    async fn a_remote_plugin_notice_reaches_the_local_host_and_a_link_click_asks_first() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let links = Arc::new(Links::default());
        let local = Arc::clone(&links) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let id = SessionId::new();
        let alert = crate::PluginAlert {
            session: id,
            plugin: "clock".into(),
            warn: false,
            title: "Break".into(),
            body: String::new(),
            actions: vec![crate::AlertAction {
                label: "Snooze".into(),
                id: "plugin:clock:snooze".into(),
            }],
        };
        let line = Line::Notification(wire::Notification {
            v: wire::VERSION,
            event: ServerEvent::PluginNotice {
                alert: Box::new(alert.clone()),
            },
        });
        server
            .write_all(wire::encode(&line).expect("encodes").as_bytes())
            .await
            .expect("write");
        drop(server);
        until_dropped(&link).await;
        assert_eq!(*lock(&links.alerts), [alert]);

        let session = RemoteSession::new(Arc::clone(&link), id, Vec::new(), Vec::new());
        assert_eq!(
            session.open_link("https://example.com/a"),
            LinkClick::Opened
        );
        assert_eq!(
            session.open_link("src/lib.rs:3"),
            LinkClick::File {
                path: "src/lib.rs".into(),
                line: Some(3)
            }
        );
        for bad in ["http://example.com", "file:///etc/passwd"] {
            assert_eq!(session.open_link(bad), LinkClick::Refused, "{bad}");
        }
        assert!(lock(&links.opened).is_empty(), "opened without asking");
        assert_eq!(
            *lock(&links.asked),
            [("devbox".to_owned(), "https://example.com/a".to_owned())]
        );
    }

    #[tokio::test]
    async fn remote_link_opens_only_after_the_person_confirms() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let links = Arc::new(Links::default());
        let local = Arc::clone(&links) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        for url in [
            "https://example.com/login",
            "file:///etc/passwd",
            "x-apple.systempreferences:",
            "http://",
        ] {
            let line = Line::Notification(wire::Notification {
                v: wire::VERSION,
                event: ServerEvent::OpenUrl { url: url.into() },
            });
            let text = wire::encode(&line).expect("encodes");
            server.write_all(text.as_bytes()).await.expect("write");
        }
        // EOF: the link reads every line, then drops.
        drop(server);
        until_dropped(&link).await;
        assert!(lock(&links.opened).is_empty(), "opened without asking");
        assert_eq!(
            *lock(&links.asked),
            [("devbox".to_owned(), "https://example.com/login".to_owned())]
        );
    }

    fn append(block: &str, text: &str) -> TimelinePatch {
        TimelinePatch::AppendText {
            id: crate::patch::BlockId(block.into()),
            text: text.into(),
        }
    }

    fn patches_line(session: SessionId, patches: Vec<TimelinePatch>) -> String {
        let line = Line::Notification(wire::Notification {
            v: wire::VERSION,
            event: ServerEvent::Patches { session, patches },
        });
        wire::encode(&line).expect("encodes")
    }

    /// Waits until the link has read everything the fake remote wrote.
    async fn until_dropped(link: &Link) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while link.connected.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the link ends at EOF");
    }

    #[tokio::test]
    async fn remote_patches_fold_while_the_ui_does_not_pull() {
        let (client, mut server) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let session = SessionId::new();
        let stream = link.take_stream(session);
        for n in 0..500 {
            let text = patches_line(session, vec![append("a", &n.to_string())]);
            server.write_all(text.as_bytes()).await.expect("write");
        }
        let text = patches_line(session, vec![append("b", "x")]);
        server.write_all(text.as_bytes()).await.expect("write");
        drop(server);
        until_dropped(&link).await;
        // 501 batches from a stalled UI's point of view: one patch per block.
        assert_eq!(lock(&stream.queue).len(), 2);
        let all: String = (0..500).map(|n| n.to_string()).collect();
        assert_eq!(
            stream.next().await,
            Some(vec![append("a", &all), append("b", "x")])
        );
        // The connection dropped, so the stream ends once drained.
        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn a_second_handle_on_a_remote_session_gets_a_closed_stream() {
        let (client, _server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let session = SessionId::new();
        let first = link.take_stream(session);
        let second = link.take_stream(session);
        link.patches(session, vec![append("a", "1")]);
        assert_eq!(second.next().await, None);
        assert_eq!(first.next().await, Some(vec![append("a", "1")]));
    }
    fn frame(id: &str, title: &str) -> cox_protocol::plugin::Frame {
        cox_protocol::plugin::Frame {
            id: id.into(),
            title: title.into(),
            width_pt: 90_000,
            html: "<p>page</p>".into(),
        }
    }

    /// plugin-canvas §7: the client caps a server's canvas update again.
    #[tokio::test]
    async fn an_over_size_remote_canvas_is_capped_by_the_client() {
        let (client, mut server) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let id = SessionId::new();
        let session = RemoteSession::new(Arc::clone(&link), id, Vec::new(), Vec::new());
        let mut frames: Vec<_> = (0..14).map(|n| frame(&format!("f{n}"), "Page")).collect();
        frames[0].title = "Home\u{1b}[2J".into();
        frames.push(frame("Bad!", "x"));
        let mut order: Vec<String> = frames.iter().map(|f| f.id.clone()).collect();
        order.push("f0".into());
        let update = CanvasUpdate {
            plugin: "weft\u{7}".into(),
            order,
            frames,
            stopped: false,
        };
        for session in [id, SessionId::new()] {
            let line = Line::Notification(wire::Notification {
                v: wire::VERSION,
                event: ServerEvent::PluginCanvas {
                    session,
                    update: Box::new(update.clone()),
                },
            });
            let text = wire::encode(&line).expect("encodes");
            server.write_all(text.as_bytes()).await.expect("write");
        }
        drop(server);
        until_dropped(&link).await;
        let got = session.next_canvas().await.expect("the session's update");
        assert_eq!(got.plugin, "weft");
        assert_eq!(got.frames.len(), cox_protocol::plugin::MAX_CANVAS_FRAMES);
        assert_eq!(got.frames[0].title, "Home");
        assert_eq!(got.frames[0].width_pt, 2560, "the width is clamped");
        assert_eq!(got.order.len(), cox_protocol::plugin::MAX_CANVAS_FRAMES);
        assert!(!got.order.iter().any(|id| id == "Bad!"));
        assert_eq!(
            session.next_canvas().await,
            None,
            "another session's update is not ours"
        );
    }

    /// §5, §7: a remote canvas declaration and attachment are checked again.
    #[tokio::test]
    async fn remote_canvas_replies_are_checked_again() {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let session = RemoteSession::new(link, SessionId::new(), Vec::new(), Vec::new());
        let canvas = |select_attr: &str| PluginCanvas {
            plugin: "weft".into(),
            title: "Screens".into(),
            symbol: "rectangle.on.rectangle".into(),
            select_attr: select_attr.into(),
        };
        let declared = vec![canvas("data-weft-path"), canvas("onclick")];
        let mut attaches = vec![
            CanvasAttach {
                name: "too big".into(),
                text: "x".repeat(cox_protocol::plugin::MAX_ATTACH_TEXT + 1),
            },
            CanvasAttach {
                name: "hero\u{1b}[31m".into(),
                text: "node hero".into(),
            },
        ];
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(text)) = lines.next_line().await {
                let Ok(Line::Request(request)) = wire::decode(&text) else {
                    continue;
                };
                let reply = match request.call {
                    Call::Canvases { .. } => Reply::Canvases(declared.clone()),
                    _ => Reply::CanvasAttach(attaches.pop()),
                };
                let line = Line::Response(wire::Response {
                    v: wire::VERSION,
                    id: request.id,
                    outcome: Outcome::Ok(reply),
                });
                let text = wire::encode(&line).expect("encodes");
                write.write_all(text.as_bytes()).await.expect("write");
            }
        });
        let shown = session.canvases().await.expect("canvases");
        assert_eq!(
            shown,
            [canvas("data-weft-path")],
            "a bad attribute is dropped"
        );
        let chip = session.select_canvas("weft", "home", "hero").await;
        assert_eq!(
            chip.expect("a reply"),
            Some(CanvasAttach {
                name: "hero".into(),
                text: "node hero".into(),
            })
        );
        let big = session.select_canvas("weft", "home", "hero").await;
        assert_eq!(
            big.expect("a reply"),
            None,
            "an over-size attachment is dropped"
        );
    }

    fn hello_info(wire: u32) -> HostInfo {
        HostInfo {
            wire,
            cox: "9.9.9".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            sandbox: None,
        }
    }

    /// A fake remote that reads the client's one request and writes
    /// `answer` back; returns the link to its client end.
    async fn link_to_server_that_says(answer: String) -> Arc<Link> {
        let (client, server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut lines = BufReader::new(read).lines();
            let _ = lines.next_line().await;
            let _ = write.write_all(answer.as_bytes()).await;
            // The server stays up: only its answer can end the call.
            std::future::pending::<()>().await;
        });
        link
    }

    fn hello_line(v: u32, wire: u32) -> String {
        let line = Line::Response(wire::Response {
            v,
            id: 1,
            outcome: Outcome::Ok(Reply::Hello(hello_info(wire))),
        });
        // `encode` always stamps ours; the other version is the point here.
        wire::encode(&line)
            .expect("encodes")
            .replace("\"v\":1", &format!("\"v\":{v}"))
    }

    #[tokio::test]
    async fn old_wire_version_is_refused_by_name() {
        let link = link_to_server_that_says(hello_line(wire::VERSION, 2)).await;
        let err = hello(&link).await.expect_err("refused");
        assert!(
            matches!(
                err,
                RemoteError::WireVersion {
                    ours: 1,
                    theirs: 2,
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "devbox speaks app-server protocol 2 but this app speaks 1; update this app so the two match"
        );

        // The other direction names the other side.
        let link = link_to_server_that_says(hello_line(wire::VERSION, 0)).await;
        let err = hello(&link).await.expect_err("refused");
        assert!(
            err.to_string()
                .ends_with("update cox on that machine so the two match")
        );

        // A server whose every line carries another envelope version never
        // gets as far as `hello`'s answer; the line itself is the evidence.
        let link = link_to_server_that_says(hello_line(2, 2)).await;
        let err = hello(&link).await.expect_err("refused");
        assert!(
            matches!(
                err,
                RemoteError::WireVersion {
                    ours: 1,
                    theirs: 2,
                    ..
                }
            ),
            "{err:?}"
        );

        // An older cox without `hello` answers an unpairable error line.
        let refusal = Line::Response(wire::Response {
            v: wire::VERSION,
            id: 0,
            outcome: Outcome::Err("not a protocol line: unknown variant `hello`".into()),
        });
        let link = link_to_server_that_says(wire::encode(&refusal).expect("encodes")).await;
        let err = hello(&link).await.expect_err("refused");
        assert!(matches!(err, RemoteError::RemoteTooOld { .. }), "{err:?}");
        assert!(err.to_string().contains("update cox on that machine"));
    }

    #[tokio::test]
    async fn a_matching_hello_keeps_the_hosts_answer() {
        let link = link_to_server_that_says(hello_line(wire::VERSION, wire::VERSION)).await;
        assert_eq!(
            hello(&link).await.expect("hello"),
            hello_info(wire::VERSION)
        );
    }

    /// A launcher that ignores its arguments and runs `body`.
    #[cfg(unix)]
    fn launcher(dir: &Path, body: &str) -> PathBuf {
        named_launcher(dir, "ssh", body)
    }

    #[cfg(unix)]
    fn named_launcher(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the launcher");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// T68.4: a configured docker target runs `docker exec -i web cox …`
    /// through the configured launcher, and the host's hello is the answer.
    #[cfg(unix)]
    #[tokio::test]
    async fn connect_reaches_a_docker_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = dir.path().join("args");
        let docker = named_launcher(
            dir.path(),
            "docker",
            &format!(
                "echo \"$@\" > {args:?}\nread request\nprintf '%s\\n' '{}'\ncat >/dev/null",
                hello_line(wire::VERSION, wire::VERSION)
            ),
        );
        let config = format!(
            "[launchers]\ndocker = {docker:?}\n\n[targets.web]\ntarget = \"docker:web\"\ncwd = \"/srv/app\"\n"
        );
        std::fs::write(dir.path().join("config.toml"), config).expect("write the config");
        let app = App::new(Some(dir.path().to_owned()), Arc::new(Links::default())).expect("app");
        let workspace = app.connect_target("web").await.expect("connects");
        assert_eq!(workspace.host(), "docker:web");
        assert_eq!(workspace.info(), hello_info(wire::VERSION));
        assert_eq!(
            std::fs::read_to_string(&args)
                .expect("recorded args")
                .trim(),
            "exec -i web cox --cwd /srv/app app-server --stdio"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_remote_cox_says_how_to_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ssh = launcher(
            dir.path(),
            "echo 'sh: cox: command not found' >&2\nexit 127",
        );
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let err = RemoteWorkspace::connect_with(&ssh, "devbox", local)
            .await
            .err()
            .expect("no cox there");
        assert!(matches!(err, RemoteError::NotInstalled { .. }), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("devbox"), "{text}");
        assert!(text.contains("not on the PATH"), "{text}");
        assert!(text.contains("docs/app-server.md"), "{text}");
    }

    #[test]
    fn launcher_failures_name_cox_only_when_it_is_the_cause() {
        let not_installed = |status, text| {
            matches!(
                classify_failure("devbox", status, text),
                RemoteError::NotInstalled { .. }
            )
        };
        assert!(not_installed(Some(127), ""));
        assert!(not_installed(Some(9009), ""));
        assert!(not_installed(
            Some(1),
            "'cox' is not recognized as an internal or external command,\r\noperable program or batch file."
        ));
        assert!(not_installed(
            Some(1),
            "cox : The term 'cox' is not recognized as the name of a cmdlet"
        ));
        assert!(!not_installed(
            Some(1),
            "'git' is not recognized as an internal or external command"
        ));
        assert!(not_installed(None, "bash: cox: command not found"));
        assert!(not_installed(
            Some(126),
            "exec: \"cox\": executable file not found in $PATH"
        ));
        assert!(not_installed(
            Some(1),
            "/usr/bin/env: 'cox': No such file or directory"
        ));
        assert!(!not_installed(
            Some(255),
            "ssh: Could not resolve hostname devbox: Name or service not known"
        ));
        assert!(!not_installed(Some(1), "bash: git: command not found"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_stderr_is_sanitized_and_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ssh = launcher(
            dir.path(),
            "head -c 20000 /dev/zero | tr '\\0' 'x' >&2\n\
             printf '\\n\\033]0;pwned\\007\\342\\200\\256reversed \\033[2Jlast words\\n' >&2\n\
             exit 1",
        );
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let err = RemoteWorkspace::connect_with(&ssh, "devbox", local)
            .await
            .err()
            .expect("the launcher fails");
        let RemoteError::Unreachable { target, detail } = &err else {
            panic!("not unreachable: {err:?}");
        };
        assert_eq!(target, "devbox");
        assert!(detail.contains("last words"), "{detail}");
        assert!(
            detail.chars().count() <= DETAIL_CHARS + 1,
            "{}",
            detail.len()
        );
        assert!(
            !detail.chars().any(|c| c.is_control() && c != '\n'),
            "{detail:?}"
        );
        assert!(!detail.contains('\u{202e}'), "{detail:?}");
        assert!(!err.to_string().contains('\u{1b}'));
    }

    #[test]
    fn stderr_tail_keeps_only_the_last_bytes() {
        let mut kept = Vec::new();
        keep_last(&mut kept, b"abcdef", 4);
        keep_last(&mut kept, b"gh", 4);
        assert_eq!(kept, b"efgh");
    }

    #[tokio::test]
    async fn id_zero_after_hello_completes_no_call() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let (tx, rx) = oneshot::channel();
        lock(&link.pending).insert(7, tx);
        // No hello is waiting, so the unpairable error is dropped; lines are
        // read in order, so a wrongly paired call would see it first.
        let garbage = Line::Response(wire::Response {
            v: wire::VERSION,
            id: 0,
            outcome: Outcome::Err("not for you".into()),
        });
        let answer = Line::Response(wire::Response {
            v: wire::VERSION,
            id: 7,
            outcome: Outcome::Ok(Reply::Closed),
        });
        for line in [garbage, answer] {
            let text = wire::encode(&line).expect("encodes");
            server.write_all(text.as_bytes()).await.expect("write");
        }
        assert_eq!(rx.await.expect("answered"), Outcome::Ok(Reply::Closed));
    }

    #[tokio::test(start_paused = true)]
    async fn hello_without_an_answer_times_out() {
        let (client, _server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let local = Arc::new(Links::default()) as Arc<dyn Host>;
        let link = Link::start("devbox", reader, writer, None, local);
        let err = hello(&link).await.expect_err("no answer");
        let RemoteError::Unreachable { target, detail } = &err else {
            panic!("not unreachable: {err:?}");
        };
        assert_eq!(target, "devbox");
        assert_eq!(detail, "no answer within 30 s");
    }
}
