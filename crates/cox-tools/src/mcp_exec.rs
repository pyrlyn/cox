// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `mcp_exec`: one fresh sandboxed Python process per call. The program may
//! `search`, `describe` and `call` through [`McpExecBridge`]; the model sees
//! only what it prints. Separate from `cox-mcp` so a test can fake the bridge
//! and so tool schemas never ride along in the result. The process is not
//! reused, and nothing is persisted: a long-lived interpreter outlives the
//! directory it was given.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::{
    Concurrency, LinuxBackend, Risk, SandboxMode, SandboxPolicy, Tool, ToolCx, ToolError,
    ToolOutput, ToolSpec,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Summary only. `input_schema` stays on the host.
#[derive(Clone, Debug)]
pub struct ToolDoc {
    pub name: String,
    pub description: String,
}

/// Implemented by `cox-mcp` later. Tests use a fake.
#[async_trait]
pub trait McpExecBridge: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> Vec<ToolDoc>;
    async fn describe(&self, name: &str) -> Result<ToolDoc, ToolError>;
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, ToolError>;
}

const VISIBLE_BYTES: usize = 8_000;
const TRAILER: &str = "… truncated";
const STDERR_TAIL: usize = 4_000;

/// Original driver. User prints stay on a buffer; RPC lines are the only
/// bytes on the real stdout. No `save_tool`, `save_memory`, or files.
const DRIVER: &str = r#"import asyncio, io, json, sys
rin, rout = sys.stdin, sys.stdout
sys.stdout = io.StringIO()

def emit(kind, payload):
    rout.write(json.dumps({"type": kind, **payload}) + "\n")
    rout.flush()

def rpc(kind, payload):
    emit(kind, payload)
    line = rin.readline()
    if not line:
        raise RuntimeError("host closed the pipe")
    msg = json.loads(line)
    if msg.get("type") == "error":
        raise RuntimeError(str(msg.get("text", "rpc failed")))
    return msg.get("value")

def search(query, limit=5):
    return rpc("search", {"query": str(query), "limit": int(limit)})

def describe(name):
    return rpc("describe", {"name": str(name)})

async def call(name, args=None):
    return rpc("call", {"name": str(name), "args": {} if args is None else args})

def main():
    lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
    joined = "\n".join(("    " + ln if ln.strip() else ln) for ln in lines)
    body = joined if joined.strip() else "    pass"
    ns = {"search": search, "describe": describe, "call": call}
    exec(compile("async def __cox_main():\n" + body + "\n", "user", "exec"), ns, ns)
    asyncio.run(ns["__cox_main"]())
    emit("result", {"text": sys.stdout.getvalue()})

try:
    main()
except Exception as exc:
    sys.stderr.write("%s: %s\n" % (type(exc).__name__, exc))
    sys.exit(1)
"#;

pub struct McpExecTool {
    bridge: Arc<dyn McpExecBridge>,
    /// Driver paths from this process, so a test can see each call used a new directory.
    #[cfg(test)]
    drivers: std::sync::Mutex<Vec<PathBuf>>,
}

impl McpExecTool {
    pub fn new(bridge: Arc<dyn McpExecBridge>) -> Self {
        Self {
            bridge,
            #[cfg(test)]
            drivers: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    fn remember(&self, path: &Path) {
        self.drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(path.to_path_buf());
    }

    #[cfg(test)]
    fn driver_paths(&self) -> Vec<PathBuf> {
        self.drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Tool for McpExecTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_exec".to_string(),
            description: "Run Python that may search(query), describe(name), and await \
                call(name, args), and print one small JSON result."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string"},
                    "timeout_s": {"type": "integer", "minimum": 1, "maximum": 30}
                },
                "required": ["code"]
            }),
            deferred: false,
            risk: Risk::Write,
            // A process, same as `bash`: one call at a time.
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let code = crate::write::str_field(&input, "code")?;
        let timeout = timeout_of(&input)?;
        let python = program("python3")?;
        let scratch = Scratch::new()?;
        let driver = write_file(&scratch.0, "driver.py", DRIVER)?;
        let source = write_file(&scratch.0, "code.py", &code)?;
        #[cfg(test)]
        self.remember(&driver);
        let policy = isolated(cx.sandbox.linux_backend);
        let mut child = spawn(&scratch.0, &python, &driver, &source, &policy)?;
        exchange(&*self.bridge, &mut child, timeout, &cx.cancel).await
    }
}

