// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox app-server --stdio` (DT§4.4, T52.19): serves the `wire` protocol
//! to one client over a reader and a writer. Requests call this process's
//! `App` and `Workspace`; each open session's patches stream as
//! notifications, pulled with the same coalescing as the FFI, so a stalled
//! client costs memory per changed block and never delays a turn (DT§4.5).
//! Separate from `wire` so a client links the shapes without this loop.
//!
//! Trust: the server's `Host` never answers a secret — keys come from this
//! machine's own env or keyring — and a URL or inbox item only becomes a
//! notification the client decides about. Nothing is written to stdout but
//! protocol lines; logs go to cox's log file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cox_protocol::ids::SessionId;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::InboxItem;
use crate::app::{App, AppError, Host};
use crate::live::LiveSession;
use crate::wire::{self, Call, Line, Notification, Outcome, Reply, Response, ServerEvent};

/// Lines waiting for the writer. A stalled client fills it; then each
/// session's pump waits and its controller coalesces instead.
const OUT_QUEUE: usize = 64;

/// What serving can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("the client's stream: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    App(#[from] AppError),
}

/// The server's `Host`: platform calls become notifications. The inbox is
/// rare enough for an unbounded queue; the patch stream is what can be
/// large, and it is bounded.
pub struct ServerHost {
    events: mpsc::UnboundedSender<ServerEvent>,
}

impl ServerHost {
    /// The host and the events it pushes, for `serve`.
    pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<ServerEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        (Arc::new(Self { events }), rx)
    }

    fn push(&self, event: ServerEvent) {
        // The client is gone once the receiver is; nothing is left to tell.
        let _ = self.events.send(event);
    }
}

impl Host for ServerHost {
    fn notify(&self, item: InboxItem, badge: u32) {
        self.push(ServerEvent::Inbox {
            item: Box::new(item),
            badge,
        });
    }

    fn badge(&self, badge: u32) {
        self.push(ServerEvent::Badge { badge });
    }

    /// The client opens it only after the user confirms, and only `http(s)`.
    fn open_url(&self, url: &str) {
        self.push(ServerEvent::OpenUrl {
            url: url.to_owned(),
        });
    }

    /// Never: a key does not cross the wire in either direction.
    fn secret(&self, _section: &str) -> Option<String> {
        None
    }
}

