// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! LSP stdio transport: `Content-Length` framing and a JSON-RPC 2.0 client
//! over any `AsyncRead`/`AsyncWrite`. Hand-rolled on `serde_json` because no
//! maintained, async, client-side LSP crate exists (plan.md P41). Separate
//! from the server lifecycle (T41.4) so the wire is tested over an in-memory
//! pipe, with no process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Largest body either side may send. A server that claims more is treated
/// as broken instead of being buffered.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// A header line with no newline within this many bytes is not LSP framing.
const MAX_HEADER_LINE: u64 = 8 * 1024;
/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("language server i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("language server framing or JSON: {0}")]
    Parse(String),
    #[error("language server message of {bytes} bytes exceeds the {cap}-byte cap")]
    TooLarge { bytes: usize, cap: usize },
    #[error("language server request `{method}` timed out")]
    Timeout { method: String },
    #[error("language server connection closed")]
    Closed,
    #[error("language server error {code}: {message}")]
    Server { code: i64, message: String },
}

/// A message from the server that expects no answer (`publishDiagnostics`,
/// `$/progress`, `window/logMessage`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    /// `Null` when the server sent none.
    pub params: Value,
}

/// Reads one framed message. `Ok(None)` is a clean end of stream between
/// messages; an end of stream inside one is `Closed`.
pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<Option<Value>, LspError> {
    let mut len = None;
    let mut first = true;
    loop {
        let mut line = Vec::new();
        // `take` bounds the line so a server that never sends `\n` cannot
        // grow this buffer without limit.
        let n = (&mut *r)
            .take(MAX_HEADER_LINE)
            .read_until(b'\n', &mut line)
            .await
            .map_err(io)?;
        if n == 0 && first {
            return Ok(None);
        }
        first = false;
        if line.last() != Some(&b'\n') {
            return Err(if n as u64 == MAX_HEADER_LINE {
                LspError::Parse("header line too long".into())
            } else {
                LspError::Closed
            });
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(LspError::Parse(format!("malformed header `{line}`")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            let n = value
                .parse::<usize>()
                .map_err(|_| LspError::Parse(format!("bad Content-Length `{value}`")))?;
            len = Some(n);
        }
    }
    let len = len.ok_or_else(|| LspError::Parse("missing Content-Length".into()))?;
    if len > MAX_MESSAGE_BYTES {
        return Err(LspError::TooLarge {
            bytes: len,
            cap: MAX_MESSAGE_BYTES,
        });
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).await.map_err(io)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| LspError::Parse(e.to_string()))
}

/// Writes one framed message and flushes it.
pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> Result<(), LspError> {
    let body = serde_json::to_vec(msg).map_err(|e| LspError::Parse(e.to_string()))?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(LspError::TooLarge {
            bytes: body.len(),
            cap: MAX_MESSAGE_BYTES,
        });
    }
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await.map_err(io)?;
    w.write_all(&body).await.map_err(io)?;
    w.flush().await.map_err(io)
}

/// A pipe that ended mid-message or under a write is a closed connection,
/// not an I/O fault the caller could retry.
fn io(e: std::io::Error) -> LspError {
    use std::io::ErrorKind::{BrokenPipe, ConnectionReset, UnexpectedEof};
    match e.kind() {
        UnexpectedEof | BrokenPipe | ConnectionReset => LspError::Closed,
        _ => LspError::Io(e),
    }
}

type Pending = HashMap<i64, oneshot::Sender<Result<Value, LspError>>>;
type Writer = Arc<tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;

/// One JSON-RPC connection to a language server.
pub struct Client {
    writer: Writer,
    /// `None` once the reader has stopped: new requests fail at once.
    pending: Arc<Mutex<Option<Pending>>>,
    next_id: AtomicI64,
    reader: JoinHandle<()>,
}