/// `command` only mounts extra writable roots in `workspace-write`, and
/// bubblewrap hides `/tmp` unless the path is also a root. Network stays off
/// even when the session allows it. The workspace is not in this set.
fn isolated(backend: LinuxBackend) -> SandboxPolicy {
    SandboxPolicy {
        mode: SandboxMode::WorkspaceWrite,
        network: false,
        writable: Vec::new(),
        readonly_in_workspace: Vec::new(),
        linux_backend: backend,
    }
}

fn timeout_of(input: &Value) -> Result<Duration, ToolError> {
    let seconds = match input.get("timeout_s") {
        None | Some(Value::Null) => 30,
        Some(Value::Number(n)) => {
            n.as_i64()
                .map(|s| s.clamp(1, 30))
                .ok_or_else(|| ToolError::Denied {
                    why: "timeout_s must be an integer".into(),
                })?
        }
        Some(_) => {
            return Err(ToolError::Denied {
                why: "timeout_s must be an integer".into(),
            });
        }
    };
    Ok(Duration::from_secs(u64::try_from(seconds).unwrap_or(30)))
}

/// Fixed directories, never `PATH`: the program the sandbox execs must not
/// follow an environment the workspace can rewrite.
fn program(name: &str) -> Result<PathBuf, ToolError> {
    ["/usr/bin", "/bin", "/usr/local/bin", "/opt/homebrew/bin"]
        .into_iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| ToolError::Denied {
            why: format!("{name} is missing"),
        })
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, ToolError> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cox-mcp-exec-{}-{n}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|_| ToolError::Io)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_file(dir: &Path, name: &str, body: &str) -> Result<PathBuf, ToolError> {
    let path = dir.join(name);
    std::fs::write(&path, body).map_err(|_| ToolError::Io)?;
    Ok(path)
}

/// `sandbox::command` always inserts `-c`, so the shell replaces itself with
/// `python3 -I -u <driver>` and does not stay resident.
fn spawn(
    dir: &Path,
    python: &Path,
    driver: &Path,
    code: &Path,
    policy: &SandboxPolicy,
) -> Result<tokio::process::Child, ToolError> {
    let sh = program("sh")?;
    let line = format!(
        "exec {} -I -u {} {}",
        quote(python),
        quote(driver),
        quote(code)
    );
    let root = dir.to_path_buf();
    let mut cmd = cox_sandbox::sandbox::command(
        policy,
        std::slice::from_ref(&root),
        std::slice::from_ref(&root),
        &sh,
        &line,
    )
    .map_err(|_| ToolError::Io)?;
    cmd.current_dir(dir)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in cox_protocol::config::CHILD_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    cmd.spawn().map_err(|_| ToolError::Io)
}

enum LoopEnd {
    Result(String),
    Rejected(String),
    Eof,
    TimedOut,
    Cancelled,
}

async fn exchange(
    bridge: &dyn McpExecBridge,
    child: &mut tokio::process::Child,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let mut stdin = child.stdin.take().ok_or(ToolError::Io)?;
    let stdout = child.stdout.take().ok_or(ToolError::Io)?;
    let stderr = child.stderr.take().ok_or(ToolError::Io)?;
    let logged = tokio::spawn(read_tail(stderr));
    let end = tokio::select! {
        biased;
        () = cancel.cancelled() => LoopEnd::Cancelled,
        () = tokio::time::sleep(timeout) => LoopEnd::TimedOut,
        end = read_rpc(bridge, stdout, &mut stdin) => end.unwrap_or(LoopEnd::Eof),
    };
    drop(stdin);
    if matches!(end, LoopEnd::Cancelled) {
        let _ = reap(child, true).await;
        logged.abort();
        return Err(ToolError::Cancelled);
    }
    let kill = matches!(end, LoopEnd::TimedOut | LoopEnd::Rejected(_));
    let status = reap(child, kill).await?;
    let tail = logged.await.unwrap_or_default();
    Ok(match end {
        LoopEnd::Result(text) if status.success() => plain(cap_text(&text), false),
        LoopEnd::Rejected(kind) => plain(
            error_text(&format!("rejected rpc type {kind}"), &tail),
            true,
        ),
        LoopEnd::TimedOut => plain(error_text("timed out", &tail), true),
        LoopEnd::Result(_) | LoopEnd::Eof | LoopEnd::Cancelled => {
            plain(error_text(&exit_label(&status), &tail), true)
        }
    })
}