/// `cox app-server --stdio`: one client on stdin and stdout until stdin
/// closes. `home` is `COX_HOME` (`None`: `~/.cox`).
pub async fn serve_stdio(home: Option<PathBuf>) -> Result<(), ServerError> {
    // A non-interactive ssh shell has a thin environment; tools and env-var
    // keys resolve as in a terminal only after the login shell's is read.
    let _warning = crate::app::load_login_env().await;
    let (host, events) = ServerHost::channel();
    let app = App::new(home, host)?;
    serve(app, events, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Serves `input`'s requests until it closes, then stops every stream and
/// flushes what the client has not read yet.
pub async fn serve<R, W>(
    app: Arc<App>,
    events: mpsc::UnboundedReceiver<ServerEvent>,
    input: R,
    output: W,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, queued) = mpsc::channel(OUT_QUEUE);
    let writer = tokio::spawn(write(queued, output));
    let mut server = Server {
        app,
        out,
        sessions: HashMap::new(),
        fresh: Vec::new(),
        tasks: JoinSet::new(),
    };
    server.tasks.spawn(forward(events, server.out.clone()));
    let mut lines = BufReader::new(input).lines();
    while let Some(text) = lines.next_line().await? {
        if text.trim().is_empty() {
            continue;
        }
        // A line that is not a request gets id 0: the client cannot pair it
        // with a call, but sees why it was refused.
        let (id, outcome) = match wire::decode(&text) {
            Ok(Line::Request(request)) => (request.id, server.call(request.call).await),
            Ok(_) => (0, Outcome::Err("the server reads only requests".into())),
            Err(e) => (0, Outcome::Err(e.to_string())),
        };
        let response = Line::Response(Response {
            v: wire::VERSION,
            id,
            outcome,
        });
        if server.out.send(response).await.is_err() {
            break;
        }
        // After the response, so the client knows the session before its
        // first patch.
        server.stream_fresh();
    }
    server.stop().await;
    match writer.await {
        Ok(written) => Ok(written?),
        Err(joined) => Err(std::io::Error::other(joined).into()),
    }
}

struct Server {
    app: Arc<App>,
    out: mpsc::Sender<Line>,
    sessions: HashMap<SessionId, Arc<LiveSession>>,
    /// Opened by the last call; their pumps start after its response.
    fresh: Vec<Arc<LiveSession>>,
    tasks: JoinSet<()>,
}

impl Server {
    async fn call(&mut self, call: Call) -> Outcome {
        match self.answer(call).await {
            Ok(reply) => Outcome::Ok(reply),
            Err(e) => Outcome::Err(e.to_string()),
        }
    }

    async fn answer(&mut self, call: Call) -> Result<Reply, AppError> {
        Ok(match call {
            Call::Projects { limit } => Reply::Projects(self.app.workspace().projects(limit)?),
            Call::Sessions { project, limit } => {
                Reply::Sessions(self.app.workspace().sessions(&project, limit)?)
            }
            Call::Search { query, limit } => {
                Reply::Search(self.app.workspace().search(&query, limit)?)
            }
            Call::Open { cwd, resume, theme } => {
                let live = self.app.open(cwd, resume, theme).await?;
                let reply = Reply::Opened {
                    session: live.id(),
                    blocks: live.snapshot(),
                    warnings: live.warnings().to_vec(),
                };
                self.adopt(live);
                reply
            }
            Call::Send { session, intent } => {
                let child = self.live(session)?.send(intent).await?;
                Reply::Sent {
                    child: child.map(|live| self.adopt(live)),
                }
            }
            Call::Snapshot { session } => Reply::Snapshot(self.live(session)?.snapshot()),
            Call::Expand { session, archive } => {
                Reply::Expand(self.live(session)?.output(&archive)?)
            }
            Call::Complete {
                session,
                token,
                limit,
            } => Reply::Complete(self.live(session)?.complete(&token, limit as usize)),
            Call::Changes { session } => Reply::Changes(self.live(session)?.changes().await?),
            Call::Plan { session } => Reply::Plan(self.live(session)?.plan()),
            Call::Close { session } => {
                if let Some(live) = self.sessions.remove(&session) {
                    live.close();
                }
                Reply::Closed
            }
        })
    }

    fn live(&self, session: SessionId) -> Result<Arc<LiveSession>, AppError> {
        self.sessions
            .get(&session)
            .cloned()
            .ok_or(AppError::NotOpen(session))
    }

    fn adopt(&mut self, live: Arc<LiveSession>) -> SessionId {
        let id = live.id();
        self.sessions.insert(id, Arc::clone(&live));
        self.fresh.push(live);
        id
    }

    fn stream_fresh(&mut self) {
        for live in self.fresh.drain(..) {
            self.tasks.spawn(pump(live, self.out.clone()));
        }
    }

    /// Closes every stream and waits for the pumps, so only the writer
    /// still holds the queue when this returns.
    async fn stop(mut self) {
        for live in self.sessions.values() {
            live.close();
        }
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }
}

/// One session's batches, as `next_patches` coalesces them. `send` waits
/// while the queue is full, which is the backpressure: the controller keeps
/// folding meanwhile.
async fn pump(live: Arc<LiveSession>, out: mpsc::Sender<Line>) {
    let session = live.id();
    while let Some(patches) = live.next_patches().await {
        if out
            .send(notification(ServerEvent::Patches { session, patches }))
            .await
            .is_err()
        {
            return;
        }
    }
    let _ = out.send(notification(ServerEvent::Ended { session })).await;
}

async fn forward(mut events: mpsc::UnboundedReceiver<ServerEvent>, out: mpsc::Sender<Line>) {
    while let Some(event) = events.recv().await {
        if out.send(notification(event)).await.is_err() {
            return;
        }
    }
}

fn notification(event: ServerEvent) -> Line {
    Line::Notification(Notification {
        v: wire::VERSION,
        event,
    })
}

/// Writes each line whole and flushes it; ends once every sender is gone,
/// then closes the stream so the client reads its end.
async fn write<W: AsyncWrite + Unpin>(
    mut queued: mpsc::Receiver<Line>,
    mut output: W,
) -> std::io::Result<()> {
    while let Some(line) = queued.recv().await {
        let text = wire::encode(&line).map_err(std::io::Error::other)?;
        output.write_all(text.as_bytes()).await?;
        output.flush().await?;
    }
    output.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_host_turns_platform_calls_into_notifications() {
        let (host, mut events) = ServerHost::channel();
        host.badge(2);
        host.open_url("https://example.com/");
        assert_eq!(
            events.try_recv().ok(),
            Some(ServerEvent::Badge { badge: 2 })
        );
        assert_eq!(
            events.try_recv().ok(),
            Some(ServerEvent::OpenUrl {
                url: "https://example.com/".into()
            })
        );
        assert_eq!(host.secret("anthropic"), None);
    }
}