impl Client {
    /// Starts the reader task. The receiver yields server notifications and
    /// ends when the connection does.
    pub fn start<R, W>(reader: R, writer: W) -> (Self, mpsc::UnboundedReceiver<Notification>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let writer: Writer = Arc::new(tokio::sync::Mutex::new(Box::new(writer)));
        let pending = Arc::new(Mutex::new(Some(HashMap::new())));
        // Unbounded on purpose: a bounded channel would stall the reader,
        // and with it every response, while the consumer awaits a request.
        // The consumer (T41.4) drains it.
        let (tx, rx) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_loop(
            BufReader::new(reader),
            writer.clone(),
            pending.clone(),
            tx,
        ));
        let client = Self {
            writer,
            pending,
            next_id: AtomicI64::new(1),
            reader,
        };
        (client, rx)
    }

    /// Sends a request and waits up to `timeout` for its response.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending)
            .as_mut()
            .ok_or(LspError::Closed)?
            .insert(id, tx);
        if let Err(e) = send(&self.writer, &envelope(Some(id), method, params)).await {
            self.forget(id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            // The reader dropped every waiter: the connection ended.
            Ok(Err(_)) => Err(LspError::Closed),
            Err(_) => {
                self.forget(id);
                // Best effort, so the server can drop the work; a dead pipe
                // surfaces on the next call instead of hiding the timeout.
                let _ = self.notify("$/cancelRequest", json!({ "id": id })).await;
                Err(LspError::Timeout {
                    method: method.to_owned(),
                })
            }
        }
    }

    /// Sends a notification. `Null` params are left off the wire.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        send(&self.writer, &envelope(None, method, params)).await
    }

    fn forget(&self, id: i64) {
        if let Some(pending) = lock(&self.pending).as_mut() {
            pending.remove(&id);
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn read_loop<R: AsyncBufRead + Unpin>(
    mut r: R,
    writer: Writer,
    pending: Arc<Mutex<Option<Pending>>>,
    notes: mpsc::UnboundedSender<Notification>,
) {
    // A framing error desynchronises the stream, so it ends the connection
    // like EOF does.
    while let Ok(Some(msg)) = read_message(&mut r).await {
        let Value::Object(mut msg) = msg else {
            continue;
        };
        let method = msg.remove("method");
        let method = method.as_ref().and_then(Value::as_str);
        match (method, msg.remove("id")) {
            (Some(method), Some(id)) => {
                let reply = answer(method, msg.get("params"), id);
                // A dropped receiver only means nobody listens any more; the
                // server still gets its acknowledgement below.
                if method == "workspace/diagnostic/refresh" {
                    let _ = notes.send(Notification {
                        method: method.to_owned(),
                        params: Value::Null,
                    });
                }
                let writer = writer.clone();
                // Replying from its own task keeps this loop reading while a
                // large client write holds the writer, or both sides block.
                tokio::spawn(async move {
                    let _ = send(&writer, &reply).await;
                });
            }
            (Some(method), None) => {
                // A dropped receiver only means nobody listens any more.
                let _ = notes.send(Notification {
                    method: method.to_owned(),
                    params: msg.remove("params").unwrap_or(Value::Null),
                });
            }
            (None, Some(id)) => {
                let waiter = id
                    .as_i64()
                    .and_then(|id| lock(&pending).as_mut()?.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(outcome(msg));
                }
            }
            (None, None) => {}
        }
    }
    // Dropping every sender wakes each waiter with `Closed`; `None` refuses
    // later requests.
    lock(&pending).take();
}

/// The reply to a server request. The few requests a diagnostics client
/// meets get a harmless result so the server never blocks on us.
fn answer(method: &str, params: Option<&Value>, id: Value) -> Value {
    let result = match method {
        "workspace/configuration" => {
            let items = params
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Value::Array(vec![Value::Null; items])
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        // Acknowledged so the server does not see "method not found" and
        // give up asking; the pull-mode consumer reacts to it (T41.10).
        | "workspace/diagnostic/refresh" => Value::Null,
        _ => {
            return json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": METHOD_NOT_FOUND,
                "message": format!("cox does not handle `{method}`"),
            }});
        }
    };
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn outcome(mut msg: Map<String, Value>) -> Result<Value, LspError> {
    match msg.remove("error") {
        Some(err) if !err.is_null() => Err(LspError::Server {
            code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        _ => Ok(msg.remove("result").unwrap_or(Value::Null)),
    }
}

fn envelope(id: Option<i64>, method: &str, params: Value) -> Value {
    let mut msg = Map::new();
    msg.insert("jsonrpc".into(), "2.0".into());
    if let Some(id) = id {
        msg.insert("id".into(), id.into());
    }
    msg.insert("method".into(), method.into());
    // JSON-RPC 2.0 allows params to be absent but not `null`.
    if !params.is_null() {
        msg.insert("params".into(), params);
    }
    Value::Object(msg)
}

async fn send(writer: &Writer, msg: &Value) -> Result<(), LspError> {
    write_message(&mut *writer.lock().await, msg).await
}

/// A panic elsewhere must not wedge the connection, and the map stays valid
/// whatever the panicking holder was doing.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{DuplexStream, ReadHalf, WriteHalf, duplex, split};

    type ServerEnd = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);

    fn connect() -> (Client, mpsc::UnboundedReceiver<Notification>, ServerEnd) {
        let (ours, theirs) = duplex(64 * 1024);
        let (r, w) = split(ours);
        let (client, notes) = Client::start(r, w);
        let (sr, sw) = split(theirs);
        (client, notes, (BufReader::new(sr), sw))
    }

    async fn read_one(bytes: &[u8]) -> Result<Option<Value>, LspError> {
        let mut r = bytes;
        read_message(&mut r).await
    }

    const LONG: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn framing_round_trips() {
        let (mut a, b) = duplex(1024);
        let msg = json!({"jsonrpc": "2.0", "method": "x", "params": {"text": "héllo ✓"}});
        write_message(&mut a, &msg).await.unwrap();
        drop(a);
        let mut r = BufReader::new(b);
        assert_eq!(read_message(&mut r).await.unwrap(), Some(msg));
        assert!(read_message(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn split_headers_and_back_to_back_messages_are_framed() {
        // A 3-byte pipe forces every header and body to arrive in pieces.
        let (mut a, b) = duplex(3);
        let bytes = b"content-length: 7\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{\"a\":1}Content-Length: 7\r\n\r\n{\"b\":2}";
        tokio::spawn(async move { a.write_all(bytes).await });
        let mut r = BufReader::new(b);
        assert_eq!(read_message(&mut r).await.unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_message(&mut r).await.unwrap(), Some(json!({"b": 2})));
        assert!(read_message(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn partial_message_is_closed() {
        let body = read_one(b"Content-Length: 10\r\n\r\n{\"a\"").await;
        assert!(matches!(body, Err(LspError::Closed)), "{body:?}");
        let header = read_one(b"Content-Len").await;
        assert!(matches!(header, Err(LspError::Closed)), "{header:?}");
    }

    #[tokio::test]
    async fn missing_or_bad_content_length_is_a_parse_error() {
        let long_line = vec![b'a'; 9000];
        for bytes in [
            &b"Content-Type: x\r\n\r\n{}"[..],
            b"Content-Length: -1\r\n\r\n",
            b"Content-Length: abc\r\n\r\n",
            b"no colon here\r\n\r\n",
            b"Content-Length: 3\r\n\r\nabc",
            &long_line,
        ] {
            let got = read_one(bytes).await;
            assert!(matches!(got, Err(LspError::Parse(_))), "{got:?}");
        }
    }

    #[tokio::test]
    async fn oversized_message_is_rejected() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        let got = read_one(header.as_bytes()).await;
        assert!(
            matches!(got, Err(LspError::TooLarge { bytes, cap }) if bytes == MAX_MESSAGE_BYTES + 1 && cap == MAX_MESSAGE_BYTES),
            "{got:?}"
        );

        // A client whose server sends one stops, failing the waiter.
        let (client, _notes, (mut sr, mut sw)) = connect();
        let server = async {
            read_message(&mut sr).await.unwrap();
            sw.write_all(header.as_bytes()).await.unwrap();
        };
        let (got, ()) = tokio::join!(client.request("a", json!({}), LONG), server);
        assert!(matches!(got, Err(LspError::Closed)), "{got:?}");
    }

    #[tokio::test]
    async fn server_request_is_answered() {
        let (_client, _notes, (mut sr, mut sw)) = connect();
        for (request, want) in [
            (
                json!({"jsonrpc": "2.0", "id": 7, "method": "workspace/configuration", "params": {"items": [{}, {}]}}),
                json!({"jsonrpc": "2.0", "id": 7, "result": [null, null]}),
            ),
            (
                json!({"jsonrpc": "2.0", "id": 8, "method": "window/workDoneProgress/create", "params": {"token": "t"}}),
                json!({"jsonrpc": "2.0", "id": 8, "result": null}),
            ),
        ] {
            write_message(&mut sw, &request).await.unwrap();
            assert_eq!(read_message(&mut sr).await.unwrap(), Some(want));
        }
        let unknown = json!({"jsonrpc": "2.0", "id": "x", "method": "foo/bar"});
        write_message(&mut sw, &unknown).await.unwrap();
        let reply = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(reply["id"], "x");
        assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn diagnostic_refresh_is_acknowledged_and_forwarded() {
        let (_client, mut notes, (mut sr, mut sw)) = connect();
        write_message(
            &mut sw,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "workspace/diagnostic/refresh"}),
        )
        .await
        .unwrap();
        let ack = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(ack, json!({"jsonrpc": "2.0", "id": 1, "result": null}));
        let note = notes.recv().await.unwrap();
        assert_eq!(note.method, "workspace/diagnostic/refresh");
    }

    #[tokio::test]
    async fn request_times_out() {
        let (client, _notes, (mut sr, _sw)) = connect();
        let got = client
            .request("slow", json!({}), Duration::from_millis(50))
            .await;
        assert!(
            matches!(&got, Err(LspError::Timeout { method }) if method == "slow"),
            "{got:?}"
        );
        let sent = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(sent["method"], "slow");
        let cancel = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(
            cancel,
            json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": sent["id"]}})
        );
    }

    #[tokio::test]
    async fn closed_pipe_fails_pending_requests() {
        let (client, mut notes, (mut sr, sw)) = connect();
        let server = async move {
            read_message(&mut sr).await.unwrap();
            drop((sr, sw));
        };
        let (got, ()) = tokio::join!(client.request("a", json!({}), LONG), server);
        assert!(matches!(got, Err(LspError::Closed)), "{got:?}");
        assert!(
            notes.recv().await.is_none(),
            "the stream ends with the pipe"
        );
        let after = client.request("b", json!({}), LONG).await;
        assert!(matches!(after, Err(LspError::Closed)), "{after:?}");
    }

    #[tokio::test]
    async fn responses_match_ids_out_of_order() {
        let (client, _notes, (mut sr, mut sw)) = connect();
        let server = async {
            let mut ids = HashMap::new();
            for _ in 0..2 {
                let m = read_message(&mut sr).await.unwrap().unwrap();
                ids.insert(m["method"].as_str().unwrap().to_owned(), m["id"].clone());
            }
            let replies = [
                json!({"jsonrpc": "2.0", "id": 999, "result": "stray"}),
                json!({"jsonrpc": "2.0", "id": ids["b"], "result": {"for": "b"}}),
                json!({"jsonrpc": "2.0", "id": ids["a"], "error": {"code": -32000, "message": "nope"}}),
            ];
            for reply in replies {
                write_message(&mut sw, &reply).await.unwrap();
            }
        };
        let (a, b, ()) = tokio::join!(
            client.request("a", json!({}), LONG),
            client.request("b", json!({}), LONG),
            server
        );
        assert!(
            matches!(&a, Err(LspError::Server { code: -32000, message }) if message == "nope"),
            "{a:?}"
        );
        assert_eq!(b.unwrap(), json!({"for": "b"}));
    }

    #[tokio::test]
    async fn notifications_reach_the_stream() {
        let (client, mut notes, (mut sr, mut sw)) = connect();
        let diags = json!({"uri": "file:///a.rs", "diagnostics": []});
        write_message(
            &mut sw,
            &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": diags}),
        )
        .await
        .unwrap();
        write_message(&mut sw, &json!({"jsonrpc": "2.0", "method": "$/ping"}))
            .await
            .unwrap();
        let first = notes.recv().await.unwrap();
        assert_eq!(first.method, "textDocument/publishDiagnostics");
        assert_eq!(first.params, diags);
        assert_eq!(notes.recv().await.unwrap().params, Value::Null);

        client.notify("exit", Value::Null).await.unwrap();
        assert_eq!(
            read_message(&mut sr).await.unwrap(),
            Some(json!({"jsonrpc": "2.0", "method": "exit"}))
        );
    }
}