async fn reap(
    child: &mut tokio::process::Child,
    kill: bool,
) -> Result<std::process::ExitStatus, ToolError> {
    if kill {
        let _ = child.kill().await;
    }
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(result) => result.map_err(|_| ToolError::Io),
        Err(_) => {
            let _ = child.kill().await;
            child.wait().await.map_err(|_| ToolError::Io)
        }
    }
}

async fn read_rpc(
    bridge: &dyn McpExecBridge,
    stdout: tokio::process::ChildStdout,
    stdin: &mut tokio::process::ChildStdin,
) -> Result<LoopEnd, ToolError> {
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .await
            .map_err(|_| ToolError::Io)?
            == 0
        {
            return Ok(LoopEnd::Eof);
        }
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&trimmed) {
            Ok(msg) => msg,
            Err(_) => return Ok(LoopEnd::Rejected("invalid json".into())),
        };
        if msg.get("type").and_then(Value::as_str) == Some("result") {
            let text = msg.get("text").and_then(Value::as_str).unwrap_or("");
            return Ok(LoopEnd::Result(text.to_string()));
        }
        match dispatch(bridge, &msg).await {
            Ok(reply) => write_line(stdin, &reply).await?,
            Err(kind) => return Ok(LoopEnd::Rejected(kind)),
        }
    }
}

/// `describe` is name and description only: the schema stays on the host.
async fn dispatch(bridge: &dyn McpExecBridge, msg: &Value) -> Result<Value, String> {
    let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");
    Ok(match kind {
        "search" => {
            let query = msg.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = msg
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .min(20);
            let docs = bridge
                .search(query, usize::try_from(limit).unwrap_or(5))
                .await;
            let value: Vec<_> = docs
                .into_iter()
                .map(|doc| json!({"name": doc.name, "description": doc.description}))
                .collect();
            json!({"type": "search", "value": value})
        }
        "describe" => {
            let name = msg.get("name").and_then(Value::as_str).unwrap_or("");
            match bridge.describe(name).await {
                Ok(doc) => {
                    json!({"type": "describe", "value": {"name": doc.name, "description": doc.description}})
                }
                Err(err) => json!({"type": "error", "text": err.to_string()}),
            }
        }
        "call" => {
            let name = msg.get("name").and_then(Value::as_str).unwrap_or("");
            let args = msg.get("args").cloned().unwrap_or_else(|| json!({}));
            match bridge.call_tool(name, args).await {
                Ok(value) => json!({"type": "call", "value": value}),
                Err(err) => json!({"type": "error", "text": err.to_string()}),
            }
        }
        other => return Err(other.to_string()),
    })
}

async fn write_line(
    stdin: &mut tokio::process::ChildStdin,
    value: &Value,
) -> Result<(), ToolError> {
    use tokio::io::AsyncWriteExt;
    let mut bytes = serde_json::to_vec(value).map_err(|_| ToolError::Io)?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.map_err(|_| ToolError::Io)?;
    stdin.flush().await.map_err(|_| ToolError::Io)
}

async fn read_tail(mut stderr: tokio::process::ChildStderr) -> String {
    use tokio::io::AsyncReadExt;
    let mut acc = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        acc.extend_from_slice(&buf[..n]);
        if acc.len() > STDERR_TAIL {
            acc.drain(..acc.len() - STDERR_TAIL);
        }
    }
    String::from_utf8_lossy(&acc).into_owned()
}

fn cap_text(text: &str) -> String {
    if text.len() <= VISIBLE_BYTES {
        return text.to_string();
    }
    let mut end = VISIBLE_BYTES.saturating_sub(TRAILER.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRAILER}", &text[..end])
}

