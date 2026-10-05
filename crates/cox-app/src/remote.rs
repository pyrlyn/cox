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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::types::TodoItem;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, oneshot};

use crate::app::{App, Host};
use crate::coalesce;
use crate::wire::{self, Call, Line, Outcome, Reply, Request, ServerEvent, WireError};
use crate::{Block, Changes, Completion, Intent, Project, SearchHit, SessionEntry, TimelinePatch};

/// The system ssh; a fixed path, never one found through `PATH`.
pub const SSH: &str = "/usr/bin/ssh";

/// What ssh may still see of this process's environment: enough to find
/// the person's ssh config (`HOME`), name them and reach their local agent
/// for authentication. `ForwardAgent=no` keeps that agent off the remote.
const KEPT_ENV: [&str; 5] = ["HOME", "USER", "LOGNAME", "PATH", "SSH_AUTH_SOCK"];

/// What a remote call can fail with.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("{0:?} is not an ssh host name")]
    BadHost(String),
    #[error("could not start ssh: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the connection to {0} dropped; reconnect to go on")]
    Disconnected(String),
    /// The remote cox's own error, as it worded it.
    #[error("{0}")]
    Remote(String),
    #[error("the remote cox answered another call")]
    Unexpected,
    #[error(transparent)]
    Wire(#[from] WireError),
}

/// Refuses a host ssh could take for an option (`-oProxyCommand=…`) or
/// split into several words.
pub fn check_host(host: &str) -> Result<(), RemoteError> {
    let bad = host.is_empty()
        || host.starts_with('-')
        || host.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad {
        return Err(RemoteError::BadHost(host.to_owned()));
    }
    Ok(())
}

/// The ssh invocation for `host`, not yet started.
pub fn ssh_command(ssh: &Path, host: &str) -> Result<tokio::process::Command, RemoteError> {
    check_host(host)?;
    let mut command = tokio::process::Command::new(ssh);
    command
        .args(["-T", "-o", "BatchMode=yes", "-o", "ForwardAgent=no"])
        .args(["-o", "ClearAllForwardings=yes", "--", host])
        .args(["cox", "app-server", "--stdio"])
        .env_clear()
        .envs(
            KEPT_ENV
                .iter()
                .filter_map(|k| Some((*k, std::env::var_os(*k)?))),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
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
    writer: tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Outcome>>>,
    streams: Mutex<HashMap<SessionId, Arc<Stream>>>,
    connected: AtomicBool,
    /// Killed when the link goes (`kill_on_drop`).
    _child: Mutex<Option<tokio::process::Child>>,
}

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
            writer: tokio::sync::Mutex::new(Box::new(writer)),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            connected: AtomicBool::new(true),
            _child: Mutex::new(child),
        });
        tokio::spawn(read(Arc::clone(&link), reader, local));
        link
    }

    async fn call(&self, call: Call) -> Result<Reply, RemoteError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
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
    }

    /// The connection is gone: every waiting call fails and every stream
    /// ends, so each session shows as disconnected.
    fn drop_all(&self) {
        self.connected.store(false, Ordering::Release);
        lock(&self.pending).clear();
        for (_, stream) in lock(&self.streams).drain() {
            stream.close();
        }
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

/// Routes each line the remote writes; ends the link at EOF or an error.
async fn read(link: Arc<Link>, reader: impl AsyncRead + Unpin, local: Arc<dyn Host>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        // A line that does not parse is skipped, not fatal: the stream
        // stays in step because every line is whole.
        let Ok(line) = wire::decode(&text) else {
            continue;
        };
        match line {
            Line::Response(response) => {
                if let Some(tx) = lock(&link.pending).remove(&response.id) {
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

/// One host's workspace over ssh.
pub struct RemoteWorkspace {
    host: String,
    ssh: PathBuf,
    local: Arc<dyn Host>,
    link: Mutex<Arc<Link>>,
}

impl RemoteWorkspace {
    /// Connects to `host` with the system ssh.
    pub async fn connect(host: &str, local: Arc<dyn Host>) -> Result<Arc<Self>, RemoteError> {
        Self::connect_with(Path::new(SSH), host, local).await
    }

    /// Connects through `ssh`: the system one, or a test's fake.
    pub async fn connect_with(
        ssh: &Path,
        host: &str,
        local: Arc<dyn Host>,
    ) -> Result<Arc<Self>, RemoteError> {
        let link = spawn(ssh, host, Arc::clone(&local))?;
        Ok(Arc::new(Self {
            host: host.to_owned(),
            ssh: ssh.to_owned(),
            local,
            link: Mutex::new(link),
        }))
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
        let link = spawn(&self.ssh, &self.host, Arc::clone(&self.local))?;
        *lock(&self.link) = link;
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

fn spawn(ssh: &Path, host: &str, local: Arc<dyn Host>) -> Result<Arc<Link>, RemoteError> {
    let mut child = ssh_command(ssh, host)?.spawn()?;
    let missing = || std::io::Error::other("ssh started without its pipes");
    let stdin = child.stdin.take().ok_or_else(missing)?;
    let stdout = child.stdout.take().ok_or_else(missing)?;
    Ok(Link::start(host, stdout, stdin, Some(child), local))
}

/// A session on the remote machine: the calls a `LiveSession` answers,
/// each a request over the link.
pub struct RemoteSession {
    id: SessionId,
    link: Arc<Link>,
    opened: Vec<Block>,
    warnings: Vec<String>,
    patches: Arc<Stream>,
}

impl RemoteSession {
    fn new(link: Arc<Link>, id: SessionId, opened: Vec<Block>, warnings: Vec<String>) -> Arc<Self> {
        let patches = link.take_stream(id);
        Arc::new(Self {
            id,
            link,
            opened,
            warnings,
            patches,
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
        RemoteWorkspace::connect(host, Arc::clone(&self.host)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_host_with_leading_dash_is_refused() {
        for host in [
            "-oProxyCommand=touch /tmp/x",
            "-p22",
            "",
            "dev box",
            "dev\nbox",
            "a\u{7}",
        ] {
            assert!(
                matches!(check_host(host), Err(RemoteError::BadHost(_))),
                "{host:?} was accepted"
            );
            assert!(ssh_command(Path::new(SSH), host).is_err());
        }
        for host in ["devbox", "me@devbox.local", "devbox:2222", "[::1]"] {
            assert!(check_host(host).is_ok(), "{host:?} was refused");
        }
    }

    /// Remembers every link it was asked to open, and how.
    #[derive(Default)]
    struct Links {
        opened: Mutex<Vec<String>>,
        asked: Mutex<Vec<(String, String)>>,
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

    #[test]
    // why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
    #[allow(unsafe_code)]
    fn remote_spawn_forwards_no_agent_and_no_env() {
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-must-not-leave") };
        let command = ssh_command(Path::new(SSH), "devbox").expect("a valid host");
        let std = command.as_std();
        assert_eq!(std.get_program(), SSH);
        let args: Vec<_> = std
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ClearAllForwardings=yes",
                "--",
                "devbox",
                "cox",
                "app-server",
                "--stdio",
            ]
        );
        // `env_clear` leaves only what is set explicitly, and that is the
        // allow-list.
        for (key, _) in std.get_envs() {
            let key = key.to_string_lossy();
            assert!(KEPT_ENV.contains(&&*key), "{key} reaches ssh");
        }
        assert!(std.get_envs().all(|(k, _)| k != "ANTHROPIC_API_KEY"));
    }
}