fn error_text(why: &str, tail: &str) -> String {
    let text = if tail.trim().is_empty() {
        why.to_string()
    } else {
        format!("{why}\n{tail}")
    };
    cap_text(&text)
}

fn exit_label(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "exit signal".to_string(),
    }
}

fn plain(text: String, is_error: bool) -> ToolOutput {
    ToolOutput {
        text,
        is_error,
        diff: None,
        structured: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use cox_protocol::{Archive, ArchiveId, ArchivePut, SessionId, StoreError, Tool};
    use tokio_util::sync::CancellationToken;

    use super::*;

    struct Fake {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl McpExecBridge for Fake {
        async fn search(&self, _query: &str, _limit: usize) -> Vec<ToolDoc> {
            Vec::new()
        }

        async fn describe(&self, name: &str) -> Result<ToolDoc, ToolError> {
            Ok(ToolDoc {
                name: name.to_string(),
                description: "a tool".to_string(),
            })
        }

        async fn call_tool(&self, _name: &str, _args: Value) -> Result<Value, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let marker = "PAYLOAD_MARKER";
            let mut payload = String::with_capacity(10_000);
            payload.push_str(marker);
            payload.extend(std::iter::repeat_n('x', 10_000 - marker.len()));
            Ok(Value::String(payload))
        }
    }

    struct CountArchive(AtomicUsize);

    #[async_trait]
    impl Archive for CountArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ArchiveId::new())
        }

        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn cx(archive: Arc<CountArchive>) -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        crate::tool_cx(
            vec![PathBuf::from("/workspace")],
            PathBuf::from("/tmp"),
            SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                network: true,
                writable: vec![PathBuf::from("/workspace")],
                readonly_in_workspace: vec![],
                linux_backend: LinuxBackend::Auto,
            },
            archive,
            CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    #[test]
    fn mcp_exec_is_a_present_write_tool_with_no_network() {
        let tool = McpExecTool::new(Arc::new(Fake {
            calls: AtomicUsize::new(0),
        }));
        let spec = tool.spec();
        assert_eq!(spec.name, "mcp_exec");
        assert!(!spec.deferred);
        assert_eq!(spec.risk, Risk::Write);
        assert_eq!(spec.input_schema["required"], json!(["code"]));
        assert!(!spec.description.contains("schema"));
        let policy = isolated(LinuxBackend::Auto);
        assert!(!policy.network);
        assert!(policy.writable.is_empty());
    }

    #[test]
    fn result_text_over_8000_bytes_ends_with_truncated_trailer() {
        let out = cap_text(&"y".repeat(9_000));
        assert!(out.len() <= VISIBLE_BYTES);
        assert!(out.ends_with(TRAILER));
        assert_eq!(cap_text("ok"), "ok");
    }

    #[tokio::test]
    async fn two_large_tool_results_leave_only_the_programs_print() {
        let fake = Arc::new(Fake {
            calls: AtomicUsize::new(0),
        });
        let tool = McpExecTool::new(fake.clone());
        let archive = Arc::new(CountArchive(AtomicUsize::new(0)));
        let context = cx(archive.clone());
        let code = r#"
a = await call("one", {"i": 1})
b = await call("two", {"i": 2})
d = describe("alpha")
print('{"n":2}')
print(",".join(sorted(d)))
"#;
        let out = tool
            .call(json!({"code": code, "timeout_s": 15}), &context)
            .await
            .expect("mcp_exec");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("{\"n\":2}"), "{}", out.text);
        assert!(out.text.contains("description,name"), "{}", out.text);
        assert!(!out.text.contains("input_schema"), "{}", out.text);
        assert!(!out.text.contains("PAYLOAD_MARKER"), "{}", out.text);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
        assert_eq!(archive.0.load(Ordering::SeqCst), 0);

        let again = tool
            .call(json!({"code": "print('{\"n\":0}')"}), &context)
            .await
            .expect("second call");
        assert!(again.text.contains("{\"n\":0}"), "{}", again.text);
        let paths = tool.driver_paths();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1]);
        assert_ne!(paths[0].parent(), paths[1].parent());
    }
}
